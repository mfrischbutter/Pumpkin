use std::sync::Arc;

use crate::entity::player::Player;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::{Block, BlockDirection};
use pumpkin_macros::{Event, cancellable};
use pumpkin_util::Hand;
use pumpkin_util::math::{position::BlockPos, vector3::Vector3};

use super::PlayerEvent;

/// Event that is triggered when a player interacts with a block or air.
///
/// This event includes information about the player, the action performed,
/// the item in the player's hand, the block interacted with, and the position clicked (if any).
/// It can be cancelled to prevent the default interaction behavior.
#[cancellable]
#[derive(Event, Clone)]
pub struct PlayerInteractEvent {
    /// The player who performed the interaction.
    pub player: Arc<Player>,

    /// The type of action the player performed.
    pub action: InteractAction,

    /// The position of the block that was clicked, if any.
    pub clicked_pos: Option<BlockPos>,

    /// The block that was interacted with.
    pub block: &'static Block,

    /// The hand the player interacted with, if known.
    pub hand: Option<Hand>,

    /// A copy of the stack in that hand, or `None` if the hand is empty or unknown.
    pub item: Option<ItemStack>,

    /// The clicked block face, if the client sent one.
    pub face: Option<BlockDirection>,

    /// The clicked point relative to the block's minimum corner, if the client sent one.
    pub cursor: Option<Vector3<f64>>,
}

impl PlayerInteractEvent {
    /// Creates a new instance of `PlayerInteractEvent`.
    ///
    /// # Arguments
    ///
    /// - `player`: A reference-counted pointer to the player who triggered the event.
    /// - `action`: The type of interaction performed.
    /// - `block`: The block that was interacted with.
    /// - `clicked_pos`: The optional position of the block that was clicked.
    ///
    /// # Returns
    ///
    /// A new `PlayerInteractEvent` instance with the specified data.
    pub fn new(
        player: &Arc<Player>,
        action: InteractAction,
        block: &'static Block,
        clicked_pos: Option<BlockPos>,
    ) -> Self {
        Self {
            player: Arc::clone(player),
            action,
            block,
            clicked_pos,
            hand: None,
            item: None,
            face: None,
            cursor: None,
            cancelled: false,
        }
    }

    /// Sets the hand the player interacted with and the stack it holds.
    #[must_use]
    pub fn with_hand(mut self, hand: Hand, item: &ItemStack) -> Self {
        self.hand = Some(hand);
        self.item = (!item.is_empty()).then(|| item.clone());
        self
    }

    /// Sets the clicked block face and, if the client sent it, the clicked point on it.
    #[must_use]
    pub const fn with_face(mut self, face: BlockDirection, cursor: Option<Vector3<f64>>) -> Self {
        self.face = Some(face);
        self.cursor = cursor;
        self
    }
}

/// Enum representing possible player interaction actions.
#[derive(Clone, PartialEq, Eq)]
pub enum InteractAction {
    /// Left-clicking a block
    LeftClickBlock,

    /// Left-clicking the air
    LeftClickAir,

    /// Right-clicking the air
    RightClickAir,

    /// Right-clicking a block
    RightClickBlock,
}

impl InteractAction {
    /// Gets whether this action is a result of a left click.
    #[must_use]
    #[inline]
    pub fn is_left_click(&self) -> bool {
        Self::LeftClickAir.eq(self) || Self::LeftClickBlock.eq(self)
    }

    /// Gets whether this action is a result of a right click.
    #[must_use]
    #[inline]
    pub fn is_right_click(&self) -> bool {
        Self::RightClickAir.eq(self) || Self::RightClickBlock.eq(self)
    }
}

impl PlayerEvent for PlayerInteractEvent {
    fn get_player(&self) -> &Arc<Player> {
        &self.player
    }
}
