use pumpkin_data::{BlockId, BlockStateId};
use pumpkin_util::random::{RandomGenerator, RandomImpl};

pub struct RandomBlockMatchRuleTest {
    pub block: BlockId,
    pub probability: f32,
}

impl RandomBlockMatchRuleTest {
    pub fn test(&self, state: BlockStateId, random: &mut RandomGenerator) -> bool {
        state.to_block_id().as_u16() == self.block.as_u16() && random.next_f32() < self.probability
    }
}

#[cfg(test)]
mod tests {
    use pumpkin_data::Block;
    use pumpkin_util::random::legacy_rand::LegacyRand;

    use super::*;

    #[test]
    fn matches_any_state_of_the_block_and_draws_once() {
        let rule = RandomBlockMatchRuleTest {
            block: Block::DEEPSLATE.id,
            probability: 1.0,
        };
        let state = Block::DEEPSLATE.default_state.id;
        assert_ne!(state.as_u16(), Block::DEEPSLATE.id.as_u16());

        let mut random = RandomGenerator::Legacy(LegacyRand::from_seed(1));
        let mut fresh = RandomGenerator::Legacy(LegacyRand::from_seed(1));
        assert!(rule.test(state, &mut random));
        fresh.next_f32();
        assert_eq!(random.next_i32(), fresh.next_i32());
    }

    #[test]
    fn other_blocks_do_not_draw() {
        let rule = RandomBlockMatchRuleTest {
            block: Block::DEEPSLATE.id,
            probability: 1.0,
        };
        let mut random = RandomGenerator::Legacy(LegacyRand::from_seed(1));
        let mut fresh = RandomGenerator::Legacy(LegacyRand::from_seed(1));
        assert!(!rule.test(Block::STONE.default_state.id, &mut random));
        assert_eq!(random.next_i32(), fresh.next_i32());
    }
}
