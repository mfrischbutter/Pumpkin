use pumpkin_util::{
    math::{int_provider::IntProvider, position::BlockPos},
    random::{RandomGenerator, RandomImpl},
};

use pumpkin_data::BlockState;
use pumpkin_data::block_properties::Axis;

use super::FoliagePlacer;
use crate::generation::block_state_provider::BlockStateProvider;
use crate::generation::feature::features::tree::TreeNode;
use crate::generation::feature::features::tree::trunk::poplar::PoplarTrunkPlacer;
use crate::generation::proto_chunk::GenerationCache;

pub struct PoplarFoliagePlacer {
    pub height: IntProvider,
    pub side_hole_chance: f32,
}

impl PoplarFoliagePlacer {
    #[expect(clippy::too_many_arguments)]
    pub fn generate<T: GenerationCache>(
        &self,
        chunk: &mut T,
        random: &mut RandomGenerator,
        node: &TreeNode,
        foliage_height: i32,
        leaf_radius: i32,
        offset: i32,
        foliage_provider: &BlockStateProvider,
        trunk_state: &BlockState,
    ) -> Vec<BlockPos> {
        let mut foliage_positions = Vec::new();
        let pos = node.center.up_height(offset);
        let current_radius = leaf_radius + node.foliage_radius - 1;
        let flipped = random.next_bool();

        self.generate_rhombus(
            &mut foliage_positions,
            chunk,
            random,
            pos,
            current_radius - 2,
            foliage_height - 1,
            node.giant_trunk,
            foliage_provider,
            foliage_height,
            flipped,
        );
        self.generate_rhombus(
            &mut foliage_positions,
            chunk,
            random,
            pos,
            current_radius - 1,
            foliage_height - 2,
            node.giant_trunk,
            foliage_provider,
            foliage_height,
            flipped,
        );
        self.generate_rhombus(
            &mut foliage_positions,
            chunk,
            random,
            pos,
            current_radius - 1,
            foliage_height - 3,
            node.giant_trunk,
            foliage_provider,
            foliage_height,
            flipped,
        );
        for y in (1..=foliage_height - 4).rev() {
            self.generate_rhombus(
                &mut foliage_positions,
                chunk,
                random,
                pos,
                current_radius,
                y,
                node.giant_trunk,
                foliage_provider,
                foliage_height,
                flipped,
            );
        }
        Self::replace_leaves_with_log(
            chunk,
            random,
            pos,
            current_radius,
            foliage_height - 4,
            node.giant_trunk,
            foliage_height,
            flipped,
            foliage_provider,
            trunk_state,
        );
        self.generate_rhombus(
            &mut foliage_positions,
            chunk,
            random,
            pos,
            current_radius - 1,
            0,
            node.giant_trunk,
            foliage_provider,
            foliage_height,
            flipped,
        );
        self.generate_rhombus(
            &mut foliage_positions,
            chunk,
            random,
            pos,
            (current_radius - 2).clamp(1, 2),
            -1,
            node.giant_trunk,
            foliage_provider,
            foliage_height,
            flipped,
        );
        foliage_positions
    }

    /// Vanilla `PoplarFoliagePlacer.replaceLeavesWithLog`: in the row four below the top, leaves
    /// at least four blocks in from the edge along the axes become logs lying along that axis.
    #[expect(clippy::too_many_arguments)]
    fn replace_leaves_with_log<T: GenerationCache>(
        chunk: &mut T,
        random: &mut RandomGenerator,
        center_pos: BlockPos,
        current_radius: i32,
        y: i32,
        giant_trunk: bool,
        foliage_height: i32,
        flipped: bool,
        foliage_provider: &BlockStateProvider,
        trunk_state: &BlockState,
    ) {
        let partial = foliage_height - 1 == y || foliage_height - 2 == y;
        let i = i32::from(giant_trunk);
        for dx in -current_radius..=(current_radius + i) {
            for dz in -current_radius..=(current_radius + i) {
                let (abs_dx, abs_dz) = (dx.abs(), dz.abs());
                let corner = Self::corner_blocks_to_cut(dx, dz, current_radius, partial, flipped);
                let within = abs_dx + abs_dz <= current_radius * 2 - (corner + 2);
                let along_x = abs_dz == 0 && current_radius - abs_dx >= 4;
                let along_z = abs_dx == 0 && current_radius - abs_dz >= 4;
                if !within || !(along_x || along_z) {
                    continue;
                }
                let pos = BlockPos(center_pos.0.add_raw(dx, y, dz));
                // Vanilla's `tryPlaceLog`: only where the foliage provider's leaf still stands.
                let leaf = foliage_provider.get_for_bonemeal(random, pos);
                if leaf
                    .is_some_and(|leaf| GenerationCache::get_block_state(chunk, &pos.0) == leaf.id)
                {
                    let axis = if along_x { Axis::X } else { Axis::Z };
                    let log = PoplarTrunkPlacer::get_sideways_state(trunk_state.id, axis);
                    chunk.set_block_state(&pos.0, log.to_state());
                }
            }
        }
    }

    pub fn get_random_height(&self, random: &mut RandomGenerator) -> i32 {
        self.height.get(random)
    }

    // Not `FoliagePlacer::generate_square`, which folds away the sign of the offsets
    #[expect(clippy::too_many_arguments)]
    fn generate_rhombus<T: GenerationCache>(
        &self,
        foliage_positions: &mut Vec<BlockPos>,
        chunk: &mut T,
        random: &mut RandomGenerator,
        center_pos: BlockPos,
        radius: i32,
        y: i32,
        giant_trunk: bool,
        foliage_provider: &BlockStateProvider,
        foliage_height: i32,
        flipped: bool,
    ) {
        let i = i32::from(giant_trunk);

        for x in -radius..=(radius + i) {
            for z in -radius..=(radius + i) {
                if self.is_invalid_for_leaves(random, x, y, z, radius, foliage_height, flipped) {
                    continue;
                }
                let pos = BlockPos(center_pos.0.add_raw(x, y, z));
                if FoliagePlacer::place_foliage_block(chunk, random, pos, foliage_provider) {
                    foliage_positions.push(pos);
                }
            }
        }
    }

    #[expect(clippy::too_many_arguments)]
    fn is_invalid_for_leaves(
        &self,
        random: &mut RandomGenerator,
        dx: i32,
        y: i32,
        dz: i32,
        current_radius: i32,
        foliage_height: i32,
        flipped: bool,
    ) -> bool {
        // The top two rows are pulled in
        let partial = foliage_height - 1 == y || foliage_height - 2 == y;
        let abs_dx = dx.abs();
        let abs_dz = dz.abs();
        if partial && (abs_dx == current_radius || abs_dz == current_radius) {
            return true;
        }

        let corner = Self::corner_blocks_to_cut(dx, dz, current_radius, partial, flipped);
        let side_hole = i32::from(random.next_f32() <= self.side_hole_chance);
        abs_dx + abs_dz > current_radius * 2 - (corner + side_hole)
    }

    // Two opposite corners are cut back further, giving the canopy its lopsided outline
    const fn corner_blocks_to_cut(
        dx: i32,
        dz: i32,
        current_radius: i32,
        partial: bool,
        flipped: bool,
    ) -> i32 {
        let small_corner = if flipped {
            dx > 0 && dz > 0 || dz < 0 && dx < 0
        } else {
            dx > 0 && dz < 0 || dz > 0 && dx < 0
        };

        if small_corner {
            current_radius - 1
        } else if partial {
            current_radius + 1
        } else {
            current_radius
        }
    }
}

#[cfg(test)]
mod tests {
    use pumpkin_data::Block;
    use pumpkin_data::block_properties::PaleOakWoodLikeProperties;
    use pumpkin_util::random::legacy_rand::LegacyRand;

    use super::*;
    use crate::generation::block_state_provider::SimpleStateProvider;
    use crate::generation::feature::features::sculk::test_utils::{empty_region, set};

    #[test]
    fn wide_poplars_grow_logs_into_their_crown_like_vanilla() {
        let placer = PoplarFoliagePlacer {
            height: IntProvider::Constant(6),
            side_hole_chance: 0.0,
        };
        let provider = BlockStateProvider::Simple(SimpleStateProvider {
            state: Block::ORANGE_POPLAR_LEAVES.default_state,
        });
        let trunk = Block::POPLAR_LOG.default_state;
        let center = BlockPos::new(24, 70, 24);
        let mut level = empty_region();
        for y in 60..77 {
            set(&mut level, BlockPos::new(24, y, 24), trunk);
        }
        let node = TreeNode {
            center,
            foliage_radius: 0,
            giant_trunk: false,
        };
        let mut random = RandomGenerator::Legacy(LegacyRand::from_seed(1));
        // A leaf radius of 6 is 5 around the trunk: the row four below the top (y + 2) gets logs
        // one block out along both axes.
        placer.generate(&mut level, &mut random, &node, 6, 6, 0, &provider, trunk);
        for (dx, dz, axis) in [
            (1, 0, Axis::X),
            (-1, 0, Axis::X),
            (0, 1, Axis::Z),
            (0, -1, Axis::Z),
        ] {
            let pos = BlockPos::new(24 + dx, 72, 24 + dz);
            let state = GenerationCache::get_block_state(&level, &pos.0);
            assert_eq!(state.to_block_id(), Block::POPLAR_LOG.id, "{pos:?}");
            assert_eq!(
                PaleOakWoodLikeProperties::from_state_id(state).axis,
                axis,
                "{pos:?}"
            );
        }
        // Two out stays a leaf.
        let state = GenerationCache::get_block_state(&level, &BlockPos::new(26, 72, 24).0);
        assert_eq!(state.to_block_id(), Block::ORANGE_POPLAR_LEAVES.id);
    }
}
