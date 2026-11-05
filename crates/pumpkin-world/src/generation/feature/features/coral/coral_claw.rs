use crate::{generation::proto_chunk::GenerationCache, world::WorldPortalExt};
use pumpkin_data::BlockDirection;
use pumpkin_util::{
    math::position::BlockPos,
    random::{RandomGenerator, RandomImpl},
};

use crate::generation::feature::placed_features::PlacedFeatureWrapper;

/// Vanilla's `CoralClawFeature`: a block and two or three claws, every block placed by
/// `feature`.
pub struct CoralClawFeature {
    pub feature: Box<PlacedFeatureWrapper>,
}

impl CoralClawFeature {
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
        if !place(chunk, random, pos) {
            return false;
        }
        let claw_dir =
            BlockDirection::from_cardinal_direction(BlockDirection::random_horizontal(random));
        let n_branches = random.next_bounded_i32(2) + 2;
        let mut possible_directions = [
            claw_dir,
            claw_dir.rotate_clockwise(),
            claw_dir.rotate_counter_clockwise(),
        ];
        for i in (1..possible_directions.len()).rev() {
            let j = random.next_bounded_i32(i as i32 + 1) as usize;
            possible_directions.swap(i, j);
        }

        for &branch_direction in possible_directions.iter().take(n_branches as usize) {
            let mut mut_pos = pos;
            let sideway_length = random.next_bounded_i32(2) + 1;
            mut_pos = mut_pos.offset(branch_direction.to_offset());

            let (segment_direction, inway_length) = if branch_direction == claw_dir {
                (claw_dir, random.next_bounded_i32(3) + 2)
            } else {
                mut_pos = mut_pos.up();
                let seg_dir = segment_direction(branch_direction, random);
                (seg_dir, random.next_bounded_i32(3) + 3)
            };

            for _ in 0..sideway_length {
                if !place(chunk, random, mut_pos) {
                    break;
                }
                mut_pos = mut_pos.offset(segment_direction.to_offset());
            }

            mut_pos = mut_pos.offset(segment_direction.opposite().to_offset());
            mut_pos = mut_pos.up();

            for _ in 0..inway_length {
                mut_pos = mut_pos.offset(claw_dir.to_offset());
                if !place(chunk, random, mut_pos) {
                    break;
                }
                if random.next_f32() < 0.25 {
                    mut_pos = mut_pos.up();
                }
            }
        }
        true
    }
}

/// The direction a side branch grows in: vanilla's
/// `Util.getRandom(new Direction[]{branchDirection, Direction.UP}, random)`.
fn segment_direction(
    branch_direction: BlockDirection,
    random: &mut RandomGenerator,
) -> BlockDirection {
    [branch_direction, BlockDirection::Up][random.next_bounded_i32(2) as usize]
}

#[cfg(test)]
mod tests {
    use pumpkin_util::random::xoroshiro128::Xoroshiro;

    use super::*;

    #[test]
    fn side_branch_direction_indexes_like_util_get_random() {
        let mut saw_up = false;
        for seed in 0..16 {
            let mut random = RandomGenerator::Xoroshiro(Xoroshiro::from_seed(seed));
            let mut expected = RandomGenerator::Xoroshiro(Xoroshiro::from_seed(seed));
            let index = expected.next_bounded_i32(2);
            let direction = segment_direction(BlockDirection::North, &mut random);
            let wanted = if index == 1 {
                BlockDirection::Up
            } else {
                BlockDirection::North
            };
            assert_eq!(direction, wanted, "seed {seed}");
            saw_up |= index == 1;
        }
        assert!(saw_up);
    }
}
