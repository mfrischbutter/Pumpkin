use crate::{generation::proto_chunk::GenerationCache, world::WorldPortalExt};
use pumpkin_data::BlockDirection;
use pumpkin_util::{
    math::position::BlockPos,
    random::{RandomGenerator, RandomImpl},
};

use crate::generation::feature::placed_features::PlacedFeatureWrapper;

/// Vanilla's `CoralTreeFeature`: a trunk with branches, every block placed by `feature`.
pub struct CoralTreeFeature {
    pub feature: Box<PlacedFeatureWrapper>,
}

impl CoralTreeFeature {
    #[expect(clippy::too_many_arguments)]
    pub fn generate<T: GenerationCache>(
        &self,
        chunk: &mut T,
        block_registry: &dyn WorldPortalExt,
        min_y: i8,
        height: u16,
        feature_name: pumpkin_data::placed_feature::PlacedFeature,
        random: &mut RandomGenerator,
        pos: BlockPos,
    ) -> bool {
        // Every block goes through the configured feature, like vanilla's `feature.value().place`.
        let place = |chunk: &mut T, random: &mut RandomGenerator, pos: BlockPos| {
            self.feature.get().is_some_and(|feature| {
                feature.generate(
                    chunk,
                    block_registry,
                    min_y,
                    height,
                    feature_name,
                    random,
                    pos,
                )
            })
        };
        let mut cur_pos = pos;
        let trunk_height = random.next_bounded_i32(3) + 1;
        for _ in 0..trunk_height {
            if !place(chunk, random, cur_pos) {
                return true;
            }
            cur_pos = cur_pos.up();
        }

        let trunk_top_pos = cur_pos;
        let n_branches = random.next_bounded_i32(3) + 2;

        let mut directions: Vec<BlockDirection> = BlockDirection::horizontal_worldgen()
            .iter()
            .copied()
            .map(BlockDirection::from_cardinal_direction)
            .collect();
        for i in (1..directions.len()).rev() {
            let j = random.next_bounded_i32(i as i32 + 1) as usize;
            directions.swap(i, j);
        }

        for &branch_direction in directions.iter().take(n_branches as usize) {
            let mut branch_pos = trunk_top_pos.offset(branch_direction.to_offset());
            let branch_height = random.next_bounded_i32(5) + 2;
            let mut segment_length = 0;

            for j in 0..branch_height {
                if !place(chunk, random, branch_pos) {
                    break;
                }
                segment_length += 1;
                branch_pos = branch_pos.up();
                if j == 0 || (segment_length >= 2 && random.next_f32() < 0.25) {
                    branch_pos = branch_pos.offset(branch_direction.to_offset());
                    segment_length = 0;
                }
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use pumpkin_data::configured_feature::ConfiguredFeature as FeatureKey;
    use pumpkin_data::{Block, BlockId};
    use pumpkin_util::random::legacy_rand::LegacyRand;

    use super::*;
    use crate::generation::feature::configured_features::{CONFIGURED_FEATURES, ConfiguredFeature};
    use crate::generation::feature::features::sculk::test_utils::{empty_region, set};
    use crate::generation::feature::placed_features::Feature;
    use crate::generation::proto_chunk_test::stage_parity::TestBlockRegistry;

    #[test]
    fn warm_ocean_trees_build_from_their_coral_block() {
        // 26.3's warm ocean vegetation: the first choice is the tube coral tree.
        let vegetation = CONFIGURED_FEATURES.get(&FeatureKey::WarmOceanVegetation);
        let Some(ConfiguredFeature::SimpleRandomSelector(selector)) = vegetation else {
            panic!("warm ocean vegetation is a simple random selector");
        };
        let tree = selector.features.first().map(|placed| &placed.feature);
        let Some(Feature::Inlined(tree)) = tree else {
            panic!("its first choice is an inlined feature");
        };
        assert!(matches!(tree.as_ref(), ConfiguredFeature::CoralTree(_)));

        let origin = BlockPos::new(24, 60, 24);
        for seed in 0..8 {
            let mut level = empty_region();
            for x in 16..32 {
                for y in 56..80 {
                    for z in 16..32 {
                        set(
                            &mut level,
                            BlockPos::new(x, y, z),
                            Block::WATER.default_state,
                        );
                    }
                }
            }
            let mut random = RandomGenerator::Legacy(LegacyRand::from_seed(seed));
            assert!(tree.generate(
                &mut level,
                &TestBlockRegistry,
                -64,
                384,
                pumpkin_data::placed_feature::PlacedFeature::WarmOceanVegetation,
                &mut random,
                origin,
            ));
            let mut blocks = 0;
            for x in 16..32 {
                for y in 56..80 {
                    for z in 16..32 {
                        let id =
                            GenerationCache::get_block_state(&level, &BlockPos::new(x, y, z).0)
                                .to_block_id();
                        if id.has_tag(pumpkin_data::tag::Block::MINECRAFT_CORAL_BLOCKS) {
                            assert_eq!(id, BlockId::TUBE_CORAL_BLOCK, "seed {seed} at {x} {y} {z}");
                            blocks += 1;
                        }
                    }
                }
            }
            assert!(blocks > 0, "seed {seed}: the trunk stands");
        }
    }
}
