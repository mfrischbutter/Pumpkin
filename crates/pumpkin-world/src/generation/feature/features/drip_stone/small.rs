use pumpkin_data::BlockDirection;
use pumpkin_util::{
    math::position::BlockPos,
    random::{RandomGenerator, RandomImpl},
};

use super::{grow_speleothem, is_base, is_empty_or_water, place_base_block_if_possible};
use crate::{block::BlockStateCodec, generation::proto_chunk::GenerationCache};

/// Vanilla's `SpeleothemFeature`: a single pointed dripstone or sulfur spike of one or two
/// blocks on a small patch of its base block.
pub struct SpeleothemFeature {
    pub base_block: BlockStateCodec,
    pub pointed_block: BlockStateCodec,
    pub replaceable_blocks: &'static [u16],
    pub chance_of_taller_generation: f32,
    pub chance_of_directional_spread: f32,
    pub chance_of_spread_radius2: f32,
    pub chance_of_spread_radius3: f32,
}

impl SpeleothemFeature {
    pub fn generate<T: GenerationCache>(
        &self,
        chunk: &mut T,
        random: &mut RandomGenerator,
        origin: BlockPos,
    ) -> bool {
        let Some(tip_direction) = self.get_tip_direction(chunk, origin, random) else {
            return false;
        };
        let root_pos = origin.offset(tip_direction.opposite().to_offset());
        self.create_patch_of_base_blocks(chunk, random, root_pos);
        let beyond_tip = origin.offset(tip_direction.to_offset());
        let height = if random.next_f32() < self.chance_of_taller_generation
            && is_empty_or_water(GenerationCache::get_block_state(chunk, &beyond_tip.0))
        {
            2
        } else {
            1
        };
        grow_speleothem(
            chunk,
            origin,
            tip_direction,
            height,
            false,
            self.base_block.get_block(),
            self.pointed_block.get_block(),
            self.replaceable_blocks,
        );
        true
    }

    fn get_tip_direction<T: GenerationCache>(
        &self,
        chunk: &T,
        pos: BlockPos,
        random: &mut RandomGenerator,
    ) -> Option<BlockDirection> {
        let base_block = self.base_block.get_block();
        let can_place_above = is_base(
            GenerationCache::get_block_state(chunk, &pos.up().0),
            base_block,
            self.replaceable_blocks,
        );
        let can_place_below = is_base(
            GenerationCache::get_block_state(chunk, &pos.down().0),
            base_block,
            self.replaceable_blocks,
        );
        if can_place_above && can_place_below {
            Some(if random.next_bool() {
                BlockDirection::Down
            } else {
                BlockDirection::Up
            })
        } else if can_place_above {
            Some(BlockDirection::Down)
        } else if can_place_below {
            Some(BlockDirection::Up)
        } else {
            None
        }
    }

    fn create_patch_of_base_blocks<T: GenerationCache>(
        &self,
        chunk: &mut T,
        random: &mut RandomGenerator,
        pos: BlockPos,
    ) {
        let base_block = self.base_block.get_block();
        place_base_block_if_possible(chunk, pos, base_block, self.replaceable_blocks);
        for direction in BlockDirection::horizontal_worldgen() {
            if random.next_f32() > self.chance_of_directional_spread {
                continue;
            }
            let pos1 = pos.offset(direction.to_offset());
            place_base_block_if_possible(chunk, pos1, base_block, self.replaceable_blocks);
            if random.next_f32() > self.chance_of_spread_radius2 {
                continue;
            }
            let pos2 = pos1.offset(BlockDirection::random(random).to_offset());
            place_base_block_if_possible(chunk, pos2, base_block, self.replaceable_blocks);
            if random.next_f32() > self.chance_of_spread_radius3 {
                continue;
            }
            let pos3 = pos2.offset(BlockDirection::random(random).to_offset());
            place_base_block_if_possible(chunk, pos3, base_block, self.replaceable_blocks);
        }
    }
}
