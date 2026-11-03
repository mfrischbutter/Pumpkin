//! The block in a sulfur cube's body slot: swallowing, shearing and what it does to the cube.

use std::sync::atomic::Ordering::Relaxed;

use pumpkin_data::data_component_impl::EquipmentSlot;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::Sound;
use pumpkin_data::tag::{self, Taggable};

use super::archetype::{BodyEffects, matching_archetypes};
use super::{PICKUP_TIMER_DURATION, SulfurCubeEntity};
use crate::entity::EntityBase;
use crate::entity::ageable::AgeableMob;
use crate::entity::attributes::Modifier;
use crate::entity::item::ItemEntity;

/// Vanilla `SulfurCube.isSwallowableItem`.
#[must_use]
pub fn is_swallowable_item(stack: &ItemStack) -> bool {
    stack
        .item
        .has_tag(&tag::Item::MINECRAFT_SULFUR_CUBE_SWALLOWABLE)
}

impl SulfurCubeEntity {
    pub(super) fn body_item(&self) -> ItemStack {
        self.mob_entity
            .living_entity
            .entity_equipment
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&EquipmentSlot::BODY)
    }

    /// Vanilla `SulfurCube.hasBodyItem`.
    #[must_use]
    pub fn has_body_item(&self) -> bool {
        !self.body_item().is_empty()
    }

    /// Vanilla `SulfurCube.canHoldItem`.
    #[must_use]
    pub fn can_hold_item(&self, stack: &ItemStack) -> bool {
        !self.has_body_item() && is_swallowable_item(stack) && !self.is_baby()
    }

    /// Vanilla `SulfurCube.equipItem`: swallows one of `held`, spitting out what it held.
    pub fn equip_item(&self, held: &ItemStack) -> bool {
        if self.is_baby() {
            return false;
        }
        let swallowed = self.body_item();
        if !swallowed.is_empty() {
            if held.item == swallowed.item {
                return false;
            }
            self.spawn_at_body_offset(swallowed.clone());
        }
        self.mob_entity
            .set_item_slot_and_drop_when_killed(&EquipmentSlot::BODY, held.copy_with_count(1));
        // A cube with a block has no goals to take out, so only the rest of the change is due.
        if !swallowed.is_empty() {
            self.collect_equipment_changes();
        }
        self.play_entity_sound(Sound::EntitySulfurCubeAbsorb, 1.0, 1.0);
        true
    }

    /// Vanilla `SulfurCube.readyForShearing`.
    #[must_use]
    pub fn ready_for_shearing(&self) -> bool {
        self.has_body_item()
    }

    /// Vanilla `SulfurCube.shear`: the block comes out and the cube leaves items alone a while.
    pub fn shear(&self) {
        let stack = self
            .mob_entity
            .set_item_slot(&EquipmentSlot::BODY, ItemStack::EMPTY.clone());
        self.spawn_at_body_offset(stack);
        self.play_entity_sound(Sound::EntitySulfurCubeEject, 1.0, 1.0);
        self.pickup_timer.store(PICKUP_TIMER_DURATION, Relaxed);
    }

    /// Vanilla `spawnAtLocation` at the average passenger attachment, the top of the cube.
    fn spawn_at_body_offset(&self, stack: ItemStack) {
        let entity = self.get_entity();
        entity.spawn_at_location_with_offset(stack, f64::from(entity.height()));
    }

    /// Vanilla `SulfurCube.canPickUpLoot`, as the flag the scan of `Mob.aiStep` reads and the
    /// save writes.
    pub(super) fn sync_can_pick_up_loot(&self) {
        let can_pick_up = !self.has_body_item();
        if self.mob_entity.can_pick_up_loot() != can_pick_up {
            self.mob_entity.set_can_pick_up_loot(can_pick_up);
        }
    }

    pub(super) fn pick_up_items(&self) {
        self.mob_entity
            .for_each_item_in_pickup_reach(|item| self.pick_up_item(item));
    }

    /// Vanilla `SulfurCube.pickUpItem`.
    fn pick_up_item(&self, item: &ItemEntity) {
        let stack = item
            .get_item_stack()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        if stack.is_empty() || !self.can_hold_item(&stack) || self.pickup_timer.load(Relaxed) > 0 {
            return;
        }
        let one = stack.copy_with_count(1);
        if !self
            .mob_entity
            .living_entity
            .pickup(item.get_entity(), Some(&one), 1)
        {
            return;
        }
        let (taken, emptied) = {
            let mut held = item
                .get_item_stack()
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let taken = held.split(1);
            (taken, held.is_empty())
        };
        if emptied {
            item.get_entity().remove();
        } else {
            item.init_data_tracker();
        }
        self.mob_entity.set_item_slot(&EquipmentSlot::BODY, taken);
        self.play_entity_sound(Sound::EntitySulfurCubeAbsorb, 1.0, 1.0);
        self.mob_entity.set_guaranteed_drop(&EquipmentSlot::BODY);
    }

    /// Vanilla `SulfurCube.collectEquipmentChanges`, the archetype part.
    pub(super) fn collect_equipment_changes(&self) {
        let current = self.body_item();
        let previous = {
            let mut last = self
                .last_body
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if (last.is_empty() && current.is_empty()) || last.are_equal(&current) {
                return;
            }
            std::mem::replace(&mut *last, current.clone())
        };
        self.apply_archetype_changes(&previous, &current);
    }

    fn apply_archetype_changes(&self, previous: &ItemStack, current: &ItemStack) {
        let living = &self.mob_entity.living_entity;
        let world = living.entity.world.load();
        for archetype in matching_archetypes(&world, previous) {
            for modifier in &archetype.attribute_modifiers {
                living.update_attribute(modifier.attribute, |instance| {
                    instance.remove_modifier(&modifier.id);
                });
            }
        }

        let mut effects = BodyEffects::default();
        for archetype in matching_archetypes(&world, current) {
            if archetype.buoyant {
                effects.floats_in_liquids = true;
            }
            if archetype.explosion.is_some() {
                effects.explosion = archetype.explosion;
            }
            if let Some(contact_damage) = archetype.contact_damage {
                effects.contact_damages.push(contact_damage);
            }
            effects.knockback_modifier = archetype.knockback_modifiers;
            effects.sound_settings = archetype.sound_settings;
            for modifier in archetype.attribute_modifiers {
                living.update_attribute(modifier.attribute, |instance| {
                    instance.add_or_replace_modifier(Modifier {
                        id: modifier.id,
                        amount: modifier.amount,
                        operation: modifier.operation,
                    });
                });
            }
        }
        *self.effects() = effects;
    }

    /// Vanilla `SulfurCube.collectEquipmentChanges`, the goal part: a cube with a block has none.
    pub(super) fn sync_goals(&self) {
        let has_body = self.has_body_item();
        let registered = self.goals_registered.load(Relaxed);
        if has_body && registered {
            self.mob_entity.goals_selector.clear(self);
            self.mob_entity.set_speed(0.0);
            self.goals_registered.store(false, Relaxed);
        } else if !has_body && !registered {
            self.register_goals();
        }
    }

    /// Vanilla `SulfurCube.travelInFluid`: a buoyant block bobs at the surface.
    pub(super) fn float_in_fluid(&self) {
        if !self.has_body_item() || !self.effects().floats_in_liquids {
            return;
        }
        let living = &self.mob_entity.living_entity;
        let entity = &living.entity;
        let vibe = 0.2f32 * (entity.tick_count.load(Relaxed) as f32 * 0.4f32).sin();
        let fluid_height = if entity.is_in_water() {
            entity.water_height.load()
        } else {
            entity.lava_height.load()
        };
        let immersion = fluid_height - living.get_swim_height() + f64::from(vibe);
        if immersion > 0.0 {
            let mut velocity = entity.velocity.load();
            velocity.y += immersion.min(1.0) * f64::from(0.04f32);
            entity.velocity.store(velocity);
        }
    }
}
