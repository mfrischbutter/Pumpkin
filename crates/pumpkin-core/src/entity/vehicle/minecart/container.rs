use std::any::Any;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, RwLock};

use pumpkin_data::item_stack::ItemStack;
use pumpkin_inventory::generic_container_screen_handler::{create_generic_9x3, create_hopper};
use pumpkin_inventory::player::player_inventory::PlayerInventory;
use pumpkin_inventory::screen_handler::{
    InventoryPlayer, ScreenHandlerFactory, SharedScreenHandler,
};
use pumpkin_inventory::{Clearable, Inventory};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_util::text::TextComponent;

use crate::entity::{Entity, player::Player};

/// Vanilla `ContainerEntity`'s items and loot table, which chest boats share.
pub(in crate::entity::vehicle) struct MinecartInventory {
    items: RwLock<Vec<ItemStack>>,
    size: usize,
    loot_table: Mutex<Option<(String, i64)>>,
    /// The contents drop once, however often a removal reaches them.
    drops_claimed: AtomicBool,
}

impl MinecartInventory {
    pub(in crate::entity::vehicle) fn new(size: usize) -> Self {
        Self {
            items: RwLock::new(vec![ItemStack::EMPTY.clone(); size]),
            size,
            loot_table: Mutex::new(None),
            drops_claimed: AtomicBool::new(false),
        }
    }

    fn claim_drops(&self) -> bool {
        !self.drops_claimed.swap(true, Ordering::AcqRel)
    }

    pub(in crate::entity::vehicle) fn read_nbt(&self, nbt: &NbtCompound) {
        let loot_table = nbt.get_string("LootTable").map(|loot_table| {
            (
                loot_table.to_owned(),
                nbt.get_long("LootTableSeed").unwrap_or(0),
            )
        });
        let has_loot_table = loot_table.is_some();
        if let Ok(mut guard) = self.loot_table.try_lock() {
            *guard = loot_table;
        }

        if !has_loot_table && let Ok(mut items) = self.items.try_write() {
            items.fill_with(|| ItemStack::EMPTY.clone());
            self.read_data(nbt, &mut items);
        }
    }

    pub(in crate::entity::vehicle) fn write_nbt(&self, nbt: &mut NbtCompound) {
        let loot_table = self
            .loot_table
            .try_lock()
            .ok()
            .and_then(|guard| guard.clone());
        if let Some((loot_table, seed)) = loot_table {
            nbt.put_string("LootTable", loot_table);
            if seed != 0 {
                nbt.put_long("LootTableSeed", seed);
            }
        } else if let Ok(items) = self.items.try_read() {
            let mut list: Vec<pumpkin_nbt::tag::NbtTag> = Vec::new();
            for (slot, stack) in items.iter().enumerate() {
                if !stack.is_empty() {
                    let mut compound = NbtCompound::new();
                    compound.put_byte("Slot", slot as i8);
                    stack.write_item_stack(&mut compound);
                    list.push(pumpkin_nbt::tag::NbtTag::Compound(compound));
                }
            }
            nbt.put("Items", pumpkin_nbt::tag::NbtTag::List(list));
        }
    }

    pub(super) fn has_loot_table(&self) -> bool {
        self.loot_table
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_some()
    }

    /// Fills the inventory from its loot table. `minecart` and `player` go into the
    /// `LootGenerateEvent`; without `minecart` no event is fired.
    pub(super) fn unpack_loot(
        self: &Arc<Self>,
        minecart: Option<&Entity>,
        player: Option<&Player>,
        params: &crate::world::loot::LootContextParameters<'_>,
    ) {
        let loot_table = self
            .loot_table
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        let Some((loot_table, seed)) = loot_table else {
            return;
        };
        let Some(table) = crate::world::loot::get_loot_table(&loot_table) else {
            *self
                .loot_table
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some((loot_table, seed));
            return;
        };

        let inventory: Arc<dyn Inventory> = self.clone();
        crate::world::loot::fill_chest_inventory_handle(
            &inventory,
            &table,
            seed,
            params,
            |items| {
                let Some(minecart) = minecart else {
                    return items;
                };
                minecart.world.load().fire_loot_generate(
                    &loot_table,
                    None,
                    Some(minecart.entity_id),
                    player,
                    items,
                )
            },
        );
    }
}

impl Inventory for MinecartInventory {
    fn size(&self) -> usize {
        self.size
    }

    fn is_empty(&self) -> bool {
        let items = self
            .items
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        items.iter().all(ItemStack::is_empty)
    }

    fn get_stack(&self, slot: usize) -> ItemStack {
        self.items
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)[slot]
            .clone()
    }

    fn remove_stack(&self, slot: usize) -> ItemStack {
        let mut items = self
            .items
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        std::mem::replace(&mut items[slot], ItemStack::EMPTY.clone())
    }

    fn remove_stack_specific(&self, slot: usize, amount: u8) -> ItemStack {
        let mut items = self
            .items
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !items[slot].is_empty() && amount > 0 {
            items[slot].split(amount)
        } else {
            ItemStack::EMPTY.clone()
        }
    }

    fn set_stack(&self, slot: usize, stack: ItemStack) {
        self.items
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)[slot] = stack;
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl Clearable for MinecartInventory {
    fn clear(&self) {
        self.items
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .fill_with(|| ItemStack::EMPTY.clone());
    }
}

struct MinecartScreenFactory {
    inventory: Arc<MinecartInventory>,
    title: TextComponent,
    hopper: bool,
}

impl ScreenHandlerFactory for MinecartScreenFactory {
    fn create_screen_handler(
        &self,
        sync_id: u8,
        player_inventory: &Arc<PlayerInventory>,
        player: &dyn InventoryPlayer,
    ) -> Option<SharedScreenHandler> {
        if self.inventory.has_loot_table() && player.is_spectator() {
            return None;
        }
        let inventory: Arc<dyn Inventory> = self.inventory.clone();
        let handler = if self.hopper {
            create_hopper(sync_id, player_inventory, inventory, player)
        } else {
            create_generic_9x3(sync_id, player_inventory, inventory, player)
        };
        Some(Arc::new(Mutex::new(handler)) as SharedScreenHandler)
    }

    fn get_display_name(&self) -> TextComponent {
        self.title.clone()
    }
}

/// Vanilla `ContainerEntity.chestVehicleDestroyed`'s `Containers.dropContents`, which unpacks a
/// loot table first. Only the first call drops anything.
pub(in crate::entity::vehicle) fn drop_contents(
    entity: &Entity,
    inventory: &Arc<MinecartInventory>,
) {
    if !inventory.claim_drops() {
        return;
    }
    let world = entity.world.load();
    inventory.unpack_loot(
        Some(entity),
        None,
        &crate::world::loot::LootContextParameters {
            world: Some(&world),
            position: Some(entity.pos.load()),
            ..Default::default()
        },
    );
    let inventory: Arc<dyn Inventory> = inventory.clone();
    world.scatter_inventory(&entity.block_pos.load(), &inventory);
}

pub(in crate::entity::vehicle) fn open(
    entity: &Entity,
    player: &Arc<Player>,
    inventory: &Arc<MinecartInventory>,
    title: TextComponent,
    hopper: bool,
) -> bool {
    if !player.is_spectator() {
        let world = entity.world.load();
        inventory.unpack_loot(
            Some(entity),
            Some(player.as_ref()),
            &crate::world::loot::LootContextParameters {
                world: Some(&world),
                position: Some(entity.pos.load()),
                this_entity: Some(player.as_ref()),
                luck: player
                    .living_entity
                    .get_attribute_value(&pumpkin_data::attributes::Attributes::LUCK)
                    as f32,
                ..Default::default()
            },
        );
    }

    player
        .open_handled_screen(
            &MinecartScreenFactory {
                inventory: inventory.clone(),
                title: entity.custom_name.load().as_ref().clone().unwrap_or(title),
                hopper,
            },
            None,
        )
        .is_some()
}

/// Vanilla `AbstractContainerMenu.getRedstoneSignalFromContainer`.
#[must_use]
pub fn redstone_signal_from_container(container: &dyn Inventory) -> u8 {
    let mut total_percent = 0.0f32;
    for slot in 0..container.size() {
        let stack = container.get_stack(slot);
        if !stack.is_empty() {
            total_percent += f32::from(stack.item_count) / f32::from(stack.get_max_stack_size());
        }
    }
    total_percent /= container.size() as f32;
    // Vanilla `Mth.lerpDiscrete(total_percent, 0, 15)`.
    (total_percent * 14.0).floor() as u8 + u8::from(total_percent > 0.0)
}

/// Vanilla `AbstractMinecartContainer.applyNaturalSlowdown`: the fuller, the slower.
pub(super) fn natural_slowdown(
    entity: &Entity,
    inventory: &MinecartInventory,
    delta: Vector3<f64>,
) -> Vector3<f64> {
    let mut keep = 0.98f32;
    if !inventory.has_loot_table() {
        let emptiness = 15 - redstone_signal_from_container(inventory);
        keep += f32::from(emptiness) * 0.001;
    }
    if entity.is_in_water() {
        keep *= 0.95;
    }
    let keep = f64::from(keep);
    delta.multiply(keep, 0.0, keep)
}

#[cfg(test)]
mod tests {
    use super::MinecartInventory;
    use pumpkin_data::item::Item;
    use pumpkin_data::item_stack::ItemStack;
    use pumpkin_inventory::Inventory;
    use pumpkin_nbt::compound::NbtCompound;

    #[test]
    fn deferred_mineshaft_loot_is_preserved_until_unpacked() {
        let inventory = std::sync::Arc::new(MinecartInventory::new(27));
        let mut source = NbtCompound::new();
        source.put_string(
            "LootTable",
            "minecraft:chests/abandoned_mineshaft".to_string(),
        );
        source.put_long("LootTableSeed", 1234);
        inventory.read_nbt(&source);

        let mut deferred = NbtCompound::new();
        inventory.write_nbt(&mut deferred);
        assert_eq!(
            deferred.get_string("LootTable"),
            Some("minecraft:chests/abandoned_mineshaft")
        );
        assert_eq!(deferred.get_long("LootTableSeed"), Some(1234));
        assert!(deferred.get_list("Items").is_none());

        inventory.unpack_loot(
            None,
            None,
            &crate::world::loot::LootContextParameters::default(),
        );
        assert!(!inventory.is_empty());

        let mut unpacked = NbtCompound::new();
        inventory.write_nbt(&mut unpacked);
        assert!(unpacked.get_string("LootTable").is_none());
        assert!(unpacked.get_list("Items").is_some());
    }

    #[test]
    fn chest_minecart_items_round_trip_through_nbt() {
        let inventory = MinecartInventory::new(27);
        inventory.set_stack(8, ItemStack::new(3, &Item::POWERED_RAIL));

        let mut nbt = NbtCompound::new();
        inventory.write_nbt(&mut nbt);

        let restored = MinecartInventory::new(27);
        restored.read_nbt(&nbt);
        let stack = restored.get_stack(8);
        assert_eq!(stack.get_item().id, Item::POWERED_RAIL.id);
        assert_eq!(stack.item_count, 3);
    }
}
