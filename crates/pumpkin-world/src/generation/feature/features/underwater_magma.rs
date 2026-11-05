use crate::generation::proto_chunk::GenerationCache;
use pumpkin_data::{Block, BlockDirection, BlockState};
use pumpkin_util::{
    math::position::BlockPos,
    random::{RandomGenerator, RandomImpl},
};

pub struct UnderwaterMagmaFeature {
    /// vertical search limit from origin for a water column floor
    pub floor_search_range: i32,
    /// horizontal radius from the floor spot to sample
    pub placement_radius: i32,
    /// chance each inspected block turns into magma
    pub placement_probability: f32,
}

impl UnderwaterMagmaFeature {
    /// Find the Y coordinate of a solid floor beneath a water column at the origin's XZ.
    fn get_floor_y<T: GenerationCache>(&self, chunk: &T, origin: BlockPos) -> Option<i32> {
        let x = origin.0.x;
        let z = origin.0.z;
        let mut y = origin.0.y;

        if GenerationCache::get_block_state(chunk, &BlockPos::new(x, y, z).0).to_block_id()
            != Block::WATER
        {
            return None;
        }

        for _ in 1..self.floor_search_range {
            if GenerationCache::get_block_state(chunk, &BlockPos::new(x, y, z).0).to_block_id()
                != Block::WATER
            {
                break;
            }
            y -= 1;
        }

        let floor_id =
            GenerationCache::get_block_state(chunk, &BlockPos::new(x, y, z).0).to_block_id();
        (floor_id != Block::WATER).then_some(y)
    }

    /// Vanilla `isValidPlacement`: a block that is neither water nor air, whose top face is
    /// covered by the block below and whose horizontal faces are all covered by their neighbours.
    fn is_valid_placement<T: GenerationCache>(chunk: &T, target: &BlockPos) -> bool {
        let target_state = GenerationCache::get_block_state(chunk, &target.0);
        if target_state.to_block_id() == Block::WATER || target_state.to_state().is_air() {
            return false;
        }

        let below = target.offset(BlockDirection::Down.to_offset());
        if Self::is_visible_from_outside(chunk, &below, BlockDirection::Up) {
            return false;
        }

        for dir in BlockDirection::horizontal().map(BlockDirection::from_cardinal_direction) {
            let neighbour = target.offset(dir.to_offset());
            if Self::is_visible_from_outside(chunk, &neighbour, dir.opposite()) {
                return false;
            }
        }

        true
    }

    /// Vanilla `isVisibleFromOutside`: the face `covered_direction` of the block at `pos` does
    /// not occlude a full face. Blocks that cannot occlude (water, lava, plants, glass) have no
    /// occlusion shape; for the others the full sturdy face stands in for the face of the
    /// occlusion shape, the two being the same for the blocks of the sea floor.
    fn is_visible_from_outside<T: GenerationCache>(
        chunk: &T,
        pos: &BlockPos,
        covered_direction: BlockDirection,
    ) -> bool {
        let state = GenerationCache::get_block_state(chunk, &pos.0).to_state();
        !(state.can_occlude() && state.is_side_solid(covered_direction))
    }

    #[allow(clippy::too_many_arguments)]
    pub fn generate<T: GenerationCache>(
        &self,
        chunk: &mut T,
        _min_y: i8,
        _height: u16,
        _feature_name: pumpkin_data::placed_feature::PlacedFeature,
        random: &mut RandomGenerator,
        pos: BlockPos,
    ) -> bool {
        // Locate floor; abort if none
        let Some(floor_y) = self.get_floor_y(chunk, pos) else {
            return false;
        };

        let floor_pos = BlockPos::new(pos.0.x, floor_y, pos.0.z);

        // Sample a cube around the floor, placing magma with probability and validity checks
        let mut placed = 0i32;
        let r = self.placement_radius;

        for dz in -r..=r {
            for dy in -r..=r {
                for dx in -r..=r {
                    if random.next_f32() >= self.placement_probability {
                        continue;
                    }

                    let target_pos =
                        BlockPos::new(floor_pos.0.x + dx, floor_pos.0.y + dy, floor_pos.0.z + dz);

                    if !Self::is_valid_placement(chunk, &target_pos) {
                        continue;
                    }

                    let magma_state = BlockState::from_id(Block::MAGMA_BLOCK.default_state.id);
                    chunk.set_block_state(&target_pos.0, magma_state);
                    placed += 1;
                }
            }
        }

        placed > 0
    }
}

#[cfg(test)]
mod tests {
    use pumpkin_data::BlockState;

    use super::*;
    use crate::generation::feature::features::sculk::test_utils::{empty_chunk, set};

    /// A stone block at `target` buried in stone, under water.
    fn buried_stone(target: BlockPos) -> crate::generation::proto_chunk::ProtoChunk {
        let mut chunk = empty_chunk();
        for pos in BlockPos::iterate(target.add(-1, -1, -1), target.add(1, 0, 1)) {
            set(&mut chunk, pos, Block::STONE.default_state);
        }
        set(&mut chunk, target.up(), Block::WATER.default_state);
        chunk
    }

    fn valid_with(neighbour: BlockPos, state: &BlockState) -> bool {
        let target = BlockPos::new(8, 40, 8);
        let mut chunk = buried_stone(target);
        set(&mut chunk, neighbour, state);
        UnderwaterMagmaFeature::is_valid_placement(&chunk, &target)
    }

    #[test]
    fn buried_stone_is_valid() {
        let target = BlockPos::new(8, 40, 8);
        assert!(UnderwaterMagmaFeature::is_valid_placement(
            &buried_stone(target),
            &target
        ));
    }

    #[test]
    fn cave_air_is_not_valid() {
        let target = BlockPos::new(8, 40, 8);
        assert!(!valid_with(target, Block::CAVE_AIR.default_state));
    }

    #[test]
    fn faces_that_do_not_occlude_are_visible() {
        let west = BlockPos::new(7, 40, 8);
        assert!(!valid_with(west, Block::GLOW_LICHEN.default_state));
        assert!(!valid_with(west, Block::LAVA.default_state));
        assert!(!valid_with(west, Block::CAVE_AIR.default_state));
        assert!(valid_with(west, Block::MAGMA_BLOCK.default_state));
    }

    #[test]
    fn below_needs_a_covered_top_face() {
        let below = BlockPos::new(8, 39, 8);
        assert!(!valid_with(below, Block::SEAGRASS.default_state));
        assert!(!valid_with(below, Block::GLASS.default_state));
        assert!(valid_with(below, Block::MUD.default_state));
    }
}
