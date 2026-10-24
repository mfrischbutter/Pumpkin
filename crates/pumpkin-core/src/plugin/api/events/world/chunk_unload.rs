use crate::world::World;
use pumpkin_macros::Event;
use pumpkin_util::math::vector2::Vector2;
use std::sync::Arc;

/// An event that occurs when a chunk is unloaded.
///
/// It fires on the world tick after the chunk system dropped the chunk, so the chunk is no longer
/// loaded when handlers run.
#[derive(Event, Clone)]
pub struct ChunkUnloadEvent {
    /// The world the chunk was unloaded from.
    pub world: Arc<World>,

    /// Chunk coordinates.
    pub chunk_pos: Vector2<i32>,
}

impl ChunkUnloadEvent {
    #[must_use]
    pub const fn new(world: Arc<World>, chunk_pos: Vector2<i32>) -> Self {
        Self { world, chunk_pos }
    }
}
