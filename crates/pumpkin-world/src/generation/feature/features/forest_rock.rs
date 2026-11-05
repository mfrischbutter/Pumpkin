use crate::generation::{block_predicate::BlockPredicate, proto_chunk::GenerationCache};
use crate::world::WorldPortalExt;
use pumpkin_data::BlockState;
use pumpkin_util::{
    math::position::BlockPos,
    random::{RandomGenerator, RandomImpl},
};

pub struct ForestRockFeature {
    pub state: &'static BlockState,
    pub can_place_on: BlockPredicate,
}

impl ForestRockFeature {
    pub fn generate<T: GenerationCache>(
        &self,
        chunk: &mut T,
        block_registry: &dyn WorldPortalExt,
        random: &mut RandomGenerator,
        pos: BlockPos,
    ) -> bool {
        let Some(mut pos) = self.find_origin(chunk, block_registry, pos) else {
            return false;
        };

        let block_state = self.state;

        for _ in 0..3 {
            let xr = random.next_bounded_i32(2);
            let yr = random.next_bounded_i32(2);
            let zr = random.next_bounded_i32(2);
            let threshold = (xr + yr + zr) as f32 * 0.333f32 + 0.5f32;
            let threshold_sq = threshold * threshold;

            let start = BlockPos::new(pos.0.x - xr, pos.0.y - yr, pos.0.z - zr);
            let end = BlockPos::new(pos.0.x + xr, pos.0.y + yr, pos.0.z + zr);

            for candidate in BlockPos::iterate(start, end) {
                let dx = (candidate.0.x - pos.0.x) as f32;
                let dy = (candidate.0.y - pos.0.y) as f32;
                let dz = (candidate.0.z - pos.0.z) as f32;
                if dx * dx + dy * dy + dz * dz <= threshold_sq {
                    chunk.set_block_state(&candidate.0, block_state);
                }
            }

            pos = pos.add(
                -1 + random.next_bounded_i32(2),
                -random.next_bounded_i32(2),
                -1 + random.next_bounded_i32(2),
            );
        }

        true
    }
    /// Vanilla's descent: down while the block below fails `can_place_on`,
    /// giving up at the world's minimum Y + 3.
    fn find_origin<T: GenerationCache>(
        &self,
        chunk: &T,
        block_registry: &dyn WorldPortalExt,
        mut pos: BlockPos,
    ) -> Option<BlockPos> {
        let min_y = chunk.bottom_y() as i32 + 3;
        while pos.0.y > min_y && !self.can_place_on.test(block_registry, chunk, &pos.down()) {
            pos = pos.down();
        }
        (pos.0.y > min_y).then_some(pos)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generation::block_predicate::{
        MatchingBlockTagPredicate, OffsetBlocksBlockPredicate,
    };
    use crate::generation::feature::features::sculk::test_utils::{empty_chunk, set};
    use crate::generation::proto_chunk_test::stage_parity::TestBlockRegistry;
    use pumpkin_data::{Block, tag};
    use pumpkin_util::random::legacy_rand::LegacyRand;

    #[test]
    fn rock_over_grass_stays_on_top_of_it() {
        let rock = ForestRockFeature {
            state: Block::MOSSY_COBBLESTONE.default_state,
            can_place_on: BlockPredicate::MatchingBlockTag(MatchingBlockTagPredicate {
                offset: OffsetBlocksBlockPredicate { offset: None },
                tag: tag::Block::MINECRAFT_FOREST_ROCK_CAN_PLACE_ON,
            }),
        };
        let mut chunk = empty_chunk();
        set(
            &mut chunk,
            BlockPos::new(8, 62, 8),
            Block::STONE.default_state,
        );
        set(
            &mut chunk,
            BlockPos::new(8, 63, 8),
            Block::GRASS_BLOCK.default_state,
        );

        let start = BlockPos::new(8, 70, 8);
        let origin = rock.find_origin(&chunk, &TestBlockRegistry, start);
        assert_eq!(origin, Some(BlockPos::new(8, 64, 8)));

        let mut random = RandomGenerator::Legacy(LegacyRand::from_seed(42));
        assert!(rock.generate(&mut chunk, &TestBlockRegistry, &mut random, start));
        assert_eq!(
            GenerationCache::get_block_state(&chunk, &BlockPos::new(8, 64, 8).0),
            Block::MOSSY_COBBLESTONE.default_state.id,
        );
    }
}
