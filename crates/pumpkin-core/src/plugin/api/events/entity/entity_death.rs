use crate::entity::player::Player;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_macros::{Event, cancellable};
use pumpkin_util::text::TextComponent;
use std::sync::Arc;

/// An event that occurs when an entity dies, after its loot is generated and before it drops.
#[derive(Event, Clone)]
pub struct EntityDeathEvent {
    /// The ID of the entity that died.
    pub entity_id: i32,

    /// The amount of experience dropped by the entity.
    pub dropped_exp: i32,

    /// The loot table and equipment drops. Only the stacks left in this list are dropped.
    pub drops: Vec<ItemStack>,
}

impl EntityDeathEvent {
    #[must_use]
    pub const fn new(entity_id: i32, dropped_exp: i32, drops: Vec<ItemStack>) -> Self {
        Self {
            entity_id,
            dropped_exp,
            drops,
        }
    }
}

/// An event that occurs when a player dies.
#[cancellable]
#[derive(Event, Clone)]
pub struct PlayerDeathEvent {
    /// The player that died.
    pub player: Arc<Player>,

    /// The death message to be broadcasted.
    pub death_message: TextComponent,

    /// The amount of experience dropped by the player.
    pub dropped_exp: i32,

    /// Whether the player keeps their inventory upon death.
    pub keep_inventory: bool,
}

impl PlayerDeathEvent {
    #[must_use]
    pub const fn new(player: Arc<Player>, death_message: TextComponent, dropped_exp: i32) -> Self {
        Self {
            player,
            death_message,
            dropped_exp,
            keep_inventory: false,
            cancelled: false,
        }
    }
}
