use std::sync::Arc;

use pumpkin_util::math::position::BlockPos;

use super::OneShot;
use crate::entity::ai::brain::memory::position_tracker::{BlockPosTracker, PositionTracker};
use crate::entity::ai::brain::memory::walk_target::WalkTarget;
use crate::entity::ai::brain::memory::{MemoryModuleType, MemoryStatus, types};

/// Vanilla `GoToTargetLocation`: walks near a remembered place, unless close enough to it.
#[must_use]
pub fn go_to_target_location(
    memory: MemoryModuleType<BlockPos>,
    close_enough_distance: i32,
    speed_modifier: f32,
) -> OneShot {
    let close_enough_sq = f64::from(close_enough_distance * close_enough_distance);
    OneShot::new(
        "GoToTargetLocation",
        vec![
            (memory.id(), MemoryStatus::ValuePresent),
            (types::ATTACK_TARGET.id(), MemoryStatus::ValueAbsent),
            (types::WALK_TARGET.id(), MemoryStatus::ValueAbsent),
            (types::LOOK_TARGET.id(), MemoryStatus::Registered),
        ],
        move |tick| {
            let Some(location) = tick.state.get(memory).copied() else {
                return false;
            };
            let here = tick.mob.get_entity().block_pos.load();
            let (dx, dy, dz) = (
                f64::from(location.0.x - here.0.x),
                f64::from(location.0.y - here.0.y),
                f64::from(location.0.z - here.0.z),
            );
            if dx * dx + dy * dy + dz * dz >= close_enough_sq {
                let nearby = BlockPos::new(
                    location.0.x + rand::random_range(-1..=1),
                    location.0.y,
                    location.0.z + rand::random_range(-1..=1),
                );
                tick.state.set(
                    types::LOOK_TARGET,
                    Arc::new(BlockPosTracker::new(nearby)) as Arc<dyn PositionTracker>,
                );
                tick.state.set(
                    types::WALK_TARGET,
                    WalkTarget::from_block_pos(nearby, speed_modifier, 0),
                );
            }
            true
        },
    )
}
