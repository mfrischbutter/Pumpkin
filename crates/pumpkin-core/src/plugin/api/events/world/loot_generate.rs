use pumpkin_data::item_stack::ItemStack;
use pumpkin_macros::{Event, cancellable};
use pumpkin_util::math::position::BlockPos;
use std::sync::Arc;

use crate::{
    entity::{EntityBase, player::Player},
    world::World,
};

/// An event that occurs when loot is generated from a loot table, before it is placed.
///
/// Plugins can change `items`. Cancelling the event generates no loot.
#[cancellable]
#[derive(Event, Clone)]
pub struct LootGenerateEvent {
    /// The world the loot is generated in.
    pub world: Arc<World>,

    /// Loot table identifier.
    pub loot_table: String,

    /// The block the loot is for, such as a chest or a brushed block.
    pub block_pos: Option<BlockPos>,

    /// The entity the loot is for, such as a chest minecart.
    pub entity: Option<Arc<dyn EntityBase>>,

    /// The player who caused the loot to be generated.
    pub player: Option<Arc<Player>>,

    /// The generated items.
    pub items: Vec<ItemStack>,
}

impl LootGenerateEvent {
    #[must_use]
    pub const fn new(
        world: Arc<World>,
        loot_table: String,
        block_pos: Option<BlockPos>,
        entity: Option<Arc<dyn EntityBase>>,
        player: Option<Arc<Player>>,
        items: Vec<ItemStack>,
    ) -> Self {
        Self {
            world,
            loot_table,
            block_pos,
            entity,
            player,
            items,
            cancelled: false,
        }
    }
}
