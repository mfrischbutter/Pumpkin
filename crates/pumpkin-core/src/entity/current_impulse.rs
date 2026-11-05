//! Vanilla `LivingEntity.currentImpulseImpactPos` and `currentImpulseContextResetGraceTime`:
//! a launch by a wind charge costs no fall damage down to where it started.

use std::sync::atomic::{AtomicI32, Ordering::Relaxed};

use crossbeam::atomic::AtomicCell;

/// Vanilla `setIgnoreFallDamageFromCurrentImpulse`: ticks the launch stays remembered in the air.
const POST_IMPULSE_GRACE_TICKS: i32 = 40;

/// The launch a living entity is still falling from. Only the height of vanilla's impact
/// position matters to the fall.
#[derive(Default)]
pub struct CurrentImpulse {
    impact_y: AtomicCell<Option<f64>>,
    grace_ticks: AtomicI32,
}

impl CurrentImpulse {
    /// Vanilla `setIgnoreFallDamageFromCurrentImpulse`.
    pub fn set_ignore_fall_damage(&self, ignore: bool, impact_y: f64) {
        if ignore {
            self.grace_ticks
                .fetch_max(POST_IMPULSE_GRACE_TICKS, Relaxed);
            self.impact_y.store(Some(impact_y));
        } else {
            self.grace_ticks.store(0, Relaxed);
        }
    }

    /// The start of vanilla `LivingEntity.causeFallDamage`: a fall counts only below the launch.
    pub fn effective_fall_distance(&self, fall_distance: f32, y: f64) -> f32 {
        let Some(impact_y) = self.impact_y.load() else {
            return fall_distance;
        };
        let effective = f64::from(fall_distance).min(impact_y - y);
        if effective <= 0.0 {
            self.reset();
        } else {
            self.try_reset();
        }
        effective as f32
    }

    /// Vanilla `tryResetCurrentImpulseContext`.
    pub fn try_reset(&self) {
        if self.grace_ticks.load(Relaxed) == 0 {
            self.reset();
        }
    }

    /// Vanilla `resetCurrentImpulseContext`.
    pub fn reset(&self) {
        self.grace_ticks.store(0, Relaxed);
        self.impact_y.store(None);
    }

    /// Vanilla `LivingEntity.tick`: the grace time runs out.
    pub fn tick(&self) {
        let ticks = self.grace_ticks.load(Relaxed);
        if ticks > 0 {
            self.grace_ticks.store(ticks - 1, Relaxed);
        }
    }
}
