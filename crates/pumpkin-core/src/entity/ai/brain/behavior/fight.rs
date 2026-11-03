//! The brain behaviours vanilla's fighting mobs share: picking a target, walking to it,
//! hitting it and giving up on it.

use std::sync::Arc;

use super::super::memory::{
    MemoryModuleId, MemoryStatus,
    position_tracker::{EntityTracker, PositionTracker},
    types,
    walk_target::WalkTarget,
};
use super::super::{BrainState, BrainTick};
use super::OneShot;
use crate::entity::EntityBase;
use crate::entity::mob::Mob;

/// Vanilla `StopAttackingIfTargetInvalid.TIMEOUT_TO_GET_WITHIN_ATTACK_RANGE`.
const TIMEOUT_TO_GET_WITHIN_ATTACK_RANGE: i64 = 200;

/// Vanilla `StartAttacking`: `finder` picks the target, if the mob may attack it.
#[must_use]
pub fn start_attacking(finder: fn(&BrainTick<'_>) -> Option<Arc<dyn EntityBase>>) -> OneShot {
    start_attacking_with(move |tick| finder(tick))
}

/// [`start_attacking`] for a finder that also writes to the brain.
#[must_use]
pub fn start_attacking_with(
    mut finder: impl FnMut(&mut BrainTick<'_>) -> Option<Arc<dyn EntityBase>> + Send + Sync + 'static,
) -> OneShot {
    OneShot::new(
        "StartAttacking",
        vec![
            (types::ATTACK_TARGET.id(), MemoryStatus::ValueAbsent),
            (
                types::CANT_REACH_WALK_TARGET_SINCE.id(),
                MemoryStatus::Registered,
            ),
        ],
        move |tick| {
            let Some(target) = finder(tick) else {
                return false;
            };
            if !tick.mob.can_attack(target.as_ref()) {
                return false;
            }
            tick.state.set(types::ATTACK_TARGET, target);
            tick.state.erase(types::CANT_REACH_WALK_TARGET_SINCE.id());
            true
        },
    )
}

/// Vanilla `StopAttackingIfTargetInvalid`: drops a target that is dead, gone or could not be
/// reached for 10 seconds, telling `on_erased`.
#[must_use]
pub fn stop_attacking_if_target_invalid(on_erased: fn(&dyn Mob, &Arc<dyn EntityBase>)) -> OneShot {
    stop_attacking_if(|_, _| false, on_erased)
}

/// Vanilla `StopAttackingIfTargetInvalid.create(stopCondition, ...)`: also drops the target when
/// `stop` says so.
#[must_use]
pub fn stop_attacking_if(
    stop: impl FnMut(&BrainTick<'_>, &dyn EntityBase) -> bool + Send + Sync + 'static,
    on_erased: fn(&dyn Mob, &Arc<dyn EntityBase>),
) -> OneShot {
    stop_attacking(stop, on_erased, true)
}

/// The same with `canGrowTiredOfTryingToReachTarget` false: a target out of reach stays one.
#[must_use]
pub fn stop_attacking_if_never_tired(
    stop: impl FnMut(&BrainTick<'_>, &dyn EntityBase) -> bool + Send + Sync + 'static,
    on_erased: fn(&dyn Mob, &Arc<dyn EntityBase>),
) -> OneShot {
    stop_attacking(stop, on_erased, false)
}

fn stop_attacking(
    mut stop: impl FnMut(&BrainTick<'_>, &dyn EntityBase) -> bool + Send + Sync + 'static,
    on_erased: fn(&dyn Mob, &Arc<dyn EntityBase>),
    can_grow_tired: bool,
) -> OneShot {
    OneShot::new(
        "StopAttackingIfTargetInvalid",
        vec![
            (types::ATTACK_TARGET.id(), MemoryStatus::ValuePresent),
            (
                types::CANT_REACH_WALK_TARGET_SINCE.id(),
                MemoryStatus::Registered,
            ),
        ],
        move |tick| {
            let Some(target) = tick.state.get(types::ATTACK_TARGET).cloned() else {
                return false;
            };
            let tired = can_grow_tired
                && tick
                    .state
                    .get(types::CANT_REACH_WALK_TARGET_SINCE)
                    .is_some_and(|since| tick.time - since > TIMEOUT_TO_GET_WITHIN_ATTACK_RANGE);
            let entity = target.get_entity();
            let valid = !stop(tick, target.as_ref())
                && tick.mob.can_attack(target.as_ref())
                && !tired
                && entity.is_alive()
                && Arc::ptr_eq(&entity.world.load_full(), tick.world);
            if !valid {
                on_erased(tick.mob, &target);
                tick.state.erase(types::ATTACK_TARGET.id());
            }
            true
        },
    )
}

/// Whether the attack target is visible and within melee reach.
fn target_in_reach(tick: &BrainTick<'_>, target: &dyn EntityBase) -> bool {
    tick.state
        .get(types::NEAREST_VISIBLE_LIVING_ENTITIES)
        .is_some_and(|visible| visible.contains(target, &tick.visibility()))
        && tick.mob.mob_weapon_range().map_or_else(
            || tick.mob.get_mob_entity().is_in_attack_range(target),
            |range| {
                target
                    .get_entity()
                    .pos
                    .load()
                    .squared_distance_to_vec(&tick.mob.get_entity().pos.load())
                    < (range - 1.0) * (range - 1.0)
            },
        )
}

/// Vanilla `SetWalkTargetFromAttackTargetIfTargetOutOfReach`, for melee mobs.
#[must_use]
pub fn set_walk_target_from_attack_target(speed: fn(&dyn Mob) -> f32) -> OneShot {
    OneShot::new(
        "SetWalkTargetFromAttackTargetIfTargetOutOfReach",
        vec![
            (types::WALK_TARGET.id(), MemoryStatus::Registered),
            (types::LOOK_TARGET.id(), MemoryStatus::Registered),
            (types::ATTACK_TARGET.id(), MemoryStatus::ValuePresent),
            (
                types::NEAREST_VISIBLE_LIVING_ENTITIES.id(),
                MemoryStatus::Registered,
            ),
        ],
        move |tick| {
            let Some(target) = tick.state.get(types::ATTACK_TARGET).cloned() else {
                return false;
            };
            if target_in_reach(tick, target.as_ref()) {
                tick.state.erase(types::WALK_TARGET.id());
            } else {
                tick.state.set(
                    types::LOOK_TARGET,
                    Arc::new(EntityTracker::new(target.clone(), true)) as Arc<dyn PositionTracker>,
                );
                tick.state.set(
                    types::WALK_TARGET,
                    WalkTarget::new(
                        Arc::new(EntityTracker::new(target, false)),
                        speed(tick.mob),
                        0,
                    ),
                );
            }
            true
        },
    )
}

/// Vanilla `MeleeAttack`: hits a visible target in reach, then cools down.
#[must_use]
pub fn melee_attack(cooldown_between_attacks: i64) -> OneShot {
    OneShot::new(
        "MeleeAttack",
        vec![
            (types::LOOK_TARGET.id(), MemoryStatus::Registered),
            (types::ATTACK_TARGET.id(), MemoryStatus::ValuePresent),
            (types::ATTACK_COOLING_DOWN.id(), MemoryStatus::ValueAbsent),
            (
                types::NEAREST_VISIBLE_LIVING_ENTITIES.id(),
                MemoryStatus::ValuePresent,
            ),
        ],
        move |tick| {
            let Some(target) = tick.state.get(types::ATTACK_TARGET).cloned() else {
                return false;
            };
            if tick.mob.holds_non_melee_weapon() || !target_in_reach(tick, target.as_ref()) {
                return false;
            }
            tick.state.set(
                types::LOOK_TARGET,
                Arc::new(EntityTracker::new(target.clone(), true)) as Arc<dyn PositionTracker>,
            );
            tick.mob.get_mob_entity().living_entity.swing_hand();
            tick.mob.do_hurt_target(target.as_ref());
            tick.state
                .set_with_expiry(types::ATTACK_COOLING_DOWN, true, cooldown_between_attacks);
            true
        },
    )
}

/// Vanilla `EraseMemoryIf`.
#[must_use]
pub fn erase_memory_if(memory: MemoryModuleId, predicate: fn(&BrainState) -> bool) -> OneShot {
    OneShot::new(
        "EraseMemoryIf",
        vec![(memory, MemoryStatus::ValuePresent)],
        move |tick| {
            if !predicate(tick.state) {
                return false;
            }
            tick.state.erase(memory);
            true
        },
    )
}
