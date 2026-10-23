use crate::entity::player::Player;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_macros::{Event, cancellable};
use std::sync::Arc;

/// An event that occurs when a player consumes an item (food, potion, etc.).
#[cancellable]
#[derive(Event, Clone)]
pub struct PlayerItemConsumeEvent {
    /// The player consuming the item.
    pub player: Arc<Player>,

    /// The registry name of the item being consumed.
    pub item_name: String,

    /// The stack in the hand that is used.
    pub item: ItemStack,
}

impl PlayerItemConsumeEvent {
    #[must_use]
    pub const fn new(player: Arc<Player>, item_name: String, item: ItemStack) -> Self {
        Self {
            player,
            item_name,
            item,
            cancelled: false,
        }
    }
}
