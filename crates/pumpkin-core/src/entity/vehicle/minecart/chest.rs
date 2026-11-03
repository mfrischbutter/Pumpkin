//! Vanilla `MinecartChest`.

use std::sync::Arc;

use pumpkin_data::game_event::GameEvent;
use pumpkin_data::translation;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::text::TextComponent;

use crate::entity::Entity;
use crate::entity::mob::piglin_anger::anger_nearby_piglins;
use crate::entity::player::Player;
use crate::world::game_event::GameEventContext;

use super::container::{self, MinecartInventory};

#[derive(Clone)]
pub(super) struct ChestMinecart {
    inventory: Arc<MinecartInventory>,
}

impl ChestMinecart {
    pub(super) fn new() -> Self {
        Self {
            inventory: Arc::new(MinecartInventory::new(27)),
        }
    }

    pub(super) const fn inventory(&self) -> &Arc<MinecartInventory> {
        &self.inventory
    }

    /// Vanilla `MinecartChest.interact`.
    pub(super) fn interact(&self, entity: &Entity, player: &Arc<Player>) -> bool {
        container::open(
            entity,
            player,
            &self.inventory,
            TextComponent::translate_cross(
                translation::java::ENTITY_MINECRAFT_CHEST_MINECART,
                translation::bedrock::ENTITY_CHEST_MINECART_NAME,
                [],
            ),
            false,
        );
        let world = entity.world.load();
        let opener: &Player = player;
        world.emit_game_event_with_context(
            GameEvent::ContainerOpen.name(),
            entity.pos.load(),
            &GameEventContext {
                source_entity: Some(opener),
            },
        );
        anger_nearby_piglins(&world, player, true);
        true
    }

    pub(super) fn write_nbt(&self, nbt: &mut NbtCompound) {
        self.inventory.write_nbt(nbt);
    }

    pub(super) fn read_nbt(&self, nbt: &NbtCompound) {
        self.inventory.read_nbt(nbt);
    }
}
