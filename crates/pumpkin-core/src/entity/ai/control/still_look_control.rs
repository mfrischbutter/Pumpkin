use pumpkin_util::math::vector3::Vector3;
use pumpkin_util::math::wrap_degrees;

use super::look_control::LookControl;
use super::{Control, LookControlTrait};
use crate::entity::mob::Mob;
use crate::entity::mob::ghast::face_movement_direction;

/// Vanilla `HappyGhast.HappyGhastLookControl`: faces straight along an axis while it holds still
/// for the players on it, else faces what it looks at or where it flies.
pub struct StillLookControl {
    inner: LookControl,
    holds_still: fn(&dyn Mob) -> bool,
}

impl StillLookControl {
    #[must_use]
    pub fn new(holds_still: fn(&dyn Mob) -> bool) -> Self {
        Self {
            inner: LookControl::default(),
            holds_still,
        }
    }
}

impl Control for StillLookControl {}

/// Vanilla `Mth.wrapDegrees90`.
fn wrap_degrees_90(angle: f32) -> f32 {
    let mut wrapped = angle % 90.0;
    if wrapped >= 45.0 {
        wrapped -= 90.0;
    }
    if wrapped < -45.0 {
        wrapped += 90.0;
    }
    wrapped
}

impl LookControlTrait for StillLookControl {
    fn tick(&mut self, mob: &dyn Mob) {
        let entity = mob.get_entity();
        if (self.holds_still)(mob) {
            let yaw = entity.yaw.load();
            let straight = yaw - wrap_degrees_90(yaw);
            entity.yaw.store(straight);
            entity.head_yaw.store(straight);
        } else if self.inner.look_at_timer > 0 {
            self.inner.look_at_timer -= 1;
            let pos = entity.pos.load();
            let wanted = self.inner.position;
            let yaw =
                wrap_degrees(-((wanted.x - pos.x).atan2(wanted.z - pos.z) as f32).to_degrees());
            entity.yaw.store(yaw);
            entity.body_yaw.store(yaw);
            entity.head_yaw.store(yaw);
        } else {
            face_movement_direction(mob);
        }
    }

    fn look_at_with_range(
        &mut self,
        x: f64,
        y: f64,
        z: f64,
        max_yaw_change: f32,
        max_pitch_change: f32,
    ) {
        self.inner
            .look_at_with_range(x, y, z, max_yaw_change, max_pitch_change);
    }

    fn wanted_position(&self) -> Vector3<f64> {
        self.inner.position
    }

    fn is_looking_at_target(&self) -> bool {
        self.inner.look_at_timer > 0
    }
}
