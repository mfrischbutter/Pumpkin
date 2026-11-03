//! Vanilla `DragonSittingAttackingPhase`: roars for two seconds, then breathes.

use std::sync::atomic::{AtomicI32, Ordering};

use super::{EnderDragonPhase, Phase};
use crate::entity::boss::ender_dragon::EnderDragonEntity;

const ROAR_DURATION: i32 = 40;

#[derive(Default)]
pub struct SittingAttackingPhase {
    attacking_ticks: AtomicI32,
}

impl Phase for SittingAttackingPhase {
    fn tick(&self, dragon: &EnderDragonEntity) {
        if self.attacking_ticks.fetch_add(1, Ordering::Relaxed) >= ROAR_DURATION {
            dragon.set_phase(EnderDragonPhase::SittingFlaming);
        }
    }

    fn begin(&self) {
        self.attacking_ticks.store(0, Ordering::Relaxed);
    }
}
