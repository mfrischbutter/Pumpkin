use crate::entity::projectile::owner::ProjectileOwner;
use std::sync::atomic::{AtomicBool, Ordering};

use pumpkin_data::damage::DamageType;
use pumpkin_util::math::{position::BlockPos, vector3::Vector3};

use crate::{
    entity::{
        Entity, EntityBase,
        living::LivingEntity,
        projectile::{ProjectileHit, ThrownItemEntity, hurting},
    },
    server::Server,
    world::World,
};

pub const LLAMA_SPIT_GRAVITY: f64 = 0.06;
/// Vanilla `LlamaSpit.getAirDrag`.
const AIR_DRAG: f32 = 0.99;
/// Vanilla `LlamaSpit.onHitEntity` damage.
const HIT_DAMAGE: f32 = 1.0;

pub struct LlamaSpitEntity {
    pub thrown: ThrownItemEntity,
}

impl LlamaSpitEntity {
    #[must_use]
    pub const fn new(entity: Entity) -> Self {
        let thrown = ThrownItemEntity {
            entity,
            owner: ProjectileOwner::none(),
            collides_with_projectiles: false,
            has_hit: AtomicBool::new(false),
            gravity: LLAMA_SPIT_GRAVITY,
        };

        Self { thrown }
    }

    #[must_use]
    pub fn new_shot(entity: Entity, shooter: &Entity) -> Self {
        let owner_pos = shooter.pos.load();
        let body_yaw_rad = f64::from(shooter.body_yaw.load()).to_radians();
        let bb_width = f64::from(shooter.entity_dimension.load().width);
        let offset = f64::midpoint(bb_width, 1.0);
        let x = owner_pos.x - offset * body_yaw_rad.sin();
        let y = owner_pos.y + shooter.get_eye_height() - 0.1;
        let z = owner_pos.z + offset * body_yaw_rad.cos();
        entity.pos.store(Vector3::new(x, y, z));

        let thrown = ThrownItemEntity {
            entity,
            owner: ProjectileOwner::new(Some(shooter)),
            collides_with_projectiles: false,
            has_hit: AtomicBool::new(false),
            gravity: LLAMA_SPIT_GRAVITY,
        };

        Self { thrown }
    }
}

/// Vanilla `findBlocksIn(box).filterState(isAir).noneMatched()`: no air anywhere in its box.
fn touches_no_air(world: &World, entity: &Entity) -> bool {
    let bounds = entity.bounding_box.load();
    for pos in BlockPos::iterate(bounds.min_block_pos(), bounds.max_block_pos()) {
        if world.get_block_state(&pos).is_air() {
            return false;
        }
    }
    true
}

impl EntityBase for LlamaSpitEntity {
    fn get_projectile_owner(&self) -> Option<&ProjectileOwner> {
        Some(&self.thrown.owner)
    }

    /// Vanilla `LlamaSpit.tick`. It moves by the velocity from before the drag and whatever
    /// the hit did, and a hit on an entity does not end it.
    fn tick(&self, caller: &dyn EntityBase, server: &Server) {
        let entity = self.get_entity();
        self.thrown.tick_projectile_state();
        entity.tick(caller, server);
        if entity.is_removed() {
            return;
        }

        let start = entity.pos.load();
        let movement = entity.velocity.load();
        if let Some(hit) = self.thrown.find_hit(caller, start, movement) {
            self.thrown.hit_target_or_deflect_self(caller, hit);
            if entity.is_removed() {
                return;
            }
        }

        hurting::update_rotation(entity);
        if touches_no_air(&entity.world.load(), entity)
            || entity.touching_water.load(Ordering::Relaxed)
        {
            entity.remove();
            return;
        }

        let mut next = movement * f64::from(AIR_DRAG);
        if !entity.has_no_gravity() {
            next.y -= self.thrown.gravity;
        }
        entity.velocity.store(next);
        entity.set_pos(start + movement);
        self.thrown.broadcast_velocity();
    }

    fn get_entity(&self) -> &Entity {
        self.thrown.get_entity()
    }

    fn get_living_entity(&self) -> Option<&LivingEntity> {
        None
    }

    fn cast_any(&self) -> &dyn std::any::Any {
        self
    }

    /// Vanilla `LlamaSpit.onHitEntity`, only with a living owner, and `onHitBlock`.
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
                    .thrown
                    .owner
                    .get(&world)
                    .filter(|owner| owner.get_living_entity().is_some());
                if owner.is_some() {
                    let _ = target.damage_with_context(
                        target.as_ref(),
                        HIT_DAMAGE,
                        DamageType::SPIT,
                        Some(*hit_pos),
                        Some(self),
                        owner.as_deref(),
                    );
                }
            }
            ProjectileHit::Block { .. } => entity.remove(),
        }
    }
}
