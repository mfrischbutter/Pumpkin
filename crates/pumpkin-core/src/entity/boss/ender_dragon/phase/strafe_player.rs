//! Vanilla `DragonStrafePlayerPhase`: flies at a player and spits a fireball at them.

use std::sync::{Arc, Mutex, MutexGuard};

use pumpkin_util::math::vector3::Vector3;

use super::{EnderDragonPhase, Phase, circling_node, next_path_target, on_ring};
use crate::entity::boss::ender_dragon::path::DragonPath;
use crate::entity::boss::ender_dragon::{EnderDragonEntity, Vector3Ext, horizontal_aim_angle};
use crate::entity::player::Player;

const FIREBALL_CHARGE_AMOUNT: i32 = 5;

#[derive(Default)]
struct State {
    fireball_charge: i32,
    path: Option<DragonPath>,
    target: Option<Vector3<f64>>,
    attack_target: Option<Arc<Player>>,
    clockwise: bool,
}

#[derive(Default)]
pub struct StrafePlayerPhase {
    state: Mutex<State>,
}

/// Vanilla's height above the player to arrive at, by the distance still to fly.
fn height_offset(horizontal_dist: f64) -> f64 {
    (0.4 + horizontal_dist / 80.0 - 1.0).min(10.0)
}

impl StrafePlayerPhase {
    fn state(&self) -> MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Vanilla `setTarget`.
    pub fn set_target(&self, dragon: &EnderDragonEntity, player: &Arc<Player>) {
        let pos = player.living_entity.entity.pos.load();
        let own = dragon.position();
        let current = dragon.find_closest_node();
        let end = dragon.find_closest_node_to(pos);
        let (x, z) = (pos.x.floor(), pos.z.floor());
        let offset = height_offset((x - own.x).hypot(z - own.z));
        let final_node = Vector3::new(x as i32, (pos.y + offset).floor() as i32, z as i32);

        let mut state = self.state();
        state.attack_target = Some(player.clone());
        state.path = dragon.find_path(current, end, Some(final_node));
        if let Some(path) = &mut state.path {
            path.advance();
        }
        if let Some(target) = state.path.as_mut().and_then(next_path_target) {
            state.target = Some(target);
        }
    }

    fn find_new_target(&self, dragon: &EnderDragonEntity) {
        let mut state = self.state();
        if state.path.as_ref().is_none_or(DragonPath::is_done) {
            let current = dragon.find_closest_node();
            let target = circling_node(current, &mut state.clockwise);
            let outer = dragon.alive_crystals().unwrap_or(0) > 0;
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

    /// Charges while the player is in sight within 64 blocks, and fires when lined up.
    fn tick_fireball(&self, dragon: &EnderDragonEntity, player: &Arc<Player>) {
        let target_pos = player.living_entity.entity.pos.load();
        let in_sight = target_pos.distance_squared(dragon.position()) < 4096.0
            && dragon.sees(&player.living_entity.entity);
        let mut state = self.state();
        if !in_sight {
            state.fireball_charge = (state.fireball_charge - 1).max(0);
            return;
        }
        state.fireball_charge += 1;
        let angle = horizontal_aim_angle(dragon, target_pos);
        if state.fireball_charge >= FIREBALL_CHARGE_AMOUNT && (0.0..10.0).contains(&angle) {
            state.fireball_charge = 0;
            if let Some(path) = &mut state.path {
                path.finish();
            }
            drop(state);
            dragon.shoot_fireball(player);
            dragon.set_phase(EnderDragonPhase::HoldingPattern);
        }
    }
}

impl Phase for StrafePlayerPhase {
    fn tick(&self, dragon: &EnderDragonEntity) {
        let Some(player) = self.state().attack_target.clone() else {
            dragon.set_phase(EnderDragonPhase::HoldingPattern);
            return;
        };
        let own = dragon.position();
        let target_pos = player.living_entity.entity.pos.load();
        let dist = {
            let mut state = self.state();
            if state.path.as_ref().is_some_and(DragonPath::is_done) {
                let offset = height_offset((target_pos.x - own.x).hypot(target_pos.z - own.z));
                state.target = Some(Vector3::new(
                    target_pos.x,
                    target_pos.y + offset,
                    target_pos.z,
                ));
            }
            state
                .target
                .map_or(0.0, |target| target.distance_squared(own))
        };
        if !(100.0..=22500.0).contains(&dist) {
            self.find_new_target(dragon);
        }
        self.tick_fireball(dragon, &player);
    }

    fn begin(&self) {
        let mut state = self.state();
        state.fireball_charge = 0;
        state.target = None;
        state.path = None;
        state.attack_target = None;
    }

    fn fly_target(&self) -> Option<Vector3<f64>> {
        self.state().target
    }
}
