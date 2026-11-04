//! Vanilla `MinecartHopper`, with the parts of `HopperBlockEntity` it runs as a `Hopper`.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::block::entities::hopper::{HopperBlockEntity, entity_container_at};
use crate::entity::item::ItemEntity;
use crate::entity::{Entity, player::Player};
use crate::world::World;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::translation;
use pumpkin_inventory::Inventory;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::math::boundingbox::BoundingBox;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_util::text::TextComponent;

use super::container::{self, MinecartInventory};

#[derive(Clone)]
pub(super) struct HopperMinecart {
    enabled: Arc<AtomicBool>,
    consumed_item_this_frame: Arc<AtomicBool>,
    inventory: Arc<MinecartInventory>,
}

impl HopperMinecart {
    pub(super) fn new() -> Self {
        Self {
            enabled: Arc::new(AtomicBool::new(true)),
            consumed_item_this_frame: Arc::new(AtomicBool::new(false)),
            inventory: Arc::new(MinecartInventory::new(5)),
        }
    }

    pub(super) const fn inventory(&self) -> &Arc<MinecartInventory> {
        &self.inventory
    }

    /// Vanilla `MinecartHopper.activateMinecart`.
    pub(super) fn activate(&self, powered: bool) {
        self.enabled.store(!powered, Ordering::Relaxed);
    }

    /// The start of vanilla `MinecartHopper.tick`.
    pub(super) fn begin_tick(&self) {
        self.consumed_item_this_frame
            .store(false, Ordering::Relaxed);
    }

    /// Vanilla `MinecartHopper.tryConsumeItems`.
    pub(super) fn try_consume_items(&self, entity: &Entity) {
        if entity.is_alive()
            && self.enabled.load(Ordering::Relaxed)
            && !self.consumed_item_this_frame.load(Ordering::Relaxed)
            && self.suck_in_items(entity)
        {
            self.consumed_item_this_frame.store(true, Ordering::Relaxed);
        }
    }

    /// Vanilla `MinecartHopper.suckInItems`.
    fn suck_in_items(&self, entity: &Entity) -> bool {
        let world = entity.world.load();
        let pos = entity.pos.load();
        // Vanilla `HopperBlockEntity.suckInItems`: `getLevelY()` is half a block above the cart.
        let source_center = Vector3::new(pos.x, pos.y + 1.5, pos.z);
        let source_pos = BlockPos::floored_v(source_center);
        let source_block_entity = world.get_block_entity(&source_pos);
        // Vanilla `tryTakeInItemFromSlot` reads the source's slots, which unpacks its loot table.
        if let Some(source_block_entity) = &source_block_entity {
            source_block_entity
                .clone()
                .unpack_loot_table_before_read(&world);
        }
        let source = source_block_entity
            .and_then(crate::block::entities::BlockEntity::get_inventory)
            .or_else(|| entity_container_at(&world, source_center));
        if let Some(source) = source {
            return self.take_one_from(source.as_ref());
        }
        // Vanilla `Hopper.SUCK_AABB`, moved to the cart. It is not grid aligned, so nothing
        // solid above blocks it.
        let suction_box = BoundingBox::new(
            Vector3::new(pos.x - 0.5, pos.y + 0.6875, pos.z - 0.5),
            Vector3::new(pos.x + 0.5, pos.y + 2.0, pos.z + 0.5),
        );
        self.pick_up_items_in(&world, &suction_box)
            || self.pick_up_items_in(&world, &entity.bounding_box.load().expand(0.25, 0.0, 0.25))
    }

    /// Vanilla `tryTakeInItemFromSlot` over the source's slots.
    fn take_one_from(&self, source: &dyn Inventory) -> bool {
        for slot in 0..source.size() {
            let stack = source.get_stack(slot);
            if stack.is_empty() || !source.can_transfer_to(self.inventory.as_ref(), slot, &stack) {
                continue;
            }
            let one = source.remove_stack_specific(slot, 1);
            if HopperBlockEntity::add_one_item(source, self.inventory.as_ref(), &one) {
                source.mark_dirty();
                return true;
            }
            source.set_stack(slot, stack);
        }
        false
    }

    fn pick_up_items_in(&self, world: &Arc<World>, search_box: &BoundingBox) -> bool {
        world
            .get_entities_at_box(search_box)
            .iter()
            .filter(|entity| entity.get_entity().is_alive())
            .filter_map(|entity| entity.get_item_entity())
            .any(|item| self.add_item(item))
    }

    /// Vanilla `HopperBlockEntity.addItem(Container, ItemEntity)`: takes the whole stack, or as
    /// much as fits, and only counts as done when the item entity is used up.
    fn add_item(&self, item: &ItemEntity) -> bool {
        let mut stack = item
            .get_item_stack()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        if stack.is_empty() {
            return false;
        }
        let inventory = self.inventory.as_ref();
        for slot in 0..inventory.size() {
            if stack.is_empty() {
                break;
            }
            let mut current = inventory.get_stack(slot);
            if current.is_empty() {
                inventory.set_stack(
                    slot,
                    std::mem::replace(&mut stack, ItemStack::EMPTY.clone()),
                );
            } else if current.are_items_and_components_equal(&stack) {
                let space = stack
                    .get_max_stack_size()
                    .saturating_sub(current.item_count);
                let count = stack.item_count.min(space);
                if count > 0 {
                    stack.item_count -= count;
                    current.item_count += count;
                    inventory.set_stack(slot, current);
                }
            }
        }
        let used_up = stack.is_empty();
        item.set_item_stack(stack);
        if used_up {
            item.get_entity().remove();
        }
        used_up
    }

    pub(super) fn interact(&self, entity: &Entity, player: &Arc<Player>) -> bool {
        container::open(
            entity,
            player,
            &self.inventory,
            TextComponent::translate_cross(
                translation::java::ENTITY_MINECRAFT_HOPPER_MINECART,
                translation::bedrock::ENTITY_HOPPER_MINECART_NAME,
                [],
            ),
            true,
        )
    }

    pub(super) fn write_nbt(&self, nbt: &mut NbtCompound) {
        self.inventory.write_nbt(nbt);
        nbt.put_bool("Enabled", self.enabled.load(Ordering::Relaxed));
    }

    pub(super) fn read_nbt(&self, nbt: &NbtCompound) {
        self.inventory.read_nbt(nbt);
        self.enabled
            .store(nbt.get_bool("Enabled").unwrap_or(true), Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::HopperMinecart;
    use pumpkin_inventory::Inventory;

    #[test]
    fn hopper_minecart_inventory_has_five_slots() {
        assert_eq!(HopperMinecart::new().inventory.size(), 5);
    }
}
