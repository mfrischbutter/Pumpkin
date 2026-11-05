use crate::entity::projectile::owner::ProjectileOwner;
use std::sync::atomic::{AtomicBool, Ordering};

use pumpkin_data::item_stack::ItemStack;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_world::world::BlockFlags;

use crate::{
    block::blocks::fire::FireBlockBase,
    entity::{
        Entity, EntityBase,
        projectile::{
            ProjectileHit, ThrownItemEntity,
            fireball::{FireballItem, INITIAL_ACCELERATION_POWER, fireball_damage},
            hurting::{self, AccelerationPower, HurtingMotion},
        },
    },
    server::Server,
};

const GRAVITY: f64 = 0.0;
const HIT_DAMAGE: f32 = 5.0;
const HIT_FIRE_SECONDS: f32 = 5.0;

pub struct SmallFireballEntity {
    pub thrown: ThrownItemEntity,
    pub item: FireballItem,
    pub acceleration_power: AccelerationPower,
}

impl SmallFireballEntity {
    #[must_use]
    pub fn new(entity: Entity) -> Self {
        let thrown = ThrownItemEntity {
            entity,
            owner: ProjectileOwner::none(),
            collides_with_projectiles: false,
            has_hit: AtomicBool::new(false),
            gravity: GRAVITY,
        };
        Self::from_thrown(thrown)
    }

    /// Vanilla `SmallFireball(level, mob, direction)`, where `entity` stands.
    #[must_use]
    pub fn new_shot(entity: Entity, shooter: &Entity, direction: Vector3<f64>) -> Self {
        Self::from_thrown(ThrownItemEntity::new_hurting(
            entity,
            shooter,
            direction,
            INITIAL_ACCELERATION_POWER,
        ))
    }

    fn from_thrown(thrown: ThrownItemEntity) -> Self {
        Self {
            thrown,
            item: FireballItem::default(),
            acceleration_power: AccelerationPower::new(INITIAL_ACCELERATION_POWER),
        }
    }

    /// Vanilla `Fireball.setItem`.
    pub fn set_item(&self, source: &ItemStack) {
        self.item.set(self.get_entity(), source);
    }
}

impl EntityBase for SmallFireballEntity {
    fn get_projectile_owner(&self) -> Option<&ProjectileOwner> {
        Some(&self.thrown.owner)
    }

    fn get_projectile_item(&self) -> Option<ItemStack> {
        Some(self.item.get())
    }

    fn write_custom_nbt(&self, nbt: &mut NbtCompound) {
        self.acceleration_power.write_nbt(nbt);
        self.item.write_nbt(nbt);
    }

    fn read_custom_nbt(&self, nbt: &NbtCompound) {
        self.acceleration_power.read_nbt(nbt);
        self.item.read_nbt(self.get_entity(), nbt);
    }

    fn init_data_tracker(&self) {
        self.item.init_data_tracker(self.get_entity());
    }

    fn tick(&self, caller: &dyn EntityBase, server: &Server) {
        self.thrown.tick_hurting(
            caller,
            server,
            self.acceleration_power.get(),
            &HurtingMotion::FIREBALL,
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

    /// Vanilla `SmallFireball.onHitEntity` and `onHitBlock`.
    fn on_hit(&self, hit: ProjectileHit) {
        let world = self.get_entity().world.load_full();
        let owner = self.thrown.owner.get(&world);
        match hit {
            ProjectileHit::Entity {
                ref entity,
                hit_pos,
                ..
            } => {
                let target = entity.get_entity();
                let remaining_fire_ticks = target.fire_ticks.load(Ordering::Relaxed);
                entity.set_on_fire_for(HIT_FIRE_SECONDS);
                let damaged = entity.damage_with_context(
                    entity.as_ref(),
                    HIT_DAMAGE,
                    fireball_damage(owner.is_some()),
                    Some(hit_pos),
                    Some(self),
                    owner.as_deref(),
                );
                if !damaged {
                    target
                        .fire_ticks
                        .store(remaining_fire_ticks, Ordering::Relaxed);
                }
            }
            ProjectileHit::Block { pos, face, .. } => {
                let shot_by_mob = owner.is_some_and(|owner| owner.get_mob().is_some());
                if shot_by_mob && !world.level_info.load().game_rules.mob_griefing {
                    return;
                }
                let fire_pos = pos.offset(face.to_offset());
                if world.get_block_state(&fire_pos).is_air() {
                    let state = FireBlockBase::get_state(&world, &fire_pos);
                    world.set_block_state(&fire_pos, state, BlockFlags::NOTIFY_ALL);
                }
            }
        }
    }
}
