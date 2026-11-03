//! Vanilla `DragonLandingPhase`: comes down on the podium and starts looking around.

use std::sync::{Mutex, MutexGuard};

use pumpkin_util::math::vector3::Vector3;
use pumpkin_world::chunk::ChunkHeightmapType;

use super::{EnderDragonPhase, Phase};
use crate::entity::boss::ender_dragon::{EnderDragonEntity, Vector3Ext};

#[derive(Default)]
pub struct LandingPhase {
    target: Mutex<Option<Vector3<f64>>>,
}

impl LandingPhase {
    fn target(&self) -> MutexGuard<'_, Option<Vector3<f64>>> {
        self.target
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl Phase for LandingPhase {
    fn tick(&self, dragon: &EnderDragonEntity) {
        let target = *self
            .target()
            .get_or_insert_with(|| dragon.podium(ChunkHeightmapType::MotionBlockingNoLeaves));
        if target.distance_squared(dragon.position()) < 1.0 {
            dragon.phase_manager.sitting_flaming().reset_flame_count();
            dragon.set_phase(EnderDragonPhase::SittingScanning);
        }
    }

    fn begin(&self) {
        *self.target() = None;
    }

    fn fly_speed(&self) -> f32 {
        1.5
    }

    fn turn_speed(&self, dragon: &EnderDragonEntity) -> f32 {
        let rot_speed = dragon.horizontal_speed() + 1.0;
        rot_speed.min(40.0) / rot_speed
    }

    fn fly_target(&self) -> Option<Vector3<f64>> {
        *self.target()
    }
}
