use pumpkin_macros::{Event, cancellable};
use pumpkin_util::text::TextComponent;
use std::sync::Arc;

use crate::entity::player::Player;

use super::PlayerEvent;

/// An event that occurs when a player leaves the game.
///
/// This event contains information about the player leaving and a message to display upon leaving.
#[cancellable]
#[derive(Event, Clone)]
pub struct PlayerLeaveEvent {
    /// The player who is leaving the game.
    pub player: Arc<Player>,

    /// The message to display when the player leaves.
    pub leave_message: TextComponent,

    /// Why the player left.
    pub reason: PlayerLeaveReason,
}

/// Why a player left the server.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PlayerLeaveReason {
    /// The connection closed for any reason not listed here, usually because the client quit.
    #[default]
    Disconnected,
    /// The server kicked the player.
    Kicked,
    /// The client stopped answering keep-alives.
    TimedOut,
    /// The server is shutting down.
    ServerStopping,
}

impl PlayerLeaveEvent {
    /// Creates a new instance of `PlayerLeaveEvent`.
    ///
    /// # Arguments
    /// - `player`: A reference to the player leaving the game.
    /// - `leave_message`: The message to display upon leaving.
    /// - `reason`: Why the player left.
    ///
    /// # Returns
    /// A new instance of `PlayerLeaveEvent`.
    pub const fn new(
        player: Arc<Player>,
        leave_message: TextComponent,
        reason: PlayerLeaveReason,
    ) -> Self {
        Self {
            player,
            leave_message,
            reason,
            cancelled: false,
        }
    }
}

impl PlayerEvent for PlayerLeaveEvent {
    fn get_player(&self) -> &Arc<Player> {
        &self.player
    }
}
