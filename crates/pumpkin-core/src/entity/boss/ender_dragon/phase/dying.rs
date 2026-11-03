//! Vanilla `DragonDeathPhase`: flies to the podium at 1 health and dies when it gets there.

use std::sync::{Mutex, MutexGuard};

use pumpkin_util::math::vector3::Vector3;
use pumpkin_world::chunk::ChunkHeightmapType;

use super::Phase;
use crate::entity::boss::ender_dragon::{EnderDragonEntity, Vector3Ext};

#[derive(Default)]
pub struct DyingPhase {
    target: Mutex<Option<Vector3<f64>>>,
}

impl DyingPhase {
    fn target(&self) -> MutexGuard<'_, Option<Vector3<f64>>> {
        self.target
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl Phase for DyingPhase {
    fn tick(&self, dragon: &EnderDragonEntity) {
        let target = *self
            .target()
            .get_or_insert_with(|| dragon.podium(ChunkHeightmapType::MotionBlocking));
        let dist = target.distance_squared(dragon.position());
        let living = &dragon.mob_entity.living_entity;
        living.set_health(if (100.0..=22500.0).contains(&dist) {
            1.0
        } else {
            0.0
        });
    }

    fn begin(&self) {
        *self.target() = None;
    }

    fn fly_speed(&self) -> f32 {
        3.0
    }

    fn fly_target(&self) -> Option<Vector3<f64>> {
        *self.target()
    }
}
