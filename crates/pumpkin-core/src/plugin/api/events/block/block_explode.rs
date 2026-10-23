use std::sync::Arc;

use pumpkin_macros::{Event, cancellable};
use pumpkin_util::math::position::BlockPos;

use crate::world::World;

/// An event that occurs when an explosion without a source entity goes off, such as a bed or a
/// respawn anchor.
///
/// It fires once per explosion, after the affected blocks are known. Cancelling it cancels the
/// whole explosion.
#[cancellable]
#[derive(Event, Clone)]
pub struct BlockExplodeEvent {
    pub block_pos: BlockPos,
    /// The share of destroyed blocks that drop their loot, from 0.0 to 1.0.
    pub yield_rate: f32,
    /// The world the explosion happens in.
    pub world: Arc<World>,
    /// The blocks the explosion destroys. Only the blocks left in this list are destroyed.
    pub blocks: Vec<BlockPos>,
}

impl BlockExplodeEvent {
    #[must_use]
    pub const fn new(
        world: Arc<World>,
        block_pos: BlockPos,
        blocks: Vec<BlockPos>,
        yield_rate: f32,
    ) -> Self {
        Self {
            block_pos,
            yield_rate,
            world,
            blocks,
            cancelled: false,
        }
    }
}
