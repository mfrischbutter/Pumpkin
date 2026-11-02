use pumpkin_data::Block;
use pumpkin_data::tag::{self, Taggable};
use pumpkin_util::math::position::{BlockPos, OutwardIterator};

use super::super::memory::{MemoryStatus, types};
use super::{OneShot, utils};
use crate::entity::mob::Mob;
use crate::world::World;

/// Vanilla `TryFindLiquid`: a mob out of the liquid walks to the nearest liquid block, every 2
/// seconds at most.
#[must_use]
pub fn try_find_liquid(range: i32, speed_modifier: f32, fluid_tag: &'static tag::Tag) -> OneShot {
    let mut next_ok_start_time = 0;
    OneShot::new(
        "TryFindLiquid",
        vec![
            (types::ATTACK_TARGET.id(), MemoryStatus::ValueAbsent),
            (types::WALK_TARGET.id(), MemoryStatus::ValueAbsent),
            (types::LOOK_TARGET.id(), MemoryStatus::Registered),
        ],
        move |tick| {
            let entity = tick.mob.get_entity();
            let world = entity.world.load();
            if world
                .get_exact_fluid(&entity.block_pos.load())
                .has_tag(fluid_tag)
            {
                return false;
            }
            if tick.time < next_ok_start_time {
                next_ok_start_time = tick.time + 22;
                return true;
            }
            if let Some(pos) = find_liquid(tick.mob, &world, range, fluid_tag) {
                utils::set_walk_and_look_target_memories_to_block(
                    tick.state,
                    pos,
                    speed_modifier,
                    0,
                );
            }
            next_ok_start_time = tick.time + 40;
            true
        },
    )
}

/// The nearest liquid block with air above, else the nearest submerged one a bit away.
fn find_liquid(
    mob: &dyn Mob,
    world: &World,
    range: i32,
    fluid_tag: &'static tag::Tag,
) -> Option<BlockPos> {
    let entity = mob.get_entity();
    let center = entity.block_pos.load();
    let position = entity.pos.load();
    let mut found = None;
    for pos in OutwardIterator::new(center, range, range, range) {
        if pos.0.x == center.0.x && pos.0.z == center.0.z {
            continue;
        }
        let block = world.get_block(&pos);
        let is_liquid = block.id == Block::WATER.id || block.id == Block::LAVA.id;
        if !is_liquid || !world.get_exact_fluid(&pos).has_tag(fluid_tag) {
            continue;
        }
        if world.get_block_state(&pos.up()).is_air() {
            return Some(pos);
        }
        if found.is_none() && pos.to_centered_f64().squared_distance_to_vec(&position) >= 2.25 {
            found = Some(pos);
        }
    }
    found
}
