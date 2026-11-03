//! Vanilla `OldMinecartBehavior`: how minecarts move unless `minecart_improvements` is enabled.

use std::sync::atomic::Ordering;

use pumpkin_data::Block;
use pumpkin_data::block_properties::{PoweredRailLikeProperties, RailShape};
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_util::math::wrap_degrees;

use crate::entity::EntityBase;

use super::MinecartEntity;
use super::movement::last_client_move_intent;
use super::rails::{exits, is_rail, rail_shape};

/// Vanilla `moveAlongTrack`'s slide speed on a slope.
const SLIDE_SPEED: f64 = 0.0078125;
pub(super) const MAX_SPEED_IN_WATER: f64 = 0.2;
pub(super) const MAX_SPEED_ON_LAND: f64 = 0.4;
/// Vanilla `getSlowdownFactor` with and without a passenger.
pub(super) const SLOWDOWN_WITH_PASSENGER: f64 = 0.997;
pub(super) const SLOWDOWN_EMPTY: f64 = 0.96;
/// Height of the rail's track above its block.
const TRACK_HEIGHT: f64 = 0.0625;

impl MinecartEntity {
    /// Vanilla `OldMinecartBehavior.tick`. `y_rot_o` is the yaw at the start of the tick.
    pub(super) fn old_tick(&self, caller: &dyn EntityBase, y_rot_o: f32) {
        let entity = &self.vehicle.entity;
        self.apply_gravity();
        let pos = self.current_block_pos_or_rail_below();
        let (block, state_id) = entity.world.load().get_block_and_state_id(&pos);
        let on_rails = is_rail(block);
        self.on_rails.store(on_rails, Ordering::Relaxed);
        if on_rails {
            self.old_move_along_track(caller);
            if block == &Block::ACTIVATOR_RAIL {
                self.activate_minecart(PoweredRailLikeProperties::from_state_id(state_id).powered);
            }
        } else {
            self.come_off_track(caller);
        }
        self.apply_effects_from_blocks(caller);
        self.old_update_rotation(y_rot_o);
        self.old_push_and_pickup_entities(caller);
    }

    /// The rotation part of vanilla `OldMinecartBehavior.tick`.
    fn old_update_rotation(&self, y_rot_o: f32) {
        let entity = &self.vehicle.entity;
        entity.pitch.store(0.0);
        let pos = entity.pos.load();
        let old = entity.last_pos.load();
        let x_diff = old.x - pos.x;
        let z_diff = old.z - pos.z;
        let mut yaw = entity.yaw.load();
        if x_diff * x_diff + z_diff * z_diff > 0.001 {
            yaw = z_diff.atan2(x_diff).to_degrees() as f32;
            if self.flipped.load(Ordering::Relaxed) {
                yaw += 180.0;
            }
        }
        let rot_diff = wrap_degrees(yaw - y_rot_o);
        if !(-170.0..170.0).contains(&rot_diff) {
            yaw += 180.0;
            self.flipped.fetch_not(Ordering::Relaxed);
        }
        entity.yaw.store(yaw % 360.0);
    }

    /// Vanilla `OldMinecartBehavior.moveAlongTrack`.
    fn old_move_along_track(&self, caller: &dyn EntityBase) {
        let entity = &self.vehicle.entity;
        let pos = self.current_block_pos_or_rail_below();
        let (block, state_id) = entity.world.load().get_block_and_state_id(&pos);
        self.fall_distance.store(0.0);
        let start = entity.pos.load();
        let old_pos = self.old_get_pos(start.x, start.y, start.z);
        let (power_track, halt_track) = if block == &Block::POWERED_RAIL {
            let powered = PoweredRailLikeProperties::from_state_id(state_id).powered;
            (powered, !powered)
        } else {
            (false, false)
        };

        let shape = rail_shape(block, state_id);
        let y = f64::from(pos.0.y) + if shape.is_ascending() { 1.0 } else { 0.0 };
        self.old_slide_on_slope(shape);
        let halt_track = self.old_follow_rail_direction(shape, halt_track);
        if halt_track {
            let velocity = entity.velocity.load();
            entity
                .velocity
                .store(if velocity.horizontal_length() < 0.03 {
                    Vector3::default()
                } else {
                    velocity.multiply(0.5, 0.0, 0.5)
                });
        }

        // Snap onto the rail's line, then move along it.
        let (exit0, exit1) = exits(shape);
        let x0 = f64::from(pos.0.x) + 0.5 + f64::from(exit0.x) * 0.5;
        let z0 = f64::from(pos.0.z) + 0.5 + f64::from(exit0.z) * 0.5;
        let x1 = f64::from(pos.0.x) + 0.5 + f64::from(exit1.x) * 0.5;
        let z1 = f64::from(pos.0.z) + 0.5 + f64::from(exit1.z) * 0.5;
        let x_d = x1 - x0;
        let z_d = z1 - z0;
        let progress = if x_d == 0.0 {
            start.z - f64::from(pos.0.z)
        } else if z_d == 0.0 {
            start.x - f64::from(pos.0.x)
        } else {
            ((start.x - x0) * x_d + (start.z - z0) * z_d) * 2.0
        };
        entity.set_pos(Vector3::new(x0 + x_d * progress, y, z0 + z_d * progress));

        let scale = if entity.has_passengers() { 0.75 } else { 1.0 };
        let max_speed = self.max_speed();
        let movement = entity.velocity.load();
        self.cart_move(
            caller,
            Vector3::new(
                (scale * movement.x).clamp(-max_speed, max_speed),
                0.0,
                (scale * movement.z).clamp(-max_speed, max_speed),
            ),
        );
        self.old_climb_at_exit(pos, exit0, exit1);

        entity
            .velocity
            .store(self.apply_natural_slowdown(entity.velocity.load()));
        self.old_apply_height_change(old_pos);
        self.old_turn_into_next_block(pos);
        if power_track {
            self.old_boost(pos, shape);
        }
    }

    /// The slope part of vanilla `moveAlongTrack`.
    fn old_slide_on_slope(&self, shape: RailShape) {
        let entity = &self.vehicle.entity;
        let slide_speed = if entity.is_in_water() {
            SLIDE_SPEED * 0.2
        } else {
            SLIDE_SPEED
        };
        let mut velocity = entity.velocity.load();
        match shape {
            RailShape::AscendingEast => velocity.x -= slide_speed,
            RailShape::AscendingWest => velocity.x += slide_speed,
            RailShape::AscendingNorth => velocity.z += slide_speed,
            RailShape::AscendingSouth => velocity.z -= slide_speed,
            _ => {}
        }
        entity.velocity.store(velocity);
    }

    /// Turns the movement along the rail and adds the rider's push, which also lets a ridden cart
    /// start from an unpowered powered rail. Returns whether the track still halts the cart.
    fn old_follow_rail_direction(&self, shape: RailShape, halt_track: bool) -> bool {
        let entity = &self.vehicle.entity;
        let (exit0, exit1) = exits(shape);
        let mut x_d = f64::from(exit1.x - exit0.x);
        let mut z_d = f64::from(exit1.z - exit0.z);
        let length = x_d.hypot(z_d);
        let movement = entity.velocity.load();
        if movement.x * x_d + movement.z * z_d < 0.0 {
            x_d = -x_d;
            z_d = -z_d;
        }
        let pow = movement.horizontal_length().min(2.0);
        entity.velocity.store(Vector3::new(
            pow * x_d / length,
            movement.y,
            pow * z_d / length,
        ));

        let Some(rider) = self.first_passenger() else {
            return halt_track;
        };
        let Some(player) = rider.get_player() else {
            return halt_track;
        };
        let move_intent = last_client_move_intent(player);
        let velocity = entity.velocity.load();
        if move_intent.length_squared() > 0.0 && velocity.horizontal_length_squared() < 0.01 {
            entity.velocity.store(Vector3::new(
                velocity.x + move_intent.x * 0.001,
                velocity.y,
                velocity.z + move_intent.z * 0.001,
            ));
            return false;
        }
        halt_track
    }

    /// Lifts the cart by the exit's height when it crossed into the block of a sloped exit.
    fn old_climb_at_exit(&self, pos: BlockPos, exit0: Vector3<i32>, exit1: Vector3<i32>) {
        let entity = &self.vehicle.entity;
        let now = entity.pos.load();
        let x_off = now.x.floor() as i32 - pos.0.x;
        let z_off = now.z.floor() as i32 - pos.0.z;
        for exit in [exit0, exit1] {
            if exit.y != 0 && x_off == exit.x && z_off == exit.z {
                entity.set_pos(Vector3::new(now.x, now.y + f64::from(exit.y), now.z));
                return;
            }
        }
    }

    /// Speeds the cart up by what it lost in height and puts it on the rail's height.
    fn old_apply_height_change(&self, old_pos: Option<Vector3<f64>>) {
        let entity = &self.vehicle.entity;
        let now = entity.pos.load();
        let (Some(new_pos), Some(old_pos)) = (self.old_get_pos(now.x, now.y, now.z), old_pos)
        else {
            return;
        };
        let speed = (old_pos.y - new_pos.y) * 0.05;
        let velocity = entity.velocity.load();
        let other_pow = velocity.horizontal_length();
        if other_pow > 0.0 {
            let factor = (other_pow + speed) / other_pow;
            entity
                .velocity
                .store(velocity.multiply(factor, 1.0, factor));
        }
        entity.set_pos(Vector3::new(now.x, new_pos.y, now.z));
    }

    /// Points the movement at the block the cart just crossed into.
    fn old_turn_into_next_block(&self, pos: BlockPos) {
        let entity = &self.vehicle.entity;
        let now = entity.pos.load();
        let xn = now.x.floor() as i32;
        let zn = now.z.floor() as i32;
        if xn != pos.0.x || zn != pos.0.z {
            let velocity = entity.velocity.load();
            let other_pow = velocity.horizontal_length();
            entity.velocity.store(Vector3::new(
                other_pow * f64::from(xn - pos.0.x),
                velocity.y,
                other_pow * f64::from(zn - pos.0.z),
            ));
        }
    }

    /// The `powerTrack` part of vanilla `moveAlongTrack`.
    fn old_boost(&self, pos: BlockPos, shape: RailShape) {
        const BOOST: f64 = 0.06;
        const KICK_OFF_WALL: f64 = 0.02;
        let entity = &self.vehicle.entity;
        let velocity = entity.velocity.load();
        let speed = velocity.horizontal_length();
        if speed > 0.01 {
            entity.velocity.store(Vector3::new(
                velocity.x + velocity.x / speed * BOOST,
                velocity.y,
                velocity.z + velocity.z / speed * BOOST,
            ));
            return;
        }
        let mut kicked = velocity;
        match shape {
            RailShape::EastWest => {
                if self.is_redstone_conductor(&pos.west()) {
                    kicked.x = KICK_OFF_WALL;
                } else if self.is_redstone_conductor(&pos.east()) {
                    kicked.x = -KICK_OFF_WALL;
                }
            }
            RailShape::NorthSouth => {
                if self.is_redstone_conductor(&pos.north()) {
                    kicked.z = KICK_OFF_WALL;
                } else if self.is_redstone_conductor(&pos.south()) {
                    kicked.z = -KICK_OFF_WALL;
                }
            }
            _ => return,
        }
        entity.velocity.store(kicked);
    }

    /// Vanilla `OldMinecartBehavior.getPos`: the point on the rail at this position.
    fn old_get_pos(&self, x: f64, y: f64, z: f64) -> Option<Vector3<f64>> {
        let world = self.vehicle.entity.world.load();
        let xt = x.floor() as i32;
        let mut yt = y.floor() as i32;
        let zt = z.floor() as i32;
        if is_rail(world.get_block(&BlockPos::new(xt, yt - 1, zt))) {
            yt -= 1;
        }
        let (block, state_id) = world.get_block_and_state_id(&BlockPos::new(xt, yt, zt));
        if !is_rail(block) {
            return None;
        }
        let (exit0, exit1) = exits(rail_shape(block, state_id));
        let x0 = f64::from(xt) + 0.5 + f64::from(exit0.x) * 0.5;
        let y0 = f64::from(yt) + TRACK_HEIGHT + f64::from(exit0.y) * 0.5;
        let z0 = f64::from(zt) + 0.5 + f64::from(exit0.z) * 0.5;
        let x1 = f64::from(xt) + 0.5 + f64::from(exit1.x) * 0.5;
        let y1 = f64::from(yt) + TRACK_HEIGHT + f64::from(exit1.y) * 0.5;
        let z1 = f64::from(zt) + 0.5 + f64::from(exit1.z) * 0.5;
        let x_d = x1 - x0;
        let y_d = (y1 - y0) * 2.0;
        let z_d = z1 - z0;
        let progress = if x_d == 0.0 {
            z - f64::from(zt)
        } else if z_d == 0.0 {
            x - f64::from(xt)
        } else {
            ((x - x0) * x_d + (z - z0) * z_d) * 2.0
        };
        let slope_lift = if y_d < 0.0 {
            1.0
        } else if y_d > 0.0 {
            0.5
        } else {
            0.0
        };
        Some(Vector3::new(
            x0 + x_d * progress,
            y0 + y_d * progress + slope_lift,
            z0 + z_d * progress,
        ))
    }
}
