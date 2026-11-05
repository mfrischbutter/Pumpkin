//! Vanilla `AbstractArrow.tick` and its steps.

use std::sync::atomic::Ordering;

use pumpkin_data::BlockState;
use pumpkin_util::math::boundingbox::BoundingBox;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;

use super::abstract_arrow::{AbstractArrow, ArrowHooks};
use crate::entity::projectile::hit_result::{
    self, ClipResult, EntityHit, HitOutcome, clip_including_border, hit_target_or_deflect_self,
    lerp_rotation,
};
use crate::entity::projectile_deflection::ProjectileDeflectionType;
use crate::entity::{Entity, EntityBase};
use crate::server::Server;

/// Vanilla `AABB.contains`.
fn contains(bounding_box: &BoundingBox, position: Vector3<f64>) -> bool {
    position.x >= bounding_box.min.x
        && position.x < bounding_box.max.x
        && position.y >= bounding_box.min.y
        && position.y < bounding_box.max.y
        && position.z >= bounding_box.min.z
        && position.z < bounding_box.max.z
}

impl AbstractArrow {
    /// Vanilla `AbstractArrow.tick`. The crit and bubble particles are the client's: on the
    /// server they are `Level.addParticle`, which does nothing.
    pub(crate) fn tick(&self, hooks: &dyn ArrowHooks, caller: &dyn EntityBase, server: &Server) {
        let entity = &self.entity;
        let physics_enabled = !self.is_no_physics();
        let movement = entity.velocity.load();
        let block_pos = entity.block_pos.load();
        let state = entity.world.load().get_block_state(&block_pos);
        if physics_enabled && Self::is_inside_shape(state, block_pos, entity.pos.load()) {
            entity.velocity.store(Vector3::new(0.0, 0.0, 0.0));
            self.set_in_ground(true);
        }

        let shake = self.shake_time.load(Ordering::Relaxed);
        if shake > 0 {
            self.shake_time.store(shake - 1, Ordering::Relaxed);
        }

        if entity.is_in_water_or_rain() {
            entity.extinguish();
        }

        if self.is_in_ground() && physics_enabled {
            self.tick_in_ground(hooks, caller, state);
        } else {
            self.tick_flying(hooks, caller, server, movement);
        }
    }

    fn is_inside_shape(state: &BlockState, block_pos: BlockPos, position: Vector3<f64>) -> bool {
        !state.is_air()
            && state
                .get_block_collision_shapes_at(&block_pos)
                .any(|shape| contains(&shape.at_pos(block_pos), position))
    }

    /// The in-ground branch of vanilla's tick, which skips `Entity.tick`: a stuck arrow keeps
    /// burning until rain or water reach it.
    fn tick_in_ground(&self, hooks: &dyn ArrowHooks, caller: &dyn EntityBase, state: &BlockState) {
        let entity = &self.entity;
        if self.last_state.load() != Some(state.id) && self.should_fall() {
            self.start_falling();
        } else {
            hooks.tick_despawn();
        }

        self.in_ground_time.fetch_add(1, Ordering::Relaxed);
        if entity.is_alive() {
            entity.tick_block_collisions(caller);
        }
        entity.set_on_fire(entity.fire_ticks.load(Ordering::Relaxed) > 0);
    }

    /// The flying branch of vanilla's tick. The move uses the movement from before the water's
    /// drag, and the air's drag and gravity come after it: a shot's first tick is at full speed.
    fn tick_flying(
        &self,
        hooks: &dyn ArrowHooks,
        caller: &dyn EntityBase,
        server: &Server,
        movement: Vector3<f64>,
    ) {
        let entity = &self.entity;
        let physics_enabled = !self.is_no_physics();
        self.in_ground_time.store(0, Ordering::Relaxed);
        let origin = entity.pos.load();
        if entity.is_in_water() {
            self.apply_inertia(hooks.water_inertia());
        }

        Self::rotate_towards(entity, movement, physics_enabled);
        self.owner.check_left_owner(entity);
        if physics_enabled {
            let clip = clip_including_border(&entity.world.load(), origin, origin.add(&movement));
            self.step_move_and_hit(hooks, caller, &clip);
        } else {
            entity.set_pos(origin.add(&movement));
            entity.tick_block_collisions(caller);
        }

        if !entity.is_in_water() {
            self.apply_inertia(Self::INERTIA);
        }
        if physics_enabled && !self.is_in_ground() && !entity.has_no_gravity() {
            let velocity = entity.velocity.load();
            entity
                .velocity
                .store(velocity.add_raw(0.0, -Self::GRAVITY, 0.0));
        }

        hit_result::emit_shoot_event(caller, &self.has_been_shot);
        entity.tick(caller, server);
    }

    fn rotate_towards(entity: &Entity, movement: Vector3<f64>, physics_enabled: bool) {
        let yaw = if physics_enabled {
            movement.x.atan2(movement.z)
        } else {
            (-movement.x).atan2(-movement.z)
        };
        let pitch = movement.y.atan2(movement.horizontal_length());
        entity.set_rotation(
            lerp_rotation(entity.yaw.load(), (yaw as f32).to_degrees()),
            lerp_rotation(entity.pitch.load(), (pitch as f32).to_degrees()),
        );
    }

    /// Vanilla `applyInertia`.
    fn apply_inertia(&self, inertia: f64) {
        let velocity = self.entity.velocity.load();
        self.entity
            .velocity
            .store(velocity.multiply(inertia, inertia, inertia));
    }

    /// Vanilla `stepMoveAndHit`. A piercing arrow goes on from each entity until it stops, is
    /// used up or has nothing left in its way; every entity it hit is ignored from then on, so
    /// the loop ends.
    fn step_move_and_hit(
        &self,
        hooks: &dyn ArrowHooks,
        caller: &dyn EntityBase,
        clip: &ClipResult,
    ) {
        let entity = &self.entity;
        while entity.is_alive() {
            let start = entity.pos.load();
            let mut hits = hooks.find_hit_entities(start, clip.hit.location);
            let distance = |hit: &EntityHit| {
                start.squared_distance_to_vec(&hit.entity.get_entity().pos.load())
            };
            hits.sort_by(|a, b| distance(a).total_cmp(&distance(b)));
            let next = hits.first().map_or(clip.hit.location, |hit| hit.location);
            entity.set_pos(next);
            entity.tick_block_collisions(caller);
            if !entity.is_alive() {
                return;
            }

            if hits.is_empty() {
                if !clip.hit.is_miss {
                    self.hit_block_or_bounce(caller, clip);
                    entity.velocity_dirty.store(true, Ordering::Relaxed);
                }
                return;
            }
            let outcome = Self::hit_targets_or_deflect_self(caller, hits);
            entity.velocity_dirty.store(true, Ordering::Relaxed);
            if self.get_pierce_level() == 0 || outcome != HitOutcome::Hit {
                return;
            }
        }
    }

    /// Vanilla `hitTargetOrDeflectSelf` with a block: the world border sends an arrow back.
    fn hit_block_or_bounce(&self, caller: &dyn EntityBase, clip: &ClipResult) {
        if clip.world_border {
            ProjectileDeflectionType::reverse(caller, Vector3::new(0.2, 0.2, 0.2));
        } else {
            hit_target_or_deflect_self(caller, clip.to_projectile_hit(self.entity.velocity.load()));
        }
    }

    /// Vanilla `hitTargetsOrDeflectSelf`.
    fn hit_targets_or_deflect_self(caller: &dyn EntityBase, hits: Vec<EntityHit>) -> HitOutcome {
        for hit in hits {
            let velocity = caller.get_entity().velocity.load();
            let outcome = hit_target_or_deflect_self(caller, hit.into_projectile_hit(velocity));
            if !caller.get_entity().is_alive() || outcome != HitOutcome::Hit {
                return outcome;
            }
        }
        HitOutcome::Hit
    }

    /// Vanilla `shouldFall`.
    fn should_fall(&self) -> bool {
        let position = self.entity.pos.load();
        self.is_in_ground()
            && self
                .entity
                .world
                .load()
                .is_space_empty(BoundingBox::new(position, position).expand_all(0.06))
    }

    /// Vanilla `startFalling`.
    fn start_falling(&self) {
        self.set_in_ground(false);
        let velocity = self.entity.velocity.load();
        self.entity.velocity.store(velocity.multiply(
            f64::from(rand::random::<f32>() * 0.2),
            f64::from(rand::random::<f32>() * 0.2),
            f64::from(rand::random::<f32>() * 0.2),
        ));
        self.life.store(0, Ordering::Relaxed);
    }
}
