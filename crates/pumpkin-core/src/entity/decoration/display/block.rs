use std::sync::{
    Arc,
    atomic::{AtomicI32, Ordering},
};

use pumpkin_data::{Block, BlockStateId, tracked_data::block_display as tracked};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_protocol::codec::var_int::VarInt;

use super::DisplayEntity;
use crate::{
    entity::{
        Entity, EntityBase,
        living::LivingEntity,
        mob::enderman::{block_state_from_nbt, block_state_to_nbt},
    },
    server::Server,
};

/// Vanilla `Display.BlockDisplay`.
pub struct BlockDisplayEntity {
    pub display: DisplayEntity,
    pub block_state: AtomicI32,
}

impl BlockDisplayEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let air = i32::from(Block::AIR.default_state.id.as_u16());
        entity.synched_data.define(tracked::BLOCK_STATE, VarInt(air));
        Arc::new(Self {
            display: DisplayEntity::new(entity),
            block_state: AtomicI32::new(air),
        })
    }

    #[must_use]
    pub fn get_block_state(&self) -> i32 {
        self.block_state.load(Ordering::Relaxed)
    }

    pub fn set_block_state(&self, block_state: i32) {
        self.block_state.store(block_state, Ordering::Relaxed);
        self.display
            .entity
            .set_synced_data(tracked::BLOCK_STATE, VarInt(block_state));
    }
}

impl EntityBase for BlockDisplayEntity {
    /// Vanilla `BlockDisplay.addAdditionalSaveData`: `BlockState.CODEC`, not a raw id.
    fn write_custom_nbt(&self, nbt: &mut NbtCompound) {
        self.display.write_display_nbt(nbt);
        let state = u16::try_from(self.get_block_state())
            .ok()
            .and_then(BlockStateId::new)
            .unwrap_or(Block::AIR.default_state.id);
        nbt.put("block_state", block_state_to_nbt(state));
    }

    /// Vanilla `BlockDisplay.readAdditionalSaveData`, and the raw id Pumpkin saved before.
    fn read_custom_nbt(&self, nbt: &NbtCompound) {
        self.display.read_display_nbt(nbt);
        let state = nbt
            .get("block_state")
            .and_then(block_state_from_nbt)
            .unwrap_or(Block::AIR.default_state.id);
        self.set_block_state(i32::from(state.as_u16()));
    }

    fn tick(&self, _caller: &dyn EntityBase, _server: &Server) {
        self.display.tick_display();
    }

    /// Every value already reached the synced data through its setter.
    fn init_data_tracker(&self) {}

    fn get_entity(&self) -> &Entity {
        &self.display.entity
    }

    fn get_living_entity(&self) -> Option<&LivingEntity> {
        None
    }

    fn cast_any(&self) -> &dyn std::any::Any {
        self
    }

    fn is_pushed_by_fluids(&self) -> bool {
        false
    }

    fn is_immune_to_explosion(&self) -> bool {
        true
    }
}
