//! Vanilla `ItemEntity.playerTouch`.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use crate::entity::EntityBase;
use crate::entity::player::Player;
use crate::entity::player::advancement::trigger::AdvancementTrigger;
use crate::entity::player::statistics::StatisticCategory;

use super::ItemEntity;

impl ItemEntity {
    /// Vanilla `playerTouch`: a delay of exactly 0, and no other player as the `target`.
    pub(super) fn player_touch(&self, player: &Arc<Player>) {
        if self.is_never_pickup()
            || self.pickup_delay.load(Ordering::Relaxed) != 0
            || self
                .target
                .load()
                .is_some_and(|target| target != player.gameprofile.id)
            || player.living_entity.health.load() <= 0.0
            || player.is_spectator()
        {
            return;
        }

        // Fire the pickup event before the stack moves, so a cancel leaves it on the ground.
        let offered = self
            .item_stack
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        if (player.is_creative() || player.inventory.can_insert_stack_anywhere(&offered))
            && !player.living_entity.fire_pickup_event(
                &self.entity,
                Some(&offered),
                offered.item_count.into(),
            )
        {
            return;
        }

        let item_id = offered.item.id;
        let count_before = offered.item_count;
        let mut local_stack = offered;
        let added = player.inventory.insert_stack_anywhere(&mut local_stack);
        // Vanilla `Inventory.add`: with infinite materials what does not fit is gone as well.
        let inserted = if player.is_creative() && !local_stack.is_empty() {
            local_stack.item_count = 0;
            true
        } else {
            added
        };
        let is_empty = local_stack.is_empty();
        *self
            .item_stack
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = local_stack;

        if !inserted {
            return;
        }
        player.inventory_changed.store(true, Ordering::Relaxed);

        // Vanilla counts the whole stack here, also when only a part of it fit.
        if let Some(thrower) = self.get_owner_entity() {
            player.trigger_advancement(AdvancementTrigger::ThrownItemPickedUpByPlayer {
                thrower_type: format!(
                    "minecraft:{}",
                    thrower.get_entity().entity_type.resource_name
                ),
            });
        }
        player.increment_stat(
            StatisticCategory::PickedUp,
            item_id as i32,
            i32::from(count_before),
        );
        player
            .living_entity
            .send_pickup_animation(&self.entity, count_before.into());

        player
            .current_screen_handler
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .send_content_updates();

        if is_empty {
            self.entity.remove();
        } else {
            self.on_count_changed();
        }
    }
}
