//! Vanilla `DragonLandingApproachPhase`: flies round to the side away from the player and in.

use std::sync::{Mutex, MutexGuard};

use pumpkin_util::math::vector3::Vector3;
use pumpkin_world::chunk::ChunkHeightmapType;

use super::{EnderDragonPhase, Phase, next_path_target};
use crate::entity::ai::target_predicate::TargetPredicate;
use crate::entity::boss::ender_dragon::path::DragonPath;
use crate::entity::boss::ender_dragon::{EnderDragonEntity, Vector3Ext};

#[derive(Default)]
struct State {
    path: Option<DragonPath>,
    target: Option<Vector3<f64>>,
}

#[derive(Default)]
pub struct LandingApproachPhase {
    state: Mutex<State>,
}

impl LandingApproachPhase {
    fn state(&self) -> MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn new_path(dragon: &EnderDragonEntity) -> Option<DragonPath> {
        let current = dragon.find_closest_node();
        let egg = dragon.podium(ChunkHeightmapType::MotionBlockingNoLeaves);
        let targeting = TargetPredicate::create_attackable().ignore_visibility();
        let aim = dragon.nearest_player(egg, &targeting, |_| true).map_or(
            Vector3::new(40.0, egg.y, 0.0),
            |player| {
                let at = player.living_entity.entity.pos.load();
                let length = at.x.hypot(at.z).max(1.0E-4);
                Vector3::new(-at.x / length * 40.0, 105.0, -at.z / length * 40.0)
            },
        );
        let end = dragon.find_closest_node_to(aim);
        let final_node = Vector3::new(
            egg.x.floor() as i32,
            egg.y.floor() as i32,
            egg.z.floor() as i32,
        );
        let mut path = dragon.find_path(current, end, Some(final_node));
        if let Some(path) = &mut path {
            path.advance();
        }
        path
    }

    fn find_new_target(&self, dragon: &EnderDragonEntity) {
        let mut state = self.state();
        if state.path.as_ref().is_none_or(DragonPath::is_done) {
            state.path = Self::new_path(dragon);
        }
        if let Some(target) = state.path.as_mut().and_then(next_path_target) {
            state.target = Some(target);
        }
        let done = state.path.as_ref().is_some_and(DragonPath::is_done);
        drop(state);
        if done {
            dragon.set_phase(EnderDragonPhase::Landing);
        }
    }
}

impl Phase for LandingApproachPhase {
    fn tick(&self, dragon: &EnderDragonEntity) {
        let dist = self
            .state()
            .target
            .map_or(0.0, |target| target.distance_squared(dragon.position()));
        if !(100.0..=22500.0).contains(&dist) {
            self.find_new_target(dragon);
        }
    }

    fn begin(&self) {
        let mut state = self.state();
        state.path = None;
        state.target = None;
    }

    fn fly_target(&self) -> Option<Vector3<f64>> {
        self.state().target
    }
}
