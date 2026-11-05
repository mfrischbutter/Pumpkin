//! What a piglin does with items: its hands, its inventory and what it picks up.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use pumpkin_data::data_component_impl::{BlocksAttacksImpl, EquipmentSlot, KineticWeaponImpl};
use pumpkin_data::entity::EntityType;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::{Sound, SoundCategory};

use super::crossbow_attack_mob::CrossbowAttackMob;
use super::equipment as mob_equipment;
use super::piglin::PiglinEntity;
use super::piglin_ai::PiglinAi;
use super::piglin_brain;
use crate::entity::ai::brain::memory::types;
use crate::entity::item::ItemEntity;
use crate::entity::mob::Mob;
use crate::entity::player::Player;
use crate::entity::{Entity, EntityBase};
use crate::world::World;

/// Vanilla `Piglin.MAX_PASSENGERS_ON_ONE_HOGLIN`.
const MAX_PASSENGERS_ON_ONE_HOGLIN: usize = 3;

impl PiglinEntity {
    /// Vanilla's check in `StopHoldingItemIfNoLongerAdmiring`: a shield stays where it is.
    #[must_use]
    pub fn off_hand_blocks_nothing(&self) -> bool {
        let off_hand = self.off_hand_item();
        !off_hand.is_empty() && off_hand.get_data_component::<BlocksAttacksImpl>().is_none()
    }

    /// Vanilla `isHolding(Items.CROSSBOW)`.
    #[must_use]
    pub fn is_holding_crossbow(&self) -> bool {
        self.main_hand_item().item.id == Item::CROSSBOW.id
            || self.off_hand_item().item.id == Item::CROSSBOW.id
    }

    /// Whether either hand holds a spear, the other weapon `canUseNonMeleeWeapon` allows.
    #[must_use]
    pub fn is_holding_kinetic_weapon(&self) -> bool {
        self.main_hand_item()
            .get_data_component::<KineticWeaponImpl>()
            .is_some()
            || self
                .off_hand_item()
                .get_data_component::<KineticWeaponImpl>()
                .is_some()
    }

    pub fn start_charging_crossbow(&self) {
        let living = &self.mob_entity.living_entity;
        living.set_active_hand(pumpkin_util::Hand::Right, self.main_hand_item(), i32::MAX);
        CrossbowAttackMob::set_charging_crossbow(self, true);
        let entity = &living.entity;
        entity.world.load().play_sound(
            Sound::ItemCrossbowLoadingStart,
            SoundCategory::Hostile,
            &entity.pos.load(),
        );
    }

    pub fn finish_charging_crossbow(&self) {
        let living = &self.mob_entity.living_entity;
        living.clear_active_hand();
        CrossbowAttackMob::set_charging_crossbow(self, false);
        let entity = &living.entity;
        entity.world.load().play_sound(
            Sound::ItemCrossbowLoadingEnd,
            SoundCategory::Hostile,
            &entity.pos.load(),
        );
    }

    pub fn cancel_charging_crossbow(&self) {
        self.mob_entity.living_entity.clear_active_hand();
        CrossbowAttackMob::set_charging_crossbow(self, false);
    }

    /// Vanilla `Piglin.startRiding`: a baby climbs to the top of up to 3 riders of a hoglin.
    pub fn start_riding(mob: &dyn Mob, world: &Arc<World>, target: Arc<dyn EntityBase>) {
        let mut vehicle = target;
        if vehicle.get_entity().entity_type == &EntityType::HOGLIN
            && mob
                .cast_any()
                .downcast_ref::<Self>()
                .is_some_and(Self::is_baby)
        {
            for _ in 1..MAX_PASSENGERS_ON_ONE_HOGLIN {
                let next = vehicle
                    .get_entity()
                    .passengers
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .first()
                    .cloned();
                let Some(next) = next else {
                    break;
                };
                vehicle = next;
            }
        }
        if let Some(rider) = world.get_entity_by_id(mob.get_entity().entity_id) {
            vehicle.get_entity().add_passenger(vehicle.clone(), rider);
        }
    }

    #[must_use]
    pub fn can_add_to_inventory(&self, item: &ItemStack) -> bool {
        let inv = self
            .inventory
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        inv.len() < Self::INVENTORY_SIZE
            || inv.iter().any(|slot| {
                slot.are_items_and_components_equal(item)
                    && slot.item_count < slot.get_max_stack_size()
            })
    }

    pub fn add_to_inventory(&self, item: ItemStack) -> Option<ItemStack> {
        if item.is_empty() {
            return None;
        }
        let mut inv = self
            .inventory
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut remaining = item;
        for slot in inv.iter_mut() {
            if !slot.are_items_and_components_equal(&remaining) {
                continue;
            }
            let space = slot.get_max_stack_size().saturating_sub(slot.item_count);
            let moved = remaining.item_count.min(space);
            slot.increment(moved);
            remaining.decrement(moved);
            if remaining.is_empty() {
                return None;
            }
        }
        if inv.len() < Self::INVENTORY_SIZE {
            inv.push(remaining);
            return None;
        }
        Some(remaining)
    }

    #[must_use]
    pub fn can_replace_current_item_for(&self, new_item: &ItemStack) -> bool {
        let slot = mob_equipment::get_equipment_slot_for_item(new_item);
        let current = self
            .mob_entity
            .living_entity
            .entity_equipment
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&slot);
        Mob::can_replace_current_item(self, new_item, &current, &slot)
    }

    pub(super) fn put_in_inventory(&self, item: ItemStack) {
        let remainder = self
            .add_to_inventory(item)
            .unwrap_or_else(|| ItemStack::EMPTY.clone());
        // Vanilla swings the off hand here even when nothing was left over.
        PiglinAi::throw_items_toward_random_pos(self, vec![remainder]);
    }

    pub(super) fn hold_in_off_hand(&self, item: ItemStack) {
        if self.is_holding_item_in_off_hand() {
            self.mob_entity
                .living_entity
                .entity
                .spawn_at_location(self.off_hand_item());
        }
        let keep_loaded = !PiglinAi::is_barter_currency(&item);
        self.mob_entity
            .set_item_slot_and_drop_when_killed(&EquipmentSlot::OFF_HAND, item);
        if keep_loaded {
            self.mob_entity
                .persistence_required
                .store(true, Ordering::Relaxed);
        }
    }

    pub(super) fn hold_in_main_hand(&self, item: ItemStack) {
        self.mob_entity
            .set_item_slot_and_drop_when_killed(&EquipmentSlot::MAIN_HAND, item);
        self.mob_entity
            .persistence_required
            .store(true, Ordering::Relaxed);
    }

    pub(super) fn main_hand_item(&self) -> ItemStack {
        self.mob_entity
            .living_entity
            .entity_equipment
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&EquipmentSlot::MAIN_HAND)
    }

    pub(super) fn wants_item_entity(&self, item: &ItemEntity) -> bool {
        let stack = item
            .get_item_stack()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        !stack.is_empty() && PiglinAi::wants_to_pickup(self, &stack)
    }

    pub(super) fn pick_up_nearby_items(&self) {
        self.mob_entity.for_each_item_in_pickup_reach(|item| {
            if self.wants_item_entity(item) {
                self.pick_up_item(item);
            }
        });
    }

    pub(super) fn pick_up_item(&self, item: &ItemEntity) {
        self.stop_walking();
        let (count, picked_up) = {
            let stack = item
                .get_item_stack()
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            // Nuggets are taken whole, everything else one at a time.
            let count = if stack.item.id == Item::GOLD_NUGGET.id {
                stack.item_count
            } else {
                stack.item_count.min(1)
            };
            (count, stack.copy_with_count(count))
        };
        if count == 0
            || !self.mob_entity.living_entity.pickup(
                item.get_entity(),
                Some(&picked_up),
                u32::from(count),
            )
        {
            return;
        }

        let taken = item
            .get_item_stack()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .split(count);
        let emptied = item
            .get_item_stack()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_empty();
        if emptied {
            item.get_entity().remove();
        } else {
            item.init_data_tracker();
        }

        if PiglinAi::is_loved_item(&taken) {
            if let Some(thrower_uuid) = item.get_thrower()
                && let Some(thrower_player) = self
                    .mob_entity
                    .living_entity
                    .entity
                    .world
                    .load()
                    .get_player_by_uuid(thrower_uuid)
            {
                let is_baby = self.is_baby();
                let player_wearing_safe_armor =
                    PiglinAi::is_wearing_safe_armor(&thrower_player.living_entity);
                thrower_player.trigger_advancement(
                    crate::entity::player::advancement::trigger::AdvancementTrigger::ThrownItemPickedUpByEntity {
                        entity_type: "minecraft:piglin".to_string(),
                        item_id: taken.item.registry_key.to_string(),
                        is_baby,
                        player_wearing_safe_armor,
                    },
                );
            }
            self.hold_in_off_hand(taken);
            self.admiring.store(true, Ordering::Relaxed);
            self.mob_entity.with_brain(self, |brain| {
                brain.erase(types::TIME_TRYING_TO_REACH_ADMIRE_ITEM.id());
                piglin_brain::admire_gold_item(brain);
            });
        } else if PiglinAi::is_food(&taken) && !self.has_eaten_recently() {
            self.ate_recently.store(true, Ordering::Relaxed);
            self.mob_entity
                .with_brain(self, |brain| piglin_brain::eat_for_a_while(brain));
        } else if mob_equipment::equip_item_if_possible(self, taken.clone()).is_empty() {
            self.put_in_inventory(taken);
        }
    }

    pub fn stop_holding_off_hand_item(
        &self,
        bartering_enabled: bool,
        nearest_player: Option<Arc<Player>>,
    ) {
        let item = self
            .mob_entity
            .set_item_slot(&EquipmentSlot::OFF_HAND, ItemStack::EMPTY.clone());

        if self.is_adult() {
            let is_barter = PiglinAi::is_barter_currency(&item);
            if bartering_enabled && is_barter {
                let outcomes = PiglinAi::get_barter_response_items(self);
                let entity = &self.mob_entity.living_entity.entity;

                let mut event =
                    crate::plugin::api::events::entity::piglin_barter::PiglinBarterEvent::new(
                        entity.entity_id,
                        item,
                        outcomes,
                    );
                if let Some(server) = entity.world.load().server.upgrade() {
                    server.plugin_manager.fire_blocking(&server, &mut event);
                }

                if !event.cancelled {
                    PiglinAi::throw_items(self, event.outcome, nearest_player);
                }
            } else if !is_barter
                && mob_equipment::equip_item_if_possible(self, item.clone()).is_empty()
            {
                self.put_in_inventory(item);
            }
        } else if mob_equipment::equip_item_if_possible(self, item.clone()).is_empty() {
            let main_hand = self.main_hand_item();
            if PiglinAi::is_loved_item(&main_hand) {
                self.put_in_inventory(main_hand);
            } else {
                PiglinAi::throw_items(self, vec![main_hand], nearest_player);
            }
            self.hold_in_main_hand(item);
        }
    }

    pub fn cancel_admiring(&self) {
        if self.is_admiring() && self.is_holding_item_in_off_hand() {
            self.mob_entity
                .living_entity
                .entity
                .spawn_at_location(self.off_hand_item());
            self.mob_entity
                .set_item_slot(&EquipmentSlot::OFF_HAND, ItemStack::EMPTY.clone());
        }
    }

    pub fn drop_inventory(&self) {
        let items = self
            .inventory
            .try_lock()
            .map_or_else(|_| Vec::new(), |mut inv| std::mem::take(&mut *inv));
        let entity = &self.mob_entity.living_entity.entity;
        let world = entity.world.load();
        let pos = entity.pos.load();
        for item in items {
            if !item.is_empty() {
                let item_entity =
                    ItemEntity::new(Entity::new(world.clone(), pos, &EntityType::ITEM), item);
                world.spawn_entity(Arc::new(item_entity));
            }
        }
    }

    pub fn start_admiring(&self, item: ItemStack) {
        self.hold_in_off_hand(item);
        self.admiring.store(true, Ordering::Relaxed);
        self.mob_entity.with_brain(self, |brain| {
            piglin_brain::admire_gold_item(brain);
        });
        self.stop_walking();
    }

    pub(super) fn stop_walking(&self) {
        self.mob_entity.steered().navigator().stop();
        self.mob_entity.with_brain(self, |brain| {
            brain.erase(types::WALK_TARGET.id());
        });
    }
}
