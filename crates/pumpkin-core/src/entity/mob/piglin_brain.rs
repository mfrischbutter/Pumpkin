//! Vanilla `PiglinAi` and `PiglinBruteAi`: the activities and the behaviors only piglins use.

use std::sync::Arc;

use pumpkin_data::entity::EntityType;
use pumpkin_data::environment_attribute::Activity;

use super::piglin::PiglinEntity;
use super::piglin_ai::PiglinAi;
use super::piglin_anger::{
    apply_queued_changes, as_piglin, brute_find_attack_target,
    dont_kill_any_more_hoglins_for_a_while, find_nearest_valid_attack_target,
    find_target_when_adult, hoglins_outnumber_piglins, is_kind, is_near_zombified, same_entity,
    stop_attacking_if_not_nearest,
};
use super::piglin_behaviors::{
    CrossbowAttack, avoid_when, baby_sometimes_ride_baby_hoglin, back_up_if_too_close,
    dismount_or_skip_mounting, mount, remember_if_hoglin_was_killed, start_admiring_item_if_seen,
    start_celebrating_if_target_dead, start_hunting_hoglin, stop_admiring_if_item_too_far_away,
    stop_admiring_if_tired_of_trying_to_reach_item, stop_attacking_near_zombified,
    stop_being_angry_if_target_dead, stop_holding_item_if_no_longer_admiring,
};
use crate::entity::EntityBase;
use crate::entity::ai::brain::activity::ActivityData;
use crate::entity::ai::brain::behavior::poi::{stroll_around_poi, stroll_to_poi};
use crate::entity::ai::brain::behavior::target_location::go_to_target_location;
use crate::entity::ai::brain::behavior::village::{
    interact_with, set_entity_look_target, set_look_and_interact, set_walk_target_away_from_entity,
    set_walk_target_away_from_pos,
};
use crate::entity::ai::brain::behavior::{
    DoNothing, GateBehavior, LookAtTargetSink, MoveToTargetSink, OneShot, Timed, erase_memory_if,
    go_to_wanted_item, melee_attack, random_stroll, run_one, set_walk_target_from_attack_target,
    set_walk_target_from_look_target, start_attacking,
};
use crate::entity::ai::brain::memory::types;
use crate::entity::ai::brain::{BrainState, BrainTick};
use crate::entity::mob::Mob;

const ADMIRE_DURATION: i64 = 119;
const MAX_DISTANCE_TO_WALK_TO_ITEM: i32 = 9;
const MELEE_ATTACK_COOLDOWN: i64 = 20;
const EAT_COOLDOWN: i64 = 200;
const DESIRED_DISTANCE_WHEN_AVOIDING: i32 = 12;
const HOME_CLOSE_ENOUGH_DISTANCE: i32 = 2;
const HOME_TOO_FAR_DISTANCE: i32 = 100;
const HOME_STROLL_AROUND_DISTANCE: i32 = 5;

fn is_player_holding_loved_item(entity: &Arc<dyn EntityBase>) -> bool {
    entity
        .get_player()
        .is_some_and(PiglinAi::is_player_holding_loved_item)
}

fn is_player(entity: &Arc<dyn EntityBase>) -> bool {
    is_kind(entity.as_ref(), &EntityType::PLAYER)
}

fn is_piglin(entity: &Arc<dyn EntityBase>) -> bool {
    is_kind(entity.as_ref(), &EntityType::PIGLIN)
}

fn is_brute(entity: &Arc<dyn EntityBase>) -> bool {
    is_kind(entity.as_ref(), &EntityType::PIGLIN_BRUTE)
}

fn anything(_entity: &Arc<dyn EntityBase>) -> bool {
    true
}

fn idle_look_behaviors() -> GateBehavior {
    run_one(vec![
        (Box::new(set_entity_look_target(is_player, 8.0)), 1),
        (Box::new(set_entity_look_target(is_piglin, 8.0)), 1),
        (Box::new(set_entity_look_target(anything, 8.0)), 1),
        (Box::new(DoNothing::new(30, 60)), 1),
    ])
}

fn doesnt_see_player_holding_loved_item(tick: &BrainTick<'_>) -> bool {
    !tick
        .state
        .has_memory_value(types::NEAREST_PLAYER_HOLDING_WANTED_ITEM.id())
}

fn walk_with_kind(kind: &'static EntityType, filter: fn(&Arc<dyn EntityBase>) -> bool) -> OneShot {
    interact_with(kind, 8, |_| true, filter, types::INTERACTION_TARGET, 0.6, 2)
}

fn idle_movement_behaviors() -> GateBehavior {
    run_one(vec![
        (Box::new(random_stroll::stroll(0.6)), 2),
        (Box::new(walk_with_kind(&EntityType::PIGLIN, anything)), 2),
        (
            Box::new(set_walk_target_from_look_target::create_if(
                doesnt_see_player_holding_loved_item,
                |_| 0.6,
                3,
            )),
            2,
        ),
        (Box::new(DoNothing::new(30, 60)), 1),
    ])
}

fn avoid_repellent() -> OneShot {
    set_walk_target_away_from_pos(types::NEAREST_REPELLENT, 1.0, 8)
}

fn is_dancing(mob: &dyn crate::entity::mob::Mob) -> bool {
    as_piglin(mob).is_some_and(PiglinEntity::is_dancing)
}

fn not_holding_loved_item(mob: &dyn crate::entity::mob::Mob) -> bool {
    as_piglin(mob).is_some_and(PiglinAi::is_not_holding_loved_item_in_off_hand)
}

fn wants_to_stop_fleeing(state: &BrainState) -> bool {
    let Some(avoid) = state.get(types::AVOID_TARGET) else {
        return true;
    };
    if is_kind(avoid.as_ref(), &EntityType::HOGLIN) {
        return !hoglins_outnumber_piglins(state);
    }
    if PiglinAi::is_zombified(avoid.get_entity().entity_type) {
        return state
            .get(types::NEAREST_VISIBLE_ZOMBIFIED)
            .is_none_or(|zombified| !same_entity(zombified.as_ref(), avoid.as_ref()));
    }
    false
}

/// Vanilla `PiglinAi.getActivities`.
pub(super) fn piglin_activities(_mob: &dyn Mob) -> Vec<ActivityData> {
    vec![
        piglin_core_activity(),
        piglin_idle_activity(),
        piglin_admire_activity(),
        piglin_fight_activity(),
        piglin_celebrate_activity(),
        piglin_retreat_activity(),
        piglin_ride_activity(),
    ]
}

fn piglin_core_activity() -> ActivityData {
    ActivityData::with_priority_start(
        Activity::Core,
        0,
        vec![
            Box::new(apply_queued_changes()),
            Box::new(Timed::new(LookAtTargetSink::new(45, 90))),
            Box::new(Timed::new(MoveToTargetSink::default())),
            Box::new(avoid_when(types::NEAREST_VISIBLE_NEMESIS, |tick| {
                as_piglin(tick.mob).is_some_and(PiglinEntity::is_baby)
            })),
            Box::new(avoid_when(
                types::NEAREST_VISIBLE_ZOMBIFIED,
                is_near_zombified,
            )),
            Box::new(stop_holding_item_if_no_longer_admiring()),
            Box::new(start_admiring_item_if_seen()),
            Box::new(start_celebrating_if_target_dead()),
            Box::new(stop_being_angry_if_target_dead()),
        ],
    )
}

fn piglin_idle_activity() -> ActivityData {
    ActivityData::with_priority_start(
        Activity::Idle,
        10,
        vec![
            Box::new(set_entity_look_target(is_player_holding_loved_item, 14.0)),
            Box::new(start_attacking(find_target_when_adult)),
            Box::new(start_hunting_hoglin()),
            Box::new(avoid_repellent()),
            Box::new(baby_sometimes_ride_baby_hoglin()),
            Box::new(idle_look_behaviors()),
            Box::new(idle_movement_behaviors()),
            Box::new(set_look_and_interact(&EntityType::PLAYER, 4)),
        ],
    )
}

fn piglin_admire_activity() -> ActivityData {
    ActivityData::with_gate_memory(
        Activity::AdmireItem,
        10,
        vec![
            Box::new(
                go_to_wanted_item::create(1.0, true, MAX_DISTANCE_TO_WALK_TO_ITEM)
                    .only_if(not_holding_loved_item),
            ),
            Box::new(stop_admiring_if_item_too_far_away()),
            Box::new(stop_admiring_if_tired_of_trying_to_reach_item()),
        ],
        types::ADMIRING_ITEM.id(),
    )
}

fn piglin_fight_activity() -> ActivityData {
    ActivityData::with_gate_memory(
        Activity::Fight,
        10,
        vec![
            Box::new(stop_attacking_if_not_nearest(
                find_nearest_valid_attack_target,
            )),
            Box::new(back_up_if_too_close()),
            Box::new(set_walk_target_from_attack_target(|_| 1.0)),
            Box::new(melee_attack(MELEE_ATTACK_COOLDOWN)),
            Box::new(Timed::new(CrossbowAttack::new())),
            Box::new(remember_if_hoglin_was_killed()),
            Box::new(stop_attacking_near_zombified()),
        ],
        types::ATTACK_TARGET.id(),
    )
}

fn piglin_celebrate_activity() -> ActivityData {
    ActivityData::with_gate_memory(
        Activity::Celebrate,
        10,
        vec![
            Box::new(avoid_repellent()),
            Box::new(set_entity_look_target(is_player_holding_loved_item, 14.0)),
            Box::new(start_attacking(find_target_when_adult)),
            Box::new(
                go_to_target_location(types::CELEBRATE_LOCATION, 2, 1.0)
                    .only_if(|mob| !is_dancing(mob)),
            ),
            Box::new(go_to_target_location(types::CELEBRATE_LOCATION, 4, 0.6).only_if(is_dancing)),
            Box::new(run_one(vec![
                (Box::new(set_entity_look_target(is_piglin, 8.0)), 1),
                (Box::new(random_stroll::stroll_within(0.6, 2, 1)), 1),
                (Box::new(DoNothing::new(10, 20)), 1),
            ])),
        ],
        types::CELEBRATE_LOCATION.id(),
    )
}

fn piglin_retreat_activity() -> ActivityData {
    ActivityData::with_gate_memory(
        Activity::Avoid,
        10,
        vec![
            Box::new(set_walk_target_away_from_entity(
                types::AVOID_TARGET,
                1.0,
                DESIRED_DISTANCE_WHEN_AVOIDING,
            )),
            Box::new(idle_look_behaviors()),
            Box::new(idle_movement_behaviors()),
            Box::new(erase_memory_if(
                types::AVOID_TARGET.id(),
                wants_to_stop_fleeing,
            )),
        ],
        types::AVOID_TARGET.id(),
    )
}

fn piglin_ride_activity() -> ActivityData {
    ActivityData::with_gate_memory(
        Activity::Ride,
        10,
        vec![
            Box::new(mount(0.8)),
            Box::new(set_entity_look_target(is_player_holding_loved_item, 8.0)),
            Box::new(idle_look_behaviors()),
            Box::new(dismount_or_skip_mounting()),
        ],
        types::RIDE_TARGET.id(),
    )
}

/// Vanilla `PiglinBruteAi.getActivities`.
pub(super) fn brute_activities(_mob: &dyn Mob) -> Vec<ActivityData> {
    vec![
        ActivityData::with_priority_start(
            Activity::Core,
            0,
            vec![
                Box::new(apply_queued_changes()),
                Box::new(Timed::new(LookAtTargetSink::new(45, 90))),
                Box::new(Timed::new(MoveToTargetSink::default())),
                Box::new(stop_being_angry_if_target_dead()),
            ],
        ),
        ActivityData::with_priority_start(
            Activity::Idle,
            10,
            vec![
                Box::new(start_attacking(brute_find_attack_target)),
                Box::new(run_one(vec![
                    (Box::new(set_entity_look_target(is_player, 8.0)), 1),
                    (Box::new(set_entity_look_target(is_piglin, 8.0)), 1),
                    (Box::new(set_entity_look_target(is_brute, 8.0)), 1),
                    (Box::new(set_entity_look_target(anything, 8.0)), 1),
                    (Box::new(DoNothing::new(30, 60)), 1),
                ])),
                Box::new(run_one(vec![
                    (Box::new(random_stroll::stroll(0.6)), 2),
                    (Box::new(walk_with_kind(&EntityType::PIGLIN, anything)), 2),
                    (
                        Box::new(walk_with_kind(&EntityType::PIGLIN_BRUTE, anything)),
                        2,
                    ),
                    (
                        Box::new(stroll_to_poi(
                            types::HOME,
                            0.6,
                            HOME_CLOSE_ENOUGH_DISTANCE,
                            HOME_TOO_FAR_DISTANCE,
                        )),
                        2,
                    ),
                    (
                        Box::new(stroll_around_poi(
                            types::HOME,
                            0.6,
                            HOME_STROLL_AROUND_DISTANCE,
                        )),
                        2,
                    ),
                    (Box::new(DoNothing::new(30, 60)), 1),
                ])),
                Box::new(set_look_and_interact(&EntityType::PLAYER, 4)),
            ],
        ),
        ActivityData::with_gate_memory(
            Activity::Fight,
            10,
            vec![
                Box::new(stop_attacking_if_not_nearest(brute_find_attack_target)),
                Box::new(set_walk_target_from_attack_target(|_| 1.0)),
                Box::new(melee_attack(MELEE_ATTACK_COOLDOWN)),
            ],
            types::ATTACK_TARGET.id(),
        ),
    ]
}

/// Vanilla `PiglinAi.updateActivity`'s choice.
pub(super) fn piglin_update_activity(state: &mut BrainState) {
    state.set_active_activity_to_first_valid(&[
        Activity::AdmireItem,
        Activity::Fight,
        Activity::Avoid,
        Activity::Celebrate,
        Activity::Ride,
        Activity::Idle,
    ]);
}

/// Vanilla `PiglinAi.eat`.
pub(super) fn eat_for_a_while(state: &mut BrainState) {
    state.set_with_expiry(types::ATE_RECENTLY, true, EAT_COOLDOWN);
}

/// Vanilla `PiglinAi.admireGoldItem`.
pub(super) fn admire_gold_item(state: &mut BrainState) {
    state.set_with_expiry(types::ADMIRING_ITEM, true, ADMIRE_DURATION);
}

/// Vanilla `PiglinAi.initMemories`.
pub(super) fn init_memories(state: &mut BrainState) {
    dont_kill_any_more_hoglins_for_a_while(state);
}

/// Vanilla `PiglinBruteAi.updateActivity`'s choice.
pub(super) fn brute_update_activity(state: &mut BrainState) {
    state.set_active_activity_to_first_valid(&[Activity::Fight, Activity::Idle]);
}
