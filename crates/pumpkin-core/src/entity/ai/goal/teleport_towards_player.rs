use std::sync::Arc;

use super::track_target::TrackTargetGoal;
use super::{Controls, Goal, to_goal_ticks};
use crate::entity::Entity;
use crate::entity::EntityBase;
use crate::entity::ai::target_predicate::TargetPredicate;
use crate::entity::mob::Mob;
use crate::entity::mob::enderman::EndermanEntity;
use crate::entity::mob::neutral::NeutralMob;
use crate::entity::player::Player;
use crate::plugin::api::events::entity::entity_target::TargetReason;
use crate::world::World;
use pumpkin_util::math::wrap_degrees;

const STARE_CLOSE_DISTANCE_SQ: f64 = 16.0;
const TELEPORT_FAR_DISTANCE_SQ: f64 = 256.0;
/// Degrees per check the enderman turns towards a player during the warm-up.
const PENDING_LOOK_MAX_TURN: f32 = 10.0;

/// Vanilla `Mob.rotlerp`: `from` turned towards `to` by at most `max_change` degrees.
fn rotlerp(from: f32, to: f32, max_change: f32) -> f32 {
    from + wrap_degrees(to - from).clamp(-max_change, max_change)
}

/// Vanilla `Mob.lookAt`: turns the body itself towards the eyes of `target`.
fn look_at(entity: &Entity, target: &Entity, max_yaw: f32, max_pitch: f32) {
    let pos = entity.pos.load();
    let target_pos = target.pos.load();
    let dx = target_pos.x - pos.x;
    let dz = target_pos.z - pos.z;
    let dy = target.get_eye_y() - entity.get_eye_y();
    let horizontal = dx.hypot(dz);
    let yaw = dz.atan2(dx).to_degrees() as f32 - 90.0;
    let pitch = -(dy.atan2(horizontal).to_degrees() as f32);
    entity.set_pitch(rotlerp(entity.pitch.load(), pitch, max_pitch));
    entity.yaw.store(rotlerp(entity.yaw.load(), yaw, max_yaw));
}

/// Vanilla `EndermanLookForPlayerGoal`: targets a staring or hated player.
///
/// The target is set after a short warm-up.
pub struct TeleportTowardsPlayerGoal {
    enderman: Arc<EndermanEntity>,
    track_target_goal: TrackTargetGoal,
    /// The player found by `can_start`, waiting for the warm-up to end.
    pending_target: Option<Arc<Player>>,
    /// The goal's own target, vanilla `NearestAttackableTargetGoal.target`.
    target: Option<Arc<dyn EntityBase>>,
    aggro_time: i32,
    teleport_time: i32,
    start_aggro_predicate: TargetPredicate,
    continue_aggro_predicate: TargetPredicate,
}

impl TeleportTowardsPlayerGoal {
    #[must_use]
    pub fn new(enderman: Arc<EndermanEntity>) -> Self {
        Self {
            enderman,
            track_target_goal: TrackTargetGoal::with_default(false),
            pending_target: None,
            target: None,
            aggro_time: 0,
            teleport_time: 0,
            start_aggro_predicate: TargetPredicate::create_attackable(),
            continue_aggro_predicate: TargetPredicate::create_attackable().ignore_visibility(),
        }
    }

    /// Vanilla `isAngerInducing`: staring or hated, and not riding the enderman.
    fn is_anger_inducing(&self, player: &Player, world: &World) -> bool {
        (self.enderman.is_player_staring(player) || self.enderman.is_angry_at(player, world))
            && !self
                .enderman
                .get_entity()
                .has_indirect_passenger(player.get_entity())
    }

    fn find_anger_inducing_player(&self) -> Option<Arc<Player>> {
        let entity = self.enderman.get_entity();
        let world = entity.world.load();
        let pos = entity.pos.load();

        // A nearer player that is not anger-inducing must not hide a farther one that is.
        // The conditions carry the follow range.
        let enderman: &dyn EntityBase = self.enderman.as_ref();
        world.get_nearest_player(pos, f64::INFINITY, |player| {
            self.start_aggro_predicate
                .test(&world, Some(enderman), player.as_ref())
                && self.is_anger_inducing(player, &world)
        })
    }

    /// The warm-up is over: the pending player becomes the target.
    fn commit_pending_target(&mut self, mob: &dyn Mob) {
        self.target = self
            .pending_target
            .take()
            .map(|player| player as Arc<dyn EntityBase>);
        // Vanilla `NearestAttackableTargetGoal.start`.
        self.track_target_goal.start_targeting(
            mob,
            self.target.clone(),
            TargetReason::ClosestPlayer,
        );
        self.track_target_goal.start(mob);
    }
}

impl Goal for TeleportTowardsPlayerGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        self.start_aggro_predicate.base_max_distance =
            self.track_target_goal.follow_range(mob.get_mob_entity());
        self.pending_target = self.find_anger_inducing_player();
        self.pending_target.is_some()
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        let entity = self.enderman.get_entity();
        let world = entity.world.load();
        if let Some(pending) = &self.pending_target {
            if !self.is_anger_inducing(pending, &world) {
                return false;
            }
            look_at(
                entity,
                pending.get_entity(),
                PENDING_LOOK_MAX_TURN,
                PENDING_LOOK_MAX_TURN,
            );
            return true;
        }

        if let Some(target) = &self.target {
            if entity.has_indirect_passenger(target.get_entity()) {
                return false;
            }
            let enderman: &dyn EntityBase = self.enderman.as_ref();
            if self
                .continue_aggro_predicate
                .test(&world, Some(enderman), target.as_ref())
            {
                return true;
            }
        }
        self.track_target_goal.should_continue(mob)
    }

    fn start(&mut self, _mob: &dyn Mob) {
        self.aggro_time = to_goal_ticks(5);
        self.teleport_time = 0;
        self.enderman.set_provoked(true);
    }

    fn tick(&mut self, mob: &dyn Mob) {
        if mob.get_mob_entity().get_target().is_none() {
            self.target = None;
        }

        if self.pending_target.is_some() {
            self.aggro_time -= 1;
            if self.aggro_time <= 0 {
                self.commit_pending_target(mob);
            }
            return;
        }

        let Some(target) = self.target.clone() else {
            return;
        };
        let entity = self.enderman.get_entity();
        if entity.has_vehicle() {
            return;
        }

        let dist_sq = entity
            .pos
            .load()
            .squared_distance_to_vec(&target.get_entity().pos.load());
        if target
            .get_player()
            .is_some_and(|player| self.enderman.is_player_staring(player))
        {
            if dist_sq < STARE_CLOSE_DISTANCE_SQ {
                self.enderman.teleport_randomly();
            }
            self.teleport_time = 0;
        } else if dist_sq > TELEPORT_FAR_DISTANCE_SQ {
            // Vanilla counts up first and resets only after a successful teleport.
            let waited = self.teleport_time >= to_goal_ticks(30);
            self.teleport_time += 1;
            if waited && self.enderman.teleport_towards(target.as_ref()) {
                self.teleport_time = 0;
            }
        }
    }

    fn stop(&mut self, mob: &dyn Mob) {
        self.pending_target = None;
        self.target = None;
        self.track_target_goal.stop(mob);
    }

    fn controls(&self) -> Controls {
        Controls::TARGET
    }
}
