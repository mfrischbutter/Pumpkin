use pumpkin_data::tag::Taggable;
use pumpkin_data::{Block, BlockState, block_properties::BrownMushroomBlockLikeProperties, tag};
use pumpkin_util::{math::position::BlockPos, random::RandomGenerator};

use super::huge_brown_mushroom::place_mushroom_block;
use crate::generation::proto_chunk::GenerationCache;

pub struct HugeRedMushroomFeature;

impl HugeRedMushroomFeature {
    const FOLIAGE_RADIUS: i32 = 2;

    /// Vanilla's `getTreeRadiusForHeight`.
    const fn tree_radius_for_height(tree_height: i32, dy: i32) -> i32 {
        if (dy < tree_height && dy >= tree_height - 3) || dy == tree_height {
            Self::FOLIAGE_RADIUS
        } else {
            0
        }
    }

    #[allow(clippy::unused_self)]
    pub fn generate<T: GenerationCache>(
        &self,
        chunk: &mut T,
        min_y: i8,
        height: u16,
        _feature: pumpkin_data::placed_feature::PlacedFeature,
        random: &mut RandomGenerator,
        pos: BlockPos,
    ) -> bool {
        let tree_height = super::huge_brown_mushroom::mushroom_tree_height(random);

        let min_y = i32::from(min_y);
        // Vanilla's `getMaxY()`: the highest block inside the world.
        let max_y = min_y + i32::from(height) - 1;
        if pos.0.y < min_y + 1 || pos.0.y + tree_height + 1 > max_y {
            return false;
        }

        let below_block = chunk.get_block(&pos.down());
        if !below_block.has_tag(&tag::Block::MINECRAFT_HUGE_RED_MUSHROOM_CAN_PLACE_ON) {
            return false;
        }

        for dy in 0..=tree_height {
            // Vanilla's `isValidPosition` asks `getTreeRadiusForHeight(-1, -1, ..)`, so only the
            // stem column has to be free.
            let radius = Self::tree_radius_for_height(-1, dy);
            for dx in -radius..=radius {
                for dz in -radius..=radius {
                    let check_pos = BlockPos::new(pos.0.x + dx, pos.0.y + dy, pos.0.z + dz);
                    let (block, state) = chunk.get_block_and_state(&check_pos);
                    if !state.is_air() && !block.has_tag(&tag::Block::MINECRAFT_LEAVES) {
                        return false;
                    }
                }
            }
        }

        let radius = Self::FOLIAGE_RADIUS;
        for i in (tree_height - 3)..=tree_height {
            let j = if i < tree_height { radius } else { radius - 1 };
            let k = radius - 2;

            for l in -j..=j {
                for m in -j..=j {
                    let on_x_edge = l == -j || l == j;
                    let on_z_edge = m == -j || m == j;

                    if i < tree_height && on_x_edge == on_z_edge {
                        continue;
                    }

                    let props = BrownMushroomBlockLikeProperties {
                        up: i >= tree_height - 1,
                        down: false,
                        west: l < -k,
                        east: l > k,
                        north: m < -k,
                        south: m > k,
                    };
                    let state_id = props.to_state_id(&Block::RED_MUSHROOM_BLOCK);
                    let cap_pos = BlockPos::new(pos.0.x + l, pos.0.y + i, pos.0.z + m);
                    place_mushroom_block(chunk, &cap_pos, BlockState::from_id(state_id));
                }
            }
        }

        let stem_props = BrownMushroomBlockLikeProperties {
            up: false,
            down: false,
            north: true,
            east: true,
            south: true,
            west: true,
        };
        let stem_state = BlockState::from_id(stem_props.to_state_id(&Block::MUSHROOM_STEM));
        for i in 0..tree_height {
            let stem_pos = BlockPos::new(pos.0.x, pos.0.y + i, pos.0.z);
            place_mushroom_block(chunk, &stem_pos, stem_state);
        }

        true
    }
}

#[cfg(test)]
mod tests {
    use pumpkin_data::Block;
    use pumpkin_util::random::legacy_rand::LegacyRand;

    use super::*;
    use crate::generation::feature::features::sculk::test_utils::{empty_chunk, set};

    #[test]
    fn only_the_stem_column_has_to_be_free() {
        let mut chunk = empty_chunk();
        let origin = BlockPos::new(8, 100, 8);
        set(&mut chunk, origin.down(), Block::GRASS_BLOCK.default_state);
        // Inside the cap's radius at every height the cap can reach.
        for y in 100..=113 {
            set(
                &mut chunk,
                BlockPos::new(10, y, 8),
                Block::STONE.default_state,
            );
        }

        let mut random = RandomGenerator::Legacy(LegacyRand::from_seed(1));
        assert!(HugeRedMushroomFeature.generate(
            &mut chunk,
            -64,
            384,
            pumpkin_data::placed_feature::PlacedFeature::DarkForestVegetation,
            &mut random,
            origin,
        ));
        assert_eq!(
            GenerationCache::get_block_state(&chunk, &origin.0).to_block_id(),
            Block::MUSHROOM_STEM.id,
        );
    }
}
