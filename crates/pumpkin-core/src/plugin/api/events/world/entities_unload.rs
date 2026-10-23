use crate::entity::EntityBase;
use crate::world::World;
use pumpkin_macros::{Event, cancellable};
use pumpkin_util::math::vector2::Vector2;
use std::sync::Arc;

/// An event that occurs when the entities of a chunk are unloaded, before they are saved.
///
/// The unload goes ahead regardless, so cancelling it has no effect.
#[cancellable]
#[derive(Event, Clone)]
pub struct EntitiesUnloadEvent {
    /// Chunk coordinates.
    pub chunk_pos: Vector2<i32>,
    /// Entity count.
    pub entity_count: usize,
    /// The world the chunk belongs to.
    pub world: Arc<World>,
    /// The entities that are unloaded.
    pub entities: Vec<Arc<dyn EntityBase>>,
}

impl EntitiesUnloadEvent {
    #[must_use]
    pub const fn new(
        chunk_pos: Vector2<i32>,
        world: Arc<World>,
        entities: Vec<Arc<dyn EntityBase>>,
    ) -> Self {
        Self {
            chunk_pos,
            entity_count: entities.len(),
            world,
            entities,
            cancelled: false,
        }
    }
}
