use std::sync::Arc;

use pumpkin_macros::{Event, cancellable};
use pumpkin_util::math::position::BlockPos;

use crate::world::World;

/// An event that occurs when TNT is primed.
#[cancellable]
#[derive(Event, Clone)]
pub struct TNTPrimeEvent {
    /// The world the TNT is primed in.
    pub world: Arc<World>,
    pub block_pos: BlockPos,
    pub prime_reason: String,
}

impl TNTPrimeEvent {
    #[must_use]
    pub const fn new(world: Arc<World>, block_pos: BlockPos, prime_reason: String) -> Self {
        Self {
            world,
            block_pos,
            prime_reason,
            cancelled: false,
        }
    }
}
