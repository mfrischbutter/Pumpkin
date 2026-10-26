use crate::world::World;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_macros::{Event, cancellable};
use pumpkin_util::math::vector3::Vector3;
use std::sync::Arc;

/// An event that occurs when a projectile hits an entity or block.
///
/// Cancelling it skips the hit: the projectile keeps flying as if nothing was in its way.
#[cancellable]
#[derive(Event, Clone)]
pub struct ProjectileHitEvent {
    /// The ID of the projectile entity.
    pub entity_id: i32,
    /// Hit position.
    pub hit_position: Vector3<f64>,
    /// ID of hit entity if applicable.
    pub hit_entity_id: Option<i32>,
    /// The item stack the projectile carries, if it has one.
    pub item: Option<ItemStack>,
    /// The world the projectile is in.
    pub world: Arc<World>,
}

impl ProjectileHitEvent {
    #[must_use]
    pub const fn new(
        entity_id: i32,
        hit_position: Vector3<f64>,
        hit_entity_id: Option<i32>,
        item: Option<ItemStack>,
        world: Arc<World>,
    ) -> Self {
        Self {
            entity_id,
            hit_position,
            hit_entity_id,
            item,
            world,
            cancelled: false,
        }
    }
}
