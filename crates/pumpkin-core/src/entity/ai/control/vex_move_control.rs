use pumpkin_util::math::vector3::Vector3;

use super::{
    Control, MoveControlTrait,
    move_control::{MoveControl, Operation},
};
use crate::entity::mob::Mob;

/// Share of the remaining way added to the velocity per tick at speed modifier 1.
const ACCELERATION: f64 = 0.05;
/// Velocity kept when the wanted position is reached.
const ARRIVAL_DAMPING: f64 = 0.5;

/// Vanilla `Vex.VexMoveControl`: flies straight at the wanted position, ignoring blocks, and
/// keeps wanting it until it is closer than the vex's average box side.
#[derive(Default)]
pub struct VexMoveControl {
    inner: MoveControl,
}

impl Control for VexMoveControl {}

impl MoveControlTrait for VexMoveControl {
    fn tick(&mut self, mob: &dyn Mob) {
        if self.inner.operation != Operation::MoveTo {
            return;
        }
        let mob_entity = mob.get_mob_entity();
        let entity = &mob_entity.living_entity.entity;
        let pos = entity.pos.load();
        let delta = Vector3::new(
            self.inner.wanted_x - pos.x,
            self.inner.wanted_y - pos.y,
            self.inner.wanted_z - pos.z,
        );
        let delta_length = delta.length();
        if delta_length < entity.bounding_box.load().get_average_side_length() {
            self.inner.operation = Operation::Wait;
            entity
                .velocity
                .store(entity.velocity.load() * ARRIVAL_DAMPING);
            return;
        }

        let velocity = entity.velocity.load()
            + delta * (self.inner.speed_modifier * ACCELERATION / delta_length);
        entity.velocity.store(velocity);
        let (x, z) = mob_entity
            .get_target()
            .map_or((velocity.x, velocity.z), |target| {
                let target_pos = target.get_entity().pos.load();
                (target_pos.x - pos.x, target_pos.z - pos.z)
            });
        let yaw = -(x.atan2(z) as f32).to_degrees();
        entity.yaw.store(yaw);
        entity.body_yaw.store(yaw);
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
