//! What vanilla's `PiglinAi` and `PiglinBruteAi` share about anger, hurt and whom to fight.
//!
//! What happens to a piglin from outside its own tick, such as being hit, or another piglin's
//! anger, waits in its queue for the core activity, so no brain ever locks another.

use std::sync::{Arc, Mutex};

use pumpkin_data::entity::EntityType;
use pumpkin_data::environment_attribute::Activity;

use super::piglin::PiglinEntity;
use super::piglin_brute::PiglinBruteEntity;
use crate::entity::EntityBase;
use crate::entity::ai::brain::behavior::{OneShot, stop_attacking_if};
use crate::entity::ai::brain::memory::types;
use crate::entity::ai::brain::sensing::{
    is_entity_attackable, is_entity_attackable_ignoring_line_of_sight,
};
use crate::entity::ai::brain::{BrainState, BrainTick};
use crate::entity::mob::Mob;
use crate::entity::mob::hoglin::HoglinEntity;
use crate::entity::player::Player;
use crate::world::World;

const ANGER_DURATION: i64 = 600;
const BABY_FLEE_DURATION: i64 = 100;
const HIT_BY_PLAYER_MEMORY_TIMEOUT: i64 = 400;
const DESIRED_DISTANCE_FROM_ZOMBIFIED: f64 = 6.0;
const PLAYER_ANGER_RANGE: f64 = 16.0;

pub(super) fn seconds(min: i64, max: i64) -> i64 {
    rand::random_range(min * 20..=max * 20)
}

pub(super) fn hunt_delay() -> i64 {
    seconds(30, 120)
}

/// A change waiting for a piglin's own brain.
pub type QueuedChange = Box<dyn FnOnce(&mut BrainTick<'_>) + Send + Sync>;

/// Vanilla `AbstractPiglin`, as far as other piglins and the brain need it.
pub trait AbstractPiglin {
    fn can_hunt(&self) -> bool;

    fn is_adult(&self) -> bool;

    fn queued_changes(&self) -> &Mutex<Vec<QueuedChange>>;

    fn hunted_recently(&self) -> bool;

    fn queue(&self, change: QueuedChange) {
        self.queued_changes()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(change);
    }
}

pub(super) fn abstract_piglin(entity: &dyn EntityBase) -> Option<&dyn AbstractPiglin> {
    let any = entity.cast_any();
    any.downcast_ref::<PiglinEntity>()
        .map(|piglin| piglin as &dyn AbstractPiglin)
        .or_else(|| {
            any.downcast_ref::<PiglinBruteEntity>()
                .map(|brute| brute as &dyn AbstractPiglin)
        })
}

pub(super) fn abstract_piglin_of_mob(mob: &dyn Mob) -> Option<&dyn AbstractPiglin> {
    let any = mob.cast_any();
    any.downcast_ref::<PiglinEntity>()
        .map(|piglin| piglin as &dyn AbstractPiglin)
        .or_else(|| {
            any.downcast_ref::<PiglinBruteEntity>()
                .map(|brute| brute as &dyn AbstractPiglin)
        })
}

/// Vanilla `PiglinAi.findNearbyAdultPiglins`.
#[must_use]
pub fn is_adult_piglin(entity: &dyn EntityBase) -> bool {
    abstract_piglin(entity).is_some_and(AbstractPiglin::is_adult)
}

pub(super) fn as_piglin(mob: &dyn Mob) -> Option<&PiglinEntity> {
    mob.cast_any().downcast_ref::<PiglinEntity>()
}

pub(super) fn as_hoglin(entity: &dyn EntityBase) -> Option<&HoglinEntity> {
    entity.cast_any().downcast_ref::<HoglinEntity>()
}

pub(super) fn is_kind(entity: &dyn EntityBase, kind: &'static EntityType) -> bool {
    entity.get_entity().entity_type == kind
}

pub(super) fn same_entity(a: &dyn EntityBase, b: &dyn EntityBase) -> bool {
    a.get_entity().entity_id == b.get_entity().entity_id
}

pub(super) fn distance_to_mob(tick: &BrainTick<'_>, entity: &dyn EntityBase) -> f64 {
    entity
        .get_entity()
        .pos
        .load()
        .squared_distance_to_vec(&tick.mob.get_entity().pos.load())
}

/// Vanilla `Mob.closerThan` for the brain's own mob.
pub(super) fn closer_than(tick: &BrainTick<'_>, entity: &dyn EntityBase, distance: f64) -> bool {
    distance_to_mob(tick, entity) < distance * distance
}

/// The nearer of the two, as `BehaviorUtils.getNearestTarget`.
pub(super) fn nearest_of(
    tick: &BrainTick<'_>,
    current: Option<Arc<dyn EntityBase>>,
    new: Arc<dyn EntityBase>,
) -> Arc<dyn EntityBase> {
    match current {
        Some(current)
            if distance_to_mob(tick, current.as_ref()) < distance_to_mob(tick, new.as_ref()) =>
        {
            current
        }
        _ => new,
    }
}

pub(super) fn dont_kill_any_more_hoglins_for_a_while(state: &mut BrainState) {
    state.set_with_expiry(types::HUNTED_RECENTLY, true, hunt_delay());
}

pub(super) fn attackable_ignoring_sight(tick: &BrainTick<'_>, target: &dyn EntityBase) -> bool {
    is_entity_attackable_ignoring_line_of_sight(&tick.visibility(), target)
}

/// Vanilla `PiglinAi.setAngerTarget`.
pub(super) fn set_anger_target(tick: &mut BrainTick<'_>, target: &Arc<dyn EntityBase>) {
    if !attackable_ignoring_sight(tick, target.as_ref()) {
        return;
    }
    tick.state.erase(types::CANT_REACH_WALK_TARGET_SINCE.id());
    tick.state.set_with_expiry(
        types::ANGRY_AT,
        target.get_entity().entity_uuid,
        ANGER_DURATION,
    );
    if is_kind(target.as_ref(), &EntityType::HOGLIN)
        && abstract_piglin_of_mob(tick.mob).is_some_and(AbstractPiglin::can_hunt)
    {
        dont_kill_any_more_hoglins_for_a_while(tick.state);
    }
    if is_kind(target.as_ref(), &EntityType::PLAYER)
        && tick.world.level_info.load().game_rules.universal_anger
    {
        tick.state
            .set_with_expiry(types::UNIVERSAL_ANGER, true, ANGER_DURATION);
    }
}

pub(super) fn nearest_visible_targetable_player(
    tick: &BrainTick<'_>,
) -> Option<Arc<dyn EntityBase>> {
    tick.state
        .get(types::NEAREST_VISIBLE_ATTACKABLE_PLAYER)
        .map(|player| player.clone() as Arc<dyn EntityBase>)
}

pub(super) fn set_anger_target_to_nearest_player_if_found(
    tick: &mut BrainTick<'_>,
    fallback: &Arc<dyn EntityBase>,
) {
    let target = nearest_visible_targetable_player(tick).unwrap_or_else(|| fallback.clone());
    set_anger_target(tick, &target);
}

pub(super) fn anger_target(tick: &BrainTick<'_>) -> Option<Arc<dyn EntityBase>> {
    let uuid = *tick.state.get(types::ANGRY_AT)?;
    tick.world
        .get_entity_by_uuid(uuid)
        .filter(|entity| entity.get_living_entity().is_some())
}

/// Vanilla `setAngerTargetIfCloserThanCurrent`.
pub(super) fn set_anger_target_if_closer_than_current(
    tick: &mut BrainTick<'_>,
    new: &Arc<dyn EntityBase>,
) {
    let current = anger_target(tick);
    let nearest = nearest_of(tick, current.clone(), new.clone());
    if current.is_none_or(|current| !same_entity(current.as_ref(), nearest.as_ref())) {
        set_anger_target(tick, &nearest);
    }
}

pub(super) fn adult_piglins(tick: &BrainTick<'_>) -> Vec<Arc<dyn EntityBase>> {
    tick.state
        .get(types::NEARBY_ADULT_PIGLINS)
        .cloned()
        .unwrap_or_default()
}

/// Vanilla `broadcastAngerTarget`.
pub(super) fn broadcast_anger_target(tick: &BrainTick<'_>, target: &Arc<dyn EntityBase>) {
    for other in adult_piglins(tick) {
        let Some(piglin) = abstract_piglin(other.as_ref()) else {
            continue;
        };
        let hoglin_not_for_it = as_hoglin(target.as_ref())
            .is_some_and(|hoglin| !piglin.can_hunt() || !hoglin.can_be_hunted());
        if !hoglin_not_for_it {
            let target = target.clone();
            piglin.queue(Box::new(move |tick| {
                set_anger_target_if_closer_than_current(tick, &target);
            }));
        }
    }
}

/// Vanilla `broadcastUniversalAnger`.
pub(super) fn broadcast_universal_anger(tick: &BrainTick<'_>) {
    for other in adult_piglins(tick) {
        if let Some(piglin) = abstract_piglin(other.as_ref()) {
            piglin.queue(Box::new(|tick| {
                if let Some(player) = nearest_visible_targetable_player(tick) {
                    set_anger_target(tick, &player);
                }
            }));
        }
    }
}

/// Vanilla `PiglinAi.maybeRetaliate`.
pub(super) fn maybe_retaliate(tick: &mut BrainTick<'_>, attacker: &Arc<dyn EntityBase>) {
    if tick.state.is_active(Activity::Avoid) || !attackable_ignoring_sight(tick, attacker.as_ref())
    {
        return;
    }
    let much_further = tick.state.get(types::ATTACK_TARGET).is_some_and(|current| {
        let (to_attacker, to_current) = (
            distance_to_mob(tick, attacker.as_ref()).sqrt(),
            distance_to_mob(tick, current.as_ref()).sqrt(),
        );
        to_attacker > to_current + 4.0
    });
    if much_further {
        return;
    }
    if is_kind(attacker.as_ref(), &EntityType::PLAYER)
        && tick.world.level_info.load().game_rules.universal_anger
    {
        set_anger_target_to_nearest_player_if_found(tick, attacker);
        broadcast_universal_anger(tick);
    } else {
        set_anger_target(tick, attacker);
        broadcast_anger_target(tick, attacker);
    }
}

pub(super) fn hoglins_outnumber_piglins(state: &BrainState) -> bool {
    let piglins = state
        .get(types::VISIBLE_ADULT_PIGLIN_COUNT)
        .copied()
        .unwrap_or(0)
        + 1;
    let hoglins = state
        .get(types::VISIBLE_ADULT_HOGLIN_COUNT)
        .copied()
        .unwrap_or(0);
    hoglins > piglins
}

pub(super) fn set_avoid_target_and_dont_hunt(state: &mut BrainState, target: Arc<dyn EntityBase>) {
    state.erase(types::ANGRY_AT.id());
    state.erase(types::ATTACK_TARGET.id());
    state.erase(types::WALK_TARGET.id());
    state.set_with_expiry(types::AVOID_TARGET, target, seconds(5, 20));
    dont_kill_any_more_hoglins_for_a_while(state);
}

/// Vanilla `broadcastRetreat` and `retreatFromNearestTarget`.
pub(super) fn broadcast_retreat(tick: &BrainTick<'_>, target: &Arc<dyn EntityBase>) {
    let visible = tick
        .state
        .get(types::NEAREST_VISIBLE_ADULT_PIGLINS)
        .cloned()
        .unwrap_or_default();
    for other in visible {
        if let Some(piglin) = other.cast_any().downcast_ref::<PiglinEntity>() {
            let target = target.clone();
            piglin.queue(Box::new(move |tick| {
                let avoid = tick.state.get(types::AVOID_TARGET).cloned();
                let nearest = nearest_of(tick, avoid, target);
                let attack = tick.state.get(types::ATTACK_TARGET).cloned();
                let nearest = nearest_of(tick, attack, nearest);
                set_avoid_target_and_dont_hunt(tick.state, nearest);
            }));
        }
    }
}

/// Vanilla `PiglinAi.wasHurtBy`.
pub fn piglin_was_hurt_by(tick: &mut BrainTick<'_>, attacker: &Arc<dyn EntityBase>) {
    if abstract_piglin(attacker.as_ref())
        .is_some_and(|_| is_kind(attacker.as_ref(), &EntityType::PIGLIN))
    {
        return;
    }
    let Some(piglin) = as_piglin(tick.mob) else {
        return;
    };
    if piglin.is_holding_item_in_off_hand() {
        let player = tick.state.get(types::NEAREST_VISIBLE_PLAYER).cloned();
        piglin.stop_holding_off_hand_item(false, player);
    }
    tick.state.erase(types::CELEBRATE_LOCATION.id());
    tick.state.erase(types::DANCING.id());
    tick.state.erase(types::ADMIRING_ITEM.id());
    if is_kind(attacker.as_ref(), &EntityType::PLAYER) {
        tick.state
            .set_with_expiry(types::ADMIRING_DISABLED, true, HIT_BY_PLAYER_MEMORY_TIMEOUT);
    }
    let other_kind = tick
        .state
        .get(types::AVOID_TARGET)
        .is_some_and(|avoid| avoid.get_entity().entity_type != attacker.get_entity().entity_type);
    if other_kind {
        tick.state.erase(types::AVOID_TARGET.id());
    }
    if piglin.is_baby() {
        tick.state
            .set_with_expiry(types::AVOID_TARGET, attacker.clone(), BABY_FLEE_DURATION);
        if attackable_ignoring_sight(tick, attacker.as_ref()) {
            broadcast_anger_target(tick, attacker);
        }
    } else if is_kind(attacker.as_ref(), &EntityType::HOGLIN)
        && hoglins_outnumber_piglins(tick.state)
    {
        set_avoid_target_and_dont_hunt(tick.state, attacker.clone());
        broadcast_retreat(tick, attacker);
    } else {
        maybe_retaliate(tick, attacker);
    }
}

/// Vanilla `PiglinBruteAi.wasHurtBy`.
pub fn brute_was_hurt_by(tick: &mut BrainTick<'_>, attacker: &Arc<dyn EntityBase>) {
    if abstract_piglin(attacker.as_ref()).is_none() {
        maybe_retaliate(tick, attacker);
    }
}

/// Vanilla `PiglinAi.angerNearbyPiglins`, for a chest opened or gold broken in their sight.
pub fn anger_nearby_piglins(
    world: &World,
    player: &Arc<Player>,
    only_if_they_see_the_player: bool,
) {
    let area = player.living_entity.entity.bounding_box.load().expand(
        PLAYER_ANGER_RANGE,
        PLAYER_ANGER_RANGE,
        PLAYER_ANGER_RANGE,
    );
    for entity in world.get_entities_at_box(&area) {
        let Some(piglin) = entity.cast_any().downcast_ref::<PiglinEntity>() else {
            continue;
        };
        let player = player.clone();
        piglin.queue(Box::new(move |tick| {
            if !tick.state.is_active(Activity::Idle) {
                return;
            }
            let target = player as Arc<dyn EntityBase>;
            if only_if_they_see_the_player
                && !tick
                    .state
                    .get(types::NEAREST_VISIBLE_LIVING_ENTITIES)
                    .is_some_and(|visible| visible.contains(target.as_ref(), &tick.visibility()))
            {
                return;
            }
            if tick.world.level_info.load().game_rules.universal_anger {
                set_anger_target_to_nearest_player_if_found(tick, &target);
            } else {
                set_anger_target(tick, &target);
            }
        }));
    }
}

/// What others asked of it, the first thing in its tick.
pub(super) fn apply_queued_changes() -> OneShot {
    OneShot::new("ApplyQueuedChanges", Vec::new(), |tick| {
        let Some(piglin) = abstract_piglin_of_mob(tick.mob) else {
            return false;
        };
        let changes = std::mem::take(
            &mut *piglin
                .queued_changes()
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        );
        let any = !changes.is_empty();
        for change in changes {
            change(tick);
        }
        any
    })
}

pub(super) fn is_near_zombified(tick: &BrainTick<'_>) -> bool {
    tick.state
        .get(types::NEAREST_VISIBLE_ZOMBIFIED)
        .is_some_and(|zombified| {
            closer_than(tick, zombified.as_ref(), DESIRED_DISTANCE_FROM_ZOMBIFIED)
        })
}

/// Vanilla `PiglinAi.findNearestValidAttackTarget`.
pub(super) fn find_nearest_valid_attack_target(
    tick: &BrainTick<'_>,
) -> Option<Arc<dyn EntityBase>> {
    if is_near_zombified(tick) {
        return None;
    }
    if let Some(angry_at) = anger_target(tick)
        && attackable_ignoring_sight(tick, angry_at.as_ref())
    {
        return Some(angry_at);
    }
    if tick.state.has_memory_value(types::UNIVERSAL_ANGER.id())
        && let Some(player) = nearest_visible_targetable_player(tick)
    {
        return Some(player);
    }
    if let Some(nemesis) = tick.state.get(types::NEAREST_VISIBLE_NEMESIS) {
        return Some(nemesis.clone());
    }
    tick.state
        .get(types::NEAREST_TARGETABLE_PLAYER_NOT_WEARING_GOLD)
        .map(|player| player.clone() as Arc<dyn EntityBase>)
        .filter(|player| is_entity_attackable(&tick.visibility(), player.as_ref()))
}

/// Vanilla's `StartAttacking.create((level, piglin) -> piglin.isAdult(), ...)`.
pub(super) fn find_target_when_adult(tick: &BrainTick<'_>) -> Option<Arc<dyn EntityBase>> {
    as_piglin(tick.mob)
        .is_some_and(PiglinEntity::is_adult)
        .then(|| find_nearest_valid_attack_target(tick))
        .flatten()
}

/// Vanilla `PiglinBruteAi.findNearestValidAttackTarget`.
pub(super) fn brute_find_attack_target(tick: &BrainTick<'_>) -> Option<Arc<dyn EntityBase>> {
    if let Some(angry_at) = anger_target(tick)
        && attackable_ignoring_sight(tick, angry_at.as_ref())
    {
        return Some(angry_at);
    }
    nearest_visible_targetable_player(tick)
        .or_else(|| tick.state.get(types::NEAREST_VISIBLE_NEMESIS).cloned())
}

/// `StopAttackingIfTargetInvalid` with `!isNearestValidAttackTarget`.
pub(super) fn stop_attacking_if_not_nearest(
    find: fn(&BrainTick<'_>) -> Option<Arc<dyn EntityBase>>,
) -> OneShot {
    stop_attacking_if(
        move |tick, target| find(tick).is_none_or(|valid| !same_entity(valid.as_ref(), target)),
        |_, _| {},
    )
}
