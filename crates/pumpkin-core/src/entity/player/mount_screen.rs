//! A player opening a mount's screen: vanilla `ServerPlayer.openHorseInventory` and
//! `openNautilusInventory`.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use pumpkin_inventory::player::{
    player_inventory::PlayerInventory, player_screen_handler::PlayerScreenHandler,
};
use pumpkin_inventory::screen_handler::SharedScreenHandler;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_protocol::bedrock::client::{CUpdateEquip, HORSE_CONTAINER_TYPE};
use pumpkin_protocol::codec::{var_int::VarInt, var_long::VarLong};
use pumpkin_protocol::java::client::play::COpenMountScreen;

use super::Player;
use crate::entity::EntityBase;
use crate::plugin::api::events::inventory::inventory_open::InventoryOpenEvent;

impl Player {
    /// Vanilla `handlePlayerCommand(OPEN_INVENTORY)`: the screen of the mount this player rides.
    /// `false` when its vehicle has none, which Bedrock takes for its own inventory.
    pub fn open_vehicle_inventory(self: &Arc<Self>) -> bool {
        self.get_entity().get_vehicle().is_some_and(|vehicle| {
            vehicle
                .get_mob()
                .is_some_and(|mob| mob.open_custom_inventory_screen(self))
        })
    }

    /// Opens a screen made by `create_handler` over a mount. `false` when a plugin refused.
    pub fn open_mount_screen(
        &self,
        inventory_columns: i32,
        mount_id: i32,
        bedrock_equip_slot: usize,
        bedrock_slots: NbtCompound,
        create_handler: impl FnOnce(u8, &Arc<PlayerInventory>) -> SharedScreenHandler,
    ) -> bool {
        if !self
            .current_screen_handler
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_any()
            .is::<PlayerScreenHandler>()
        {
            self.close_handled_screen();
        }

        let world = self.world();
        if let Some(server) = world.server.upgrade()
            && let Some(player_arc) = world.get_player_by_uuid(self.gameprofile.id)
        {
            let mut event = InventoryOpenEvent::new(player_arc, world, None);
            server.plugin_manager.fire_blocking(&server, &mut event);
            if event.cancelled {
                return false;
            }
        }

        self.increment_screen_handler_sync_id();
        let sync_id = self.screen_handler_sync_id.load(Ordering::Relaxed);
        let screen_handler = create_handler(sync_id, &self.inventory);
        self.bedrock_mount_screen
            .store(Some((sync_id, bedrock_equip_slot)));
        self.try_enqueue_packet_editioned(
            &COpenMountScreen::new(sync_id, inventory_columns.into(), mount_id),
            &CUpdateEquip {
                container_id: sync_id,
                container_type: HORSE_CONTAINER_TYPE,
                size: VarInt(0),
                entity_unique_id: VarLong(i64::from(mount_id)),
                data: bedrock_slots,
            },
        );
        self.on_screen_handler_opened(&screen_handler);
        *self
            .current_screen_handler
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = screen_handler;
        self.open_container_pos.store(None);
        true
    }
}
