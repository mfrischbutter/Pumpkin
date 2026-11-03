//! Vanilla `DragonSittingScanningPhase`: turns to a player nearby, or leaves when there is none.

use std::sync::atomic::{AtomicI32, Ordering};

use pumpkin_util::math::wrap_degrees_f64;

use super::{EnderDragonPhase, Phase};
use crate::entity::ai::target_predicate::TargetPredicate;
use crate::entity::boss::ender_dragon::{EnderDragonEntity, horizontal_aim_angle};

const SITTING_SCANNING_IDLE_TICKS: i32 = 100;
const SITTING_ATTACK_Y_VIEW_RANGE: f64 = 10.0;
const SITTING_ATTACK_VIEW_RANGE: f64 = 20.0;
const SITTING_CHARGE_VIEW_RANGE: f64 = 150.0;

#[derive(Default)]
pub struct SittingScanningPhase {
    scanning_time: AtomicI32,
}

/// Turns the head of the sitting dragon a step toward `at`.
fn turn_toward(dragon: &EnderDragonEntity, at_x: f64, at_z: f64) {
    let head = dragon.parts[crate::entity::boss::ender_dragon::part::HEAD]
        .entity
        .pos
        .load();
    let (x, z) = (at_x - head.x, at_z - head.z);
    let entity = &dragon.mob_entity.living_entity.entity;
    let yaw = entity.yaw.load();
    let delta =
        wrap_degrees_f64(180.0 - x.atan2(z).to_degrees() - f64::from(yaw)).clamp(-100.0, 100.0);
    let dist = x.hypot(z) as f32 + 1.0;
    let rate = 0.7 / dist.min(40.0) / dist;
    let accel = dragon.add_yaw_acceleration(delta as f32 * rate);
    entity.yaw.store(yaw + accel);
}

impl Phase for SittingScanningPhase {
    fn tick(&self, dragon: &EnderDragonEntity) {
        let scanning_time = self.scanning_time.fetch_add(1, Ordering::Relaxed) + 1;
        let own = dragon.position();
        let scan =
            TargetPredicate::create_attackable().set_base_max_distance(SITTING_ATTACK_VIEW_RANGE);
        let near = dragon.nearest_player(own, &scan, |player| {
            (player.living_entity.entity.pos.load().y - own.y).abs() <= SITTING_ATTACK_Y_VIEW_RANGE
        });
        if let Some(player) = near {
            if scanning_time > 25 {
                dragon.set_phase(EnderDragonPhase::SittingAttacking);
                return;
            }
            let at = player.living_entity.entity.pos.load();
            let angle = horizontal_aim_angle(dragon, at);
            if !(0.0..=10.0).contains(&angle) {
                turn_toward(dragon, at.x, at.z);
            }
        } else if scanning_time >= SITTING_SCANNING_IDLE_TICKS {
            let charge = TargetPredicate::create_attackable()
                .set_base_max_distance(SITTING_CHARGE_VIEW_RANGE);
            let target = dragon.nearest_player(own, &charge, |_| true);
            dragon.set_phase(EnderDragonPhase::Takeoff);
            if let Some(player) = target {
                dragon.set_phase(EnderDragonPhase::ChargingPlayer);
                dragon
                    .phase_manager
                    .charging_player()
                    .set_target(player.living_entity.entity.pos.load());
            }
        }
    }

    fn begin(&self) {
        self.scanning_time.store(0, Ordering::Relaxed);
    }
}
