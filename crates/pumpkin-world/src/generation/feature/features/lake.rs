use pumpkin_data::Block;
use pumpkin_util::{
    math::position::BlockPos,
    random::{RandomGenerator, RandomImpl},
};

use crate::generation::{
    block_predicate::BlockPredicate, block_state_provider::BlockStateProvider,
    proto_chunk::GenerationCache,
};
use crate::world::WorldPortalExt;

pub struct LakeFeature {
    pub fluid: BlockStateProvider,
    pub barrier: BlockStateProvider,
    pub can_place_feature: BlockPredicate,
    pub can_replace_with_air_or_fluid: BlockPredicate,
    pub can_replace_with_barrier: BlockPredicate,
}

impl LakeFeature {
    #[expect(clippy::too_many_lines)]
    pub fn generate<T: GenerationCache>(
        &self,
        block_registry: &dyn WorldPortalExt,
        chunk: &mut T,
        random: &mut RandomGenerator,
        pos: BlockPos,
    ) -> bool {
        if pos.0.y <= chunk.bottom_y() as i32 + 4 {
            return false;
        }

        let origin = pos.add(-8, -4, -8);
        let mut grid = [false; 2048];
        let spots = random.next_bounded_i32(4) + 4;

        for _ in 0..spots {
            let xr = random.next_f64() * 6.0 + 3.0;
            let yr = random.next_f64() * 4.0 + 2.0;
            let zr = random.next_f64() * 6.0 + 3.0;
            let xp = random.next_f64() * (16.0 - xr - 2.0) + 1.0 + xr / 2.0;
            let yp = random.next_f64() * (8.0 - yr - 4.0) + 2.0 + yr / 2.0;
            let zp = random.next_f64() * (16.0 - zr - 2.0) + 1.0 + zr / 2.0;

            for xx in 1..15 {
                for zz in 1..15 {
                    for yy in 1..7 {
                        let xd = (xx as f64 - xp) / (xr / 2.0);
                        let yd = (yy as f64 - yp) / (yr / 2.0);
                        let zd = (zz as f64 - zp) / (zr / 2.0);
                        let d = xd * xd + yd * yd + zd * zd;
                        if d < 1.0 {
                            grid[(xx * 16 + zz) * 8 + yy] = true;
                        }
                    }
                }
            }
        }

        let fluid_state = self
            .fluid
            .get_with_context(block_registry, chunk, random, origin);

        for xx in 0..16 {
            for zz in 0..16 {
                for yyx in 0..8 {
                    let check = !grid[(xx * 16 + zz) * 8 + yyx]
                        && (xx < 15 && grid[((xx + 1) * 16 + zz) * 8 + yyx]
                            || xx > 0 && grid[((xx - 1) * 16 + zz) * 8 + yyx]
                            || zz < 15 && grid[(xx * 16 + zz + 1) * 8 + yyx]
                            || zz > 0 && grid[(xx * 16 + (zz - 1)) * 8 + yyx]
                            || yyx < 7 && grid[(xx * 16 + zz) * 8 + yyx + 1]
                            || yyx > 0 && grid[(xx * 16 + zz) * 8 + (yyx - 1)]);

                    if check {
                        let check_pos = origin.add(xx as i32, yyx as i32, zz as i32);
                        let block_state =
                            GenerationCache::get_block_state(chunk, &check_pos.0).to_state();

                        if yyx >= 4 && block_state.is_liquid() {
                            return false;
                        }

                        if yyx < 4 && !block_state.is_solid() && block_state != fluid_state {
                            return false;
                        }

                        if !self
                            .can_place_feature
                            .test(block_registry, chunk, &check_pos)
                        {
                            return false;
                        }
                    }
                }
            }
        }

        let air_state = Block::CAVE_AIR.default_state;

        for xx in 0..16 {
            for zz in 0..16 {
                for yyx in 0..8 {
                    if grid[(xx * 16 + zz) * 8 + yyx] {
                        let place_pos = origin.add(xx as i32, yyx as i32, zz as i32);
                        if self.can_replace_with_air_or_fluid.test(
                            block_registry,
                            chunk,
                            &place_pos,
                        ) {
                            let place_air = yyx >= 4;
                            chunk.set_block_state(
                                &place_pos.0,
                                if place_air { air_state } else { fluid_state },
                            );
                            if place_air {
                                chunk.mark_above_for_post_processing(&place_pos.0);
                            }
                        }
                    }
                }
            }
        }

        let barrier_state = self
            .barrier
            .get_with_context(block_registry, chunk, random, origin);
        if !barrier_state.is_air() {
            for xx in 0..16 {
                for zz in 0..16 {
                    for yyx in 0..8 {
                        let check = !grid[(xx * 16 + zz) * 8 + yyx]
                            && (xx < 15 && grid[((xx + 1) * 16 + zz) * 8 + yyx]
                                || xx > 0 && grid[((xx - 1) * 16 + zz) * 8 + yyx]
                                || zz < 15 && grid[(xx * 16 + zz + 1) * 8 + yyx]
                                || zz > 0 && grid[(xx * 16 + (zz - 1)) * 8 + yyx]
                                || yyx < 7 && grid[(xx * 16 + zz) * 8 + yyx + 1]
                                || yyx > 0 && grid[(xx * 16 + zz) * 8 + (yyx - 1)]);

                        if check && (yyx < 4 || random.next_bounded_i32(2) != 0) {
                            let barrier_pos = origin.add(xx as i32, yyx as i32, zz as i32);
                            let block_state =
                                GenerationCache::get_block_state(chunk, &barrier_pos.0).to_state();
                            if block_state.is_solid()
                                && self.can_replace_with_barrier.test(
                                    block_registry,
                                    chunk,
                                    &barrier_pos,
                                )
                            {
                                chunk.set_block_state(&barrier_pos.0, barrier_state);
                                chunk.mark_above_for_post_processing(&barrier_pos.0);
                            }
                        }
                    }
                }
            }
        }

        // Freeze top layer
        if Block::from_state_id(fluid_state.id).id == Block::WATER.id {
            for xx in 0..16 {
                for zz in 0..16 {
                    let freeze_pos = origin.add(xx, 4, zz);
                    let biome = chunk.get_biome_for_terrain_gen(
                        freeze_pos.0.x,
                        freeze_pos.0.y,
                        freeze_pos.0.z,
                    );
                    if biome.weather.compute_temperature(
                        freeze_pos.0.x as f64,
                        freeze_pos.0.y,
                        freeze_pos.0.z as f64,
                        63,
                    ) < 0.15
                        && self.can_replace_with_air_or_fluid.test(
                            block_registry,
                            chunk,
                            &freeze_pos,
                        )
                    {
                        chunk.set_block_state(&freeze_pos.0, Block::ICE.default_state);
                    }
                }
            }
        }

        true
    }
}

#[cfg(test)]
mod tests {
    use pumpkin_data::configured_feature::ConfiguredFeature as FeatureKey;
    use pumpkin_data::{BlockId, BlockState};
    use pumpkin_util::math::vector3::Vector3;
    use pumpkin_util::random::legacy_rand::LegacyRand;

    use super::*;
    use crate::chunk_system::Cache;
    use crate::generation::feature::configured_features::{CONFIGURED_FEATURES, ConfiguredFeature};
    use crate::generation::feature::features::sculk::test_utils::{empty_region, set};
    use crate::generation::feature::placed_features::Feature;
    use crate::generation::proto_chunk_test::stage_parity::TestBlockRegistry;

    /// The middle of the region's centre chunk; the lake fills the 16 by 8 by 16 blocks from
    /// 16 60 16.
    const ORIGIN: BlockPos = BlockPos::new(24, 64, 24);

    /// The lake vanilla's sulfur pool starts with.
    fn sulfur_pool_lake() -> Option<&'static LakeFeature> {
        let ConfiguredFeature::Sequence(pool) = CONFIGURED_FEATURES.get(&FeatureKey::SulfurPool)?
        else {
            return None;
        };
        let Feature::Inlined(lake) = &pool.features.first()?.get()?.feature else {
            return None;
        };
        match lake.as_ref() {
            ConfiguredFeature::Lake(lake) => Some(lake),
            _ => None,
        }
    }

    /// A region whose blocks where the lake goes are all `state`.
    fn region_of(state: &BlockState) -> Cache {
        let mut level = empty_region();
        for x in 16..32 {
            for y in 60..68 {
                for z in 16..32 {
                    set(&mut level, BlockPos::new(x, y, z), state);
                }
            }
        }
        level
    }

    fn count(level: &Cache, block: BlockId) -> usize {
        let mut count = 0;
        for x in 0..48 {
            for y in 56..72 {
                for z in 0..48 {
                    let state = GenerationCache::get_block_state(level, &Vector3::new(x, y, z));
                    if state.to_block_id() == block {
                        count += 1;
                    }
                }
            }
        }
        count
    }

    #[test]
    fn sulfur_pools_keep_away_from_sulfur_spikes() {
        // A sulfur spike counts as solid, so only the pool's own check keeps it from forming.
        let lake = sulfur_pool_lake();
        assert!(lake.is_some(), "the sulfur pool starts with a lake");
        let Some(lake) = lake else {
            return;
        };
        let mut level = region_of(Block::SULFUR_SPIKE.default_state);
        let mut random = RandomGenerator::Legacy(LegacyRand::from_seed(1));

        assert!(!lake.generate(&TestBlockRegistry, &mut level, &mut random, ORIGIN));
        assert_eq!(count(&level, BlockId::WATER), 0);
    }
}
