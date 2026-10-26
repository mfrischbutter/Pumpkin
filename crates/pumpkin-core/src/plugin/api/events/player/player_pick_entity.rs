use pumpkin_data::item_stack::ItemStack;
use pumpkin_macros::{Event, cancellable};
use std::sync::Arc;

use super::PlayerEvent;
use crate::entity::player::Player;
use crate::world::World;

/// An event that occurs when a player picks an entity (the pick block key on an entity).
///
/// Changing `item` changes what the player gets; `None` gives nothing.
#[cancellable]
#[derive(Event, Clone)]
pub struct PlayerPickEntityEvent {
    /// The player picking the entity.
    pub player: Arc<Player>,

    /// The world of the picked entity.
    pub world: Arc<World>,

    /// The ID of the picked entity.
    pub entity_id: i32,

    /// Whether the client asked to include the entity's data.
    pub include_data: bool,

    /// The item to select or give, `None` if the entity has no item.
    pub item: Option<ItemStack>,
}

impl PlayerPickEntityEvent {
    #[must_use]
    pub const fn new(
        player: Arc<Player>,
        world: Arc<World>,
        entity_id: i32,
        include_data: bool,
        item: Option<ItemStack>,
    ) -> Self {
        Self {
            player,
            world,
            entity_id,
            include_data,
            item,
            cancelled: false,
        }
    }
}

impl PlayerEvent for PlayerPickEntityEvent {
    fn get_player(&self) -> &Arc<Player> {
        &self.player
    }
}
