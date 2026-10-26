use std::sync::Arc;

use pumpkin_macros::{Event, cancellable};
use pumpkin_util::math::position::BlockPos;

use crate::world::World;

/// An event that occurs when a block physics check is run.
#[cancellable]
#[derive(Event, Clone)]
pub struct BlockPhysicsEvent {
    /// The world the physics check runs in.
    pub world: Arc<World>,
    pub block_pos: BlockPos,
    pub changed_pos: BlockPos,
}

impl BlockPhysicsEvent {
    #[must_use]
    pub const fn new(world: Arc<World>, block_pos: BlockPos, changed_pos: BlockPos) -> Self {
        Self {
            world,
            block_pos,
            changed_pos,
            cancelled: false,
        }
    }
}
