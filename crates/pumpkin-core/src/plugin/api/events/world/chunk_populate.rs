use crate::world::World;
use pumpkin_macros::Event;
use pumpkin_util::math::vector2::Vector2;
use std::sync::Arc;

/// An event that occurs once for every newly generated chunk, after its features were placed.
///
/// It fires right after the chunk's load event, while the chunk is loaded, so handlers can
/// change its blocks through the world.
#[derive(Event, Clone)]
pub struct ChunkPopulateEvent {
    /// The world the chunk was generated in.
    pub world: Arc<World>,

    /// Chunk coordinates.
    pub chunk_pos: Vector2<i32>,
}

impl ChunkPopulateEvent {
    #[must_use]
    pub const fn new(world: Arc<World>, chunk_pos: Vector2<i32>) -> Self {
        Self { world, chunk_pos }
    }
}
