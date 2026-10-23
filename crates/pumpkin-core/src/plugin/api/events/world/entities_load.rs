use crate::entity::EntityBase;
use crate::world::World;
use pumpkin_macros::{Event, cancellable};
use pumpkin_util::math::vector2::Vector2;
use std::sync::Arc;

/// An event that occurs when the entities of a chunk have been loaded into the world.
///
/// The entities are already live when it fires, so cancelling it has no effect.
#[cancellable]
#[derive(Event, Clone)]
pub struct EntitiesLoadEvent {
    /// Chunk coordinates.
    pub chunk_pos: Vector2<i32>,
    /// Entity count.
    pub entity_count: usize,
    /// The world the chunk belongs to.
    pub world: Arc<World>,
    /// The entities that were loaded.
    pub entities: Vec<Arc<dyn EntityBase>>,
}

impl EntitiesLoadEvent {
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
