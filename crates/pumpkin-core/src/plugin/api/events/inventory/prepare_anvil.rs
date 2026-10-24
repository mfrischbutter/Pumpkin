use crate::entity::player::Player;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_macros::Event;
use std::sync::Arc;

/// An event that occurs when an anvil computes its result.
///
/// Fired every time the anvil result is recomputed. Plugins can replace the result, the cost and
/// the number of repair items consumed from the right slot.
#[derive(Event, Clone)]
pub struct PrepareAnvilEvent {
    /// The player using the anvil.
    pub player: Arc<Player>,

    /// The item in the left input slot. Empty if the slot is empty.
    pub left: ItemStack,

    /// The item in the right input slot. Empty if the slot is empty.
    pub right: ItemStack,

    /// The result, or `None` if the anvil gives no result.
    pub result: Option<ItemStack>,

    /// The name prepared in the anvil input text field.
    pub rename_text: String,

    /// The repair cost in experience levels.
    pub repair_cost: u32,

    /// How many items taking the result removes from the right slot. 0 removes the whole stack,
    /// unless only the name changes.
    pub repair_item_count: u32,
}

impl PrepareAnvilEvent {
    #[must_use]
    pub const fn new(
        player: Arc<Player>,
        left: ItemStack,
        right: ItemStack,
        result: Option<ItemStack>,
        rename_text: String,
        repair_cost: u32,
        repair_item_count: u32,
    ) -> Self {
        Self {
            player,
            left,
            right,
            result,
            rename_text,
            repair_cost,
            repair_item_count,
        }
    }
}
