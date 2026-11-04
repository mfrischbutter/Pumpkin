//! Vanilla `SculkPatchFeature`: rounds of charge cursors spreading sculk from the origin.
//!
//! The catalysts and shriekers of the deep dark and ancient cities are features of their own,
//! run after it in a sequence.

use pumpkin_util::math::position::BlockPos;
use pumpkin_util::random::RandomGenerator;

use crate::generation::feature::features::sculk;
use crate::generation::feature::features::sculk::spreader::SculkSpreader;
use crate::generation::proto_chunk::GenerationCache;

pub struct SculkPatchFeature {
    pub charge_count: i32,
    pub amount_per_charge: i32,
    pub spread_attempts: i32,
    pub growth_rounds: i32,
    pub spread_rounds: i32,
}

impl SculkPatchFeature {
    pub fn generate<T: GenerationCache>(
        &self,
        chunk: &mut T,
        random: &mut RandomGenerator,
        pos: BlockPos,
    ) -> bool {
        if !sculk::can_spread_from(chunk, pos) {
            return false;
        }

        let mut spreader = SculkSpreader::new_world_gen_spreader();
        let total_rounds = self.spread_rounds + self.growth_rounds;
        for round in 0..total_rounds {
            for _ in 0..self.charge_count {
                spreader.add_cursors(pos, self.amount_per_charge);
            }
            let spread_veins = round < self.spread_rounds;
            for _ in 0..self.spread_attempts {
                spreader.update_cursors(chunk, pos, random, spread_veins);
            }
            spreader.clear();
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use pumpkin_data::{Block, BlockId};
    use pumpkin_util::random::legacy_rand::LegacyRand;

    use super::*;
    use crate::generation::feature::features::sculk::test_utils::{empty_region, set};

    #[test]
    fn sculk_stays_within_twelve_blocks_of_the_origin() {
        // A deepslate floor under air, three chunks wide. On a floor sculk only grows straight
        // above or below a vein a cursor stood on, and world generation cursors stay within
        // twelve blocks (horizontally) of the origin.
        let mut level = empty_region();
        for x in 0..48 {
            for z in 0..48 {
                set(
                    &mut level,
                    BlockPos::new(x, 0, z),
                    Block::DEEPSLATE.default_state,
                );
            }
        }
        let origin = BlockPos::new(24, 1, 24);
        let feature = SculkPatchFeature {
            charge_count: 32,
            amount_per_charge: 500,
            spread_attempts: 64,
            growth_rounds: 0,
            spread_rounds: 8,
        };
        let mut random = RandomGenerator::Legacy(LegacyRand::from_seed(1));
        assert!(feature.generate(&mut level, &mut random, origin));

        let mut sculk = 0;
        for x in 0..48 {
            for z in 0..48 {
                let pos = BlockPos::new(x, 0, z);
                if GenerationCache::get_block_state(&level, &pos.0).to_block_id() == BlockId::SCULK
                {
                    sculk += 1;
                    let (dx, dz) = (x - origin.0.x, z - origin.0.z);
                    assert!(dx * dx + dz * dz <= 144, "sculk at {pos:?}");
                }
            }
        }
        assert!(sculk > 0);
    }
}
