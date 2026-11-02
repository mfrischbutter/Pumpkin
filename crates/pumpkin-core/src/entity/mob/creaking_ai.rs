use std::sync::Arc;

use pumpkin_data::environment_attribute::Activity;

use crate::entity::EntityBase;
use crate::entity::ai::brain::activity::ActivityData;
use crate::entity::ai::brain::behavior::{
    DoNothing, LookAtTargetSink, MoveToTargetSink, Swim, Timed, melee_attack, random_stroll,
    run_one, set_entity_look_target_sometimes, set_walk_target_from_attack_target,
    set_walk_target_from_look_target, start_attacking, stop_attacking_if,
};
use crate::entity::ai::brain::memory::{MemoryStatus, types};
use crate::entity::ai::brain::{BrainState, BrainTick};
use crate::entity::mob::Mob;

use super::creaking::{ATTACK_INTERVAL, CreakingEntity, SPEED_MULTIPLIER_WHEN_IDLING};

fn as_creaking(mob: &dyn Mob) -> Option<&CreakingEntity> {
    mob.cast_any().downcast_ref::<CreakingEntity>()
}

/// Vanilla's `StartAttacking` of the idle activity: only an active one picks a player.
fn nearest_player_if_active(tick: &BrainTick<'_>) -> Option<Arc<dyn EntityBase>> {
    if !as_creaking(tick.mob).is_some_and(CreakingEntity::is_active) {
        return None;
    }
    tick.state
        .get(types::NEAREST_VISIBLE_ATTACKABLE_PLAYER)
        .map(|player| player.clone() as Arc<dyn EntityBase>)
}

/// Vanilla `CreakingAi.isAttackTargetStillReachable`.
fn is_target_still_reachable(tick: &BrainTick<'_>, target: &dyn EntityBase) -> bool {
    let id = target.get_entity().entity_id;
    tick.state
        .get(types::NEAREST_VISIBLE_ATTACKABLE_PLAYERS)
        .is_some_and(|players| {
            players
                .iter()
                .any(|player| player.get_entity().entity_id == id)
        })
}

/// Vanilla `CreakingAi.getActivities`. A frozen creaking does not run its brain at all, which
/// stands in for vanilla's `canMove` checks in the swim, the melee and the controls.
pub(super) fn activities(_mob: &dyn Mob) -> Vec<ActivityData> {
    vec![
        ActivityData::with_priority_start(
            Activity::Core,
            0,
            vec![
                Box::new(Timed::new(Swim::new(0.8))),
                Box::new(Timed::new(LookAtTargetSink::new(45, 90))),
                Box::new(Timed::new(MoveToTargetSink::default())),
            ],
        ),
        ActivityData::with_priority_start(
            Activity::Idle,
            10,
            vec![
                Box::new(start_attacking(nearest_player_if_active)),
                Box::new(set_entity_look_target_sometimes::create(8.0, 30, 60)),
                Box::new(run_one(vec![
                    (
                        Box::new(random_stroll::stroll(SPEED_MULTIPLIER_WHEN_IDLING)),
                        2,
                    ),
                    (
                        Box::new(set_walk_target_from_look_target::create(
                            SPEED_MULTIPLIER_WHEN_IDLING,
                            3,
                        )),
                        2,
                    ),
                    (Box::new(DoNothing::new(30, 60)), 1),
                ])),
            ],
        ),
        ActivityData::new(
            Activity::Fight,
            vec![
                (10, Box::new(set_walk_target_from_attack_target(|_| 1.0))),
                (11, Box::new(melee_attack(ATTACK_INTERVAL))),
                (
                    12,
                    Box::new(stop_attacking_if(
                        |tick, target| !is_target_still_reachable(tick, target),
                        |_, _| {},
                    )),
                ),
            ],
            vec![(types::ATTACK_TARGET.id(), MemoryStatus::ValuePresent)],
            Vec::new(),
        ),
    ]
}

/// Vanilla `CreakingAi.updateActivity`, for a creaking that can move.
pub(super) fn update_activity(state: &mut BrainState) {
    state.set_active_activity_to_first_valid(&[Activity::Fight, Activity::Idle]);
}
