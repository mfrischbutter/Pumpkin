use std::sync::Arc;

use pumpkin_data::{Block, fluid::Fluid};
use pumpkin_util::math::{position::BlockPos, vector2::Vector2};
use pumpkin_world::world::BlockFlags;

use super::World;
use crate::block::blocks::bubble_column;

impl World {
    /// Vanilla's `LevelChunk.postProcessGeneration`: the blocks generation marked, once the chunk
    /// ticks. A fluid runs its tick, a liquid block its block tick (a bubble column above magma or
    /// soul sand), and any other block takes the shape its neighbours give it, which removes a
    /// mushroom that cannot stay and connects the fences and bars of structures.
    pub(super) fn post_process_generation(self: &Arc<Self>, chunk_pos: Vector2<i32>) {
        let Some((sections, bottom_section)) = self.level.read_chunk_sync(&chunk_pos, |chunk| {
            let sections = std::mem::take(
                &mut *chunk
                    .post_processing
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner),
            );
            (sections, chunk.section.min_y >> 4)
        }) else {
            return;
        };
        if sections.iter().all(Vec::is_empty) {
            return;
        }
        for (index, offsets) in sections.iter().enumerate() {
            let section_y = bottom_section + index as i32;
            for &packed in offsets {
                let position = BlockPos::new(
                    (chunk_pos.x << 4) + i32::from(packed & 15),
                    (section_y << 4) + i32::from((packed >> 4) & 15),
                    (chunk_pos.y << 4) + i32::from((packed >> 8) & 15),
                );
                self.post_process_block(&position);
            }
        }
    }

    fn post_process_block(self: &Arc<Self>, position: &BlockPos) {
        let (block, state_id) = self.get_block_and_state_id(position);
        let fluid = Self::get_fluid_from_state_id(state_id);
        if fluid.id != Fluid::EMPTY.id
            && let Some(fluid_behaviour) = self.block_registry.get_pumpkin_fluid(fluid.id)
        {
            fluid_behaviour.on_scheduled_tick(self, fluid, position);
        }
        if block == &Block::WATER || block == &Block::LAVA {
            // `LiquidBlock.tick`: only a full water source starts a bubble column.
            bubble_column::update_column(self, *position);
            return;
        }
        // `Block.updateFromNeighbourShapes` in vanilla's update order; set without neighbour or
        // shape updates (flags 276 in vanilla), but sent to clients that may have the chunk.
        let mut new_state = state_id;
        for direction in pumpkin_data::BlockDirection::abstract_block_update_order() {
            let neighbor = position.offset(direction.to_offset());
            let neighbor_state = self.get_block_state_id(&neighbor);
            new_state = self.block_registry.get_state_for_neighbor_update(
                self,
                Block::from_state_id(new_state),
                new_state,
                position,
                direction,
                &neighbor,
                neighbor_state,
            );
        }
        if new_state != state_id {
            self.set_block_state(
                position,
                new_state,
                BlockFlags::NOTIFY_LISTENERS
                    | BlockFlags::UPDATE_KNOWN_SHAPE
                    | BlockFlags::SKIP_DROPS
                    | BlockFlags::SKIP_BLOCK_ENTITY_REPLACED_CALLBACK,
            );
        }
    }
}
