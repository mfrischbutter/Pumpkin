use pumpkin_data::item_stack::ItemStack;
use pumpkin_macros::{Event, cancellable};
use pumpkin_util::math::position::BlockPos;
use std::sync::Arc;

use super::PlayerEvent;
use crate::entity::player::Player;
use crate::world::World;

/// An event that occurs when a player picks a block (the pick block key).
///
/// Changing `item` changes what the player gets; `None` gives nothing.
#[cancellable]
#[derive(Event, Clone)]
pub struct PlayerPickBlockEvent {
    /// The player picking the block.
    pub player: Arc<Player>,

    /// The world of the picked block.
    pub world: Arc<World>,

    /// The position of the picked block.
    pub block_pos: BlockPos,

    /// Whether the client asked to include the block's data.
    pub include_data: bool,

    /// The item to select or give, `None` if the block has no item.
    pub item: Option<ItemStack>,
}

impl PlayerPickBlockEvent {
    #[must_use]
    pub const fn new(
        player: Arc<Player>,
        world: Arc<World>,
        block_pos: BlockPos,
        include_data: bool,
        item: Option<ItemStack>,
    ) -> Self {
        Self {
            player,
            world,
            block_pos,
            include_data,
            item,
            cancelled: false,
        }
    }
}

impl PlayerEvent for PlayerPickBlockEvent {
    fn get_player(&self) -> &Arc<Player> {
        &self.player
    }
}
