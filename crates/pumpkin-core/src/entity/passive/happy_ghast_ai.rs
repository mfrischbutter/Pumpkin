use std::sync::Arc;

use pumpkin_data::environment_attribute::Activity;

use crate::entity::EntityBase;
use crate::entity::ai::brain::BrainState;
use crate::entity::ai::brain::activity::ActivityData;
use crate::entity::ai::brain::behavior::animal::baby_follow_in_air;
use crate::entity::ai::brain::behavior::{
    AnimalPanic, CountDownCooldownTicks, FollowTemptation, LookAtTargetSink, MoveToTargetSink,
    Swim, Timed, random_stroll, run_one, set_walk_target_from_look_target,
};
use crate::entity::ai::brain::memory::{MemoryStatus, types};
use crate::entity::mob::Mob;

/// Vanilla `HappyGhastAi.SPEED_MULTIPLIER_WHEN_FOLLOWING_ADULT`.
const FOLLOW_SPEED: f32 = 1.1;
/// Vanilla `HappyGhastAi.ADULT_FOLLOW_RANGE`.
const FOLLOW_MIN: i32 = 3;
const FOLLOW_MAX: i32 = 16;

fn nearest_player(state: &BrainState) -> Option<Arc<dyn EntityBase>> {
    state
        .get(types::NEAREST_VISIBLE_PLAYER)
        .map(|player| player.clone() as Arc<dyn EntityBase>)
}

fn nearest_adult(state: &BrainState) -> Option<Arc<dyn EntityBase>> {
    state.get(types::NEAREST_VISIBLE_ADULT).cloned()
}

/// Vanilla `HappyGhastAi.getActivities`. Only a baby ticks this brain.
pub(super) fn activities(_mob: &dyn Mob) -> Vec<ActivityData> {
    vec![
        ActivityData::with_priority_start(
            Activity::Core,
            0,
            vec![
                Box::new(Timed::new(Swim::new(0.8))),
                Box::new(Timed::new(AnimalPanic::new(2.0).flying(0))),
                Box::new(Timed::new(LookAtTargetSink::new(45, 90))),
                Box::new(Timed::new(MoveToTargetSink::default())),
                Box::new(CountDownCooldownTicks::new(
                    types::TEMPTATION_COOLDOWN_TICKS,
                )),
            ],
        ),
        ActivityData::with_pairs(
            Activity::Idle,
            vec![
                (
                    1,
                    Box::new(Timed::new(
                        FollowTemptation::new(|_| 1.25, |_| 3.0).in_the_eyes(),
                    )),
                ),
                (
                    2,
                    Box::new(baby_follow_in_air(
                        types::NEAREST_VISIBLE_PLAYER.id(),
                        nearest_player,
                        FOLLOW_MIN,
                        FOLLOW_MAX,
                        FOLLOW_SPEED,
                    )),
                ),
                (
                    3,
                    Box::new(baby_follow_in_air(
                        types::NEAREST_VISIBLE_ADULT.id(),
                        nearest_adult,
                        FOLLOW_MIN,
                        FOLLOW_MAX,
                        FOLLOW_SPEED,
                    )),
                ),
                (
                    4,
                    Box::new(run_one(vec![
                        (Box::new(random_stroll::fly(1.0)), 1),
                        (
                            Box::new(set_walk_target_from_look_target::create(1.0, 3)),
                            1,
                        ),
                    ])),
                ),
            ],
        ),
        ActivityData::with_pairs_and_conditions(
            Activity::Panic,
            Vec::new(),
            vec![(types::IS_PANICKING.id(), MemoryStatus::ValuePresent)],
        ),
    ]
}

/// Vanilla `HappyGhastAi.updateActivity`.
pub(super) fn update_activity(state: &mut BrainState) {
    state.set_active_activity_to_first_valid(&[Activity::Panic, Activity::Idle]);
}
