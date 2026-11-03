//! What both rail behaviours share from vanilla `AbstractMinecart` and `Entity.move`.
//!
//! `Entity::move_entity` replaces the velocity with the distance moved, which vanilla's
//! `Entity.move` does not do: a ridden cart moves by 0.75 of its speed and must keep the rest.

use std::sync::atomic::Ordering;

use pumpkin_data::game_event::GameEvent;
use pumpkin_protocol::java::server::play::SPlayerInput;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;

use crate::entity::player::Player;
use crate::entity::{Entity, EntityBase};

use super::MinecartEntity;
use super::rails::is_rail;

/// Vanilla `AbstractMinecart.getAirDrag`.
const AIR_DRAG: f32 = 0.95;
/// Vanilla `Mth.equal`'s tolerance.
const MTH_EQUAL_TOLERANCE: f32 = 1.0e-5;

fn mth_equal(a: f64, b: f64) -> bool {
    (b - a).abs() < f64::from(MTH_EQUAL_TOLERANCE)
}

/// Vanilla `ServerPlayer.getLastClientMoveIntent`.
pub(super) fn last_client_move_intent(player: &Player) -> Vector3<f64> {
    const fn intent(positive: bool, negative: bool) -> f64 {
        if positive == negative {
            0.0
        } else if positive {
            1.0
        } else {
            -1.0
        }
    }
    let input = player.last_input.load(Ordering::Relaxed);
    let left = intent(
        input & SPlayerInput::LEFT != 0,
        input & SPlayerInput::RIGHT != 0,
    );
    let forward = intent(
        input & SPlayerInput::FORWARD != 0,
        input & SPlayerInput::BACKWARD != 0,
    );
    player
        .get_entity()
        .movement_input_to_velocity(Vector3::new(left, 0.0, forward), 1.0)
}

/// Vanilla `Entity.push(x, y, z)`.
pub(super) fn add_impulse(entity: &Entity, x: f64, z: f64) {
    if x.is_finite() && z.is_finite() {
        let velocity = entity.velocity.load();
        entity
            .velocity
            .store(Vector3::new(velocity.x + x, velocity.y, velocity.z + z));
        entity.velocity_dirty.store(true, Ordering::SeqCst);
    }
}

impl MinecartEntity {
    /// Vanilla `AbstractMinecart.getCurrentBlockPosOrRailBelow`.
    pub(super) fn current_block_pos_or_rail_below(&self) -> BlockPos {
        let entity = &self.vehicle.entity;
        let pos = entity.pos.load();
        let world = entity.world.load();
        let xt = pos.x.floor() as i32;
        let mut yt = pos.y.floor() as i32;
        let zt = pos.z.floor() as i32;
        if self.experimental_movement {
            let y = (pos.y - 0.1 - f64::from(MTH_EQUAL_TOLERANCE)).floor() as i32;
            if is_rail(world.get_block(&BlockPos::new(xt, y, zt))) {
                yt = y;
            }
        } else if is_rail(world.get_block(&BlockPos::new(xt, yt - 1, zt))) {
            yt -= 1;
        }
        BlockPos::new(xt, yt, zt)
    }

    /// Vanilla `Entity.applyGravity`.
    pub(super) fn apply_gravity(&self) {
        let entity = &self.vehicle.entity;
        let mut velocity = entity.velocity.load();
        velocity.y -= self.get_gravity();
        entity.velocity.store(velocity);
    }

    /// Vanilla `AbstractMinecart.isRedstoneConductor`.
    pub(super) fn is_redstone_conductor(&self, pos: &BlockPos) -> bool {
        self.vehicle
            .entity
            .world
            .load()
            .get_block_state(pos)
            .is_solid_block()
    }

    /// Vanilla `AbstractMinecart.comeOffTrack`.
    pub(super) fn come_off_track(&self, caller: &dyn EntityBase) {
        let entity = &self.vehicle.entity;
        let max_speed = self.max_speed();
        let movement = entity.velocity.load();
        entity.velocity.store(Vector3::new(
            movement.x.clamp(-max_speed, max_speed),
            movement.y,
            movement.z.clamp(-max_speed, max_speed),
        ));
        if entity.on_ground.load(Ordering::SeqCst) {
            entity.velocity.store(entity.velocity.load() * 0.5);
        }
        self.cart_move(caller, entity.velocity.load());
        if !entity.on_ground.load(Ordering::SeqCst) {
            entity
                .velocity
                .store(entity.velocity.load() * f64::from(AIR_DRAG));
        }
    }

    /// Vanilla `AbstractMinecart.move`.
    pub(super) fn cart_move(&self, caller: &dyn EntityBase, delta: Vector3<f64>) {
        let entity = &self.vehicle.entity;
        if self.experimental_movement {
            let to_position = entity.pos.load() + delta;
            self.move_self(caller, delta);
            if self.new_push_and_pickup_entities(caller) {
                self.move_self(caller, to_position - entity.pos.load());
            }
        } else {
            self.move_self(caller, delta);
            self.apply_effects_from_blocks(caller);
        }
    }

    /// Vanilla `Entity.move`, with the deltaMovement left to the caller.
    fn move_self(&self, caller: &dyn EntityBase, delta: Vector3<f64>) {
        let entity = &self.vehicle.entity;
        if entity.no_physics.load(Ordering::Relaxed) {
            entity.move_pos(delta);
            entity.horizontal_collision.store(false, Ordering::SeqCst);
            entity.vertical_collision.store(false, Ordering::SeqCst);
            return;
        }

        let mut delta = delta;
        let stuck_multiplier = entity.movement_multiplier.swap(Vector3::default());
        if stuck_multiplier.length_squared() > 1.0e-7 {
            delta = delta.multiply(stuck_multiplier.x, stuck_multiplier.y, stuck_multiplier.z);
            entity.velocity.store(Vector3::default());
        }

        let movement = entity.adjust_movement_for_collisions(delta, caller);
        let movement_length = movement.length_squared();
        if movement_length > 1.0e-7 || delta.length_squared() - movement_length < 1.0e-7 {
            entity.move_pos(movement);
        }

        let x_collision = !mth_equal(delta.x, movement.x);
        let z_collision = !mth_equal(delta.z, movement.z);
        let vertical_collision = delta.y != movement.y;
        let on_ground = vertical_collision && delta.y < 0.0;
        entity
            .horizontal_collision
            .store(x_collision || z_collision, Ordering::SeqCst);
        entity
            .vertical_collision
            .store(vertical_collision, Ordering::SeqCst);
        entity.on_ground.store(on_ground, Ordering::SeqCst);

        self.check_fall_damage(caller, movement.y, on_ground);
        if entity.is_removed() {
            return;
        }

        // Vanilla `restituteMovementAfterCollisions`: minecarts have no bounciness of their own.
        let (effect_pos, effect_block, effect_state) = entity.get_block_with_y_offset(0.2);
        let mut velocity = entity.velocity.load();
        if x_collision {
            velocity.x = 0.0;
        }
        if z_collision {
            velocity.z = 0.0;
        }
        entity.velocity.store(velocity);
        if vertical_collision {
            entity
                .world
                .load()
                .block_registry
                .update_entity_movement_after_fall_on(effect_block, caller);
        }

        entity.emit_movement(caller, movement, &effect_pos, effect_state);

        let speed_factor = f64::from(caller.get_block_speed_factor());
        entity.velocity.store(
            entity
                .velocity
                .load()
                .multiply(speed_factor, 1.0, speed_factor),
        );
    }

    /// Vanilla `Entity.checkFallDamage`.
    ///
    /// Block `fallOn` overrides are for living entities in Pumpkin, so the fall counts in full
    /// here, like vanilla's default `Block.fallOn`.
    fn check_fall_damage(&self, caller: &dyn EntityBase, ya: f64, on_ground: bool) {
        let entity = &self.vehicle.entity;
        if !entity.is_in_water() && ya < 0.0 {
            self.fall_distance
                .store(self.fall_distance.load() - f64::from(ya as f32));
        }
        if on_ground {
            let fall_distance = self.fall_distance.swap(0.0);
            if fall_distance > 0.0 {
                self.cause_fall_damage(fall_distance);
                entity.emit_own_game_event(caller, GameEvent::HitGround);
            }
        }
    }

    /// Vanilla `AbstractMinecart.applyEffectsFromBlocks`: step and inside effects at the current
    /// position. The new behaviour's swept check is not ported, carts step less than a block.
    pub(super) fn apply_effects_from_blocks(&self, caller: &dyn EntityBase) {
        let entity = &self.vehicle.entity;
        if !entity.is_affected_by_blocks() {
            return;
        }
        if entity.on_ground.load(Ordering::SeqCst) {
            let world = entity.world.load();
            let (pos, block, state) = entity.get_block_with_y_offset(0.2);
            world
                .block_registry
                .on_entity_step(block, &world, caller, &pos, state, false);
        }
        entity.tick_block_collisions(caller);
    }
}
