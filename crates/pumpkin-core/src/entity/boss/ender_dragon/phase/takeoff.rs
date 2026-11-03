//! Vanilla `DragonTakeoffPhase`: flies off the podium, away from where its head points.

use std::sync::{Mutex, MutexGuard};

use pumpkin_util::math::vector3::Vector3;
use pumpkin_world::chunk::ChunkHeightmapType;

use super::{EnderDragonPhase, Phase, next_path_target, on_ring};
use crate::entity::boss::ender_dragon::path::DragonPath;
use crate::entity::boss::ender_dragon::{EnderDragonEntity, Vector3Ext};

#[derive(Default)]
struct State {
    first_tick: bool,
    path: Option<DragonPath>,
    target: Option<Vector3<f64>>,
}

#[derive(Default)]
pub struct TakeoffPhase {
    state: Mutex<State>,
}

impl TakeoffPhase {
    fn state(&self) -> MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn find_new_target(state: &mut State, dragon: &EnderDragonEntity) {
        let current = dragon.find_closest_node();
        let look = dragon.head_look_vector();
        let end = dragon.find_closest_node_to(Vector3::new(-look.x * 40.0, 105.0, -look.z * 40.0));
        let outer = dragon.alive_crystals().unwrap_or(0) > 0;
        state.path = dragon.find_path(current, on_ring(end as i32, outer), None);
        if let Some(path) = &mut state.path {
            path.advance();
            if !path.is_done()
                && let Some(target) = next_path_target(path)
            {
                state.target = Some(target);
            }
        }
    }
}

impl Phase for TakeoffPhase {
    fn tick(&self, dragon: &EnderDragonEntity) {
        let mut state = self.state();
        if !state.first_tick && state.path.is_some() {
            drop(state);
            let egg = dragon.podium(ChunkHeightmapType::MotionBlockingNoLeaves);
            let center = egg + Vector3::new(0.0, 0.5, 0.0);
            if center.distance_squared(dragon.position()) >= 100.0 {
                dragon.set_phase(EnderDragonPhase::HoldingPattern);
            }
        } else {
            state.first_tick = false;
            Self::find_new_target(&mut state, dragon);
        }
    }

    fn begin(&self) {
        let mut state = self.state();
        state.first_tick = true;
        state.path = None;
        state.target = None;
    }

    fn fly_target(&self) -> Option<Vector3<f64>> {
        self.state().target
    }
}
