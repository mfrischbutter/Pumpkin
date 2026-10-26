use std::sync::Arc;

use pumpkin_macros::{Event, cancellable};
use pumpkin_util::math::position::BlockPos;

use crate::world::World;

/// An event that occurs when an entity changes a block in the world.
#[cancellable]
#[derive(Event, Clone)]
pub struct EntityChangeBlockEvent {
    /// The world the block is in.
    pub world: Arc<World>,
    /// The ID of the entity.
    pub entity_id: i32,
    /// Position of the block.
    pub block_pos: BlockPos,
    /// The new block state identifier.
    pub new_block: String,
}

impl EntityChangeBlockEvent {
    #[must_use]
    pub const fn new(
        world: Arc<World>,
        entity_id: i32,
        block_pos: BlockPos,
        new_block: String,
    ) -> Self {
        Self {
            world,
            entity_id,
            block_pos,
            new_block,
            cancelled: false,
        }
    }
}
