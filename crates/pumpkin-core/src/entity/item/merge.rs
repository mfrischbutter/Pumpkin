//! Vanilla `ItemEntity.mergeWithNeighbours` and what it calls.

use std::sync::atomic::{
    AtomicBool,
    Ordering::{self, AcqRel},
};

use pumpkin_data::item_stack::ItemStack;

use super::{ItemEntity, LIFETIME, MERGE_DISTANCE, MERGE_MAX_COUNT};

struct ItemMergeReservation<'a> {
    low: &'a AtomicBool,
    high: &'a AtomicBool,
}

impl<'a> ItemMergeReservation<'a> {
    fn acquire(low: &'a ItemEntity, high: &'a ItemEntity) -> Option<Self> {
        if std::ptr::eq(low, high)
            || low
                .merge_reserved
                .compare_exchange(false, true, AcqRel, Ordering::Acquire)
                .is_err()
        {
            return None;
        }

        if high
            .merge_reserved
            .compare_exchange(false, true, AcqRel, Ordering::Acquire)
            .is_err()
        {
            low.merge_reserved.store(false, Ordering::Release);
            return None;
        }

        Some(Self {
            low: &low.merge_reserved,
            high: &high.merge_reserved,
        })
    }
}

impl Drop for ItemMergeReservation<'_> {
    fn drop(&mut self) {
        self.high.store(false, Ordering::Release);
        self.low.store(false, Ordering::Release);
    }
}

impl ItemEntity {
    /// Vanilla `ItemEntity.areMergable`.
    #[must_use]
    pub fn are_mergeable(stack: &ItemStack, other: &ItemStack) -> bool {
        u16::from(stack.item_count) + u16::from(other.item_count)
            <= u16::from(other.get_max_stack_size())
            && stack.are_items_and_components_equal(other)
    }

    /// Vanilla `ItemEntity.isMergable`.
    pub(super) fn is_mergeable(&self) -> bool {
        if self.entity.removed.load(Ordering::SeqCst)
            || self.never_pickup.load(Ordering::Relaxed)
            || self.never_despawn.load(Ordering::Relaxed)
            || self.item_age.load(Ordering::Relaxed) >= LIFETIME
        {
            return false;
        }
        let Ok(item_stack) = self.item_stack.try_lock() else {
            return false;
        };

        item_stack.item_count < item_stack.get_max_stack_size()
    }

    /// Vanilla `ItemEntity.mergeWithNeighbours`.
    pub(super) fn merge_with_neighbours(&self) {
        if !self.is_mergeable() {
            return;
        }

        let bounding_box =
            self.entity
                .bounding_box
                .load()
                .expand(MERGE_DISTANCE, 0.0, MERGE_DISTANCE);
        let world = self.entity.world.load();
        for other in world.get_entities_at_box(&bounding_box) {
            let Some(item) = other.get_item_entity() else {
                continue;
            };
            if item.entity.entity_id == self.entity.entity_id || !item.is_mergeable() {
                continue;
            }

            self.try_merge_with(item);
            if self.entity.removed.load(Ordering::SeqCst) {
                break;
            }
        }
    }

    #[expect(clippy::too_many_lines)]
    fn try_merge_with(&self, other: &Self) {
        // Vanilla `tryToMerge`: only items meant for the same player.
        if self.target.load() != other.target.load() {
            return;
        }

        // Always lock in entity_id order to prevent deadlock when two
        // items try to merge with each other concurrently.
        let (low, high) = if self.entity.entity_id < other.entity.entity_id {
            (self, other)
        } else {
            (other, self)
        };
        let Some(_reservation) = ItemMergeReservation::acquire(low, high) else {
            return;
        };

        let (expected_low, expected_high, target_is_self) = {
            let low_stack = low
                .item_stack
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let high_stack = high
                .item_stack
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let (self_stack, other_stack) = if self.entity.entity_id < other.entity.entity_id {
                (&*low_stack, &*high_stack)
            } else {
                (&*high_stack, &*low_stack)
            };

            if !Self::are_mergeable(self_stack, other_stack) {
                return;
            }

            (
                low_stack.clone(),
                high_stack.clone(),
                other_stack.item_count < self_stack.item_count,
            )
        };
        let (target, source) = if target_is_self {
            (self, other)
        } else {
            (other, self)
        };
        if self.entity.removed.load(Ordering::SeqCst) || other.entity.removed.load(Ordering::SeqCst)
        {
            return;
        }

        let mut event = crate::plugin::api::events::entity::item_merge::ItemMergeEvent {
            entity_id: target.entity.entity_id,
            target_id: source.entity.entity_id,
            cancelled: false,
        };
        let server = self.entity.world.load().server.upgrade();
        if let Some(server) = server {
            server.plugin_manager.fire_blocking(&server, &mut event);
        }
        if event.cancelled
            || self.entity.removed.load(Ordering::SeqCst)
            || other.entity.removed.load(Ordering::SeqCst)
        {
            return;
        }

        let low_stack = low
            .item_stack
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let high_stack = high
            .item_stack
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if low_stack.uid != expected_low.uid
            || !low_stack.are_equal(&expected_low)
            || high_stack.uid != expected_high.uid
            || !high_stack.are_equal(&expected_high)
            || self.entity.removed.load(Ordering::SeqCst)
            || other.entity.removed.load(Ordering::SeqCst)
        {
            return;
        }

        let (self_stack, other_stack) = if self.entity.entity_id < other.entity.entity_id {
            (low_stack, high_stack)
        } else {
            (high_stack, low_stack)
        };
        if !Self::are_mergeable(&self_stack, &other_stack)
            || (other_stack.item_count < self_stack.item_count) != target_is_self
        {
            return;
        }
        let (mut stack1, mut stack2) = if target_is_self {
            (self_stack, other_stack)
        } else {
            (other_stack, self_stack)
        };

        // Vanilla `ItemEntity.merge(to, from, 64)`.
        let max_size = stack1.get_max_stack_size().min(MERGE_MAX_COUNT);
        let moved = stack2
            .item_count
            .min(max_size.saturating_sub(stack1.item_count));
        stack1.increment(moved);
        stack2.decrement(moved);
        let source_empty = stack2.is_empty();
        drop(stack1);
        drop(stack2);

        // `is_mergeable` already excluded never-despawn and never-pickup items.
        target.pickup_delay.fetch_max(
            source.pickup_delay.load(Ordering::Relaxed),
            Ordering::Relaxed,
        );
        target
            .item_age
            .fetch_min(source.item_age.load(Ordering::Relaxed), Ordering::Relaxed);

        target.on_count_changed();
        if source_empty {
            source.entity.remove();
        } else {
            source.on_count_changed();
        }
    }
}
