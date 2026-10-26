use crate::entity::player::Player;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_macros::{Event, cancellable};
use pumpkin_util::{Hand, math::position::BlockPos};
use std::sync::Arc;

/// An event that occurs when a player empties a bucket.
#[cancellable]
#[derive(Event, Clone)]
pub struct PlayerBucketEmptyEvent {
    /// The player emptying the bucket.
    pub player: Arc<Player>,

    /// The target block position where the bucket is emptied.
    pub block_pos: BlockPos,

    /// The bucket item used.
    pub bucket: String,

    /// The hand holding the bucket.
    pub hand: Hand,

    /// The bucket stack used.
    pub item: ItemStack,

    /// The stack that replaces the bucket in `hand`, or `None` to leave the hand as it is.
    pub result: Option<ItemStack>,
}

impl PlayerBucketEmptyEvent {
    #[must_use]
    pub const fn new(
        player: Arc<Player>,
        block_pos: BlockPos,
        bucket: String,
        hand: Hand,
        item: ItemStack,
        result: Option<ItemStack>,
    ) -> Self {
        Self {
            player,
            block_pos,
            bucket,
            hand,
            item,
            result,
            cancelled: false,
        }
    }
}

/// An event that occurs when a player fills a bucket.
#[cancellable]
#[derive(Event, Clone)]
pub struct PlayerBucketFillEvent {
    /// The player filling the bucket.
    pub player: Arc<Player>,

    /// The target block position being targeted for filling.
    pub block_pos: BlockPos,

    /// The bucket item used.
    pub bucket: String,

    /// The hand holding the bucket.
    pub hand: Hand,

    /// The bucket stack used.
    pub item: ItemStack,

    /// The stack the player gets for one bucket of `item`, or `None` to leave the hand as it is.
    pub result: Option<ItemStack>,
}

impl PlayerBucketFillEvent {
    #[must_use]
    pub const fn new(
        player: Arc<Player>,
        block_pos: BlockPos,
        bucket: String,
        hand: Hand,
        item: ItemStack,
        result: Option<ItemStack>,
    ) -> Self {
        Self {
            player,
            block_pos,
            bucket,
            hand,
            item,
            result,
            cancelled: false,
        }
    }
}
