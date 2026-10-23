use std::sync::Arc;

use pumpkin_macros::{Event, cancellable};
use pumpkin_util::math::position::BlockPos;

use crate::world::World;

/// An event that occurs when a piston extends.
///
/// Fired once the piston knows which blocks it pushes, before any of them move.
#[cancellable]
#[derive(Event, Clone)]
pub struct BlockPistonExtendEvent {
    /// The world the piston extends in.
    pub world: Arc<World>,
    pub block_pos: BlockPos,
    pub direction: String,
    /// The positions of the blocks the piston pushes, before they move.
    pub blocks: Vec<BlockPos>,
    /// The positions of the blocks the piston breaks.
    pub broken: Vec<BlockPos>,
}

impl BlockPistonExtendEvent {
    #[must_use]
    pub const fn new(
        world: Arc<World>,
        block_pos: BlockPos,
        direction: String,
        blocks: Vec<BlockPos>,
        broken: Vec<BlockPos>,
    ) -> Self {
        Self {
            world,
            block_pos,
            direction,
            blocks,
            broken,
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
    /// The positions of the blocks a sticky piston pulls, before they move. Empty if it pulls
    /// nothing.
    pub blocks: Vec<BlockPos>,
    /// The positions of the blocks a sticky piston breaks while pulling.
    pub broken: Vec<BlockPos>,
}

impl BlockPistonRetractEvent {
    #[must_use]
    pub const fn new(
        world: Arc<World>,
        block_pos: BlockPos,
        direction: String,
        blocks: Vec<BlockPos>,
        broken: Vec<BlockPos>,
    ) -> Self {
        Self {
            world,
            block_pos,
            direction,
            blocks,
            broken,
            cancelled: false,
        }
    }
}
