//! The behaviors only piglins use, vanilla's classes in `monster.piglin` and those it borrows.

use std::sync::Arc;

use pumpkin_data::entity::EntityType;
use rand::RngExt;

use super::piglin::PiglinEntity;
use super::piglin_ai::PiglinAi;
use super::piglin_anger::{
    AbstractPiglin, abstract_piglin, anger_target, as_piglin, broadcast_anger_target, closer_than,
    dont_kill_any_more_hoglins_for_a_while, is_kind, is_near_zombified, seconds, set_anger_target,
};
use crate::entity::EntityBase;
use crate::entity::ai::brain::BrainTick;
use crate::entity::ai::brain::behavior::OneShot;
use crate::entity::ai::brain::behavior::timed::Behavior;
use crate::entity::ai::brain::memory::position_tracker::{EntityTracker, PositionTracker};
use crate::entity::ai::brain::memory::walk_target::WalkTarget;
use crate::entity::ai::brain::memory::{MemoryStatus, types};
use crate::entity::ai::goal::ranged_crossbow_attack::RangedCrossbowAttackGoal;
use crate::entity::living::LivingEntity;

const ADMIRE_DURATION: i64 = 119;
const MAX_DISTANCE_TO_WALK_TO_ITEM: i32 = 9;
const MAX_TIME_TO_WALK_TO_ITEM: i32 = 200;
const DISABLE_ADMIRE_WALKING_TIME: i64 = 200;
const CELEBRATION_TIME: i64 = 300;
const MAX_WALK_DISTANCE_TO_START_RIDING: f64 = 8.0;
const MELEE_ATTACK_COOLDOWN: i64 = 20;
const EAT_COOLDOWN: i64 = 200;
const DESIRED_DISTANCE_WHEN_AVOIDING: i32 = 12;
const MIN_DESIRED_DIST_WITH_CROSSBOW: f64 = 5.0;
const STRAFE_SPEED: f32 = 0.75;
const CROSSBOW_TIMEOUT: i32 = 1200;
const CROSSBOW_CHARGE_DURATION: i32 = 25;
const CROSSBOW_RANGE: f64 = 8.0;
const HOME_CLOSE_ENOUGH_DISTANCE: i32 = 2;
const HOME_TOO_FAR_DISTANCE: i32 = 100;
const HOME_STROLL_AROUND_DISTANCE: i32 = 5;

/// Vanilla `StopHoldingItemIfNoLongerAdmiring`.
pub(super) fn stop_holding_item_if_no_longer_admiring() -> OneShot {
    OneShot::new(
        "StopHoldingItemIfNoLongerAdmiring",
        vec![(types::ADMIRING_ITEM.id(), MemoryStatus::ValueAbsent)],
        |tick| {
            let Some(piglin) = as_piglin(tick.mob) else {
                return false;
            };
            if !piglin.off_hand_blocks_nothing() {
                return false;
            }
            let player = tick.state.get(types::NEAREST_VISIBLE_PLAYER).cloned();
            piglin.stop_holding_off_hand_item(true, player);
            true
        },
    )
}

/// Vanilla `StartAdmiringItemIfSeen`.
pub(super) fn start_admiring_item_if_seen() -> OneShot {
    OneShot::new(
        "StartAdmiringItemIfSeen",
        vec![
            (
                types::NEAREST_VISIBLE_WANTED_ITEM.id(),
                MemoryStatus::ValuePresent,
            ),
            (types::ADMIRING_ITEM.id(), MemoryStatus::ValueAbsent),
            (types::ADMIRING_DISABLED.id(), MemoryStatus::ValueAbsent),
            (
                types::DISABLE_WALK_TO_ADMIRE_ITEM.id(),
                MemoryStatus::ValueAbsent,
            ),
        ],
        |tick| {
            let loved = tick
                .state
                .get(types::NEAREST_VISIBLE_WANTED_ITEM)
                .and_then(|item| item.get_item_entity().map(PiglinAi::item_entity_is_loved))
                .unwrap_or(false);
            if loved {
                tick.state
                    .set_with_expiry(types::ADMIRING_ITEM, true, ADMIRE_DURATION);
            }
            loved
        },
    )
}

/// Vanilla `StartCelebratingIfTargetDead`, only hoglins are danced over and rarely.
pub(super) fn start_celebrating_if_target_dead() -> OneShot {
    OneShot::new(
        "StartCelebratingIfTargetDead",
        vec![
            (types::ATTACK_TARGET.id(), MemoryStatus::ValuePresent),
            (types::ANGRY_AT.id(), MemoryStatus::Registered),
            (types::CELEBRATE_LOCATION.id(), MemoryStatus::ValueAbsent),
            (types::DANCING.id(), MemoryStatus::Registered),
        ],
        |tick| {
            let Some(target) = tick.state.get(types::ATTACK_TARGET).cloned() else {
                return false;
            };
            if target
                .get_living_entity()
                .is_none_or(|living| !living.is_dead_or_dying())
            {
                return false;
            }
            if PiglinAi::wants_to_dance(target.get_entity().entity_type) {
                tick.state
                    .set_with_expiry(types::DANCING, true, CELEBRATION_TIME);
            }
            tick.state.set_with_expiry(
                types::CELEBRATE_LOCATION,
                target.get_entity().block_pos.load(),
                CELEBRATION_TIME,
            );
            if !is_kind(target.as_ref(), &EntityType::PLAYER)
                || tick.world.level_info.load().game_rules.forgive_dead_players
            {
                tick.state.erase(types::ATTACK_TARGET.id());
                tick.state.erase(types::ANGRY_AT.id());
            }
            true
        },
    )
}

/// Vanilla `StopBeingAngryIfTargetDead`.
pub(super) fn stop_being_angry_if_target_dead() -> OneShot {
    OneShot::new(
        "StopBeingAngryIfTargetDead",
        vec![(types::ANGRY_AT.id(), MemoryStatus::ValuePresent)],
        |tick| {
            let forgiven = anger_target(tick).is_some_and(|target| {
                target
                    .get_living_entity()
                    .is_some_and(LivingEntity::is_dead_or_dying)
                    && (!is_kind(target.as_ref(), &EntityType::PLAYER)
                        || tick.world.level_info.load().game_rules.forgive_dead_players)
            });
            if forgiven {
                tick.state.erase(types::ANGRY_AT.id());
            }
            true
        },
    )
}

/// Vanilla `CopyMemoryWithExpiry` into `AVOID_TARGET`.
pub(super) fn avoid_when(
    source: crate::entity::ai::brain::memory::MemoryModuleType<Arc<dyn EntityBase>>,
    condition: fn(&BrainTick<'_>) -> bool,
) -> OneShot {
    OneShot::new(
        "CopyMemoryWithExpiry",
        vec![
            (source.id(), MemoryStatus::ValuePresent),
            (types::AVOID_TARGET.id(), MemoryStatus::ValueAbsent),
        ],
        move |tick| {
            if !condition(tick) {
                return false;
            }
            let Some(entity) = tick.state.get(source).cloned() else {
                return false;
            };
            tick.state
                .set_with_expiry(types::AVOID_TARGET, entity, seconds(5, 7));
            true
        },
    )
}

/// Vanilla `babySometimesRideBabyHoglin`, with its `Ticker`.
pub(super) fn baby_sometimes_ride_baby_hoglin() -> OneShot {
    let mut ticks_until_next = 0;
    OneShot::new(
        "CopyMemoryWithExpiry",
        vec![
            (
                types::NEAREST_VISIBLE_BABY_HOGLIN.id(),
                MemoryStatus::ValuePresent,
            ),
            (types::RIDE_TARGET.id(), MemoryStatus::ValueAbsent),
        ],
        move |tick| {
            if !as_piglin(tick.mob).is_some_and(PiglinEntity::is_baby) {
                return false;
            }
            if ticks_until_next > 0 {
                ticks_until_next -= 1;
                return false;
            }
            ticks_until_next = seconds(10, 40);
            let Some(hoglin) = tick.state.get(types::NEAREST_VISIBLE_BABY_HOGLIN).cloned() else {
                return false;
            };
            tick.state
                .set_with_expiry(types::RIDE_TARGET, hoglin, seconds(10, 30));
            true
        },
    )
}

/// Vanilla `Mount`, climbing up to the top of what is already riding.
pub(super) fn mount(speed: f32) -> OneShot {
    OneShot::new(
        "Mount",
        vec![
            (types::LOOK_TARGET.id(), MemoryStatus::Registered),
            (types::WALK_TARGET.id(), MemoryStatus::ValueAbsent),
            (types::RIDE_TARGET.id(), MemoryStatus::ValuePresent),
        ],
        move |tick| {
            if tick.mob.get_entity().has_vehicle() {
                return false;
            }
            let Some(target) = tick.state.get(types::RIDE_TARGET).cloned() else {
                return false;
            };
            if closer_than(tick, target.as_ref(), 1.0) {
                PiglinEntity::start_riding(tick.mob, tick.world, target);
            } else {
                tick.state.set(
                    types::LOOK_TARGET,
                    Arc::new(EntityTracker::new(target.clone(), true)) as Arc<dyn PositionTracker>,
                );
                tick.state.set(
                    types::WALK_TARGET,
                    WalkTarget::from_entity(target, speed, 1),
                );
            }
            true
        },
    )
}

/// Vanilla `PiglinAi.wantsToStopRiding`.
pub(super) fn wants_to_stop_riding(tick: &BrainTick<'_>, vehicle: &dyn EntityBase) -> bool {
    let Some(mob) = vehicle.get_mob() else {
        return false;
    };
    let vehicle_hurt = mob
        .get_mob_entity()
        .living_entity
        .hurt_cooldown
        .load(std::sync::atomic::Ordering::Relaxed)
        > 0;
    !mob.get_mob_entity().living_entity.is_baby()
        || !vehicle.get_entity().is_alive()
        || tick.state.has_memory_value(types::HURT_BY.id())
        || vehicle_hurt
        || (is_kind(vehicle, &EntityType::PIGLIN) && !vehicle.get_entity().has_vehicle())
}

/// Vanilla `DismountOrSkipMounting`.
pub(super) fn dismount_or_skip_mounting() -> OneShot {
    OneShot::new(
        "DismountOrSkipMounting",
        vec![(types::RIDE_TARGET.id(), MemoryStatus::Registered)],
        |tick| {
            let entity = tick.mob.get_entity();
            let current = entity.get_vehicle();
            let vehicle = current
                .clone()
                .or_else(|| tick.state.get(types::RIDE_TARGET).cloned());
            let Some(vehicle) = vehicle else {
                return false;
            };
            let valid = vehicle.get_entity().is_alive()
                && closer_than(tick, vehicle.as_ref(), MAX_WALK_DISTANCE_TO_START_RIDING);
            if valid && !wants_to_stop_riding(tick, vehicle.as_ref()) {
                return false;
            }
            if let Some(current) = current {
                current.get_entity().remove_passenger(entity.entity_id);
            }
            tick.state.erase(types::RIDE_TARGET.id());
            true
        },
    )
}

/// Vanilla `StartHuntingHoglin`.
pub(super) fn start_hunting_hoglin() -> OneShot {
    OneShot::new(
        "StartHuntingHoglin",
        vec![
            (
                types::NEAREST_VISIBLE_HUNTABLE_HOGLIN.id(),
                MemoryStatus::ValuePresent,
            ),
            (types::ANGRY_AT.id(), MemoryStatus::ValueAbsent),
            (types::HUNTED_RECENTLY.id(), MemoryStatus::ValueAbsent),
            (
                types::NEAREST_VISIBLE_ADULT_PIGLINS.id(),
                MemoryStatus::Registered,
            ),
        ],
        |tick| {
            let Some(piglin) = as_piglin(tick.mob) else {
                return false;
            };
            if piglin.is_baby() || !piglin.can_hunt() {
                return false;
            }
            let others = tick
                .state
                .get(types::NEAREST_VISIBLE_ADULT_PIGLINS)
                .cloned()
                .unwrap_or_default();
            if others.iter().any(|other| {
                abstract_piglin(other.as_ref()).is_some_and(AbstractPiglin::hunted_recently)
            }) {
                return false;
            }
            let Some(target) = tick
                .state
                .get(types::NEAREST_VISIBLE_HUNTABLE_HOGLIN)
                .cloned()
            else {
                return false;
            };
            set_anger_target(tick, &target);
            dont_kill_any_more_hoglins_for_a_while(tick.state);
            broadcast_anger_target(tick, &target);
            for other in others {
                if let Some(other) = abstract_piglin(other.as_ref()) {
                    other.queue(Box::new(|tick| {
                        dont_kill_any_more_hoglins_for_a_while(tick.state);
                    }));
                }
            }
            true
        },
    )
}

/// Vanilla `RememberIfHoglinWasKilled`.
pub(super) fn remember_if_hoglin_was_killed() -> OneShot {
    OneShot::new(
        "RememberIfHoglinWasKilled",
        vec![
            (types::ATTACK_TARGET.id(), MemoryStatus::ValuePresent),
            (types::HUNTED_RECENTLY.id(), MemoryStatus::Registered),
        ],
        |tick| {
            let killed = tick.state.get(types::ATTACK_TARGET).is_some_and(|target| {
                is_kind(target.as_ref(), &EntityType::HOGLIN)
                    && target
                        .get_living_entity()
                        .is_some_and(LivingEntity::is_dead_or_dying)
            });
            if killed {
                dont_kill_any_more_hoglins_for_a_while(tick.state);
            }
            true
        },
    )
}

/// Vanilla `EraseMemoryIf.create(PiglinAi::isNearZombified, ATTACK_TARGET)`.
pub(super) fn stop_attacking_near_zombified() -> OneShot {
    OneShot::new(
        "EraseMemoryIf",
        vec![(types::ATTACK_TARGET.id(), MemoryStatus::ValuePresent)],
        |tick| {
            let near = is_near_zombified(tick);
            if near {
                tick.state.erase(types::ATTACK_TARGET.id());
            }
            near
        },
    )
}

/// Vanilla `StopAdmiringIfItemTooFarAway`.
pub(super) fn stop_admiring_if_item_too_far_away() -> OneShot {
    OneShot::new(
        "StopAdmiringIfItemTooFarAway",
        vec![
            (types::ADMIRING_ITEM.id(), MemoryStatus::ValuePresent),
            (
                types::NEAREST_VISIBLE_WANTED_ITEM.id(),
                MemoryStatus::Registered,
            ),
        ],
        |tick| {
            if as_piglin(tick.mob).is_none_or(PiglinEntity::is_holding_item_in_off_hand) {
                return false;
            }
            let near = tick
                .state
                .get(types::NEAREST_VISIBLE_WANTED_ITEM)
                .is_some_and(|item| {
                    closer_than(tick, item.as_ref(), f64::from(MAX_DISTANCE_TO_WALK_TO_ITEM))
                });
            if near {
                return false;
            }
            tick.state.erase(types::ADMIRING_ITEM.id());
            true
        },
    )
}

/// Vanilla `StopAdmiringIfTiredOfTryingToReachItem`.
pub(super) fn stop_admiring_if_tired_of_trying_to_reach_item() -> OneShot {
    OneShot::new(
        "StopAdmiringIfTiredOfTryingToReachItem",
        vec![
            (types::ADMIRING_ITEM.id(), MemoryStatus::ValuePresent),
            (
                types::NEAREST_VISIBLE_WANTED_ITEM.id(),
                MemoryStatus::ValuePresent,
            ),
            (
                types::TIME_TRYING_TO_REACH_ADMIRE_ITEM.id(),
                MemoryStatus::Registered,
            ),
            (
                types::DISABLE_WALK_TO_ADMIRE_ITEM.id(),
                MemoryStatus::Registered,
            ),
        ],
        |tick| {
            if as_piglin(tick.mob).is_none_or(PiglinEntity::is_holding_item_in_off_hand) {
                return false;
            }
            match tick
                .state
                .get(types::TIME_TRYING_TO_REACH_ADMIRE_ITEM)
                .copied()
            {
                None => tick.state.set(types::TIME_TRYING_TO_REACH_ADMIRE_ITEM, 0),
                Some(time) if time > MAX_TIME_TO_WALK_TO_ITEM => {
                    tick.state.erase(types::ADMIRING_ITEM.id());
                    tick.state
                        .erase(types::TIME_TRYING_TO_REACH_ADMIRE_ITEM.id());
                    tick.state.set_with_expiry(
                        types::DISABLE_WALK_TO_ADMIRE_ITEM,
                        true,
                        DISABLE_ADMIRE_WALKING_TIME,
                    );
                }
                Some(time) => tick
                    .state
                    .set(types::TIME_TRYING_TO_REACH_ADMIRE_ITEM, time + 1),
            }
            true
        },
    )
}

/// Vanilla `BackUpIfTooClose.create(5, 0.75F)`, for those with a crossbow.
pub(super) fn back_up_if_too_close() -> OneShot {
    OneShot::new(
        "BackUpIfTooClose",
        vec![
            (types::WALK_TARGET.id(), MemoryStatus::ValueAbsent),
            (types::LOOK_TARGET.id(), MemoryStatus::Registered),
            (types::ATTACK_TARGET.id(), MemoryStatus::ValuePresent),
            (
                types::NEAREST_VISIBLE_LIVING_ENTITIES.id(),
                MemoryStatus::ValuePresent,
            ),
        ],
        |tick| {
            if !as_piglin(tick.mob).is_some_and(PiglinEntity::is_holding_crossbow) {
                return false;
            }
            let Some(target) = tick.state.get(types::ATTACK_TARGET).cloned() else {
                return false;
            };
            let visible = tick
                .state
                .get(types::NEAREST_VISIBLE_LIVING_ENTITIES)
                .is_some_and(|visible| visible.contains(target.as_ref(), &tick.visibility()));
            if !closer_than(tick, target.as_ref(), MIN_DESIRED_DIST_WITH_CROSSBOW) || !visible {
                return false;
            }
            tick.state.set(
                types::LOOK_TARGET,
                Arc::new(EntityTracker::new(target, true)) as Arc<dyn PositionTracker>,
            );
            tick.mob
                .get_mob_entity()
                .steered()
                .move_control()
                .strafe(-STRAFE_SPEED, 0.0);
            true
        },
    )
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CrossbowState {
    Uncharged,
    Charging,
    Charged,
    ReadyToAttack,
}

/// Vanilla `CrossbowAttack`.
pub(super) struct CrossbowAttack {
    state: CrossbowState,
    charge_ticks: i32,
    attack_delay: i32,
}

impl CrossbowAttack {
    pub(super) const fn new() -> Self {
        Self {
            state: CrossbowState::Uncharged,
            charge_ticks: 0,
            attack_delay: 0,
        }
    }
}

pub(super) fn can_shoot(tick: &BrainTick<'_>) -> bool {
    let Some(target) = tick.state.get(types::ATTACK_TARGET) else {
        return false;
    };
    as_piglin(tick.mob).is_some_and(PiglinEntity::is_holding_crossbow)
        && tick
            .state
            .get(types::NEAREST_VISIBLE_LIVING_ENTITIES)
            .is_some_and(|visible| visible.contains(target.as_ref(), &tick.visibility()))
        && closer_than(tick, target.as_ref(), CROSSBOW_RANGE)
}

impl Behavior for CrossbowAttack {
    fn entry_conditions(
        &self,
    ) -> &[(
        crate::entity::ai::brain::memory::MemoryModuleId,
        MemoryStatus,
    )] {
        const CONDITIONS: [(
            crate::entity::ai::brain::memory::MemoryModuleId,
            MemoryStatus,
        ); 2] = [
            (types::LOOK_TARGET.id(), MemoryStatus::Registered),
            (types::ATTACK_TARGET.id(), MemoryStatus::ValuePresent),
        ];
        &CONDITIONS
    }

    fn min_duration(&self) -> i32 {
        CROSSBOW_TIMEOUT
    }

    fn max_duration(&self) -> i32 {
        CROSSBOW_TIMEOUT
    }

    fn check_extra_start_conditions(&mut self, tick: &mut BrainTick<'_>) -> bool {
        can_shoot(tick)
    }

    fn can_still_use(&mut self, tick: &mut BrainTick<'_>) -> bool {
        can_shoot(tick)
    }

    fn tick(&mut self, tick: &mut BrainTick<'_>) {
        let Some(target) = tick.state.get(types::ATTACK_TARGET).cloned() else {
            return;
        };
        tick.state.set(
            types::LOOK_TARGET,
            Arc::new(EntityTracker::new(target.clone(), true)) as Arc<dyn PositionTracker>,
        );
        let Some(piglin) = as_piglin(tick.mob) else {
            return;
        };
        match self.state {
            CrossbowState::Uncharged => {
                piglin.start_charging_crossbow();
                self.state = CrossbowState::Charging;
                self.charge_ticks = 0;
            }
            CrossbowState::Charging => {
                self.charge_ticks += 1;
                if self.charge_ticks >= CROSSBOW_CHARGE_DURATION {
                    piglin.finish_charging_crossbow();
                    self.state = CrossbowState::Charged;
                    self.attack_delay = 20 + rand::rng().random_range(0..20);
                }
            }
            CrossbowState::Charged => {
                self.attack_delay -= 1;
                if self.attack_delay <= 0 {
                    self.state = CrossbowState::ReadyToAttack;
                }
            }
            CrossbowState::ReadyToAttack => {
                RangedCrossbowAttackGoal::shoot(tick.mob, &target);
                self.state = CrossbowState::Uncharged;
            }
        }
    }

    fn stop(&mut self, tick: &mut BrainTick<'_>) {
        if let Some(piglin) = as_piglin(tick.mob) {
            piglin.cancel_charging_crossbow();
        }
        self.state = CrossbowState::Uncharged;
    }

    fn debug_name(&self) -> &'static str {
        "CrossbowAttack"
    }
}
