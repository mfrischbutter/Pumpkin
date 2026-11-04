use pumpkin_data::packet::clientbound::play::ADD_TRANSIENT_BLOCK;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::version::JavaMinecraftVersion;

use pumpkin_macros::java_packet;
use std::io::Write;

use crate::{
    ClientPacket, VarInt,
    ser::{NetworkWriteExt, WritingError},
};

/// Vanilla `ClientboundAddTransientBlockPacket`: a falling block just landed at this location,
/// so the client keeps drawing it there until its own block update has taken effect.
#[java_packet(ADD_TRANSIENT_BLOCK)]
pub struct CAddTransientBlock {
    /// The world coordinates of the landed block.
    pub location: BlockPos,
    /// The block state ID.
    pub state_id: VarInt,
}

impl CAddTransientBlock {
    #[must_use]
    pub const fn new(location: BlockPos, state_id: VarInt) -> Self {
        Self { location, state_id }
    }
}

impl ClientPacket for CAddTransientBlock {
    fn write_packet_data(
        &self,
        write: impl Write,
        version: &JavaMinecraftVersion,
    ) -> Result<(), WritingError> {
        let mut write = write;
        write.write_block_pos(&self.location, version)?;
        write.write_var_int(&self.state_id)?;

        Ok(())
    }
}
