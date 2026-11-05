use pumpkin_data::{Block, BlockState, block_properties::PaleHangingMossLikeProperties};
use pumpkin_util::{
    math::position::BlockPos,
    random::{RandomGenerator, RandomImpl},
};

use crate::generation::feature::configured_features::CONFIGURED_FEATURES;
use crate::generation::proto_chunk::GenerationCache;
use crate::world::WorldPortalExt;

pub struct PaleMossTreeDecorator {
    pub leaves_probability: f32,
    pub trunk_probability: f32,
    pub ground_probability: f32,
}

impl PaleMossTreeDecorator {
    pub fn generate<T: GenerationCache>(
        &self,
        chunk: &mut T,
        block_registry: &dyn WorldPortalExt,
        random: &mut RandomGenerator,
        log_positions: &[BlockPos],
        foliage_positions: &[BlockPos],
        decorations: &mut Vec<BlockPos>,
    ) {
        // Vanilla's `Util.shuffledCopy` of the logs; the patch grows above the first lowest one.
        let mut shuffled = log_positions.to_vec();
        for i in (1..shuffled.len()).rev() {
            let j = random.next_bounded_i32((i + 1) as i32) as usize;
            shuffled.swap(i, j);
        }
        let Some(origin) = shuffled.into_iter().min_by_key(|pos| pos.0.y) else {
            return;
        };

        if random.next_f32() < self.ground_probability {
            Self::place_moss_patch(chunk, block_registry, random, origin.up());
        }

        for &pos in log_positions {
            if random.next_f32() < self.trunk_probability {
                let down = pos.down();
                if chunk.is_air(&down.0) {
                    Self::add_moss_hanger(chunk, random, down, decorations);
                }
            }
        }

        for &pos in foliage_positions {
            if random.next_f32() < self.leaves_probability {
                let down = pos.down();
                if chunk.is_air(&down.0) {
                    Self::add_moss_hanger(chunk, random, down, decorations);
                }
            }
        }
    }

    /// Vanilla places the configured `pale_moss_patch` with the tree's random.
    fn place_moss_patch<T: GenerationCache>(
        chunk: &mut T,
        block_registry: &dyn WorldPortalExt,
        random: &mut RandomGenerator,
        pos: BlockPos,
    ) {
        let Some(moss_patch) = CONFIGURED_FEATURES
            .get(&pumpkin_data::configured_feature::ConfiguredFeature::PaleMossPatch)
        else {
            return;
        };
        let center = chunk.get_center_chunk();
        let (min_y, height) = (center.generation_bottom_y(), center.generation_height());
        moss_patch.generate(
            chunk,
            block_registry,
            min_y,
            height,
            pumpkin_data::placed_feature::PlacedFeature::PaleMossPatch,
            random,
            pos,
        );
    }

    fn add_moss_hanger<T: GenerationCache>(
        chunk: &mut T,
        random: &mut RandomGenerator,
        mut pos: BlockPos,
        decorations: &mut Vec<BlockPos>,
    ) {
        while chunk.is_air(&pos.down().0) && random.next_f32() >= 0.5 {
            let props = PaleHangingMossLikeProperties { tip: false };
            let state_id = props.to_state_id(&Block::PALE_HANGING_MOSS);
            chunk.set_block_state(&pos.0, BlockState::from_id(state_id));
            decorations.push(pos);
            pos = pos.down();
        }

        let props = PaleHangingMossLikeProperties { tip: true };
        let state_id = props.to_state_id(&Block::PALE_HANGING_MOSS);
        chunk.set_block_state(&pos.0, BlockState::from_id(state_id));
        decorations.push(pos);
    }
}

#[cfg(test)]
mod tests {
    use pumpkin_util::random::legacy_rand::LegacyRand;

    use super::*;
    use crate::generation::feature::features::sculk::test_utils::{empty_region, set};
    use crate::generation::proto_chunk_test::stage_parity::TestBlockRegistry;

    fn pale_moss_blocks_with_ground_probability(ground_probability: f32) -> usize {
        let mut level = empty_region();
        for x in 16..32 {
            for z in 16..32 {
                set(
                    &mut level,
                    BlockPos::new(x, 63, z),
                    Block::STONE.default_state,
                );
            }
        }
        let logs = [BlockPos::new(24, 64, 24), BlockPos::new(24, 65, 24)];
        for log in logs {
            set(&mut level, log, Block::PALE_OAK_LOG.default_state);
        }

        let decorator = PaleMossTreeDecorator {
            leaves_probability: 0.0,
            trunk_probability: 0.0,
            ground_probability,
        };
        let mut random = RandomGenerator::Legacy(LegacyRand::from_seed(3));
        decorator.generate(
            &mut level,
            &TestBlockRegistry,
            &mut random,
            &logs,
            &[],
            &mut Vec::new(),
        );

        let mut count = 0;
        for x in 16..32 {
            for z in 16..32 {
                let id = GenerationCache::get_block_state(&level, &BlockPos::new(x, 63, z).0)
                    .to_block_id();
                if id == Block::PALE_MOSS_BLOCK.id {
                    count += 1;
                }
            }
        }
        count
    }

    #[test]
    fn ground_patch_grows_around_the_lowest_log() {
        assert!(pale_moss_blocks_with_ground_probability(1.0) > 0);
        assert_eq!(pale_moss_blocks_with_ground_probability(0.0), 0);
    }
}
