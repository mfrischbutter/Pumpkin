//! Vanilla `DragonHoverPhase`: what a summoned dragon does, staying where it is.

use std::sync::{Mutex, MutexGuard};

use pumpkin_util::math::vector3::Vector3;

use super::Phase;
use crate::entity::boss::ender_dragon::EnderDragonEntity;

#[derive(Default)]
pub struct HoverPhase {
    target: Mutex<Option<Vector3<f64>>>,
}

impl HoverPhase {
    fn target(&self) -> MutexGuard<'_, Option<Vector3<f64>>> {
        self.target
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl Phase for HoverPhase {
    fn tick(&self, dragon: &EnderDragonEntity) {
        self.target().get_or_insert_with(|| dragon.position());
    }

    fn begin(&self) {
        *self.target() = None;
    }

    fn fly_speed(&self) -> f32 {
        1.0
    }

    fn fly_target(&self) -> Option<Vector3<f64>> {
        *self.target()
    }
}
