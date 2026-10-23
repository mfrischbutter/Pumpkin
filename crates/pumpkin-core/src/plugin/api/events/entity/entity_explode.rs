use std::sync::Arc;

use pumpkin_macros::{Event, cancellable};
use pumpkin_util::math::{position::BlockPos, vector3::Vector3};

use crate::world::World;

/// An event that occurs when an entity explodes.
///
/// It fires once per explosion, after the affected blocks are known. Cancelling it cancels the
/// whole explosion.
#[cancellable]
#[derive(Event, Clone)]
pub struct EntityExplodeEvent {
    /// The ID of the exploding entity.
    pub entity_id: i32,

    /// The position where the explosion occurred.
    pub position: Vector3<f64>,

    /// The share of destroyed blocks that drop their loot, from 0.0 to 1.0.
    pub yield_rate: f32,

    /// The world the explosion happens in.
    pub world: Arc<World>,

    /// The blocks the explosion destroys. Only the blocks left in this list are destroyed.
    pub blocks: Vec<BlockPos>,
}

impl EntityExplodeEvent {
    #[must_use]
    pub const fn new(
        world: Arc<World>,
        entity_id: i32,
        position: Vector3<f64>,
        blocks: Vec<BlockPos>,
        yield_rate: f32,
    ) -> Self {
        Self {
            entity_id,
            position,
            yield_rate,
            world,
            blocks,
            cancelled: false,
        }
    }
}
