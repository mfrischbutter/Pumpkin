//! Vanilla `AbstractChestBoat`: the chest of a chest boat or chest raft.

use std::sync::Arc;

use pumpkin_data::game_event::GameEvent;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::text::TextComponent;

use crate::entity::mob::piglin_anger::anger_nearby_piglins;
use crate::entity::player::Player;
use crate::entity::vehicle::minecart::container::{self, MinecartInventory};
use crate::entity::{Entity, EntityBase};
use crate::world::game_event::GameEventContext;

/// Vanilla `AbstractChestBoat.CONTAINER_SIZE`.
const CONTAINER_SIZE: usize = 27;

/// Vanilla `ContainerEntity`, which chest minecarts share.
pub(super) struct ChestBoat {
    inventory: Arc<MinecartInventory>,
}

impl ChestBoat {
    pub(super) fn new() -> Self {
        Self {
            inventory: Arc::new(MinecartInventory::new(CONTAINER_SIZE)),
        }
    }

    /// Vanilla `AbstractChestBoat.interact`, past the seat check: `createMenu` and the
    /// `CONTAINER_OPEN` that follows.
    pub(super) fn open(&self, entity: &Entity, player: &Arc<Player>, title: TextComponent) -> bool {
        container::open(entity, player, &self.inventory, title, false);
        let world = entity.world.load();
        world.emit_game_event_with_context(
            GameEvent::ContainerOpen.name(),
            entity.pos.load(),
            &GameEventContext {
                source_entity: Some(&**player),
            },
        );
        anger_nearby_piglins(&world, player, true);
        true
    }

    /// Vanilla `AbstractChestBoat.remove`.
    pub(super) fn drop_contents(&self, entity: &Entity) {
        container::drop_contents(entity, &self.inventory);
    }

    /// The part of vanilla `ContainerEntity.chestVehicleDestroyed` left after `remove` dropped
    /// the contents.
    pub(super) fn anger_piglins(entity: &Entity, direct: Option<&dyn EntityBase>) {
        let world = entity.world.load();
        if let Some(player) = direct
            .and_then(EntityBase::get_player)
            .and_then(|player| world.get_player_by_id(player.entity_id()))
        {
            anger_nearby_piglins(&world, &player, true);
        }
    }

    pub(super) fn write_nbt(&self, nbt: &mut NbtCompound) {
        self.inventory.write_nbt(nbt);
    }

    pub(super) fn read_nbt(&self, nbt: &NbtCompound) {
        self.inventory.read_nbt(nbt);
    }
}
