use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use pumpkin_data::attributes::Attributes;
use pumpkin_data::tracked_data;

use super::{
    Control, MoveControlTrait,
    move_control::{MoveControl, Operation},
};
use crate::entity::mob::Mob;

const SPEED_LERP: f32 = 0.125;
const LOOK_LERP: f64 = 0.125;

/// Vanilla `Guardian.GuardianMoveControl`: swims straight at the waypoint with a wobble and
/// looks two blocks ahead. `moving` is the guardian's synced `DATA_ID_MOVING` flag.
pub struct GuardianMoveControl {
    inner: MoveControl,
    moving: Arc<AtomicBool>,
}

impl GuardianMoveControl {
    #[must_use]
    pub fn new(moving: Arc<AtomicBool>) -> Self {
        Self {
            inner: MoveControl::default(),
            moving,
        }
    }

    fn set_moving(&self, mob: &dyn Mob, moving: bool) {
        if self.moving.swap(moving, Ordering::Relaxed) != moving {
            mob.get_entity()
                .set_synced_data(tracked_data::guardian::DATA_ID_MOVING, moving);
        }
    }
}

impl Control for GuardianMoveControl {}

impl MoveControlTrait for GuardianMoveControl {
    fn tick(&mut self, mob: &dyn Mob) {
        let mob_entity = mob.get_mob_entity();
        let living = &mob_entity.living_entity;
        let entity = &living.entity;
        if self.inner.operation != Operation::MoveTo || mob_entity.steered().navigator().is_done() {
            mob_entity.set_speed(0.0);
            self.set_moving(mob, false);
            return;
        }

        let pos = entity.pos.load();
        let dx = self.inner.wanted_x - pos.x;
        let dy = self.inner.wanted_y - pos.y;
        let dz = self.inner.wanted_z - pos.z;
        let length = (dx * dx + dy * dy + dz * dz).sqrt();
        let (xd, yd, zd) = (dx / length, dy / length, dz / length);

        let yaw = MoveControl::rotlerp(
            entity.yaw.load(),
            dz.atan2(dx).to_degrees() as f32 - 90.0,
            MoveControl::MAX_TURN,
        );
        entity.yaw.store(yaw);
        entity.body_yaw.store(yaw);

        let target_speed = (self.inner.speed_modifier
            * living.get_attribute_value(&Attributes::MOVEMENT_SPEED))
            as f32;
        let speed = living.get_speed() + SPEED_LERP * (target_speed - living.get_speed());
        mob_entity.set_speed(speed);

        // The wobble follows the tick count, offset per guardian by its id.
        let wobble = f64::from(entity.tick_count.load(Ordering::Relaxed) + entity.entity_id);
        let push = (wobble * 0.5).sin() * 0.05;
        let (sin, cos) = f64::from(yaw.to_radians()).sin_cos();
        let y_push = (wobble * 0.75).sin() * 0.05;
        entity.velocity.store(entity.velocity.load().add_raw(
            push * cos,
            y_push * (sin + cos) * 0.25 + f64::from(speed) * yd * 0.1,
            push * sin,
        ));

        let new_x = pos.x + xd * 2.0;
        let new_y = entity.get_eye_y() + yd / length;
        let new_z = pos.z + zd * 2.0;
        let mut look_control = mob_entity
            .look_control
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let (old_x, old_y, old_z) = if look_control.is_looking_at_target() {
            let old = look_control.wanted_position();
            (old.x, old.y, old.z)
        } else {
            (new_x, new_y, new_z)
        };
        look_control.look_at_with_range(
            old_x + LOOK_LERP * (new_x - old_x),
            old_y + LOOK_LERP * (new_y - old_y),
            old_z + LOOK_LERP * (new_z - old_z),
            10.0,
            40.0,
        );
        drop(look_control);
        self.set_moving(mob, true);
    }

    fn set_wanted_position(&mut self, x: f64, y: f64, z: f64, speed_modifier: f64) {
        self.inner.set_wanted_position(x, y, z, speed_modifier);
    }

    fn strafe(&mut self, forwards: f32, right: f32) {
        self.inner.strafe(forwards, right);
    }

    fn has_wanted(&self) -> bool {
        self.inner.has_wanted()
    }
}
