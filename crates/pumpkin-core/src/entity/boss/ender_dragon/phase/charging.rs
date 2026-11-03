//! Vanilla `DragonChargePlayerPhase`: rushes at where a player stood.

use std::sync::{Mutex, MutexGuard};

use pumpkin_util::math::vector3::Vector3;

use super::{EnderDragonPhase, Phase};
use crate::entity::boss::ender_dragon::{EnderDragonEntity, Vector3Ext};

const CHARGE_RECOVERY_TIME: i32 = 10;

#[derive(Default)]
struct State {
    target: Option<Vector3<f64>>,
    time_since_charge: i32,
}

#[derive(Default)]
pub struct ChargingPhase {
    state: Mutex<State>,
}

impl ChargingPhase {
    fn state(&self) -> MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub fn set_target(&self, target: Vector3<f64>) {
        self.state().target = Some(target);
    }
}

impl Phase for ChargingPhase {
    fn tick(&self, dragon: &EnderDragonEntity) {
        let mut state = self.state();
        let Some(target) = state.target else {
            drop(state);
            dragon.set_phase(EnderDragonPhase::HoldingPattern);
            return;
        };
        if state.time_since_charge > 0 {
            state.time_since_charge += 1;
            if state.time_since_charge > CHARGE_RECOVERY_TIME {
                drop(state);
                dragon.set_phase(EnderDragonPhase::HoldingPattern);
            }
            return;
        }
        if !(100.0..=22500.0).contains(&target.distance_squared(dragon.position())) {
            state.time_since_charge += 1;
        }
    }

    fn begin(&self) {
        let mut state = self.state();
        state.target = None;
        state.time_since_charge = 0;
    }

    fn fly_speed(&self) -> f32 {
        3.0
    }

    fn fly_target(&self) -> Option<Vector3<f64>> {
        self.state().target
    }
}
