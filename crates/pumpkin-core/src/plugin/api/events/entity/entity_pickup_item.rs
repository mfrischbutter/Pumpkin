use pumpkin_data::item_stack::ItemStack;
use pumpkin_macros::{Event, cancellable};

/// An event that occurs when an entity picks up an item stack.
#[cancellable]
#[derive(Event, Clone)]
pub struct EntityPickupItemEvent {
    /// The ID of the picking entity.
    pub entity_id: i32,

    /// The registry name of the item.
    pub item_name: String,

    /// The count of items picked up.
    pub count: u8,

    /// The stack being picked up, or `None` for an experience orb.
    pub item: Option<ItemStack>,
}

impl EntityPickupItemEvent {
    #[must_use]
    pub const fn new(
        entity_id: i32,
        item_name: String,
        count: u8,
        item: Option<ItemStack>,
    ) -> Self {
        Self {
            entity_id,
            item_name,
            count,
            item,
            cancelled: false,
        }
    }
}
