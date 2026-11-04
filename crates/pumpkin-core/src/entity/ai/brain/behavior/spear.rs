//! Vanilla `SpearApproach`, `SpearAttack` and `SpearRetreat`: a mob with a kinetic weapon walks up
//! to its target, charges through it with the weapon raised, then backs off to charge again.

use std::sync::Arc;

use pumpkin_data::data_component_impl::{EquipmentSlot, KineticWeaponImpl};
use pumpkin_data::item_stack::ItemStack;
use pumpkin_util::Hand;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;

use super::super::BrainTick;
use super::super::memory::position_tracker::{EntityTracker, PositionTracker};
use super::super::memory::{MemoryModuleId, MemoryStatus, SpearStatus, types};
use super::timed::Behavior;
use crate::entity::EntityBase;
use crate::entity::ai::util::land_random_pos;
use crate::entity::mob::Mob;
use crate::item::items::spear::SpearItem;

/// `Behavior.timedOut` overridden to never: the longest duration `Timed` can pick.
const NEVER_TIMES_OUT: i32 = i32::MAX - 1;
/// Vanilla `PathNavigation.moveTo`'s reach.
const REACH_RANGE: i32 = 1;
/// How far both behaviours look up and down for a place away from the target.
const AWAY_VERTICAL_DIST: i32 = 7;
/// Added to the distances to keep while the mob rides.
const MOUNTED_DISTANCE: f64 = 2.0;

/// Vanilla `SpearAttack.MIN_REPOSITION_DISTANCE`.
const MIN_REPOSITION_DISTANCE: f64 = 6.0;
/// Vanilla `SpearAttack.MAX_REPOSITION_DISTANCE`.
const MAX_REPOSITION_DISTANCE: f64 = 7.0;
/// Vanilla `SpearRetreat.MIN_COOLDOWN_DISTANCE`.
const MIN_COOLDOWN_DISTANCE: f64 = 9.0;
/// Vanilla `SpearRetreat.MAX_COOLDOWN_DISTANCE`.
const MAX_COOLDOWN_DISTANCE: f64 = 11.0;
/// Vanilla `SpearRetreat.MAX_FLEEING_TIME`.
const MAX_FLEEING_TIME: i32 = 100;

fn attack_target(tick: &BrainTick<'_>) -> Option<Arc<dyn EntityBase>> {
    tick.state.get(types::ATTACK_TARGET).cloned()
}

fn main_hand_item(mob: &dyn Mob) -> ItemStack {
    mob.get_mob_entity()
        .living_entity
        .entity_equipment
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(&EquipmentSlot::MAIN_HAND)
}

/// The `ableToAttack` all three share: a target, and a kinetic weapon in the main hand.
fn able_to_attack(tick: &BrainTick<'_>) -> bool {
    tick.state.has_memory_value(types::ATTACK_TARGET.id())
        && main_hand_item(tick.mob)
            .get_data_component::<KineticWeaponImpl>()
            .is_some()
}

fn is_using_item(mob: &dyn Mob) -> bool {
    mob.get_mob_entity().living_entity.is_using_item()
}

fn distance_sq_to(mob: &dyn Mob, target: &dyn EntityBase) -> f64 {
    mob.get_entity()
        .pos
        .load()
        .squared_distance_to_vec(&target.get_entity().pos.load())
}

fn mounted_distance(mob: &dyn Mob) -> f64 {
    if mob.get_entity().has_vehicle() {
        MOUNTED_DISTANCE
    } else {
        0.0
    }
}

/// The `chargeSpeedModifier` of `getRootVehicle()`, which is the mob itself when it walks.
fn charge_speed_modifier(mob: &dyn Mob) -> f64 {
    let modifier = match mob.get_entity().get_root_vehicle() {
        Some(root) => root
            .get_mob()
            .map_or(1.0f32, |vehicle| vehicle.charge_speed_modifier()),
        None => mob.charge_speed_modifier(),
    };
    f64::from(modifier)
}

fn look_at(tick: &mut BrainTick<'_>, target: &Arc<dyn EntityBase>) {
    tick.state.set(
        types::LOOK_TARGET,
        Arc::new(EntityTracker::new(target.clone(), true)) as Arc<dyn PositionTracker>,
    );
}

/// Vanilla `PathNavigation.moveTo`: searches right away, unless the path already leads there.
fn move_to(mob: &dyn Mob, target: BlockPos, speed: f64) {
    let steered = mob.get_mob_entity().steered();
    let owner = &steered.mob_entity().living_entity;
    let mut navigator = steered.navigator();
    let path = navigator.create_path_to_any(owner, &[target], REACH_RANGE);
    navigator.move_to_path(path, speed, owner);
}

fn stop_navigation(mob: &dyn Mob) {
    mob.get_mob_entity().steered().navigator().stop();
}

fn navigation_is_done(mob: &dyn Mob) -> bool {
    mob.get_mob_entity().steered().navigator().is_done()
}

/// Vanilla `SpearApproach`.
pub struct SpearApproach {
    speed_modifier_when_repositioning: f64,
    approach_distance_sq: f64,
}

impl SpearApproach {
    #[must_use]
    pub fn new(speed_modifier_when_repositioning: f64, approach_distance: f32) -> Self {
        Self {
            speed_modifier_when_repositioning,
            approach_distance_sq: f64::from(approach_distance * approach_distance),
        }
    }
}

impl Behavior for SpearApproach {
    fn entry_conditions(&self) -> &[(MemoryModuleId, MemoryStatus)] {
        const CONDITIONS: [(MemoryModuleId, MemoryStatus); 1] =
            [(types::SPEAR_STATUS.id(), MemoryStatus::ValueAbsent)];
        &CONDITIONS
    }

    fn min_duration(&self) -> i32 {
        NEVER_TIMES_OUT
    }

    fn max_duration(&self) -> i32 {
        NEVER_TIMES_OUT
    }

    fn check_extra_start_conditions(&mut self, tick: &mut BrainTick<'_>) -> bool {
        able_to_attack(tick) && !is_using_item(tick.mob)
    }

    fn start(&mut self, tick: &mut BrainTick<'_>) {
        tick.mob.get_mob_entity().set_attacking(true);
        tick.state.set(types::SPEAR_STATUS, SpearStatus::Approach);
    }

    fn can_still_use(&mut self, tick: &mut BrainTick<'_>) -> bool {
        able_to_attack(tick)
            && attack_target(tick).is_some_and(|target| {
                distance_sq_to(tick.mob, target.as_ref()) > self.approach_distance_sq
            })
    }

    fn tick(&mut self, tick: &mut BrainTick<'_>) {
        let Some(target) = attack_target(tick) else {
            return;
        };
        let speed = charge_speed_modifier(tick.mob) * self.speed_modifier_when_repositioning;
        look_at(tick, &target);
        move_to(tick.mob, target.get_entity().block_pos.load(), speed);
    }

    fn stop(&mut self, tick: &mut BrainTick<'_>) {
        stop_navigation(tick.mob);
        tick.state.set(types::SPEAR_STATUS, SpearStatus::Charging);
    }

    fn debug_name(&self) -> &'static str {
        "SpearApproach"
    }
}

/// Vanilla `SpearAttack`.
pub struct SpearAttack {
    speed_modifier_when_charging: f64,
    speed_modifier_when_repositioning: f64,
    target_in_range_radius_sq: f64,
}

impl SpearAttack {
    #[must_use]
    pub fn new(
        speed_modifier_when_charging: f64,
        speed_modifier_when_repositioning: f64,
        target_in_range_radius: f32,
    ) -> Self {
        Self {
            speed_modifier_when_charging,
            speed_modifier_when_repositioning,
            target_in_range_radius_sq: f64::from(target_in_range_radius * target_in_range_radius),
        }
    }

    /// Vanilla `getKineticWeaponUseDuration`.
    fn kinetic_weapon_use_duration(mob: &dyn Mob) -> i32 {
        main_hand_item(mob)
            .get_data_component::<KineticWeaponImpl>()
            .map_or(0, KineticWeaponImpl::compute_damage_use_duration)
    }
}

impl Behavior for SpearAttack {
    fn entry_conditions(&self) -> &[(MemoryModuleId, MemoryStatus)] {
        const CONDITIONS: [(MemoryModuleId, MemoryStatus); 1] =
            [(types::SPEAR_STATUS.id(), MemoryStatus::ValuePresent)];
        &CONDITIONS
    }

    fn min_duration(&self) -> i32 {
        NEVER_TIMES_OUT
    }

    fn max_duration(&self) -> i32 {
        NEVER_TIMES_OUT
    }

    fn check_extra_start_conditions(&mut self, tick: &mut BrainTick<'_>) -> bool {
        tick.state.get(types::SPEAR_STATUS).copied() == Some(SpearStatus::Charging)
            && able_to_attack(tick)
            && !is_using_item(tick.mob)
    }

    fn start(&mut self, tick: &mut BrainTick<'_>) {
        let mob_entity = tick.mob.get_mob_entity();
        mob_entity.set_attacking(true);
        tick.state.set(
            types::SPEAR_ENGAGE_TIME,
            Self::kinetic_weapon_use_duration(tick.mob),
        );
        tick.state.erase(types::SPEAR_CHARGE_POSITION.id());
        // Vanilla `LivingEntity.startUsingItem`.
        let weapon = main_hand_item(tick.mob);
        if !weapon.is_empty() && !is_using_item(tick.mob) {
            mob_entity
                .living_entity
                .set_active_hand(Hand::Right, weapon, SpearItem::USE_DURATION);
        }
    }

    fn can_still_use(&mut self, tick: &mut BrainTick<'_>) -> bool {
        tick.state
            .get(types::SPEAR_ENGAGE_TIME)
            .copied()
            .unwrap_or(0)
            > 0
            && able_to_attack(tick)
    }

    fn tick(&mut self, tick: &mut BrainTick<'_>) {
        let Some(target) = attack_target(tick) else {
            return;
        };
        let distance_sq = distance_sq_to(tick.mob, target.as_ref());
        let speed = charge_speed_modifier(tick.mob);
        let mount_distance = mounted_distance(tick.mob);
        look_at(tick, &target);
        let engage_time = tick
            .state
            .get(types::SPEAR_ENGAGE_TIME)
            .copied()
            .unwrap_or(0);
        tick.state.set(types::SPEAR_ENGAGE_TIME, engage_time - 1);

        if let Some(away) = tick.state.get(types::SPEAR_CHARGE_POSITION).copied() {
            move_to(
                tick.mob,
                BlockPos::floored_v(away),
                speed * self.speed_modifier_when_repositioning,
            );
            if navigation_is_done(tick.mob) {
                tick.state.erase(types::SPEAR_CHARGE_POSITION.id());
            }
            return;
        }

        let target_entity = target.get_entity();
        move_to(
            tick.mob,
            target_entity.block_pos.load(),
            speed * self.speed_modifier_when_charging,
        );
        if distance_sq < self.target_in_range_radius_sq || navigation_is_done(tick.mob) {
            let distance = distance_sq.sqrt();
            let away = land_random_pos::get_pos_away_between(
                tick.mob,
                MIN_REPOSITION_DISTANCE + mount_distance - distance,
                MAX_REPOSITION_DISTANCE + mount_distance - distance,
                AWAY_VERTICAL_DIST,
                target_entity.pos.load(),
            );
            tick.state.set_optional(types::SPEAR_CHARGE_POSITION, away);
        }
    }

    fn stop(&mut self, tick: &mut BrainTick<'_>) {
        stop_navigation(tick.mob);
        tick.mob.get_mob_entity().living_entity.clear_active_hand();
        tick.state.erase(types::SPEAR_CHARGE_POSITION.id());
        tick.state.erase(types::SPEAR_ENGAGE_TIME.id());
        tick.state.set(types::SPEAR_STATUS, SpearStatus::Retreat);
    }

    fn debug_name(&self) -> &'static str {
        "SpearAttack"
    }
}

/// Vanilla `SpearRetreat`.
pub struct SpearRetreat {
    speed_modifier_when_repositioning: f64,
}

impl SpearRetreat {
    #[must_use]
    pub const fn new(speed_modifier_when_repositioning: f64) -> Self {
        Self {
            speed_modifier_when_repositioning,
        }
    }

    /// The place to flee to, far enough from the target to charge again.
    fn fleeing_position(tick: &BrainTick<'_>, target: &dyn EntityBase) -> Option<Vector3<f64>> {
        let distance = distance_sq_to(tick.mob, target).sqrt();
        let mount_distance = mounted_distance(tick.mob);
        land_random_pos::get_pos_away_between(
            tick.mob,
            (MIN_COOLDOWN_DISTANCE + mount_distance - distance).max(0.0),
            (MAX_COOLDOWN_DISTANCE + mount_distance - distance).max(1.0),
            AWAY_VERTICAL_DIST,
            target.get_entity().pos.load(),
        )
    }
}

impl Behavior for SpearRetreat {
    fn entry_conditions(&self) -> &[(MemoryModuleId, MemoryStatus)] {
        const CONDITIONS: [(MemoryModuleId, MemoryStatus); 1] =
            [(types::SPEAR_STATUS.id(), MemoryStatus::ValuePresent)];
        &CONDITIONS
    }

    fn min_duration(&self) -> i32 {
        MAX_FLEEING_TIME
    }

    fn max_duration(&self) -> i32 {
        MAX_FLEEING_TIME
    }

    fn check_extra_start_conditions(&mut self, tick: &mut BrainTick<'_>) -> bool {
        if !able_to_attack(tick)
            || is_using_item(tick.mob)
            || tick.state.get(types::SPEAR_STATUS).copied() != Some(SpearStatus::Retreat)
        {
            return false;
        }
        let Some(target) = attack_target(tick) else {
            return false;
        };
        let Some(away) = Self::fleeing_position(tick, target.as_ref()) else {
            return false;
        };
        tick.state.set(types::SPEAR_FLEEING_POSITION, away);
        true
    }

    fn start(&mut self, tick: &mut BrainTick<'_>) {
        tick.mob.get_mob_entity().set_attacking(true);
        tick.state.set(types::SPEAR_FLEEING_TIME, 0);
    }

    fn can_still_use(&mut self, tick: &mut BrainTick<'_>) -> bool {
        tick.state
            .get(types::SPEAR_FLEEING_TIME)
            .copied()
            .unwrap_or(MAX_FLEEING_TIME)
            < MAX_FLEEING_TIME
            && tick
                .state
                .has_memory_value(types::SPEAR_FLEEING_POSITION.id())
            && !navigation_is_done(tick.mob)
            && able_to_attack(tick)
    }

    fn tick(&mut self, tick: &mut BrainTick<'_>) {
        let Some(target) = attack_target(tick) else {
            return;
        };
        let speed = charge_speed_modifier(tick.mob) * self.speed_modifier_when_repositioning;
        look_at(tick, &target);
        let fleeing_time = tick
            .state
            .get(types::SPEAR_FLEEING_TIME)
            .copied()
            .unwrap_or(0);
        tick.state.set(types::SPEAR_FLEEING_TIME, fleeing_time + 1);
        if let Some(away) = tick.state.get(types::SPEAR_FLEEING_POSITION).copied() {
            move_to(tick.mob, BlockPos::floored_v(away), speed);
        }
    }

    fn stop(&mut self, tick: &mut BrainTick<'_>) {
        stop_navigation(tick.mob);
        let mob_entity = tick.mob.get_mob_entity();
        mob_entity.set_attacking(false);
        mob_entity.living_entity.clear_active_hand();
        tick.state.erase(types::SPEAR_FLEEING_TIME.id());
        tick.state.erase(types::SPEAR_FLEEING_POSITION.id());
        tick.state.erase(types::SPEAR_STATUS.id());
    }

    fn debug_name(&self) -> &'static str {
        "SpearRetreat"
    }
}
