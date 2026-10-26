use pumpkin_data::data_component_impl::EquipmentSlot;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_macros::Event;
use std::sync::Arc;

use crate::entity::player::Player;

use super::PlayerEvent;

/// An event that occurs after the stack in one of a player's armor slots changed.
///
/// The server compares the armor slots once per tick, so the event also fires when only the
/// count or a component (such as durability) changed, and on the first tick after joining.
#[derive(Event, Clone)]
pub struct PlayerArmorChangeEvent {
    /// The player whose armor changed.
    pub player: Arc<Player>,

    /// The armor slot that changed.
    pub slot: EquipmentSlot,

    /// The stack that was in the slot before, empty if there was none.
    pub old_item: ItemStack,

    /// The stack that is in the slot now, empty if there is none.
    pub new_item: ItemStack,
}

impl PlayerArmorChangeEvent {
    #[must_use]
    pub const fn new(
        player: Arc<Player>,
        slot: EquipmentSlot,
        old_item: ItemStack,
        new_item: ItemStack,
    ) -> Self {
        Self {
            player,
            slot,
            old_item,
            new_item,
        }
    }
}

impl PlayerEvent for PlayerArmorChangeEvent {
    fn get_player(&self) -> &Arc<Player> {
        &self.player
    }
}
