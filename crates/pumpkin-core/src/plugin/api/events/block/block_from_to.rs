use std::sync::Arc;

use pumpkin_data::Block;
use pumpkin_macros::{Event, cancellable};
use pumpkin_util::math::position::BlockPos;

use crate::world::World;

/// An event that occurs when a block (such as fluid) spreads/flows from one location to another.
#[cancellable]
#[derive(Event, Clone)]
pub struct BlockFromToEvent {
    /// The world the fluid flows in.
    pub world: Arc<World>,

    /// The source block position.
    pub from_pos: BlockPos,

    /// The destination block position.
    pub to_pos: BlockPos,

    /// The block flowing.
    pub block: &'static Block,
}

impl BlockFromToEvent {
    #[must_use]
    pub const fn new(
        world: Arc<World>,
        from_pos: BlockPos,
        to_pos: BlockPos,
        block: &'static Block,
    ) -> Self {
        Self {
            world,
            from_pos,
            to_pos,
            block,
            cancelled: false,
        }
    }
}
