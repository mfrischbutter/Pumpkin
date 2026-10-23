use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::{Block, BlockStateId};
use pumpkin_macros::{Event, cancellable};
use pumpkin_util::Hand;
use pumpkin_util::math::position::BlockPos;
use std::sync::Arc;

use crate::entity::player::Player;
use crate::world::World;

use super::BlockEvent;

/// An event that occurs when a block is placed.
///
/// This event contains information about the world, the player placing the block, the block being placed,
/// the block being placed against, and whether the player can build.
#[cancellable]
#[derive(Event, Clone)]
pub struct BlockPlaceEvent {
    /// The world the block is placed in.
    pub world: Arc<World>,

    /// The player placing the block.
    pub player: Arc<Player>,

    /// The block that is being placed.
    pub block_placed: &'static Block,

    /// The block that the new block is being placed against.
    pub block_placed_against: &'static Block,

    /// The position where the block is being placed.
    pub block_position: BlockPos,

    /// A boolean indicating whether the player can build.
    pub can_build: bool,

    /// The state the block is placed with.
    pub placed_state: BlockStateId,

    /// The hand the block is placed from, if known.
    pub hand: Option<Hand>,

    /// A copy of the stack the block is placed from, before it is used up.
    pub item: Option<ItemStack>,
}

impl BlockPlaceEvent {
    #[must_use]
    pub const fn new(
        world: Arc<World>,
        player: Arc<Player>,
        block_placed: &'static Block,
        block_placed_against: &'static Block,
        block_position: BlockPos,
        can_build: bool,
        placed_state: BlockStateId,
    ) -> Self {
        Self {
            world,
            player,
            block_placed,
            block_placed_against,
            block_position,
            can_build,
            placed_state,
            hand: None,
            item: None,
            cancelled: false,
        }
    }

    /// Sets the hand the block is placed from and the stack it holds.
    #[must_use]
    pub fn with_hand(mut self, hand: Hand, item: &ItemStack) -> Self {
        self.hand = Some(hand);
        self.item = (!item.is_empty()).then(|| item.clone());
        self
    }
}

impl BlockEvent for BlockPlaceEvent {
    fn get_block(&self) -> &Block {
        self.block_placed
    }
}
