//! Vanilla `NewMinecartBehavior`: how minecarts move when `minecart_improvements` is enabled.

use std::sync::atomic::Ordering;

use pumpkin_data::block_properties::{PoweredRailLikeProperties, RailShape};
use pumpkin_data::game_rules::{GameRule, GameRuleValue};
use pumpkin_data::{Block, BlockStateId};
use pumpkin_protocol::java::client::play::MinecartStep;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_util::math::wrap_degrees;

use crate::entity::EntityBase;

use super::MinecartEntity;
use super::movement::last_client_move_intent;
use super::rails::{exits, is_rail, rail_shape};

const ON_RAIL_Y_OFFSET: f64 = 0.1;
const OPPOSING_SLOPES_REST_AT_SPEED_THRESHOLD: f64 = 0.005;
pub(super) const SLOWDOWN_EMPTY: f64 = 0.975;
const SMALL: f32 = 1.0e-5;

const fn horizontal(vector: Vector3<f64>) -> Vector3<f64> {
    Vector3::new(vector.x, 0.0, vector.z)
}

fn bottom_center(pos: BlockPos) -> Vector3<f64> {
    Vector3::new(
        f64::from(pos.0.x) + 0.5,
        f64::from(pos.0.y),
        f64::from(pos.0.z) + 0.5,
    )
}

/// Vanilla `isDecending`.
const fn is_descending(movement: &Vector3<f64>, shape: RailShape) -> bool {
    match shape {
        RailShape::AscendingEast => movement.x < 0.0,
        RailShape::AscendingWest => movement.x > 0.0,
        RailShape::AscendingNorth => movement.z > 0.0,
        RailShape::AscendingSouth => movement.z < 0.0,
        _ => false,
    }
}

/// Vanilla `NewMinecartBehavior.TrackIteration`.
struct TrackIteration {
    movement_left: f64,
    first_iteration: bool,
    has_gained_slope_speed: bool,
    has_halted: bool,
    has_boosted: bool,
}

impl TrackIteration {
    const fn new() -> Self {
        Self {
            movement_left: 0.0,
            first_iteration: true,
            has_gained_slope_speed: false,
            has_halted: false,
            has_boosted: false,
        }
    }

    fn should_iterate(&self) -> bool {
        self.first_iteration || self.movement_left > f64::from(SMALL)
    }
}

impl MinecartEntity {
    /// Vanilla `NewMinecartBehavior.tick`.
    pub(super) fn new_tick(&self, caller: &dyn EntityBase) {
        let entity = &self.vehicle.entity;
        let pos = self.current_block_pos_or_rail_below();
        let (block, state_id) = entity.world.load().get_block_and_state_id(&pos);
        if entity.first_tick.load(Ordering::Relaxed) {
            self.on_rails.store(is_rail(block), Ordering::Relaxed);
            self.adjust_to_rails(pos, block, state_id, true);
        }
        self.apply_gravity();
        self.new_move_along_track(caller);
    }

    /// Vanilla `NewMinecartBehavior.getMaxSpeed`.
    pub(super) fn new_max_speed(&self) -> f64 {
        let entity = &self.vehicle.entity;
        let world = entity.world.load();
        let blocks_per_second = match world.get_game_rule(&GameRule::MaxMinecartSpeed) {
            GameRuleValue::Int(value) => value as f64,
            GameRuleValue::Bool(_) => 0.0,
        };
        let water_factor = if entity.is_in_water() { 0.5 } else { 1.0 };
        blocks_per_second * water_factor / 20.0
    }

    fn add_lerp_step(&self, weight: f32) {
        let entity = &self.vehicle.entity;
        self.lerp_steps
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(MinecartStep {
                position: entity.pos.load(),
                movement: entity.velocity.load(),
                yaw: entity.yaw.load(),
                pitch: entity.pitch.load(),
                weight,
            });
    }

    /// Vanilla `NewMinecartBehavior.adjustToRails`.
    pub(super) fn adjust_to_rails(
        &self,
        target: BlockPos,
        block: &Block,
        state_id: BlockStateId,
        instant: bool,
    ) {
        if !is_rail(block) {
            return;
        }
        let entity = &self.vehicle.entity;
        let shape = rail_shape(block, state_id);
        let (exit0, exit1) = exits(shape);
        let exit0 = exit0.to_f64() * 0.5;
        let exit1 = exit1.to_f64() * 0.5;
        let mut out_direction = horizontal(exit0);
        let mut in_direction = horizontal(exit1);
        let movement = entity.velocity.load();
        if (movement.length() > f64::from(SMALL)
            && movement.dot(&out_direction) < movement.dot(&in_direction))
            || is_descending(&in_direction, shape)
        {
            std::mem::swap(&mut out_direction, &mut in_direction);
        }

        let flip_offset = if self.flipped.load(Ordering::Relaxed) {
            180.0f32
        } else {
            0.0f32
        };
        let mut yaw = 180.0f32 - out_direction.z.atan2(out_direction.x).to_degrees() as f32;
        yaw += flip_offset;
        let previous = entity.pos.load();
        let center = bottom_center(target);
        let target_position = if exit0.x != exit1.x && exit0.z != exit1.z {
            let from0to1 = exit1 - exit0;
            let from0_to_pos = previous - center - exit0;
            let travel = from0to1 * (from0to1.dot(&from0_to_pos) / from0to1.dot(&from0to1));
            yaw = 180.0f32 - travel.z.atan2(travel.x).to_degrees() as f32 + flip_offset;
            center + exit0 + travel
        } else {
            Vector3::new(
                if exit0.z != exit1.z {
                    center.x
                } else {
                    previous.x
                },
                f64::from(target.0.y),
                if exit0.x != exit1.x {
                    center.z
                } else {
                    previous.z
                },
            )
        };
        entity.set_pos(target_position);

        let position = entity.pos.load();
        let (lift, pitch) = if exit0.y == exit1.y {
            (ON_RAIL_Y_OFFSET, 0.0f32)
        } else {
            let distance_from_in = (center + in_direction - position).length();
            let pitch = if self.flipped.load(Ordering::Relaxed) {
                45.0
            } else {
                -45.0
            };
            (distance_from_in + ON_RAIL_Y_OFFSET, pitch)
        };
        entity.set_pos(position + Vector3::new(0.0, lift, 0.0));

        self.new_set_rotation(yaw, pitch);
        let adjust_distance = (previous - entity.pos.load()).length();
        if adjust_distance > 0.0 {
            self.add_lerp_step(if instant { 0.0 } else { adjust_distance as f32 });
        }
    }

    /// Vanilla `NewMinecartBehavior.setRotation`.
    fn new_set_rotation(&self, yaw: f32, pitch: f32) {
        let entity = &self.vehicle.entity;
        let (mut yaw, mut pitch) = (yaw, pitch);
        let yaw_diff = (yaw - entity.yaw.load()).abs();
        if (175.0..=185.0).contains(&yaw_diff) {
            self.flipped.fetch_not(Ordering::Relaxed);
            yaw -= 180.0;
            pitch *= -1.0;
        }
        entity.pitch.store(pitch.clamp(-45.0, 45.0) % 360.0);
        entity.yaw.store(yaw % 360.0);
    }

    /// Vanilla `NewMinecartBehavior.moveAlongTrack`.
    fn new_move_along_track(&self, caller: &dyn EntityBase) {
        let entity = &self.vehicle.entity;
        let mut iteration = TrackIteration::new();
        while iteration.should_iterate() && entity.is_alive() {
            let initial = entity.velocity.load();
            let current_pos = self.current_block_pos_or_rail_below();
            let (block, state_id) = entity.world.load().get_block_and_state_id(&current_pos);
            let on_rails = is_rail(block);
            if self.on_rails.load(Ordering::Relaxed) != on_rails {
                self.on_rails.store(on_rails, Ordering::Relaxed);
                self.adjust_to_rails(current_pos, block, state_id, false);
            }

            if on_rails {
                self.fall_distance.store(0.0);
                entity.update_last_pos();
                if block == &Block::ACTIVATOR_RAIL {
                    self.activate_minecart(
                        PoweredRailLikeProperties::from_state_id(state_id).powered,
                    );
                }
                let shape = rail_shape(block, state_id);
                let new_delta = self.calculate_track_speed(
                    horizontal(initial),
                    &mut iteration,
                    current_pos,
                    block,
                    state_id,
                    shape,
                );
                if iteration.first_iteration {
                    iteration.movement_left = new_delta.horizontal_length();
                } else {
                    iteration.movement_left +=
                        new_delta.horizontal_length() - initial.horizontal_length();
                }
                entity.velocity.store(new_delta);
                iteration.movement_left =
                    self.make_step_along_track(caller, current_pos, shape, iteration.movement_left);
            } else {
                self.come_off_track(caller);
                iteration.movement_left = 0.0;
            }

            let step_length = self.new_record_step(initial);
            if step_length > f64::from(SMALL) || iteration.first_iteration {
                self.apply_effects_from_blocks(caller);
                self.apply_effects_from_blocks(caller);
            }
            iteration.first_iteration = false;
        }
    }

    /// The rotation and lerp step at the end of one iteration; returns the step's length.
    fn new_record_step(&self, initial: Vector3<f64>) -> f64 {
        let entity = &self.vehicle.entity;
        let step_delta = entity.pos.load() - entity.last_pos.load();
        let step_length = step_delta.length();
        if step_length > f64::from(SMALL) {
            let on_rails = self.on_rails.load(Ordering::Relaxed);
            let on_ground = entity.on_ground.load(Ordering::SeqCst);
            if step_delta.horizontal_length_squared() > f64::from(SMALL) {
                let flipped = self.flipped.load(Ordering::Relaxed);
                let mut yaw = 180.0f32 - step_delta.z.atan2(step_delta.x).to_degrees() as f32;
                let mut pitch = if on_ground && !on_rails {
                    0.0f32
                } else {
                    90.0f32
                        - step_delta
                            .horizontal_length()
                            .atan2(step_delta.y)
                            .to_degrees() as f32
                };
                if flipped {
                    yaw += 180.0;
                    pitch *= -1.0;
                }
                self.new_set_rotation(yaw, pitch);
            } else if !on_rails {
                let pitch = entity.pitch.load();
                entity.pitch.store(if on_ground {
                    0.0
                } else {
                    pitch + 0.2 * wrap_degrees(-pitch)
                });
            }
            self.add_lerp_step(step_length.min(self.max_speed()) as f32);
        } else if initial.horizontal_length_squared() > 0.0 {
            self.add_lerp_step(1.0);
        }
        step_length
    }

    /// Vanilla `NewMinecartBehavior.calculateTrackSpeed`.
    fn calculate_track_speed(
        &self,
        delta: Vector3<f64>,
        iteration: &mut TrackIteration,
        pos: BlockPos,
        block: &Block,
        state_id: BlockStateId,
        shape: RailShape,
    ) -> Vector3<f64> {
        let mut new_delta = delta;
        if !iteration.has_gained_slope_speed {
            let sloped = self.calculate_slope_speed(delta, shape);
            if sloped.horizontal_length_squared() != delta.horizontal_length_squared() {
                iteration.has_gained_slope_speed = true;
                new_delta = sloped;
            }
        }
        if iteration.first_iteration {
            let player_input = self.calculate_player_input_speed(new_delta);
            if player_input.horizontal_length_squared() != new_delta.horizontal_length_squared() {
                iteration.has_halted = true;
                new_delta = player_input;
            }
        }
        let powered =
            block == &Block::POWERED_RAIL && PoweredRailLikeProperties::from_state_id(state_id).powered;
        if !iteration.has_halted && block == &Block::POWERED_RAIL && !powered {
            let halted = if new_delta.length() < 0.03 {
                Vector3::default()
            } else {
                new_delta * 0.5
            };
            if halted.horizontal_length_squared() != new_delta.horizontal_length_squared() {
                iteration.has_halted = true;
                new_delta = halted;
            }
        }
        if iteration.first_iteration {
            new_delta = self.apply_natural_slowdown(new_delta);
            if new_delta.length_squared() > 0.0 {
                new_delta = new_delta.normalize() * new_delta.length().min(self.max_speed());
            }
        }
        if !iteration.has_boosted && powered {
            let boosted = self.calculate_boost_track_speed(new_delta, pos);
            if boosted.horizontal_length_squared() != new_delta.horizontal_length_squared() {
                iteration.has_boosted = true;
                new_delta = boosted;
            }
        }
        new_delta
    }

    /// Vanilla `NewMinecartBehavior.calculateSlopeSpeed`.
    fn calculate_slope_speed(&self, delta: Vector3<f64>, shape: RailShape) -> Vector3<f64> {
        let mut slide_speed = (delta.horizontal_length() * 0.02).max(0.0078125);
        if self.vehicle.entity.is_in_water() {
            slide_speed *= 0.2;
        }
        match shape {
            RailShape::AscendingEast => delta.add_raw(-slide_speed, 0.0, 0.0),
            RailShape::AscendingWest => delta.add_raw(slide_speed, 0.0, 0.0),
            RailShape::AscendingNorth => delta.add_raw(0.0, 0.0, slide_speed),
            RailShape::AscendingSouth => delta.add_raw(0.0, 0.0, -slide_speed),
            _ => delta,
        }
    }

    /// Vanilla `NewMinecartBehavior.calculatePlayerInputSpeed`.
    fn calculate_player_input_speed(&self, delta: Vector3<f64>) -> Vector3<f64> {
        let Some(rider) = self.first_passenger() else {
            return delta;
        };
        let Some(player) = rider.get_player() else {
            return delta;
        };
        let move_intent = last_client_move_intent(player);
        if move_intent.length_squared() > 0.0 && delta.horizontal_length_squared() < 0.01 {
            let rider_movement = move_intent.normalize();
            return delta + horizontal(rider_movement).normalize() * 0.001;
        }
        delta
    }

    /// Vanilla `NewMinecartBehavior.calculateBoostTrackSpeed`, for a powered rail.
    fn calculate_boost_track_speed(&self, delta: Vector3<f64>, pos: BlockPos) -> Vector3<f64> {
        if delta.length() > 0.01 {
            return delta.normalize() * (delta.length() + 0.06);
        }
        let direction = self.redstone_direction(pos);
        if direction.length_squared() <= 0.0 {
            delta
        } else {
            direction * (delta.length() + 0.2)
        }
    }

    /// Vanilla `AbstractMinecart.getRedstoneDirection`, for a powered rail.
    fn redstone_direction(&self, pos: BlockPos) -> Vector3<f64> {
        let (block, state_id) = self.vehicle.entity.world.load().get_block_and_state_id(&pos);
        match rail_shape(block, state_id) {
            RailShape::EastWest => {
                if self.is_redstone_conductor(&pos.west()) {
                    return Vector3::new(1.0, 0.0, 0.0);
                }
                if self.is_redstone_conductor(&pos.east()) {
                    return Vector3::new(-1.0, 0.0, 0.0);
                }
            }
            RailShape::NorthSouth => {
                if self.is_redstone_conductor(&pos.north()) {
                    return Vector3::new(0.0, 0.0, 1.0);
                }
                if self.is_redstone_conductor(&pos.south()) {
                    return Vector3::new(0.0, 0.0, -1.0);
                }
            }
            _ => {}
        }
        Vector3::default()
    }

    /// Vanilla `AbstractMinecart.makeStepAlongTrack`, with `MinecartHopper`'s override.
    fn make_step_along_track(
        &self,
        caller: &dyn EntityBase,
        pos: BlockPos,
        shape: RailShape,
        movement_left: f64,
    ) -> f64 {
        let left = self.step_along_track(caller, pos, shape, movement_left);
        if let super::MinecartKind::Hopper(minecart) = &self.kind {
            minecart.try_consume_items(&self.vehicle.entity);
        }
        left
    }

    /// Vanilla `NewMinecartBehavior.stepAlongTrack`.
    fn step_along_track(
        &self,
        caller: &dyn EntityBase,
        pos: BlockPos,
        shape: RailShape,
        movement_left: f64,
    ) -> f64 {
        let entity = &self.vehicle.entity;
        if movement_left < f64::from(SMALL) {
            return 0.0;
        }
        let old_position = entity.pos.load();
        let (exit0, exit1) = exits(shape);
        let mut movement = horizontal(entity.velocity.load());
        if movement.length() < f64::from(SMALL) {
            entity.velocity.store(Vector3::default());
            return 0.0;
        }
        let in_hill = exit0.y != exit1.y;
        let in_direction = horizontal(exit1.to_f64() * 0.5);
        let mut out_direction = horizontal(exit0.to_f64() * 0.5);
        if movement.dot(&out_direction) < movement.dot(&in_direction) {
            out_direction = in_direction;
        }

        let mut out_position = bottom_center(pos)
            + out_direction
            + Vector3::new(0.0, ON_RAIL_Y_OFFSET, 0.0)
            + out_direction.normalize() * f64::from(SMALL);
        if in_hill && !is_descending(&movement, shape) {
            out_position = out_position + Vector3::new(0.0, 1.0, 0.0);
        }
        let towards_out = (out_position - old_position).normalize();
        movement = towards_out * (movement.length() / towards_out.horizontal_length());
        let hill_factor = if in_hill {
            f64::from(std::f32::consts::SQRT_2)
        } else {
            1.0
        };
        let next_position = old_position + movement.normalize() * (movement_left * hill_factor);
        let (new_position, left) = if old_position.squared_distance_to_vec(&out_position)
            <= old_position.squared_distance_to_vec(&next_position)
        {
            (
                out_position,
                horizontal(out_position - next_position).length(),
            )
        } else {
            (next_position, 0.0)
        };

        self.cart_move(caller, new_position - old_position);
        if in_hill && self.new_settle_on_slope(shape, new_position, out_position, movement) {
            return 0.0;
        }
        if (entity.pos.load() - old_position).length() < f64::from(SMALL)
            && (new_position - old_position).length() > f64::from(SMALL)
        {
            entity.velocity.store(Vector3::default());
            return 0.0;
        }
        entity.velocity.store(movement);
        left
    }

    /// The hill part of vanilla `stepAlongTrack`: rests the cart in a V of slopes, or else keeps
    /// it from sinking under the slope. Returns whether the cart came to rest.
    fn new_settle_on_slope(
        &self,
        shape: RailShape,
        new_position: Vector3<f64>,
        out_position: Vector3<f64>,
        movement: Vector3<f64>,
    ) -> bool {
        let entity = &self.vehicle.entity;
        let (block, state_id) = entity
            .world
            .load()
            .get_block_and_state_id(&BlockPos::containing_vec(new_position));
        if is_rail(block) && self.rest_at_v_shape(shape, rail_shape(block, state_id)) {
            return true;
        }
        let position = entity.pos.load();
        let distance_from_out = horizontal(out_position - position).length();
        let project_y = if is_descending(&movement, shape) {
            out_position.y + distance_from_out
        } else {
            out_position.y - distance_from_out
        };
        if position.y < project_y {
            entity.set_pos(Vector3::new(position.x, project_y, position.z));
        }
        false
    }

    /// Vanilla `NewMinecartBehavior.restAtVShape`.
    fn rest_at_v_shape(&self, current: RailShape, new: RailShape) -> bool {
        let entity = &self.vehicle.entity;
        let velocity = entity.velocity.load();
        if velocity.length_squared() < OPPOSING_SLOPES_REST_AT_SPEED_THRESHOLD
            && new.is_ascending()
            && is_descending(&velocity, current)
            && !is_descending(&velocity, new)
        {
            entity.velocity.store(Vector3::default());
            return true;
        }
        false
    }
}
