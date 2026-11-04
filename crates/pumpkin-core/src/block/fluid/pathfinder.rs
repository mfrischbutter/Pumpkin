use super::physics;
use crate::block::fluid::flowing_trait::FlowingFluid;
use crate::world::World;
use pumpkin_data::BlockStateId;
use pumpkin_data::{
    BlockDirection,
    fluid::{Fluid, FluidProperties},
};
use pumpkin_util::math::position::BlockPos;
use std::sync::Arc;

/// Represents a node in the BFS pathfinding queue for fluid flow calculation.
#[derive(Clone, Copy)]
pub struct PathNode {
    pub pos: BlockPos,
    pub distance: i32,
    pub exclude_dir: BlockDirection,
}

/// Vanilla `FlowingFluid.SpreadContext.isHole`: whether the fluid can flow down from `pos`.
fn is_hole<T: FlowingFluid + Sync + ?Sized>(
    fluid_impl: &T,
    world: &Arc<World>,
    fluid: &Fluid,
    pos: &BlockPos,
) -> bool {
    fluid_impl.is_water_hole(fluid, world.get_block_state_id(&pos.down()))
}

/// Determines valid spread directions for fluid flow using hole-first priority.
///
/// - Holes (downward flow opportunities) get distance 0 priority
/// - All directions with equal minimum distance are returned
/// - Returns up to 4 directions with their computed fluid states
pub fn get_spread<T: FlowingFluid + Sync + ?Sized>(
    fluid_impl: &T,
    world: &Arc<World>,
    fluid: &Fluid,
    block_pos: &BlockPos,
) -> ([(BlockDirection, BlockStateId); 4], usize) {
    let mut min_dist = 1000;
    let mut result = [(BlockDirection::North, BlockStateId::default()); 4];
    let mut result_count = 0;

    for direction in [
        BlockDirection::North,
        BlockDirection::South,
        BlockDirection::West,
        BlockDirection::East,
    ] {
        let side_pos = block_pos.offset(direction.to_offset());
        let side_state_id = world.get_block_state_id(&side_pos);

        if !fluid_impl.can_maybe_pass_through(fluid, side_state_id) {
            continue;
        }

        // Skip if no valid fluid state for this position
        let Some(new_fluid_props) = fluid_impl.get_new_liquid(world, fluid, &side_pos) else {
            continue;
        };
        if !physics::can_hold_specific_fluid(
            side_state_id,
            fluid_impl.fluid_type(fluid, &new_fluid_props),
        ) {
            continue;
        }

        let new_state_id = new_fluid_props.to_state_id(fluid);

        // Holes get distance 0
        let slope_dist = if is_hole(fluid_impl, world, fluid, &side_pos) {
            0
        } else {
            get_in_flow_down_distance_iterative(
                fluid_impl,
                world,
                fluid,
                side_pos,
                direction.opposite(),
            )
        };

        // Clear results if we find a shorter path
        if slope_dist < min_dist {
            result_count = 0;
        }

        // Add all directions with equal minimum distance
        if slope_dist <= min_dist {
            if physics::can_be_replaced_with(world, &side_pos, fluid, direction) && result_count < 4
            {
                result[result_count] = (direction, new_state_id);
                result_count += 1;
            }

            min_dist = slope_dist;
        }
    }
    (result, result_count)
}

/// Performs iterative BFS search to find the shortest distance to a downward flow opportunity.
///
/// Uses stack-allocated array for zero heap allocations. Searches up to `get_max_flow_distance`
/// (dynamic) horizontally from the starting position.
///
/// # Returns
/// Distance to nearest hole, or 1000 if no hole found within search distance
pub fn get_in_flow_down_distance_iterative<T: FlowingFluid + Sync + ?Sized>(
    fluid_impl: &T,
    world: &Arc<World>,
    fluid: &Fluid,
    start_pos: BlockPos,
    initial_exclude_dir: BlockDirection,
) -> i32 {
    const MAX_QUEUE_SIZE: usize = 64;

    let mut queue: [PathNode; MAX_QUEUE_SIZE] = [PathNode {
        pos: BlockPos::new(0, 0, 0),
        distance: 0,
        exclude_dir: BlockDirection::North,
    }; MAX_QUEUE_SIZE];

    let mut queue_start = 0;
    let mut queue_end = 0;

    queue[queue_end] = PathNode {
        pos: start_pos,
        distance: 1,
        exclude_dir: initial_exclude_dir,
    };
    queue_end = 1;

    let mut visited_bitset = [0u64; 4];
    let slope_find_distance = fluid_impl.get_max_flow_distance(world);

    let get_bit_index = |pos: BlockPos| -> Option<usize> {
        let dx = pos.0.x - start_pos.0.x + slope_find_distance;
        let dz = pos.0.z - start_pos.0.z + slope_find_distance;
        let grid_size = slope_find_distance * 2 + 1;
        (dx >= 0 && dx < grid_size && dz >= 0 && dz < grid_size)
            .then(|| (dz * grid_size + dx) as usize)
    };

    while queue_start < queue_end {
        let node = queue[queue_start];
        queue_start += 1;

        if node.distance > slope_find_distance {
            continue;
        }

        if let Some(bit_idx) = get_bit_index(node.pos) {
            let word_idx = bit_idx / 64;
            let bit_pos = bit_idx % 64;
            if (visited_bitset[word_idx] & (1u64 << bit_pos)) != 0 {
                continue;
            }
            visited_bitset[word_idx] |= 1u64 << bit_pos;
        }

        // Check for hole (downward flow opportunity)
        if is_hole(fluid_impl, world, fluid, &node.pos) {
            return node.distance;
        }

        for direction in [
            BlockDirection::North,
            BlockDirection::South,
            BlockDirection::West,
            BlockDirection::East,
        ] {
            if direction == node.exclude_dir {
                continue;
            }

            let next_pos = node.pos.offset(direction.to_offset());
            if !fluid_impl.can_pass_through(fluid, world.get_block_state_id(&next_pos)) {
                continue;
            }

            if queue_end < MAX_QUEUE_SIZE {
                queue[queue_end] = PathNode {
                    pos: next_pos,
                    distance: node.distance + 1,
                    exclude_dir: direction.opposite(),
                };
                queue_end += 1;
            }
        }
    }

    1000
}
