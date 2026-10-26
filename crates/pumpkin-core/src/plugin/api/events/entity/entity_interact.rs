use std::sync::Arc;

use pumpkin_macros::{Event, cancellable};
use pumpkin_util::math::position::BlockPos;

use crate::world::World;

/// An event that occurs when an entity interacts with a block.
///
/// Fired when an entity, players included, triggers a tripwire or a pressure plate. Cancelling it
/// leaves the block unpowered.
#[cancellable]
#[derive(Event, Clone)]
pub struct EntityInteractEvent {
    /// The ID of the entity.
    pub entity_id: i32,
    /// Position of the interacted block.
    pub block_pos: BlockPos,
    /// The world the block is in.
    pub world: Arc<World>,
}

impl EntityInteractEvent {
    #[must_use]
    pub const fn new(entity_id: i32, block_pos: BlockPos, world: Arc<World>) -> Self {
        Self {
            entity_id,
            block_pos,
            world,
            cancelled: false,
        }
    }
}
