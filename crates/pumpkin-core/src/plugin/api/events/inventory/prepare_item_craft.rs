use crate::entity::player::Player;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_macros::{Event, cancellable};
use std::sync::Arc;

/// An event that occurs when the crafting grid changes, before the result slot is updated.
///
/// Setting `result` changes the stack the result slot shows; cancelling the event empties it.
#[cancellable]
#[derive(Event, Clone)]
pub struct PrepareItemCraftEvent {
    /// The player crafting the item.
    pub player: Arc<Player>,

    /// The id of the matching plugin or datapack recipe, if any.
    pub recipe_id: Option<String>,

    /// The crafting grid, row by row; empty slots are `None`.
    pub matrix: Vec<Option<ItemStack>>,

    /// The stack the result slot shows, if any.
    pub result: Option<ItemStack>,
}

impl PrepareItemCraftEvent {
    #[must_use]
    pub const fn new(
        player: Arc<Player>,
        recipe_id: Option<String>,
        matrix: Vec<Option<ItemStack>>,
        result: Option<ItemStack>,
    ) -> Self {
        Self {
            player,
            recipe_id,
            matrix,
            result,
            cancelled: false,
        }
    }
}
