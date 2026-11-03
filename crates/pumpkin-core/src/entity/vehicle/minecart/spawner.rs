//! Vanilla `MinecartSpawner`.

use std::sync::{Mutex, PoisonError};

use pumpkin_nbt::compound::NbtCompound;
use pumpkin_protocol::java::client::play::CEntityStatus;
use pumpkin_util::math::position::BlockPos;

use crate::block::entities::BlockEntity;
use crate::block::entities::mob_spawner::MobSpawnerBlockEntity;
use crate::entity::Entity;

/// Vanilla's `BaseSpawner` field. It stays behind a lock because loading replaces its settings
/// through `&self`, and its position follows the cart.
pub(super) struct SpawnerMinecart {
    spawner: Mutex<MobSpawnerBlockEntity>,
}

impl SpawnerMinecart {
    pub(super) fn new() -> Self {
        Self {
            spawner: Mutex::new(MobSpawnerBlockEntity::new(BlockPos::new(0, 0, 0), None)),
        }
    }

    /// Vanilla `MinecartSpawner.tick`'s ticker: `spawner.serverTick(level, blockPosition())`.
    pub(super) fn tick(&self, entity: &Entity) {
        let world = entity.world.load_full();
        let mut spawner = self.spawner.lock().unwrap_or_else(PoisonError::into_inner);
        spawner.position = entity.block_pos.load();
        // Vanilla `BaseSpawner.broadcastEvent`: an entity event for a cart.
        spawner.server_tick(&world, &|id: u8| {
            world.send_to_tracking_players_and_self(
                entity,
                &CEntityStatus::new(entity.entity_id, id as i8),
            );
        });
    }

    pub(super) fn write_nbt(&self, nbt: &mut NbtCompound) {
        self.spawner
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .write_nbt(nbt);
    }

    pub(super) fn read_nbt(&self, nbt: &NbtCompound) {
        let mut spawner = self.spawner.lock().unwrap_or_else(PoisonError::into_inner);
        let position = spawner.position;
        *spawner = MobSpawnerBlockEntity::from_nbt(nbt, position);
    }
}
