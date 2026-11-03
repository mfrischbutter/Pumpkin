//! Vanilla `Mob.createEquipmentSlotContainer`: one equipment slot of a mob as an inventory, for
//! the saddle and armour slots of a mount's screen.

use std::sync::Weak;
use std::sync::atomic::Ordering;

use pumpkin_data::data_component_impl::{EquipmentSlot, EquippableImpl};
use pumpkin_data::game_event::GameEvent;
use pumpkin_data::item_stack::ItemStack;

use crate::entity::EntityBase;
use crate::entity::mob::{Mob, equipment};

pub struct EquipmentSlotContainer {
    mob: Weak<dyn EntityBase>,
    slot: EquipmentSlot,
}

impl EquipmentSlotContainer {
    #[must_use]
    pub const fn new(mob: Weak<dyn EntityBase>, slot: EquipmentSlot) -> Self {
        Self { mob, slot }
    }

    fn with_mob<R>(&self, default: R, f: impl FnOnce(&dyn Mob) -> R) -> R {
        self.mob
            .upgrade()
            .and_then(|entity| entity.get_mob().map(f))
            .unwrap_or(default)
    }

    /// Vanilla `ArmorSlot.setByPlayer` calling `LivingEntity.onEquipItem`.
    fn on_equip_item(&self, mob: &dyn Mob, stack: &ItemStack) {
        let equippable = stack.get_data_component::<EquippableImpl>();
        let entity = mob.get_entity();
        let world = entity.world.load();
        let pos = entity.pos.load();
        if !entity.is_silent()
            && let Some(equippable) = equippable.filter(|equippable| *equippable.slot == self.slot)
        {
            if self.slot == EquipmentSlot::SADDLE {
                world.play_sound(mob.saddle_sound(), mob.mob_sound_source(), &pos);
            } else {
                world.play_sound_event(&equippable.equip_sound, mob.mob_sound_source(), &pos);
            }
        }
        let event = if equippable.is_some() {
            GameEvent::Equip
        } else {
            GameEvent::Unequip
        };
        world.emit_game_event(event.name(), pos);
    }
}

impl pumpkin_inventory::Clearable for EquipmentSlotContainer {
    fn clear(&self) {
        pumpkin_inventory::Inventory::set_stack(self, 0, ItemStack::EMPTY.clone());
    }
}

impl pumpkin_inventory::Inventory for EquipmentSlotContainer {
    fn size(&self) -> usize {
        1
    }

    fn is_empty(&self) -> bool {
        self.get_stack(0).is_empty()
    }

    fn get_stack(&self, _slot: usize) -> ItemStack {
        self.with_mob(ItemStack::EMPTY.clone(), |mob| {
            mob.get_mob_entity()
                .living_entity
                .entity_equipment
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .get(&self.slot)
        })
    }

    fn remove_stack(&self, slot: usize) -> ItemStack {
        let removed = self.get_stack(slot);
        self.set_stack(slot, ItemStack::EMPTY.clone());
        removed
    }

    fn remove_stack_specific(&self, slot: usize, amount: u8) -> ItemStack {
        let mut stack = self.get_stack(slot);
        if stack.is_empty() || amount == 0 {
            return ItemStack::EMPTY.clone();
        }
        let removed = stack.split(amount);
        self.set_stack(slot, stack);
        removed
    }

    fn set_stack(&self, _slot: usize, stack: ItemStack) {
        self.with_mob((), |mob| {
            let mob_entity = mob.get_mob_entity();
            let previous = mob_entity.set_item_slot(&self.slot, stack.clone());
            if !stack.is_empty() {
                mob_entity.set_guaranteed_drop(&self.slot);
                mob_entity
                    .persistence_required
                    .store(true, Ordering::Relaxed);
            }
            if !previous.are_equal(&stack) {
                self.on_equip_item(mob, &stack);
            }
        });
    }

    fn get_max_count_per_stack(&self) -> u8 {
        1
    }

    /// Vanilla `LivingEntity.isEquippableInSlot` with the mount's `canUseSlot`.
    fn is_valid_slot_for(&self, _slot: usize, stack: &ItemStack) -> bool {
        self.with_mob(false, |mob| {
            let can_use = if self.slot == EquipmentSlot::SADDLE {
                mob.can_be_saddled()
            } else {
                mob.can_use_body_slot()
            };
            can_use
                && equipment::is_equippable_in_slot(mob.get_entity().entity_type, stack, &self.slot)
        })
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}
