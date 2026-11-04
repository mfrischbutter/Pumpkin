use crate::entity::projectile::owner::ProjectileOwner;
use pumpkin_data::damage::DamageType;
use pumpkin_data::entity::EntityType;
use pumpkin_data::particle::Particle;
use pumpkin_data::sound::Sound;
use pumpkin_data::tag;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::math::vector3::Vector3;
use std::sync::LazyLock;
use std::sync::{
    Arc,
    atomic::{AtomicU8, Ordering},
};

use crate::{
    entity::{
        Entity, EntityBase,
        living::LivingEntity,
        projectile::{
            ProjectileHit, ThrownItemEntity,
            hurting::{self, AccelerationPower, HurtingMotion},
        },
    },
    server::Server,
    world::{ExplosionInteraction, SimpleExplosionDamageCalculator, explosion::ExplosionSettings},
};

const DEFAULT_DEFLECT_COOLDOWN: u8 = 5;
pub const WIND_CHARGE_GRAVITY: f64 = 0.0;
/// Vanilla `AbstractWindCharge.JUMP_SCALE`: how far off a block's face it bursts.
const JUMP_SCALE: f64 = 0.25;
/// Vanilla `AbstractWindCharge.onHitEntity` damage.
const HIT_DAMAGE: f32 = 1.0;
/// Vanilla `AbstractWindCharge.tick`: how far above the world it may fly.
const MAX_HEIGHT_ABOVE_WORLD: i32 = 30;
/// Vanilla `WindCharge.RADIUS`.
const RADIUS: f32 = 1.2;
/// Vanilla `BreezeWindCharge.RADIUS`.
const BREEZE_RADIUS: f32 = 3.0;
/// Vanilla `AbstractWindCharge.getInertia`, `getLiquidInertia` and `shouldBurn`.
const MOTION: HurtingMotion = HurtingMotion {
    inertia: 1.0,
    liquid_inertia: 1.0,
    burns: false,
};

enum WindChargeKind {
    Normal { deflect_cooldown: AtomicU8 },
    Breeze,
}

pub struct WindChargeEntity {
    kind: WindChargeKind,
    thrown_item_entity: ThrownItemEntity,
    acceleration_power: AccelerationPower,
}

pub static WIND_CHARGE_EXPLOSION_DAMAGE_CALCULATOR: LazyLock<Arc<SimpleExplosionDamageCalculator>> =
    LazyLock::new(|| {
        Arc::new(SimpleExplosionDamageCalculator::new(
            false,
            true,
            Some(1.22),
            Some(&tag::Block::MINECRAFT_BLOCKS_WIND_CHARGE_EXPLOSIONS),
        ))
    });

pub static BREEZE_WIND_CHARGE_EXPLOSION_DAMAGE_CALCULATOR: LazyLock<
    Arc<SimpleExplosionDamageCalculator>,
> = LazyLock::new(|| {
    Arc::new(SimpleExplosionDamageCalculator::new(
        false,
        true,
        None,
        Some(&tag::Block::MINECRAFT_BLOCKS_WIND_CHARGE_EXPLOSIONS),
    ))
});

impl WindChargeEntity {
    #[must_use]
    pub const fn new_normal(thrown_item_entity: ThrownItemEntity) -> Self {
        Self {
            kind: WindChargeKind::Normal {
                deflect_cooldown: AtomicU8::new(DEFAULT_DEFLECT_COOLDOWN),
            },
            thrown_item_entity,
            acceleration_power: AccelerationPower::new(0.0),
        }
    }

    #[must_use]
    pub const fn new_breeze(thrown_item_entity: ThrownItemEntity) -> Self {
        Self {
            kind: WindChargeKind::Breeze,
            thrown_item_entity,
            acceleration_power: AccelerationPower::new(0.0),
        }
    }

    pub const fn deflect_cooldown(&self) -> Option<&AtomicU8> {
        if let WindChargeKind::Normal {
            deflect_cooldown, ..
        } = &self.kind
        {
            Some(deflect_cooldown)
        } else {
            None
        }
    }

    /// Vanilla `WindCharge.explode` and `BreezeWindCharge.explode`.
    pub fn create_explosion(&self, position: Vector3<f64>) {
        let (power, calculator, sound) = match self.kind {
            WindChargeKind::Normal { .. } => (
                RADIUS,
                WIND_CHARGE_EXPLOSION_DAMAGE_CALCULATOR.clone(),
                Sound::EntityWindChargeWindBurst,
            ),
            WindChargeKind::Breeze => (
                BREEZE_RADIUS,
                BREEZE_WIND_CHARGE_EXPLOSION_DAMAGE_CALCULATOR.clone(),
                Sound::EntityBreezeWindBurst,
            ),
        };
        let entity = self.get_entity();
        entity.world.load().explode_with_settings(
            Some(entity),
            position,
            power,
            ExplosionInteraction::Trigger,
            &ExplosionSettings {
                calculator: Some(calculator),
                fire: false,
                small_particle: Particle::GustEmitterSmall,
                large_particle: Particle::GustEmitterLarge,
                sound,
            },
        );
    }
}

impl EntityBase for WindChargeEntity {
    fn get_projectile_owner(&self) -> Option<&ProjectileOwner> {
        Some(&self.thrown_item_entity.owner)
    }

    /// Vanilla `AbstractWindCharge.tick` and `WindCharge.tick`.
    fn tick(&self, caller: &dyn EntityBase, server: &Server) {
        let entity = self.get_entity();
        let top_y = entity.world.load().get_top_y();
        if entity.block_pos.load().0.y > top_y + MAX_HEIGHT_ABOVE_WORLD {
            self.create_explosion(entity.pos.load());
            entity.remove();
            return;
        }

        self.thrown_item_entity.tick_hurting(
            caller,
            server,
            self.acceleration_power.get(),
            &MOTION,
        );

        if let Some(cooldown) = self.deflect_cooldown() {
            let cooldown_ticks = cooldown.load(Ordering::Relaxed);
            if cooldown_ticks > 0 {
                cooldown.store(cooldown_ticks - 1, Ordering::Relaxed);
            }
        }
    }

    fn get_entity(&self) -> &Entity {
        &self.thrown_item_entity.entity
    }

    fn get_living_entity(&self) -> Option<&LivingEntity> {
        None
    }

    fn cast_any(&self) -> &dyn std::any::Any {
        self
    }

    fn write_custom_nbt(&self, nbt: &mut NbtCompound) {
        self.acceleration_power.write_nbt(nbt);
    }

    fn read_custom_nbt(&self, nbt: &NbtCompound) {
        self.acceleration_power.read_nbt(nbt);
    }

    fn on_deflection(&self, by_attack: bool) {
        self.acceleration_power.on_deflection(by_attack);
    }

    /// Vanilla `WindCharge.deflect`: not in its first ticks.
    fn can_be_deflected(&self) -> bool {
        self.deflect_cooldown()
            .is_none_or(|cooldown| cooldown.load(Ordering::Relaxed) == 0)
    }

    /// Vanilla `AbstractWindCharge.canHitEntity`.
    fn projectile_can_hit(&self, target: &dyn EntityBase) -> bool {
        let kind = target.get_entity().entity_type;
        kind != &EntityType::WIND_CHARGE
            && kind != &EntityType::BREEZE_WIND_CHARGE
            && kind != &EntityType::END_CRYSTAL
            && hurting::can_hit_with_physics(target)
    }

    /// Vanilla `AbstractWindCharge.onHitEntity` and `onHitBlock`.
    fn on_hit(&self, hit: ProjectileHit) {
        let entity = self.get_entity();
        match &hit {
            ProjectileHit::Entity {
                entity: target,
                hit_pos,
                ..
            } => {
                let world = entity.world.load();
                let owner = self
                    .thrown_item_entity
                    .owner
                    .get(&world)
                    .filter(|owner| owner.get_living_entity().is_some());
                let _ = target.damage_with_context(
                    target.as_ref(),
                    HIT_DAMAGE,
                    DamageType::WIND_CHARGE,
                    Some(*hit_pos),
                    Some(self),
                    owner.as_deref(),
                );
                self.create_explosion(entity.pos.load());
            }
            ProjectileHit::Block { face, hit_pos, .. } => {
                self.create_explosion(*hit_pos + face.to_offset().to_f64() * JUMP_SCALE);
            }
        }
    }
}
