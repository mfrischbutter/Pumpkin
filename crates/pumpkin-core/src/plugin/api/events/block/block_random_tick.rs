use std::sync::Arc;

use pumpkin_data::BlockStateId;
use pumpkin_macros::Event;
use pumpkin_util::math::position::BlockPos;

use crate::world::World;

/// One random tick in a [`BlockRandomTickEvent`].
#[derive(Debug, Clone, Copy)]
pub struct BlockRandomTick {
    /// The position of the ticked block.
    pub block_pos: BlockPos,

    /// The block state id at the position when the random ticks were sampled.
    pub state_id: BlockStateId,

    /// Whether to skip vanilla's random tick of this block. States vanilla does not random tick
    /// only tick for plugins, so this changes nothing for them.
    pub cancelled: bool,
}

/// An event that occurs once per world tick, before random ticks run, with the random ticks of
/// the states plugins watch.
///
/// Fired only for states registered with `Context::watch_random_tick_states`, and only when at
/// least one of them was picked this tick.
#[derive(Event, Clone)]
pub struct BlockRandomTickEvent {
    /// The world the blocks are in.
    pub world: Arc<World>,

    /// The random ticks of watched states picked this tick.
    pub ticks: Vec<BlockRandomTick>,
}

impl BlockRandomTickEvent {
    /// Creates a new `BlockRandomTickEvent`.
    #[must_use]
    pub const fn new(world: Arc<World>, ticks: Vec<BlockRandomTick>) -> Self {
        Self { world, ticks }
    }
}
