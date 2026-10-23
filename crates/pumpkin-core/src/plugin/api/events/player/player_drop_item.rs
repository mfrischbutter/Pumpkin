use crate::entity::player::Player;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_macros::{Event, cancellable};
use std::sync::Arc;

/// An event that occurs when a player drops an item.
#[cancellable]
#[derive(Event, Clone)]
pub struct PlayerDropItemEvent {
    /// The player dropping the item.
    pub player: Arc<Player>,

    /// The registry name of the dropped item.
    pub item_name: String,

    /// The stack count of dropped items.
    pub count: u8,

    /// The stack that is dropped.
    pub item: ItemStack,
}

impl PlayerDropItemEvent {
    #[must_use]
    pub const fn new(player: Arc<Player>, item_name: String, count: u8, item: ItemStack) -> Self {
        Self {
            player,
            item_name,
            count,
            item,
            cancelled: false,
        }
    }
}
