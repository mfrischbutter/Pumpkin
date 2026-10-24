use std::sync::Arc;

use pumpkin_data::{Block, BlockDirection, BlockStateId};
use pumpkin_macros::{Event, cancellable};
use pumpkin_util::math::position::BlockPos;

use crate::world::World;

use super::BlockEvent;

/// An event that occurs before a shape update changes a block state that a plugin watches.
///
/// Fired only for states registered with `Context::watch_block_states`. Cancelling it keeps
/// the old state; changing `new_state_id` sets that state instead.
#[cancellable]
#[derive(Event, Clone)]
pub struct BlockShapeUpdateEvent {
    /// The world the block is in.
    pub world: Arc<World>,

    /// The position of the updated block.
    pub block_pos: BlockPos,

    /// The current block state id.
    pub old_state_id: BlockStateId,

    /// The block state id the shape update sets.
    pub new_state_id: BlockStateId,

    /// The direction from the updated block to the neighbour that changed.
    pub direction: BlockDirection,

    /// The position of the neighbour that changed.
    pub neighbor_pos: BlockPos,
}

impl BlockShapeUpdateEvent {
    /// Creates a new `BlockShapeUpdateEvent`.
    #[must_use]
    pub const fn new(
        world: Arc<World>,
        block_pos: BlockPos,
        old_state_id: BlockStateId,
        new_state_id: BlockStateId,
        direction: BlockDirection,
        neighbor_pos: BlockPos,
    ) -> Self {
        Self {
            world,
            block_pos,
            old_state_id,
            new_state_id,
            direction,
            neighbor_pos,
            cancelled: false,
        }
    }
}

impl BlockEvent for BlockShapeUpdateEvent {
    fn get_block(&self) -> &Block {
        Block::from_state_id(self.old_state_id)
    }
}
