//! Vanilla `DragonHoldingPatternPhase`: circles the ring until it lands or strafes someone.

use std::sync::{Arc, Mutex, MutexGuard};

use pumpkin_util::math::vector3::Vector3;
use pumpkin_world::chunk::ChunkHeightmapType;

use super::{EnderDragonPhase, Phase, circling_node, next_path_target, on_ring};
use crate::entity::ai::target_predicate::TargetPredicate;
use crate::entity::boss::ender_dragon::path::DragonPath;
use crate::entity::boss::ender_dragon::{EnderDragonEntity, Vector3Ext};
use crate::entity::player::Player;

#[derive(Default)]
struct State {
    path: Option<DragonPath>,
    target: Option<Vector3<f64>>,
    clockwise: bool,
}

#[derive(Default)]
pub struct HoldingPatternPhase {
    state: Mutex<State>,
}

impl HoldingPatternPhase {
    fn state(&self) -> MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Lands or strafes someone once a lap is done; `true` when the phase changed.
    fn try_leave(dragon: &EnderDragonEntity) -> bool {
        let egg = dragon.podium(ChunkHeightmapType::MotionBlockingNoLeaves);
        let crystals = dragon.alive_crystals().unwrap_or(0);
        if rand::random_range(0..crystals + 3) == 0 {
            dragon.set_phase(EnderDragonPhase::LandingApproach);
            return true;
        }
        let targeting = TargetPredicate::create_attackable().ignore_visibility();
        let Some(player) = dragon.nearest_player(egg, &targeting, |_| true) else {
            return false;
        };
        let dist = (egg + Vector3::new(0.0, 0.5, 0.0))
            .distance_squared(player.living_entity.entity.pos.load())
            / 512.0;
        if rand::random_range(0..(dist + 2.0) as i32) == 0
            || rand::random_range(0..crystals + 2) == 0
        {
            dragon.strafe(&player);
            return true;
        }
        false
    }

    fn find_new_target(&self, dragon: &EnderDragonEntity) {
        let lap_done = self.state().path.as_ref().is_some_and(DragonPath::is_done);
        if lap_done && Self::try_leave(dragon) {
            return;
        }

        let mut state = self.state();
        if state.path.as_ref().is_none_or(DragonPath::is_done) {
            let current = dragon.find_closest_node();
            let target = circling_node(current, &mut state.clockwise);
            // Vanilla tests for at least no crystals here, so the outer ring is always the goal.
            let outer = dragon.alive_crystals().is_some();
            let mut path = dragon.find_path(current, on_ring(target, outer), None);
            if let Some(path) = &mut path {
                path.advance();
            }
            state.path = path;
        }
        if let Some(target) = state.path.as_mut().and_then(next_path_target) {
            state.target = Some(target);
        }
    }
}

impl Phase for HoldingPatternPhase {
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

    fn on_crystal_destroyed(&self, dragon: &EnderDragonEntity, player: Option<&Arc<Player>>) {
        if let Some(player) = player
            && dragon.can_attack_player(player)
        {
            dragon.strafe(player);
        }
    }
}
