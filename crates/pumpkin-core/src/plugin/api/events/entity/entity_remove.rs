use crate::entity::EntityBase;
use pumpkin_macros::{Event, cancellable};
use std::sync::Arc;

/// An event that occurs when an entity is removed from the world.
///
/// The removal goes ahead regardless, so cancelling it has no effect.
#[cancellable]
#[derive(Event, Clone)]
pub struct EntityRemoveEvent {
    /// Entity ID.
    pub entity_id: i32,
    /// Removal cause.
    pub cause: String,
    /// The removed entity.
    pub entity: Arc<dyn EntityBase>,
}

impl EntityRemoveEvent {
    #[must_use]
    pub fn new(entity: Arc<dyn EntityBase>, cause: String) -> Self {
        Self {
            entity_id: entity.get_entity().entity_id,
            cause,
            entity,
            cancelled: false,
        }
    }
}
