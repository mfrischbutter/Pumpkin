use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU8, Ordering},
};

use pumpkin_data::{item_stack::ItemStack, tracked_data::item_display as tracked};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_protocol::codec::item_stack_seralizer::ItemStackSerializer;

use super::DisplayEntity;
use crate::{
    entity::{Entity, EntityBase, living::LivingEntity},
    server::Server,
};

/// Vanilla `ItemDisplayContext`, by id.
const ITEM_DISPLAY_CONTEXTS: [&str; 10] = [
    "none",
    "thirdperson_lefthand",
    "thirdperson_righthand",
    "firstperson_lefthand",
    "firstperson_righthand",
    "head",
    "gui",
    "ground",
    "fixed",
    "on_shelf",
];

fn empty_stack() -> ItemStack {
    ItemStack::new(0, &pumpkin_data::item::Item::AIR)
}

/// Vanilla `Display.ItemDisplay`.
pub struct ItemDisplayEntity {
    pub display: DisplayEntity,
    pub item_stack: Mutex<ItemStack>,
    pub item_display: AtomicU8,
}

impl ItemDisplayEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let data = &entity.synched_data;
        data.define(tracked::ITEM, ItemStackSerializer::from(empty_stack()));
        data.define(tracked::ITEM_DISPLAY, 0u8);
        Arc::new(Self {
            display: DisplayEntity::new(entity),
            item_stack: Mutex::new(empty_stack()),
            item_display: AtomicU8::new(0),
        })
    }

    #[must_use]
    pub fn get_item(&self) -> ItemStack {
        self.item_stack
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    pub fn set_item(&self, item: ItemStack) {
        *self
            .item_stack
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = item.clone();
        self.display
            .entity
            .set_synced_data(tracked::ITEM, ItemStackSerializer::from(item));
    }

    #[must_use]
    pub fn get_item_display_mode(&self) -> u8 {
        self.item_display.load(Ordering::Relaxed)
    }

    pub fn set_item_display_mode(&self, mode: u8) {
        self.item_display.store(mode, Ordering::Relaxed);
        self.display
            .entity
            .set_synced_data(tracked::ITEM_DISPLAY, mode);
    }
}

impl EntityBase for ItemDisplayEntity {
    /// Vanilla `ItemDisplay.addAdditionalSaveData`: an empty stack is left out.
    fn write_custom_nbt(&self, nbt: &mut NbtCompound) {
        self.display.write_display_nbt(nbt);
        let item = self.get_item();
        if !item.is_empty() {
            let mut compound = NbtCompound::new();
            item.write_item_stack(&mut compound);
            nbt.put_compound("item", compound);
        }
        let context = ITEM_DISPLAY_CONTEXTS
            .get(usize::from(self.get_item_display_mode()))
            .copied()
            .unwrap_or(ITEM_DISPLAY_CONTEXTS[0]);
        nbt.put_string("item_display", context.to_string());
    }

    /// Vanilla `ItemDisplay.readAdditionalSaveData`.
    fn read_custom_nbt(&self, nbt: &NbtCompound) {
        self.display.read_display_nbt(nbt);
        self.set_item(
            nbt.get_compound("item")
                .and_then(ItemStack::read_item_stack)
                .unwrap_or_else(empty_stack),
        );
        let context = nbt
            .get_string("item_display")
            .and_then(|name| ITEM_DISPLAY_CONTEXTS.iter().position(|&c| c == name))
            .unwrap_or(0);
        self.set_item_display_mode(context as u8);
    }

    fn tick(&self, _caller: &dyn EntityBase, _server: &Server) {
        self.display.tick_display();
    }

    /// Every value already reached the synced data through its setter.
    fn init_data_tracker(&self) {}

    fn get_entity(&self) -> &Entity {
        &self.display.entity
    }

    fn get_living_entity(&self) -> Option<&LivingEntity> {
        None
    }

    fn cast_any(&self) -> &dyn std::any::Any {
        self
    }

    fn is_pushed_by_fluids(&self) -> bool {
        false
    }

    fn is_immune_to_explosion(&self) -> bool {
        true
    }
}
