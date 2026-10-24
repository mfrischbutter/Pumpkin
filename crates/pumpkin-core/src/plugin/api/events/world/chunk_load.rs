use crate::world::World;
use pumpkin_macros::Event;
use pumpkin_util::math::vector2::Vector2;
use std::sync::Arc;

/// An event that occurs when a chunk is loaded in a world.
///
/// It fires on the world tick after the chunk system made the chunk available, both for chunks
/// read from disk and for newly generated ones.
#[derive(Event, Clone)]
pub struct ChunkLoad {
    /// The world in which the chunk was loaded.
    pub world: Arc<World>,

    /// Chunk coordinates.
    pub chunk_pos: Vector2<i32>,

    /// Whether the chunk was generated instead of read from disk.
    pub is_new: bool,
}

impl ChunkLoad {
    #[must_use]
    pub const fn new(world: Arc<World>, chunk_pos: Vector2<i32>, is_new: bool) -> Self {
        Self {
            world,
            chunk_pos,
            is_new,
        }
    }
}
