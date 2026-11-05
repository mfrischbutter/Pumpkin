use pumpkin_data::{
    Block, BlockDirection, BlockState,
    block_properties::{HorizontalFacing, ShelfMushroomLikeProperties},
};
use pumpkin_util::{
    math::position::BlockPos,
    random::{RandomGenerator, RandomImpl},
};

use crate::generation::proto_chunk::GenerationCache;

/// Vanilla's `ShelfMushroomDecorator`: shelf mushrooms on the trunk of a standing tree, from one to
/// four blocks above its base, or along a fallen log.
pub struct ShelfMushroomTreeDecorator {
    pub probability: f32,
}

impl ShelfMushroomTreeDecorator {
    pub fn generate<T: GenerationCache>(
        &self,
        chunk: &mut T,
        random: &mut RandomGenerator,
        log_positions: &[BlockPos],
        decorations: &mut Vec<BlockPos>,
    ) {
        if random.next_f32() >= self.probability {
            return;
        }
        let (Some(first), Some(last)) = (log_positions.first(), log_positions.last()) else {
            return;
        };
        // Vanilla sees a fallen log in logs that all lie at one height, a single stump log too.
        if first.0.y == last.0.y {
            let directions = if first.0.x == last.0.x {
                [HorizontalFacing::East, HorizontalFacing::West]
            } else {
                [HorizontalFacing::North, HorizontalFacing::South]
            };
            Self::place_on_fallen_log(chunk, random, log_positions, directions, decorations);
        } else {
            Self::place_on_standing_tree(chunk, random, log_positions, first.0.y, decorations);
        }
    }

    fn place_on_standing_tree<T: GenerationCache>(
        chunk: &mut T,
        random: &mut RandomGenerator,
        log_positions: &[BlockPos],
        base_y: i32,
        decorations: &mut Vec<BlockPos>,
    ) {
        let horizontal = BlockDirection::horizontal_worldgen();
        let first = horizontal[random.next_bounded_i32(4) as usize];
        let second = clockwise(first);
        for log in log_positions {
            if !(1..=4).contains(&(log.0.y - base_y)) {
                continue;
            }
            for facing in [first, second] {
                if random.next_f32() > 0.25 {
                    continue;
                }
                let pos = offset(*log, facing);
                if is_replaceable_with_shelf_mushroom(chunk, pos)
                    && !has_shelf_mushroom_at(chunk, pos.down())
                {
                    place_mushroom(chunk, random, pos, facing, decorations);
                    break;
                }
            }
        }
    }

    fn place_on_fallen_log<T: GenerationCache>(
        chunk: &mut T,
        random: &mut RandomGenerator,
        log_positions: &[BlockPos],
        directions: [HorizontalFacing; 2],
        decorations: &mut Vec<BlockPos>,
    ) {
        for log in log_positions {
            for facing in directions {
                if random.next_f32() > 0.25 {
                    continue;
                }
                let pos = offset(*log, facing);
                if is_replaceable_with_shelf_mushroom(chunk, pos)
                    && !has_adjacent_shelf_mushroom(chunk, pos)
                    && !has_adjacent_shelf_mushroom(chunk, *log)
                {
                    place_mushroom(chunk, random, pos, facing, decorations);
                }
            }
        }
    }
}

/// Vanilla's `Direction.getClockWise` for the horizontal directions.
const fn clockwise(facing: HorizontalFacing) -> HorizontalFacing {
    match facing {
        HorizontalFacing::North => HorizontalFacing::East,
        HorizontalFacing::East => HorizontalFacing::South,
        HorizontalFacing::South => HorizontalFacing::West,
        HorizontalFacing::West => HorizontalFacing::North,
    }
}

fn offset(pos: BlockPos, facing: HorizontalFacing) -> BlockPos {
    pos.offset(BlockDirection::from_cardinal_direction(facing).to_offset())
}

fn has_shelf_mushroom_at<T: GenerationCache>(chunk: &T, pos: BlockPos) -> bool {
    GenerationCache::get_block_state(chunk, &pos.0).to_block_id() == Block::SHELF_MUSHROOM.id
}

fn has_adjacent_shelf_mushroom<T: GenerationCache>(chunk: &T, pos: BlockPos) -> bool {
    BlockDirection::horizontal_worldgen()
        .into_iter()
        .any(|facing| has_shelf_mushroom_at(chunk, offset(pos, facing)))
}

/// Vanilla's `isBlockReplaceableWithShelfMushroom`: a replaceable block without water in it or
/// beside it.
fn is_replaceable_with_shelf_mushroom<T: GenerationCache>(chunk: &T, pos: BlockPos) -> bool {
    let is_water = |pos: BlockPos| {
        GenerationCache::get_block_state(chunk, &pos.0).to_block_id() == Block::WATER.id
    };
    GenerationCache::get_block_state(chunk, &pos.0)
        .to_state()
        .replaceable()
        && !is_water(pos)
        && !BlockDirection::horizontal_worldgen()
            .into_iter()
            .any(|facing| is_water(offset(pos, facing)))
}

fn place_mushroom<T: GenerationCache>(
    chunk: &mut T,
    random: &mut RandomGenerator,
    pos: BlockPos,
    facing: HorizontalFacing,
    decorations: &mut Vec<BlockPos>,
) {
    let props = ShelfMushroomLikeProperties {
        age: random.next_bounded_i32(2) as u8,
        facing,
    };
    let state_id = props.to_state_id(&Block::SHELF_MUSHROOM);
    chunk.set_block_state(&pos.0, BlockState::from_id(state_id));
    decorations.push(pos);
}

#[cfg(test)]
mod tests {
    use pumpkin_util::random::legacy_rand::LegacyRand;

    use super::*;
    use crate::generation::feature::features::sculk::test_utils::{empty_region, set};

    const ALWAYS: ShelfMushroomTreeDecorator = ShelfMushroomTreeDecorator { probability: 1.0 };

    fn mushrooms(
        level: &impl GenerationCache,
        placed: &[BlockPos],
    ) -> Vec<(BlockPos, HorizontalFacing)> {
        placed
            .iter()
            .map(|pos| {
                let state = GenerationCache::get_block_state(level, &pos.0);
                assert_eq!(state.to_block_id(), Block::SHELF_MUSHROOM.id);
                (
                    *pos,
                    ShelfMushroomLikeProperties::from_state_id(state).facing,
                )
            })
            .collect()
    }

    #[test]
    fn standing_trees_grow_mushrooms_one_to_four_above_their_base() {
        let logs: Vec<BlockPos> = (0..8).map(|dy| BlockPos::new(24, 64 + dy, 24)).collect();
        let mut grown = 0;
        for seed in 0..64 {
            let mut level = empty_region();
            let mut random = RandomGenerator::Legacy(LegacyRand::from_seed(seed));
            let mut placed = Vec::new();
            ALWAYS.generate(&mut level, &mut random, &logs, &mut placed);
            for (pos, facing) in mushrooms(&level, &placed) {
                // On the trunk side it faces away from, one to four blocks above the base.
                let log = offset(pos, opposite(facing));
                assert!(logs.contains(&log), "{pos:?} grows on a log");
                assert!(
                    (65..=68).contains(&log.0.y),
                    "{pos:?} is one to four above the base"
                );
                assert!(
                    !placed.contains(&pos.down()),
                    "{pos:?} has no mushroom right below"
                );
            }
            grown += placed.len();
        }
        assert!(grown > 0);
    }

    #[test]
    fn fallen_logs_grow_mushrooms_on_their_long_sides() {
        let logs: Vec<BlockPos> = (0..5).map(|dx| BlockPos::new(20 + dx, 64, 24)).collect();
        for seed in 0..32 {
            let mut level = empty_region();
            let mut random = RandomGenerator::Legacy(LegacyRand::from_seed(seed));
            let mut placed = Vec::new();
            ALWAYS.generate(&mut level, &mut random, &logs, &mut placed);
            for (pos, facing) in mushrooms(&level, &placed) {
                assert!(
                    matches!(facing, HorizontalFacing::North | HorizontalFacing::South),
                    "a log along x grows mushrooms north and south, {pos:?} faces {facing:?}"
                );
            }
        }
    }

    #[test]
    fn water_keeps_mushrooms_away() {
        let logs: Vec<BlockPos> = (0..6).map(|dy| BlockPos::new(24, 64 + dy, 24)).collect();
        for seed in 0..32 {
            let mut level = empty_region();
            for dy in 0..6 {
                for (dx, dz) in [(2, 0), (-2, 0), (0, 2), (0, -2)] {
                    set(
                        &mut level,
                        BlockPos::new(24 + dx, 64 + dy, 24 + dz),
                        Block::WATER.default_state,
                    );
                }
            }
            let mut random = RandomGenerator::Legacy(LegacyRand::from_seed(seed));
            let mut placed = Vec::new();
            ALWAYS.generate(&mut level, &mut random, &logs, &mut placed);
            assert!(placed.is_empty(), "seed {seed}: water beside every spot");
        }
    }

    const fn opposite(facing: HorizontalFacing) -> HorizontalFacing {
        clockwise(clockwise(facing))
    }
}
