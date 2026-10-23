use crate::entity::player::Player;
use crate::world::World;
use pumpkin_macros::{Event, cancellable};
use pumpkin_util::math::position::BlockPos;
use std::sync::Arc;

/// An event that occurs when a player opens an inventory.
#[cancellable]
#[derive(Event, Clone)]
pub struct InventoryOpenEvent {
    /// The player opening the inventory.
    pub player: Arc<Player>,

    /// The world the player opens the inventory in.
    pub world: Arc<World>,

    /// The position of the block the inventory belongs to, if it belongs to one.
    pub block_pos: Option<BlockPos>,
}

impl InventoryOpenEvent {
    #[must_use]
    pub const fn new(player: Arc<Player>, world: Arc<World>, block_pos: Option<BlockPos>) -> Self {
        Self {
            player,
            world,
            block_pos,
            cancelled: false,
        }
    }
}
