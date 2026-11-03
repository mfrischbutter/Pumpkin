//! Vanilla `Minecart`.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use crate::entity::{Entity, EntityBase, player::Player};

use super::MinecartEntity;

/// Vanilla `Minecart.activateMinecart`'s damage value, purely a wobble.
const ACTIVATOR_WOBBLE_DAMAGE: f32 = 50.0;
const WOBBLE_TIME: i32 = 10;

pub(super) struct RideableMinecart;

impl RideableMinecart {
    /// Vanilla `Minecart.interact`. Once a boarding is tried the interaction counts as handled,
    /// even when the player could not board.
    pub(super) fn interact(minecart: &MinecartEntity, player: &Arc<Player>) -> bool {
        let entity: &Entity = &minecart.vehicle.entity;
        if player.get_entity().is_sneaking() || entity.has_passengers() {
            return false;
        }
        let passenger: Arc<dyn EntityBase> = player.clone();
        minecart.start_riding(&passenger);
        true
    }

    /// Vanilla `Minecart.activateMinecart`.
    pub(super) fn activate(minecart: &MinecartEntity, powered: bool) {
        if !powered {
            return;
        }
        minecart.eject_passengers();
        let vehicle = &minecart.vehicle;
        if vehicle.get_hurt_time() == 0 {
            vehicle.set_hurt_dir(-vehicle.get_hurt_dir());
            vehicle.set_hurt_time(WOBBLE_TIME);
            vehicle.set_damage(ACTIVATOR_WOBBLE_DAMAGE);
            vehicle.entity.velocity_dirty.store(true, Ordering::SeqCst);
            vehicle.send_wobble_metadata();
        }
    }
}
