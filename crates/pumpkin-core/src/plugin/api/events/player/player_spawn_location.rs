use super::PlayerEvent;
use crate::entity::player::Player;
use crate::world::World;
use pumpkin_macros::{Event, cancellable};
use pumpkin_util::math::vector3::Vector3;
use std::sync::Arc;

/// An event that occurs when determining player spawn position: when the
/// player joins and when they respawn.
#[cancellable]
#[derive(Event, Clone)]
pub struct PlayerSpawnLocationEvent {
    /// The player.
    pub player: Arc<Player>,
    /// Spawn position.
    pub spawn_pos: Vector3<f64>,
    /// The world the player spawns in.
    pub spawn_world: Arc<World>,
    /// The yaw the player spawns with.
    pub spawn_yaw: f32,
    /// The pitch the player spawns with.
    pub spawn_pitch: f32,
}

impl PlayerSpawnLocationEvent {
    #[must_use]
    pub const fn new(
        player: Arc<Player>,
        spawn_pos: Vector3<f64>,
        spawn_world: Arc<World>,
        (spawn_yaw, spawn_pitch): (f32, f32),
    ) -> Self {
        Self {
            player,
            spawn_pos,
            spawn_world,
            spawn_yaw,
            spawn_pitch,
            cancelled: false,
        }
    }
}

impl PlayerEvent for PlayerSpawnLocationEvent {
    fn get_player(&self) -> &Arc<Player> {
        &self.player
    }
}
