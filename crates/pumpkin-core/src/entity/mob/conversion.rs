//! Vanilla `Mob.convertTo` with `ConversionType` and `ConversionParams`.
//!
//! One mob turns into another of a different type (a skeleton freezing into a stray, a
//! piglin zombifying), keeping what vanilla carries over.

use std::sync::Arc;
use std::sync::atomic::Ordering::Relaxed;
use std::sync::atomic::{AtomicBool, AtomicI32};

use pumpkin_data::data_component_impl::EquipmentSlot;
use pumpkin_data::entity::EntityType;
use pumpkin_data::tracked_data::TrackedData;
use pumpkin_data::world::WorldEvent;
use pumpkin_nbt::compound::NbtCompound;

use crate::entity::mob::Mob;
use crate::entity::{Entity, EntityBase};

/// Vanilla `ConversionType`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ConversionType {
    /// The old mob is replaced by the new one in its place.
    Single,
    /// The old mob stays (it is dying) and the new one spawns next to it.
    SplitOnDeath,
}

/// Vanilla `ConversionParams`, without the team (Pumpkin has no scoreboard teams on mobs).
#[derive(Clone, Copy, Debug)]
pub struct ConversionParams {
    pub kind: ConversionType,
    pub keep_equipment: bool,
    pub preserve_can_pick_up_loot: bool,
}

impl ConversionParams {
    /// Vanilla `ConversionParams.single`.
    #[must_use]
    pub const fn single(keep_equipment: bool, preserve_can_pick_up_loot: bool) -> Self {
        Self {
            kind: ConversionType::Single,
            keep_equipment,
            preserve_can_pick_up_loot,
        }
    }
}

/// Vanilla `Mob.convertTo`.
///
/// Creates a mob of `to_type`, carries `from` over by `params`, lets `after` adjust it,
/// spawns it and, for a single conversion, removes `from`. `None` when `from` is already
/// removed or `to_type` is not a mob.
pub fn convert_to(
    from: &dyn Mob,
    to_type: &'static EntityType,
    params: ConversionParams,
    after: impl FnOnce(&dyn Mob),
) -> Option<Arc<dyn EntityBase>> {
    let from_entity = from.get_entity();
    if from_entity.is_removed() {
        return None;
    }
    let world = from_entity.world.load_full();
    let converted = crate::entity::r#type::from_type(
        to_type,
        from_entity.pos.load(),
        &world,
        uuid::Uuid::new_v4(),
    );
    let to = converted.get_mob()?;

    if params.kind == ConversionType::Single {
        convert_single(from, to, params);
    }
    convert_common(from, to, params);
    after(to);

    let vehicle = (params.kind == ConversionType::Single)
        .then(|| from_entity.get_vehicle())
        .flatten();
    let first_passenger = from_entity
        .passengers
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .first()
        .cloned();
    let leash_holder = from_entity.get_leash_holder();

    world.spawn_entity(converted.clone());

    if params.kind == ConversionType::Single {
        // The rider moves over to the new mob, which takes the old one's seat.
        if let Some(passenger) = first_passenger {
            from_entity.remove_passenger(passenger.get_entity().entity_id);
            to.get_entity().add_passenger(converted.clone(), passenger);
        }
        if let Some(vehicle) = vehicle {
            vehicle.get_entity().remove_passenger(from_entity.entity_id);
            vehicle
                .get_entity()
                .add_passenger(vehicle.clone(), converted.clone());
        }
        if let Some(holder) = leash_holder {
            to.get_entity().leash_to(holder);
        }
        from_entity.remove();
    } else if let Some(passenger) = first_passenger {
        from_entity.remove_passenger(passenger.get_entity().entity_id);
    }

    Some(converted)
}

/// Vanilla `ConversionType.SINGLE.convert`, before the common part.
fn convert_single(from: &dyn Mob, to: &dyn Mob, params: ConversionParams) {
    let from_entity = from.get_entity();
    let to_entity = to.get_entity();
    to_entity.set_pos(from_entity.pos.load());
    to_entity.set_rotation(from_entity.yaw.load(), from_entity.pitch.load());
    to_entity.head_yaw.store(from_entity.head_yaw.load());
    to_entity.body_yaw.store(from_entity.body_yaw.load());
    to_entity.velocity.store(from_entity.velocity.load());

    if params.keep_equipment {
        let from_mob = from.get_mob_entity();
        let to_mob = to.get_mob_entity();
        let taken: Vec<(EquipmentSlot, _)> = from_mob
            .living_entity
            .entity_equipment
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .equipment
            .drain()
            .filter(|(_, stack)| !stack.is_empty())
            .collect();
        for (slot, stack) in taken {
            to_mob
                .living_entity
                .equipment_drop_chances
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .insert(slot.clone(), from_mob.drop_chance(&slot));
            to_mob
                .living_entity
                .entity_equipment
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .put(&slot, stack);
        }
    }

    let from_living = &from.get_mob_entity().living_entity;
    let to_living = &to.get_mob_entity().living_entity;
    to_living
        .fall_distance
        .store(from_living.fall_distance.load());
    // `set_fall_flying` only takes a change.
    if from_entity.is_fall_flying() != to_entity.is_fall_flying() {
        to_entity.set_fall_flying(from_entity.is_fall_flying());
    }
    to_living.last_hurt_by_player_memory_time.store(
        from_living.last_hurt_by_player_memory_time.load(Relaxed),
        Relaxed,
    );
    to_living
        .last_hurt_by_player
        .store(from_living.last_hurt_by_player.load());
    to_entity
        .on_ground
        .store(from_entity.on_ground.load(Relaxed), Relaxed);
}

/// Vanilla `ConversionType.convertCommon`.
fn convert_common(from: &dyn Mob, to: &dyn Mob, params: ConversionParams) {
    let from_mob = from.get_mob_entity();
    let to_mob = to.get_mob_entity();
    let from_entity = from.get_entity();
    let to_entity = to.get_entity();

    to_mob
        .living_entity
        .set_absorption(from_mob.living_entity.absorption.load());
    let effects: Vec<_> = from_mob
        .living_entity
        .active_effects
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .values()
        .cloned()
        .collect();
    for effect in effects {
        to_mob.living_entity.add_effect(effect);
    }

    if from_mob.living_entity.is_baby() {
        let _ = to.spawn_as_baby();
    }
    if let (Some(old), Some(new)) = (from.as_ageable(), to.as_ageable()) {
        new.set_age(old.get_age());
        let (old_data, new_data) = (old.get_ageable_data(), new.get_ageable_data());
        new_data
            .forced_age
            .store(old_data.forced_age.load(Relaxed), Relaxed);
        new_data
            .forced_age_timer
            .store(old_data.forced_age_timer.load(Relaxed), Relaxed);
    }

    if params.preserve_can_pick_up_loot {
        to_mob.set_can_pick_up_loot(from_mob.can_pick_up_loot());
    }
    to_mob.set_left_handed(from_mob.is_left_handed());
    to_mob.set_no_ai(from_mob.is_no_ai());
    if from_mob.persistence_required.load(Relaxed) {
        to_mob.set_persistence_required();
    }

    to_entity.set_custom_name_visible(from_entity.custom_name_visible.load(Relaxed));
    if from_entity.is_on_fire() {
        to_entity.set_on_fire(true);
    }
    to_entity.set_invulnerable(from_entity.invulnerable.load(Relaxed));
    // Vanilla `setInvulnerableTime`.
    to_mob
        .living_entity
        .hurt_cooldown
        .store(from_mob.living_entity.hurt_cooldown.load(Relaxed), Relaxed);
    to_entity.set_has_no_gravity(from_entity.has_no_gravity());
    to_entity
        .portal_cooldown
        .store(from_entity.portal_cooldown.load(Relaxed), Relaxed);
    to_entity.set_silent(from_entity.is_silent());
    to_entity
        .scoreboard_tags
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .extend(
            from_entity
                .scoreboard_tags
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .iter()
                .cloned(),
        );
    // Vanilla `COMPONENTS_TO_COPY`: the custom name and the custom data.
    if let Some(name) = &**from_entity.custom_name.load() {
        to_entity.set_custom_name(name.clone());
    }
    let custom_data = from_entity
        .custom_data
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    *to_entity
        .custom_data
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = custom_data;
}

/// Vanilla `ConversionTracker`.
///
/// A mob afflicted long enough (a skeleton in powder snow) starts converting, shown to
/// clients by a synced flag, and converts when the time runs out.
pub struct ConversionTracker {
    converts_to: &'static EntityType,
    conversion_event: WorldEvent,
    converting_data: TrackedData,
    is_afflicted: fn(&dyn Mob) -> bool,
    affliction_tag: &'static str,
    total_affliction_time: i32,
    conversion_tag: &'static str,
    total_conversion_time: i32,
    affliction_time: AtomicI32,
    conversion_time: AtomicI32,
    converting: AtomicBool,
}

impl ConversionTracker {
    /// `affliction` and `conversion` are each the NBT tag and the total time in ticks.
    #[must_use]
    pub const fn new(
        converts_to: &'static EntityType,
        conversion_event: WorldEvent,
        converting_data: TrackedData,
        is_afflicted: fn(&dyn Mob) -> bool,
        affliction: (&'static str, i32),
        conversion: (&'static str, i32),
    ) -> Self {
        Self {
            converts_to,
            conversion_event,
            converting_data,
            is_afflicted,
            affliction_tag: affliction.0,
            total_affliction_time: affliction.1,
            conversion_tag: conversion.0,
            total_conversion_time: conversion.1,
            affliction_time: AtomicI32::new(0),
            conversion_time: AtomicI32::new(0),
            converting: AtomicBool::new(false),
        }
    }

    pub fn tick(&self, mob: &dyn Mob) {
        if !mob.get_entity().is_alive() || mob.get_mob_entity().is_no_ai() {
            return;
        }
        if (self.is_afflicted)(mob) {
            if self.is_converting() {
                if self.conversion_time.fetch_sub(1, Relaxed) - 1 < 0 {
                    self.do_conversion(mob);
                }
            } else if self.affliction_time.fetch_add(1, Relaxed) + 1 >= self.total_affliction_time {
                self.start_conversion(mob.get_entity(), self.total_conversion_time);
            }
        } else {
            self.affliction_time.store(-1, Relaxed);
            self.set_converting(mob.get_entity(), false);
        }
    }

    #[must_use]
    pub fn is_converting(&self) -> bool {
        self.converting.load(Relaxed)
    }

    pub fn start_conversion(&self, entity: &Entity, time: i32) {
        self.conversion_time.store(time, Relaxed);
        self.set_converting(entity, true);
    }

    fn set_converting(&self, entity: &Entity, converting: bool) {
        if self.converting.swap(converting, Relaxed) != converting {
            self.sync(entity);
        }
    }

    /// Sends the converting flag to tracking clients.
    pub fn sync(&self, entity: &Entity) {
        entity.set_synced_data(self.converting_data, self.is_converting());
    }

    fn do_conversion(&self, mob: &dyn Mob) {
        let Some(converted) = convert_to(
            mob,
            self.converts_to,
            ConversionParams::single(true, true),
            |_| {},
        ) else {
            return;
        };
        let entity = converted.get_entity();
        if !entity.is_silent() {
            entity
                .world
                .load()
                .sync_world_event(self.conversion_event, entity.block_pos.load(), 0);
        }
    }

    pub fn write_nbt(&self, mob: &dyn Mob, nbt: &mut NbtCompound) {
        nbt.put_int(
            self.conversion_tag,
            if self.is_converting() {
                self.conversion_time.load(Relaxed)
            } else {
                -1
            },
        );
        nbt.put_int(
            self.affliction_tag,
            if (self.is_afflicted)(mob) {
                self.affliction_time.load(Relaxed)
            } else {
                -1
            },
        );
    }

    pub fn read_nbt(&self, entity: &Entity, nbt: &NbtCompound) {
        self.affliction_time
            .store(nbt.get_int(self.affliction_tag).unwrap_or(0), Relaxed);
        match nbt.get_int(self.conversion_tag).unwrap_or(-1) {
            -1 => self.set_converting(entity, false),
            time => self.start_conversion(entity, time),
        }
    }
}
