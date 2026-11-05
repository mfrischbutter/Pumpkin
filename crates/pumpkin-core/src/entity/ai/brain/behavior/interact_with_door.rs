use pumpkin_data::tag::{self, Taggable};
use pumpkin_util::math::position::BlockPos;
use rustc_hash::FxHashSet;

use super::super::BrainTick;
use super::super::memory::{GlobalPos, MemoryStatus, types};
use super::{OneShot, poi};
use crate::block::blocks::doors::DoorBlock;
use crate::entity::EntityBase;
use crate::world::World;

/// Vanilla `InteractWithDoor.COOLDOWN_BEFORE_RERUNNING_IN_SAME_NODE`.
const COOLDOWN_BEFORE_RERUNNING_IN_SAME_NODE: i32 = 20;
/// Vanilla `InteractWithDoor.SKIP_CLOSING_DOOR_IF_FURTHER_AWAY_THAN`.
const SKIP_CLOSING_DOOR_IF_FURTHER_AWAY_THAN: f64 = 3.0;
/// Vanilla `InteractWithDoor.MAX_DISTANCE_TO_HOLD_DOOR_OPEN_FOR_OTHER_MOBS`.
const MAX_DISTANCE_TO_HOLD_DOOR_OPEN_FOR_OTHER_MOBS: f64 = 2.0;

fn is_mob_interactable_door(world: &World, pos: &BlockPos) -> bool {
    world
        .get_block(pos)
        .has_tag(&tag::Block::MINECRAFT_MOB_INTERACTABLE_DOORS)
}

/// Vanilla `InteractWithDoor.create`: opens the doors on the mob's path, closes them behind it.
#[must_use]
pub fn create() -> OneShot {
    let mut last_checked_node: Option<BlockPos> = None;
    let mut remaining_cooldown = 0;
    OneShot::new(
        "InteractWithDoor",
        vec![
            (types::PATH.id(), MemoryStatus::ValuePresent),
            (types::DOORS_TO_CLOSE.id(), MemoryStatus::Registered),
            (
                types::NEAREST_LIVING_ENTITIES.id(),
                MemoryStatus::Registered,
            ),
        ],
        move |tick| {
            let Some(path) = tick.state.get(types::PATH).cloned() else {
                return false;
            };
            if path.not_started() || path.is_done() {
                return false;
            }
            let (Some(from_node), Some(to_node)) = (
                path.get_previous_node().map(|node| node.pos),
                path.get_next_node().map(|node| node.pos),
            ) else {
                return false;
            };
            if last_checked_node == Some(to_node) {
                remaining_cooldown = COOLDOWN_BEFORE_RERUNNING_IN_SAME_NODE;
            } else {
                remaining_cooldown -= 1;
                if remaining_cooldown > 0 {
                    return false;
                }
            }
            last_checked_node = Some(to_node);
            open_doors_on_path(tick, from_node, to_node);
            true
        },
    )
}

/// The door handling of vanilla `InteractWithDoor.create`, once per path node.
fn open_doors_on_path(tick: &mut BrainTick<'_>, from_node: BlockPos, to_node: BlockPos) {
    let world = tick.world;
    let mut doors = tick.state.get(types::DOORS_TO_CLOSE).cloned();
    if is_mob_interactable_door(world, &from_node) {
        if !DoorBlock::is_open(world, &from_node) {
            DoorBlock::set_open(world, &from_node, true);
        }
        doors
            .get_or_insert_default()
            .insert(poi::global_pos(world, from_node));
    }
    if is_mob_interactable_door(world, &to_node) && !DoorBlock::is_open(world, &to_node) {
        DoorBlock::set_open(world, &to_node, true);
        doors
            .get_or_insert_default()
            .insert(poi::global_pos(world, to_node));
    }
    if let Some(mut doors) = doors {
        close_doors_that_i_have_opened_or_passed_through(
            tick,
            Some(from_node),
            Some(to_node),
            &mut doors,
        );
        tick.state.set(types::DOORS_TO_CLOSE, doors);
    }
}

/// What vanilla `SleepInBed.start` does with the doors a sleeper still holds open.
pub fn close_doors_when_going_to_bed(tick: &mut BrainTick<'_>) {
    if let Some(mut doors) = tick.state.get(types::DOORS_TO_CLOSE).cloned() {
        close_doors_that_i_have_opened_or_passed_through(tick, None, None, &mut doors);
        tick.state.set(types::DOORS_TO_CLOSE, doors);
    }
}

/// Vanilla `closeDoorsThatIHaveOpenedOrPassedThrough`.
fn close_doors_that_i_have_opened_or_passed_through(
    tick: &BrainTick<'_>,
    moving_from_node: Option<BlockPos>,
    moving_to_node: Option<BlockPos>,
    doors: &mut FxHashSet<GlobalPos>,
) {
    let world = tick.world;
    doors.retain(|door| {
        if Some(door.pos) == moving_from_node || Some(door.pos) == moving_to_node {
            return true;
        }
        if !is_door_too_far_away(tick, door)
            && is_mob_interactable_door(world, &door.pos)
            && DoorBlock::is_open(world, &door.pos)
            && !are_other_mobs_coming_through_door(tick, door.pos)
        {
            DoorBlock::set_open(world, &door.pos, false);
        }
        false
    });
}

/// Vanilla `isDoorTooFarAway`.
fn is_door_too_far_away(tick: &BrainTick<'_>, door: &GlobalPos) -> bool {
    !poi::is_in_dimension(tick.world, door)
        || door
            .pos
            .to_centered_f64()
            .squared_distance_to_vec(&tick.mob.get_entity().pos.load())
            >= SKIP_CLOSING_DOOR_IF_FURTHER_AWAY_THAN * SKIP_CLOSING_DOOR_IF_FURTHER_AWAY_THAN
}

/// Vanilla `areOtherMobsComingThroughDoor`.
fn are_other_mobs_coming_through_door(tick: &BrainTick<'_>, door_pos: BlockPos) -> bool {
    let Some(nearest) = tick.state.get(types::NEAREST_LIVING_ENTITIES) else {
        return false;
    };
    let own_type = tick.mob.get_entity().entity_type;
    let center = door_pos.to_centered_f64();
    nearest.iter().any(|other| {
        let other_entity = other.get_entity();
        other_entity.entity_type == own_type
            && center.squared_distance_to_vec(&other_entity.pos.load())
                < MAX_DISTANCE_TO_HOLD_DOOR_OPEN_FOR_OTHER_MOBS
                    * MAX_DISTANCE_TO_HOLD_DOOR_OPEN_FOR_OTHER_MOBS
            && is_mob_coming_through_door(other.as_ref(), door_pos)
    })
}

/// Vanilla `isMobComingThroughDoor` reads the other mob's `PATH` memory. Its navigator holds the
/// same path, and taking the other brain's lock from inside this brain's tick could deadlock.
fn is_mob_coming_through_door(other: &dyn EntityBase, door_pos: BlockPos) -> bool {
    let Some(other) = other.get_mob() else {
        return false;
    };
    let navigator = other
        .get_mob_entity()
        .navigator
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(path) = navigator.get_path().filter(|path| !path.is_done()) else {
        return false;
    };
    let moving_from_node = path.get_previous_node().map(|node| node.pos);
    let moving_to_node = path.get_next_node().map(|node| node.pos);
    moving_from_node == Some(door_pos) || moving_to_node == Some(door_pos)
}
