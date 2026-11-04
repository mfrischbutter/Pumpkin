use std::collections::{HashMap, HashSet};

use pumpkin_data::{
    Block, BlockDirection, BlockId, BlockState, BlockStateId,
    block_properties::WaterLikeProperties, fluid::Fluid,
};
use pumpkin_util::{
    math::{int_provider::IntProvider, position::BlockPos},
    random::{RandomGenerator, RandomImpl, legacy_rand::LegacyRand},
};

use crate::generation::feature::features::spring_feature::BlockWrapper;
use crate::{
    block::BlockStateCodec, generation::proto_chunk::GenerationCache, world::WorldPortalExt,
};

/// Minimal wrapper around the perlin sampler used for geode generation, to avoid exposing the entire noise module to this file.
struct NormalNoise(crate::generation::noise::perlin::DoublePerlinNoiseSampler);

impl NormalNoise {
    /// The shell noise. Vanilla seeds it from the level's seed with a legacy random
    /// (`new WorldgenRandom(new LegacyRandomSource(seed))`), not from the feature's random.
    fn for_seed(seed: u64) -> Self {
        let mut random = RandomGenerator::Legacy(LegacyRand::from_seed(seed));
        Self(
            crate::generation::noise::perlin::DoublePerlinNoiseSampler::new(
                &mut random,
                -4,
                &[1.0],
                false,
            ),
        )
    }

    #[inline]
    fn get_value(&self, x: f64, y: f64, z: f64) -> f64 {
        f64::from(self.0.sample(x, y, z))
    }
}

pub struct GeodeFeature {
    /* Block settings */
    pub filling_provider: crate::generation::block_state_provider::BlockStateProvider,
    pub inner_layer_provider: crate::generation::block_state_provider::BlockStateProvider,
    pub alternate_inner_layer_provider: crate::generation::block_state_provider::BlockStateProvider,
    pub middle_layer_provider: crate::generation::block_state_provider::BlockStateProvider,
    pub outer_layer_provider: crate::generation::block_state_provider::BlockStateProvider,
    pub inner_placements: Vec<BlockStateCodec>,
    pub cannot_replace: BlockWrapper,
    pub invalid_blocks: BlockWrapper,
    /* Layer settings */
    pub filling: f64,
    pub inner_layer: f64,
    pub middle_layer: f64,
    pub outer_layer: f64,
    /* Crack settings */
    pub generate_crack_chance: f64,
    pub base_crack_size: f64,
    pub crack_point_offset: i32,
    /* Other */
    pub use_potential_placements_chance: f64,
    pub use_alternate_layer0_chance: f64,
    pub placements_require_layer0_alternate: bool,
    pub outer_wall_distance: IntProvider,
    pub distribution_points: IntProvider,
    pub point_offset: IntProvider,
    pub min_gen_offset: i32,
    pub max_gen_offset: i32,
    pub noise_multiplier: f64,
    pub invalid_blocks_threshold: i32,
}

/// The raw block ids a holder set names: `#namespace:tag` entries resolve to the tag's blocks.
fn block_ids(blocks: &BlockWrapper) -> HashSet<u16> {
    let names: &[String] = match blocks {
        BlockWrapper::Single(name) => std::slice::from_ref(name),
        BlockWrapper::Multi(names) => names,
    };
    let mut ids = HashSet::new();
    for name in names {
        if let Some(tag) = name.strip_prefix('#') {
            if let Some(tag_ids) =
                pumpkin_data::tag::get_tag_ids(pumpkin_data::tag::RegistryKey::Block, tag)
            {
                ids.extend(tag_ids.iter().copied());
            }
        } else if let Some(block) = Block::from_name(name) {
            ids.insert(block.id.as_u16());
        }
    }
    ids
}

/// Vanilla's `BuddingAmethystBlock.canClusterGrowAtState`: air, or a full water block (a source
/// or falling water).
fn can_cluster_grow_at(state: BlockStateId) -> bool {
    let state_ref = state.to_state();
    if state_ref.is_air() {
        return true;
    }
    if state.to_block_id() != BlockId::WATER {
        return false;
    }
    let level = WaterLikeProperties::from_state_id(state).level;
    level == 0 || level >= 8
}

/// Vanilla's `BlockPos.distSqr`: the squared distance in doubles.
fn dist_sqr(a: BlockPos, b: BlockPos) -> f64 {
    let dx = f64::from(a.0.x - b.0.x);
    let dy = f64::from(a.0.y - b.0.y);
    let dz = f64::from(a.0.z - b.0.z);
    dx * dx + dy * dy + dz * dz
}

const fn direction_name(direction: BlockDirection) -> &'static str {
    match direction {
        BlockDirection::Down => "down",
        BlockDirection::Up => "up",
        BlockDirection::North => "north",
        BlockDirection::South => "south",
        BlockDirection::West => "west",
        BlockDirection::East => "east",
    }
}

/// Whether a water block is a source (`FluidState.isSource`); other blocks hold no water here.
fn is_water_source(state: BlockStateId) -> bool {
    state.to_block_id() == BlockId::WATER && WaterLikeProperties::from_state_id(state).level == 0
}

impl GeodeFeature {
    fn safe_set_block<T: GenerationCache>(
        chunk: &mut T,
        pos: BlockPos,
        state: &'static BlockState,
        cannot_replace: &HashSet<u16>,
    ) {
        let existing = GenerationCache::get_block_state(chunk, &pos.0);
        if !cannot_replace.contains(&existing.to_block_id().as_u16()) {
            chunk.set_block_state(&pos.0, state);
        }
    }

    fn has_property(codec: &BlockStateCodec, property: pumpkin_data::PropertyKey) -> bool {
        let state = codec.get_state();
        // Obtain the block corresponding to this state id
        let block = Block::from_state_id(state.id);
        block
            .properties(state.id)
            .is_some_and(|props| props.to_props().iter().any(|p| p.key() == property))
    }

    /// Vanilla schedules a tick for every fluid next to a crack block, so water and lava flow
    /// into the crack once the chunk ticks.
    fn schedule_adjacent_fluid_ticks<T: GenerationCache>(chunk: &mut T, pos: BlockPos) {
        for direction in BlockDirection::all() {
            let offset = direction.to_offset();
            let adjacent = pos.add(offset.x, offset.y, offset.z);
            let (fluid, _) = chunk.get_fluid_and_fluid_state(&adjacent.0);
            if fluid.id == Fluid::EMPTY.id {
                continue;
            }
            if let Some(fluid) = Fluid::from_id(fluid.id) {
                chunk.schedule_fluid_tick_at(&adjacent.0, fluid, 0);
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    #[allow(clippy::too_many_lines)]
    pub fn generate<T: GenerationCache>(
        &self,
        chunk: &mut T,
        block_registry: &dyn WorldPortalExt,
        _min_y: i8,
        _height: u16,
        _feature_name: pumpkin_data::placed_feature::PlacedFeature,
        random: &mut RandomGenerator,
        pos: BlockPos,
    ) -> bool {
        let origin = pos;
        let num_points = self.distribution_points.get(random);
        let noise = NormalNoise::for_seed(block_registry.feature_seed());
        let invalid_blocks = block_ids(&self.invalid_blocks);
        let cannot_replace = block_ids(&self.cannot_replace);

        let mut points: Vec<(BlockPos, i32)> = Vec::with_capacity(num_points as usize);
        let mut crack_points: Vec<BlockPos> = Vec::new();
        let crack_size_adjustment =
            f64::from(num_points) / f64::from(self.outer_wall_distance.get_max());
        let inner_air = 1.0 / self.filling.sqrt();
        let innermost_block_layer = 1.0 / (self.inner_layer + crack_size_adjustment).sqrt();
        let inner_crust = 1.0 / (self.middle_layer + crack_size_adjustment).sqrt();
        let outer_crust = 1.0 / (self.outer_layer + crack_size_adjustment).sqrt();
        let crack_size = 1.0
            / (self.base_crack_size
                + random.next_f64() / 2.0
                + if num_points > 3 {
                    crack_size_adjustment
                } else {
                    0.0
                })
            .sqrt();
        // Vanilla compares the float roll with the double chance, so the comparison is in f64.
        let should_generate_crack = f64::from(random.next_f32()) < self.generate_crack_chance;
        let mut num_invalid_points = 0;

        // Sample distribution points; bail out early if too many fall into invalid blocks
        for _ in 0..num_points {
            let x = self.outer_wall_distance.get(random);
            let y = self.outer_wall_distance.get(random);
            let z = self.outer_wall_distance.get(random);
            let p = origin.add(x, y, z);
            let raw = GenerationCache::get_block_state(chunk, &p.0);
            if raw.to_state().is_air() || invalid_blocks.contains(&raw.to_block_id().as_u16()) {
                num_invalid_points += 1;
                if num_invalid_points > self.invalid_blocks_threshold {
                    return false;
                }
            }
            let offset = self.point_offset.get(random);
            points.push((p, offset));
        }

        if should_generate_crack {
            let offset_index = random.next_bounded_i32(4);
            let crack_offset = num_points * 2 + 1;
            let (dx, dz) = match offset_index {
                0 => (crack_offset, 0),
                1 => (0, crack_offset),
                2 => (crack_offset, crack_offset),
                _ => (0, 0),
            };
            crack_points.push(origin.add(dx, 7, dz));
            crack_points.push(origin.add(dx, 5, dz));
            crack_points.push(origin.add(dx, 1, dz));
        }

        let mut potential_crystal_placements: Vec<BlockPos> = Vec::new();

        let min = origin.add(
            self.min_gen_offset,
            self.min_gen_offset,
            self.min_gen_offset,
        );
        let max = origin.add(
            self.max_gen_offset,
            self.max_gen_offset,
            self.max_gen_offset,
        );

        for point_inside in BlockPos::iterate(min, max) {
            let noise_offset = noise.get_value(
                f64::from(point_inside.0.x),
                f64::from(point_inside.0.y),
                f64::from(point_inside.0.z),
            ) * self.noise_multiplier;
            let mut dist_sum_shell = 0.0;
            for (pt, off) in &points {
                let dist_sq = dist_sqr(point_inside, *pt);
                dist_sum_shell += 1.0 / (dist_sq + f64::from(*off)).sqrt() + noise_offset;
            }
            if dist_sum_shell < outer_crust {
                continue;
            }
            if dist_sum_shell >= inner_air {
                let state = self
                    .filling_provider
                    .get(random, point_inside, chunk, block_registry);
                Self::safe_set_block(chunk, point_inside, state, &cannot_replace);
                continue;
            }
            let mut dist_sum_crack = 0.0;
            for pt in &crack_points {
                let dist_sq = dist_sqr(point_inside, *pt);
                dist_sum_crack +=
                    1.0 / (dist_sq + f64::from(self.crack_point_offset)).sqrt() + noise_offset;
            }
            if should_generate_crack && dist_sum_crack >= crack_size {
                Self::safe_set_block(
                    chunk,
                    point_inside,
                    BlockStateId::AIR.to_state(),
                    &cannot_replace,
                );
                Self::schedule_adjacent_fluid_ticks(chunk, point_inside);
            } else if dist_sum_shell >= innermost_block_layer {
                let use_alternate = f64::from(random.next_f32()) < self.use_alternate_layer0_chance;
                let provider = if use_alternate {
                    &self.alternate_inner_layer_provider
                } else {
                    &self.inner_layer_provider
                };
                let state = provider.get(random, point_inside, chunk, block_registry);
                Self::safe_set_block(chunk, point_inside, state, &cannot_replace);
                if (!self.placements_require_layer0_alternate || use_alternate)
                    && f64::from(random.next_f32()) < self.use_potential_placements_chance
                {
                    potential_crystal_placements.push(point_inside);
                }
            } else if dist_sum_shell >= inner_crust {
                let state =
                    self.middle_layer_provider
                        .get(random, point_inside, chunk, block_registry);
                Self::safe_set_block(chunk, point_inside, state, &cannot_replace);
            } else {
                let state =
                    self.outer_layer_provider
                        .get(random, point_inside, chunk, block_registry);
                Self::safe_set_block(chunk, point_inside, state, &cannot_replace);
            }
        }

        for crystal_pos in potential_crystal_placements {
            let Some(base_codec) = self
                .inner_placements
                .get(random.next_bounded_i32(self.inner_placements.len() as i32) as usize)
            else {
                continue;
            };
            for dir in BlockDirection::all() {
                let offset = dir.to_offset();
                let place_pos = crystal_pos.add(offset.x, offset.y, offset.z);
                let place_raw = GenerationCache::get_block_state(chunk, &place_pos.0);
                if !can_cluster_grow_at(place_raw) {
                    continue;
                }
                let mut final_codec = base_codec.clone();
                if Self::has_property(&final_codec, pumpkin_data::PropertyKey::Facing) {
                    final_codec
                        .properties
                        .get_or_insert_with(HashMap::new)
                        .insert("facing".to_string(), direction_name(dir).to_string());
                }
                if Self::has_property(&final_codec, pumpkin_data::PropertyKey::Waterlogged) {
                    final_codec
                        .properties
                        .get_or_insert_with(HashMap::new)
                        .insert(
                            "waterlogged".to_string(),
                            is_water_source(place_raw).to_string(),
                        );
                }
                let final_state = final_codec.get_state();
                Self::safe_set_block(chunk, place_pos, final_state, &cannot_replace);
                break;
            }
        }

        true
    }
}
