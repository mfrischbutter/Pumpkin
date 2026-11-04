//! Riptide's spin: vanilla `Player.startAutoSpinAttack` and `LivingEntity.checkAutoSpinAttack`.
//!
//! The player's own movement is the client's: it pushes and bounces itself, the server only
//! finds what the spin hits and tells other players about the pose.

use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::{Arc, Mutex};

use crossbeam::atomic::AtomicCell;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_util::math::boundingbox::BoundingBox;
use pumpkin_util::math::vector3::Vector3;

use super::Player;
use crate::entity::EntityBase;

pub struct AutoSpinAttack {
    /// Vanilla `autoSpinAttackTicks`.
    ticks: AtomicI32,
    /// The flag vanilla `isAutoSpinAttack` reads.
    active: AtomicBool,
    /// Vanilla `autoSpinAttackDmg`.
    damage: AtomicCell<f32>,
    /// Vanilla `autoSpinAttackItemStack`.
    item: Mutex<Option<ItemStack>>,
    /// The box at the last check, vanilla's `beforeTravelBox`.
    last_box: AtomicCell<BoundingBox>,
}

impl Default for AutoSpinAttack {
    fn default() -> Self {
        let origin = Vector3::new(0.0, 0.0, 0.0);
        Self {
            ticks: AtomicI32::new(0),
            active: AtomicBool::new(false),
            damage: AtomicCell::new(0.0),
            item: Mutex::new(None),
            last_box: AtomicCell::new(BoundingBox::new(origin, origin)),
        }
    }
}

impl AutoSpinAttack {
    /// Vanilla `isAutoSpinAttack`.
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.active.load(Ordering::Relaxed)
    }

    /// The damage of vanilla `Player.attack` while spinning.
    #[must_use]
    pub fn damage(&self) -> Option<f32> {
        self.is_active().then(|| self.damage.load())
    }

    /// Vanilla `Player.getWeaponItem` while spinning.
    #[must_use]
    pub fn weapon(&self) -> Option<ItemStack> {
        self.item
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

/// Vanilla `AABB.minmax`.
const fn union(a: &BoundingBox, b: &BoundingBox) -> BoundingBox {
    BoundingBox::new(
        Vector3::new(
            a.min.x.min(b.min.x),
            a.min.y.min(b.min.y),
            a.min.z.min(b.min.z),
        ),
        Vector3::new(
            a.max.x.max(b.max.x),
            a.max.y.max(b.max.y),
            a.max.z.max(b.max.z),
        ),
    )
}

impl Player {
    /// Vanilla `Player.startAutoSpinAttack`.
    pub fn start_auto_spin_attack(&self, ticks: i32, damage: f32, item: ItemStack) {
        let spin = &self.auto_spin_attack;
        spin.ticks.store(ticks, Ordering::Relaxed);
        spin.active.store(true, Ordering::Relaxed);
        spin.damage.store(damage);
        *spin
            .item
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(item);
        spin.last_box
            .store(self.living_entity.entity.bounding_box.load());
        self.living_entity.set_riptide_flag(true);
    }

    /// The spin part of vanilla `LivingEntity.aiStep`.
    pub(super) fn tick_auto_spin_attack(&self) {
        let spin = &self.auto_spin_attack;
        let entity = &self.living_entity.entity;
        let current = entity.bounding_box.load();
        let old = spin.last_box.swap(current);
        let ticks = spin.ticks.load(Ordering::Relaxed);
        if ticks <= 0 {
            return;
        }
        spin.ticks.store(ticks - 1, Ordering::Relaxed);
        self.check_auto_spin_attack(&union(&old, &current));
        if spin.ticks.load(Ordering::Relaxed) <= 0 {
            self.stop_auto_spin_attack();
        }
    }

    /// Vanilla `checkAutoSpinAttack` with `Player.doAutoAttackOnTouch`.
    fn check_auto_spin_attack(&self, area: &BoundingBox) {
        let spin = &self.auto_spin_attack;
        let entity = &self.living_entity.entity;
        let targets: Vec<Arc<dyn EntityBase>> = entity
            .world
            .load()
            .get_all_at_box(area)
            .into_iter()
            .filter(|target| {
                target.get_entity().entity_id != entity.entity_id && !target.is_spectator()
            })
            .collect();
        if targets.is_empty() {
            if entity.horizontal_collision.load(Ordering::Relaxed) {
                spin.ticks.store(0, Ordering::Relaxed);
            }
            return;
        }
        if let Some(target) = targets
            .iter()
            .find(|target| target.get_living_entity().is_some())
        {
            self.attack(target);
            spin.ticks.store(0, Ordering::Relaxed);
        }
    }

    fn stop_auto_spin_attack(&self) {
        let spin = &self.auto_spin_attack;
        spin.active.store(false, Ordering::Relaxed);
        spin.damage.store(0.0);
        *spin
            .item
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
        self.living_entity.set_riptide_flag(false);
    }
}
