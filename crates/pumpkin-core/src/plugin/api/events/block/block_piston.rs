use std::sync::Arc;

use pumpkin_macros::{Event, cancellable};
use pumpkin_util::math::position::BlockPos;

use crate::world::World;

/// An event that occurs when a piston extends.
#[cancellable]
#[derive(Event, Clone)]
pub struct BlockPistonExtendEvent {
    /// The world the piston extends in.
    pub world: Arc<World>,
    pub block_pos: BlockPos,
    pub direction: String,
}

impl BlockPistonExtendEvent {
    #[must_use]
    pub const fn new(world: Arc<World>, block_pos: BlockPos, direction: String) -> Self {
        Self {
            world,
            block_pos,
            direction,
            cancelled: false,
        }
    }
}

/// An event that occurs when a piston retracts.
#[cancellable]
#[derive(Event, Clone)]
pub struct BlockPistonRetractEvent {
    /// The world the piston retracts in.
    pub world: Arc<World>,
    pub block_pos: BlockPos,
    pub direction: String,
}

impl BlockPistonRetractEvent {
    #[must_use]
    pub const fn new(world: Arc<World>, block_pos: BlockPos, direction: String) -> Self {
        Self {
            world,
            block_pos,
            direction,
            cancelled: false,
        }
    }
}
