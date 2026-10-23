use pumpkin_data::item_stack::ItemStack;
use pumpkin_macros::{Event, cancellable};
use pumpkin_util::math::position::BlockPos;

/// An event that occurs when a furnace starts smelting an item.
#[cancellable]
#[derive(Event, Clone)]
pub struct FurnaceStartSmeltEvent {
    /// The position of the furnace block.
    pub block_pos: BlockPos,

    /// The registry key of the item to smelt.
    pub source_item: String,

    /// Total cooking time required in ticks.
    pub cooking_time: u32,

    /// The stack in the input slot.
    pub source: ItemStack,
}

impl FurnaceStartSmeltEvent {
    #[must_use]
    pub const fn new(
        block_pos: BlockPos,
        source_item: String,
        cooking_time: u32,
        source: ItemStack,
    ) -> Self {
        Self {
            block_pos,
            source_item,
            cooking_time,
            source,
            cancelled: false,
        }
    }
}
