use crate::entity::projectile::owner::ProjectileOwner;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use pumpkin_data::{
    Block, damage::DamageType, effect::StatusEffect, fluid::FluidState, potion::Effect,
    tracked_data,
};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::{
    Difficulty,
    math::{position::BlockPos, vector3::Vector3},
};

use crate::{
    entity::{
        Entity, EntityBase,
        boss::wither::WitherEntity,
        projectile::{
            ProjectileHit, ThrownItemEntity,
            fireball::INITIAL_ACCELERATION_POWER,
            hurting::{self, AccelerationPower, HurtingMotion},
        },
    },
    server::Server,
    world::{
        ExplosionInteraction, World,
        explosion::{
            Explosion, ExplosionDamageCalculator, ExplosionSettings,
            default_block_explosion_resistance,
        },
    },
};

const GRAVITY: f64 = 0.0;
/// Vanilla `WitherSkull.getInertia` of a blue skull.
const DANGEROUS_INERTIA: f32 = 0.73;
/// Vanilla `WitherSkull.getBlockExplosionResistance` of a blue skull.
const DANGEROUS_MAX_RESISTANCE: f32 = 0.8;
const OWNED_HIT_DAMAGE: f32 = 8.0;
const OWNERLESS_HIT_DAMAGE: f32 = 5.0;
/// Vanilla `WitherSkull.onHitEntity`: what its owner heals for a kill.
const KILL_HEAL: f32 = 5.0;
const EXPLOSION_POWER: f32 = 1.0;
const WITHER_AMPLIFIER: u8 = 1;
const NORMAL_WITHER_TICKS: i32 = 20 * 10;
const HARD_WITHER_TICKS: i32 = 20 * 40;

/// Vanilla `WitherSkull.getBlockExplosionResistance`: a blue skull's blast goes through what
/// the wither can destroy, however strong it is.
struct DangerousSkullCalculator;

impl ExplosionDamageCalculator for DangerousSkullCalculator {
    fn get_block_explosion_resistance(
        &self,
        _explosion: &Explosion,
        _world: &World,
        _pos: &BlockPos,
        block: &Block,
        fluid: &FluidState,
    ) -> Option<f32> {
        default_block_explosion_resistance(block, fluid).map(|resistance| {
            if WitherEntity::can_destroy(block) {
                resistance.min(DANGEROUS_MAX_RESISTANCE)
            } else {
                resistance
            }
        })
    }
}

pub struct WitherSkullEntity {
    pub thrown: ThrownItemEntity,
    pub dangerous: AtomicBool,
    pub acceleration_power: AccelerationPower,
}

impl WitherSkullEntity {
    #[must_use]
    pub const fn new(entity: Entity) -> Self {
        let thrown = ThrownItemEntity {
            entity,
            owner: ProjectileOwner::none(),
            collides_with_projectiles: false,
            has_hit: AtomicBool::new(false),
            gravity: GRAVITY,
        };

        Self {
            thrown,
            dangerous: AtomicBool::new(false),
            acceleration_power: AccelerationPower::new(INITIAL_ACCELERATION_POWER),
        }
    }

    /// Vanilla `WitherSkull(level, mob, direction)` and `setPos`, where `entity` stands.
    #[must_use]
    pub fn new_shot(
        entity: Entity,
        shooter: &Entity,
        dangerous: bool,
        direction: Vector3<f64>,
    ) -> Self {
        Self {
            thrown: ThrownItemEntity::new_hurting(
                entity,
                shooter,
                direction,
                INITIAL_ACCELERATION_POWER,
            ),
            dangerous: AtomicBool::new(dangerous),
            acceleration_power: AccelerationPower::new(INITIAL_ACCELERATION_POWER),
        }
    }

    #[must_use]
    pub fn is_dangerous(&self) -> bool {
        self.dangerous.load(Ordering::Relaxed)
    }

    pub fn set_dangerous(&self, dangerous: bool) {
        self.dangerous.store(dangerous, Ordering::Relaxed);
        self.thrown
            .entity
            .set_synced_data(tracked_data::wither_skull::DATA_DANGEROUS, dangerous);
    }

    /// Vanilla `WitherSkull.onHitEntity`.
    fn hit_entity(&self, world: &World, target: &dyn EntityBase, position: Vector3<f64>) {
        let owner = self
            .thrown
            .owner
            .get(world)
            .filter(|owner| owner.get_living_entity().is_some());
        let was_hurt = if let Some(owner) = owner.as_deref() {
            let hurt = target.damage_with_context(
                target,
                OWNED_HIT_DAMAGE,
                DamageType::WITHER_SKULL,
                Some(position),
                Some(self),
                Some(owner),
            );
            if hurt && !hurting::is_alive(target) {
                heal(owner, KILL_HEAL);
            }
            hurt
        } else {
            target.damage_with_context(
                target,
                OWNERLESS_HIT_DAMAGE,
                DamageType::MAGIC,
                None,
                None,
                None,
            )
        };
        if !was_hurt {
            return;
        }

        let duration = match world.level_info.load().difficulty {
            Difficulty::Normal => NORMAL_WITHER_TICKS,
            Difficulty::Hard => HARD_WITHER_TICKS,
            Difficulty::Easy | Difficulty::Peaceful => return,
        };
        hurting::add_effect(
            target,
            Effect {
                effect_type: &StatusEffect::WITHER,
                duration,
                amplifier: WITHER_AMPLIFIER,
                ambient: false,
                show_particles: true,
                show_icon: true,
                blend: false,
            },
        );
    }
}

/// Vanilla `LivingEntity.heal`, with a player's health update.
fn heal(owner: &dyn EntityBase, amount: f32) {
    if let Some(player) = owner.get_player() {
        player.heal(amount);
    } else if let Some(living) = owner.get_living_entity() {
        living.heal(amount);
    }
}

impl EntityBase for WitherSkullEntity {
    fn get_projectile_owner(&self) -> Option<&ProjectileOwner> {
        Some(&self.thrown.owner)
    }

    fn write_custom_nbt(&self, nbt: &mut NbtCompound) {
        self.acceleration_power.write_nbt(nbt);
        nbt.put_bool("dangerous", self.is_dangerous());
    }

    fn read_custom_nbt(&self, nbt: &NbtCompound) {
        self.acceleration_power.read_nbt(nbt);
        if let Some(dangerous) = nbt.get_bool("dangerous") {
            self.set_dangerous(dangerous);
        }
    }

    fn init_data_tracker(&self) {
        let entity = self.get_entity();
        entity.set_synced_data(
            tracked_data::wither_skull::DATA_DANGEROUS,
            self.is_dangerous(),
        );
    }

    /// Vanilla `WitherSkull.getInertia` and `shouldBurn` in the hurting projectile's tick.
    fn tick(&self, caller: &dyn EntityBase, server: &Server) {
        let inertia = if self.is_dangerous() {
            DANGEROUS_INERTIA
        } else {
            HurtingMotion::FIREBALL.inertia
        };
        self.thrown.tick_hurting(
            caller,
            server,
            self.acceleration_power.get(),
            &HurtingMotion {
                inertia,
                burns: false,
                ..HurtingMotion::FIREBALL
            },
        );
    }

    fn get_entity(&self) -> &Entity {
        self.thrown.get_entity()
    }

    fn get_living_entity(&self) -> Option<&crate::entity::living::LivingEntity> {
        None
    }

    fn cast_any(&self) -> &dyn std::any::Any {
        self
    }

    fn on_deflection(&self, by_attack: bool) {
        self.acceleration_power.on_deflection(by_attack);
    }

    /// Vanilla `AbstractHurtingProjectile.canHitEntity`.
    fn projectile_can_hit(&self, target: &dyn EntityBase) -> bool {
        hurting::can_hit_with_physics(target)
    }

    /// Vanilla `WitherSkull.onHitEntity` and `onHit`.
    fn on_hit(&self, hit: ProjectileHit) {
        let entity = self.get_entity();
        let world = entity.world.load_full();

        if let ProjectileHit::Entity {
            entity: target,
            hit_pos,
            ..
        } = &hit
        {
            self.hit_entity(&world, target.as_ref(), *hit_pos);
        }

        let calculator = self
            .is_dangerous()
            .then(|| Arc::new(DangerousSkullCalculator) as Arc<dyn ExplosionDamageCalculator>);
        world.explode_with_settings(
            Some(entity),
            entity.pos.load(),
            EXPLOSION_POWER,
            ExplosionInteraction::Mob,
            ExplosionSettings {
                calculator,
                ..ExplosionSettings::default()
            },
        );
    }
}
