#[allow(clippy::wildcard_imports)]
use super::*;

impl BedrockClient {
    #[allow(clippy::too_many_lines)]
    pub fn handle_block_pick_request(&self, player: &Arc<Player>, packet: &SBlockPickRequest) {
        if !player.can_interact_with_block_at(&packet.position, 1.0) {
            return;
        }

        let world = player.world();
        let block = world.get_block(&packet.position);

        let mut stack = if block.item_id == 0 {
            None
        } else {
            pumpkin_data::item::Item::from_id(block.item_id).map(|item| ItemStack::new(1, item))
        };

        if let Some(server) = world.server.upgrade() {
            let mut event = crate::plugin::player::player_pick_block::PlayerPickBlockEvent::new(
                player.clone(),
                world,
                packet.position,
                packet.with_data,
                stack,
            );
            server.plugin_manager.fire_blocking(&server, &mut event);
            if event.cancelled {
                return;
            }
            stack = event.item;
        }

        let Some(stack) = stack.filter(|stack| !stack.is_empty()) else {
            return;
        };

        let target_hotbar_slot = packet.max_slots as usize;
        if target_hotbar_slot >= 9 {
            return;
        }

        let slot_with_stack = player.inventory().get_slot_with_stack(&stack);

        if slot_with_stack != -1 {
            if pumpkin_inventory::player::player_inventory::PlayerInventory::is_valid_hotbar_index(
                slot_with_stack as usize,
            ) {
                if slot_with_stack as usize != target_hotbar_slot {
                    let target_stack = player.inventory().get_stack(target_hotbar_slot);
                    let source_stack = player.inventory().get_stack(slot_with_stack as usize);
                    player
                        .inventory()
                        .set_stack(target_hotbar_slot, source_stack);
                    player
                        .inventory()
                        .set_stack(slot_with_stack as usize, target_stack);
                }
            } else {
                let target_stack = player.inventory().get_stack(target_hotbar_slot);
                let source_stack = player.inventory().get_stack(slot_with_stack as usize);
                player
                    .inventory()
                    .set_stack(target_hotbar_slot, source_stack);
                player
                    .inventory
                    .set_stack(slot_with_stack as usize, target_stack);
            }
        } else if player.gamemode.load() == GameMode::Creative {
            player.inventory.set_stack(target_hotbar_slot, stack);
        } else {
            return;
        }

        player.inventory.set_selected_slot(target_hotbar_slot as u8);

        // Send hotbar updates
        player.client.try_enqueue_packet_editioned(
            &CSetSelectedSlot::new(player.inventory.get_selected_slot() as i8),
            &CPlayerHotbar {
                selected_slot: VarUInt(player.inventory.get_selected_slot() as u32),
                container_id: 0,
                should_select_slot: true,
            },
        );

        // Send screen handler / Java inventory updates
        player
            .player_screen_handler
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .send_content_updates();

        // Sync main hand equipment to other players
        let stack_in_hand = player.inventory().held_item();
        let equipment = &[(EquipmentSlot::MAIN_HAND, stack_in_hand)];
        player.living_entity.send_equipment_changes(equipment);

        // Sync bedrock inventory updates
        let slots = player
            .inventory()
            .main_inventory
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .map(NetworkItemStackDescriptor::from)
            .collect();
        self.try_enqueue_client_packet(&CInventoryContent {
            container_id: VarUInt(0),
            slots,
            full_container_name: FullContainerName {
                container_name: ContainerName::Inventory,
                dynamic_id: None,
            },
            storage_item: NetworkItemStackDescriptor::default(),
        });
    }
}
