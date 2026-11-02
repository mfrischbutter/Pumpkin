use pumpkin_data::tag::{self, Taggable};
use pumpkin_data::{BlockDirection, BlockState};
use pumpkin_util::math::position::{BlockPos, OutwardIterator};

use super::super::memory::{MemoryStatus, types};
use super::{OneShot, utils};
use crate::world::World;

/// Vanilla `TryFindLand.COOLDOWN_TICKS`.
const FIND_LAND_COOLDOWN: i64 = 60;
/// The wait of vanilla `TryFindLandNearLiquid`.
const FIND_LAND_NEAR_LIQUID_COOLDOWN: i64 = 40;

fn has_no_collision(state: &BlockState) -> bool {
    state.get_block_collision_shapes().next().is_none()
}

fn horizontal_neighbours(pos: BlockPos) -> [BlockPos; 4] {
    [
        pos.offset(BlockDirection::North.to_offset()),
        pos.offset(BlockDirection::South.to_offset()),
        pos.offset(BlockDirection::West.to_offset()),
        pos.offset(BlockDirection::East.to_offset()),
    ]
}

/// Vanilla `TryFindLand.canStandOn`.
fn can_stand_on(world: &World, pos: &BlockPos) -> bool {
    has_no_collision(world.get_block_state(pos))
        && world
            .get_block_state(&pos.down())
            .is_side_solid(BlockDirection::Up)
}

/// The first of the outward walk from `center` that is not straight above or below it.
fn first_beside(
    center: BlockPos,
    range: i32,
    matches: impl Fn(&BlockPos) -> bool,
) -> Option<BlockPos> {
    OutwardIterator::new(center, range, range, range)
        .filter(|pos| pos.0.x != center.0.x || pos.0.z != center.0.z)
        .find(|pos| matches(pos))
}

/// Vanilla `TryFindLand`: a mob in water walks to the nearest spot to stand on, every 3 seconds
/// at most.
#[must_use]
pub fn try_find_land(range: i32, speed_modifier: f32) -> OneShot {
    let mut next_ok_start_time = 0;
    OneShot::new(
        "TryFindLand",
        vec![
            (types::ATTACK_TARGET.id(), MemoryStatus::ValueAbsent),
            (types::WALK_TARGET.id(), MemoryStatus::ValueAbsent),
            (types::LOOK_TARGET.id(), MemoryStatus::Registered),
        ],
        move |tick| {
            let center = tick.mob.get_entity().block_pos.load();
            let world = tick.world;
            if !world
                .get_fluid(&center)
                .has_tag(&tag::Fluid::MINECRAFT_WATER)
            {
                return false;
            }
            if tick.time >= next_ok_start_time
                && let Some(pos) = first_beside(center, range, |pos| {
                    *world.get_fluid(pos) == pumpkin_data::fluid::Fluid::EMPTY
                        && can_stand_on(world, pos)
                })
            {
                utils::set_walk_and_look_target_memories_to_block(
                    tick.state,
                    pos,
                    speed_modifier,
                    1,
                );
            }
            next_ok_start_time = tick.time + FIND_LAND_COOLDOWN;
            true
        },
    )
}

/// The lambda of vanilla `TryFindLandNearLiquid`: room to stand, ground below, liquid beside.
fn is_land_near_liquid(world: &World, pos: &BlockPos, liquid: &'static tag::Tag) -> bool {
    has_no_collision(world.get_block_state(pos))
        && !has_no_collision(world.get_block_state(&pos.down()))
        && horizontal_neighbours(*pos).iter().any(|beside| {
            world.get_block_state(beside).is_air()
                && world.get_exact_fluid(&beside.down()).has_tag(liquid)
        })
}

/// Vanilla `TryFindLandNearLiquid`: a mob out of the liquid walks to a shore.
#[must_use]
pub fn try_find_land_near_liquid(
    range: i32,
    speed_modifier: f32,
    liquid: &'static tag::Tag,
) -> OneShot {
    let mut next_ok_start_time = 0;
    OneShot::new(
        "TryFindLandNearLiquid",
        vec![
            (types::ATTACK_TARGET.id(), MemoryStatus::ValueAbsent),
            (types::WALK_TARGET.id(), MemoryStatus::ValueAbsent),
            (types::LOOK_TARGET.id(), MemoryStatus::Registered),
        ],
        move |tick| {
            let center = tick.mob.get_entity().block_pos.load();
            let world = tick.world;
            if world.get_exact_fluid(&center).has_tag(liquid) {
                return false;
            }
            if tick.time >= next_ok_start_time
                && let Some(pos) =
                    first_beside(center, range, |pos| is_land_near_liquid(world, pos, liquid))
            {
                utils::set_walk_and_look_target_memories_to_block(
                    tick.state,
                    pos,
                    speed_modifier,
                    0,
                );
            }
            next_ok_start_time = tick.time + FIND_LAND_NEAR_LIQUID_COOLDOWN;
            true
        },
    )
}
