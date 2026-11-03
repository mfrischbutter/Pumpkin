//! Mount inventories: vanilla `AbstractHorse.inventory` and `AbstractChestedHorse`'s chest, and
//! the screens over them, `HorseInventoryMenu` and `NautilusInventoryMenu`.

use std::sync::{Arc, Mutex};

use pumpkin_data::data_component_impl::EquipmentSlot;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_inventory::mount_screen_handler::MountScreenHandler;
use pumpkin_inventory::{Inventory, SimpleInventory, sync_read_items_from_nbt};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_nbt::tag::NbtTag;

use crate::entity::living::prevents_equipment_drop;
use crate::entity::mob::Mob;
use crate::entity::mob::equipment_slot_container::EquipmentSlotContainer;
use crate::entity::passive::abstract_horse::AbstractHorse;
use crate::entity::player::Player;

#[must_use]
pub fn inventory(horse: &dyn AbstractHorse) -> Arc<SimpleInventory> {
    horse
        .horse_data()
        .inventory
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
}

/// Vanilla `createInventory`: a new one for the current columns, keeping what fits. A screen
/// open on the old one closes, see `open_mount_screen`.
pub fn create_inventory(horse: &dyn AbstractHorse) {
    let columns = usize::try_from(horse.inventory_columns()).unwrap_or(0);
    let size = MountScreenHandler::get_inventory_size(columns);
    let inventory = SimpleInventory::new(size);
    let mut current = horse
        .horse_data()
        .inventory
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    for slot in 0..size.min(current.size()) {
        inventory.set_stack(slot, current.get_stack(slot));
    }
    *current = Arc::new(inventory);
}

/// Vanilla `AbstractHorse.openCustomInventoryScreen` and `Camel`'s.
pub fn open_custom_inventory_screen(horse: &dyn AbstractHorse, player: &Arc<Player>) {
    if horse.may_open_inventory(player) {
        open_mount_screen(
            horse,
            inventory(horse),
            horse.inventory_columns(),
            horse.bedrock_equip_slot(),
            player,
        );
    }
}

/// Vanilla `AbstractNautilus.openCustomInventoryScreen`, past its check: only the two equipment
/// slots.
pub fn open_nautilus_screen(nautilus: &dyn Mob, player: &Arc<Player>) {
    open_mount_screen(
        nautilus,
        Arc::new(SimpleInventory::new(0)),
        0,
        MountScreenHandler::SLOT_SADDLE,
        player,
    );
}

/// Vanilla `ServerPlayer.openHorseInventory` and `openNautilusInventory` with the menus they make
/// and `AbstractMountInventoryMenu.stillValid`.
fn open_mount_screen(
    mount: &dyn Mob,
    inventory: Arc<SimpleInventory>,
    columns: i32,
    bedrock_equip_slot: usize,
    player: &Arc<Player>,
) {
    let entity = mount.get_entity();
    let Some(mount) = entity.world.load().get_entity_by_id(entity.entity_id) else {
        return;
    };
    let mount = Arc::downgrade(&mount);
    player.open_mount_screen(
        columns,
        entity.entity_id,
        bedrock_equip_slot,
        bedrock_equip_slots(bedrock_equip_slot),
        |sync_id, player_inventory| {
            let mut handler = MountScreenHandler::new(
                sync_id,
                player_inventory,
                inventory.clone(),
                Arc::new(EquipmentSlotContainer::new(
                    mount.clone(),
                    EquipmentSlot::SADDLE,
                )),
                Arc::new(EquipmentSlotContainer::new(
                    mount.clone(),
                    EquipmentSlot::BODY,
                )),
                usize::try_from(columns).unwrap_or(0),
            );
            handler.bedrock_equip_slot = Some(bedrock_equip_slot);
            handler.validity_check = Some(Box::new(move |player| {
                let Some(player) = player.as_any().downcast_ref::<Player>() else {
                    return false;
                };
                mount.upgrade().is_some_and(|mount| {
                    // A nautilus has no chest to replace.
                    let same_inventory = mount
                        .get_mob()
                        .and_then(|mob| mob.as_abstract_horse())
                        .is_none_or(|horse| Arc::ptr_eq(&self::inventory(horse), &inventory));
                    mount.get_entity().is_alive()
                        && same_inventory
                        && player.can_interact_with_entity(mount.as_ref(), 4.0)
                })
            }));
            Arc::new(Mutex::new(handler))
        },
    );
}

/// The `UpdateEquip` slot list Bedrock draws its one equipment slot from: a llama's carpet or a
/// saddle.
fn bedrock_equip_slots(equip_slot: usize) -> NbtCompound {
    let name = if equip_slot == MountScreenHandler::SLOT_BODY_ARMOR {
        "minecraft:carpet"
    } else {
        "minecraft:saddle"
    };
    let item = || {
        let mut item = NbtCompound::new();
        // Any data value of the Bedrock item.
        item.put_short("Aux", i16::MAX);
        item.put_string("Name", name.to_string());
        item
    };
    let mut accepted = NbtCompound::new();
    accepted.put_compound("slotItem", item());
    let mut slot = NbtCompound::new();
    slot.put_list("acceptedItems", vec![NbtTag::Compound(accepted)]);
    slot.put_compound("item", item());
    slot.put_int("slotNumber", equip_slot as i32);
    let mut data = NbtCompound::new();
    data.put_list("slots", vec![NbtTag::Compound(slot)]);
    data
}

/// Vanilla `AbstractHorse.dropEquipment` with `AbstractChestedHorse`'s: the chest's items, then
/// the chest.
#[must_use]
pub fn inventory_drops(horse: &dyn AbstractHorse) -> Vec<ItemStack> {
    let inventory = inventory(horse);
    let mut drops: Vec<ItemStack> = (0..inventory.size())
        .map(|slot| inventory.get_stack(slot))
        .filter(|stack| !stack.is_empty() && !prevents_equipment_drop(stack))
        .collect();
    if horse.has_chest() {
        drops.push(ItemStack::new(1, &Item::CHEST));
    }
    drops
}

/// Vanilla `AbstractChestedHorse.addAdditionalSaveData`.
pub fn write_chest_nbt(horse: &dyn AbstractHorse, nbt: &mut NbtCompound) {
    nbt.put_bool("ChestedHorse", horse.has_chest());
    if horse.has_chest() {
        inventory(horse).write_inventory_nbt(nbt, true);
    }
}

/// Vanilla `AbstractChestedHorse.readAdditionalSaveData`, past `setChest`.
pub fn read_chest_items(horse: &dyn AbstractHorse, nbt: &NbtCompound) {
    create_inventory(horse);
    if horse.has_chest() {
        sync_read_items_from_nbt(
            nbt,
            &mut inventory(horse)
                .stacks
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        );
    }
}
