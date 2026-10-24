use std::sync::Arc;

use pumpkin_data::{Block, BlockStateId};
use pumpkin_macros::{Event, cancellable};
use pumpkin_util::math::position::BlockPos;

use crate::world::World;

use super::BlockEvent;

/// An event that occurs before a scheduled tick of a hooked block runs.
///
/// Fired only for blocks registered with `Context::register_scheduled_tick_hook`. Cancelling
/// it skips the block's scheduled tick; the tick is consumed and not rescheduled.
#[cancellable]
#[derive(Event, Clone)]
pub struct BlockScheduledTickEvent {
    /// The world the block is in.
    pub world: Arc<World>,

    /// The position of the ticked block.
    pub block_pos: BlockPos,

    /// The block state id at the position when the tick runs.
    pub state_id: BlockStateId,
}

impl BlockScheduledTickEvent {
    /// Creates a new `BlockScheduledTickEvent`.
    #[must_use]
    pub const fn new(world: Arc<World>, block_pos: BlockPos, state_id: BlockStateId) -> Self {
        Self {
            world,
            block_pos,
            state_id,
            cancelled: false,
        }
    }
}

impl BlockEvent for BlockScheduledTickEvent {
    fn get_block(&self) -> &Block {
        Block::from_state_id(self.state_id)
    }
}
