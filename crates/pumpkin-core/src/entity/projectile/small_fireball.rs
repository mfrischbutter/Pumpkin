use crate::entity::projectile::owner::ProjectileOwner;
use std::sync::atomic::{AtomicBool, Ordering};

use pumpkin_data::damage::DamageType;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_world::world::BlockFlags;

use crate::{
    block::blocks::fire::{FireBlockBase, fire::FireBlock},
    entity::{
        Entity, EntityBase,
        projectile::{ProjectileHit, ThrownItemEntity, fireball::INITIAL_ACCELERATION_POWER},
    },
    server::Server,
};

const GRAVITY: f64 = 0.0;
const HIT_DAMAGE: f32 = 5.0;
const HIT_FIRE_SECONDS: f32 = 5.0;

pub struct SmallFireballEntity {
    pub thrown: ThrownItemEntity,
}

impl SmallFireballEntity {
    #[must_use]
    pub const fn new(entity: Entity) -> Self {
        let thrown = ThrownItemEntity {
            entity,
            owner: ProjectileOwner::none(),
            collides_with_projectiles: false,
            has_hit: AtomicBool::new(false),
            gravity: GRAVITY,
        };

        Self { thrown }
    }

    #[must_use]
    pub fn new_shot(entity: Entity, shooter: &Entity, direction: Vector3<f64>) -> Self {
        let thrown = ThrownItemEntity::new(entity, shooter, GRAVITY);
        let accel = INITIAL_ACCELERATION_POWER;
        thrown
            .entity
            .velocity
            .store(direction.normalize().multiply(accel, accel, accel));
        Self { thrown }
    }
}

impl EntityBase for SmallFireballEntity {
    fn get_projectile_owner(&self) -> Option<&ProjectileOwner> {
        Some(&self.thrown.owner)
    }

    fn tick(&self, caller: &dyn EntityBase, _server: &Server) {
        self.thrown
            .process_hurting_tick(caller, INITIAL_ACCELERATION_POWER);
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
                    DamageType::FIREBALL,
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
                    let fire = FireBlockBase::get_fire_type(&world, &fire_pos);
                    let state = FireBlock.get_state_for_position(&world, &fire, &fire_pos);
                    world.set_block_state(&fire_pos, state, BlockFlags::NOTIFY_ALL);
                }
            }
        }
    }
}
