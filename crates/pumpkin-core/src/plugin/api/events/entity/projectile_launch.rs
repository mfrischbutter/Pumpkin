use crate::world::World;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_macros::{Event, cancellable};
use std::sync::Arc;

/// An event that occurs when a projectile is launched.
///
/// Cancelling it keeps the projectile from being spawned.
#[cancellable]
#[derive(Event, Clone)]
pub struct ProjectileLaunchEvent {
    /// The ID of the projectile entity.
    pub entity_id: i32,
    /// The ID of the shooter entity if applicable.
    pub shooter_id: Option<i32>,
    /// The item stack the projectile carries, if it has one.
    pub item: Option<ItemStack>,
    /// The world the projectile is launched in.
    pub world: Arc<World>,
}

impl ProjectileLaunchEvent {
    #[must_use]
    pub const fn new(
        entity_id: i32,
        shooter_id: Option<i32>,
        item: Option<ItemStack>,
        world: Arc<World>,
    ) -> Self {
        Self {
            entity_id,
            shooter_id,
            item,
            world,
            cancelled: false,
        }
    }
}
