//! Vanilla's structure references (`ChunkGenerator.createReferences`).
//!
//! The structure starts whose bounding box reaches into a chunk. The proto chunk records
//! them at its `structure_references` stage; a loaded chunk recomputes them from the
//! placement when natural spawning asks which structure a position lies in.

use std::sync::{Arc, Mutex};

use pumpkin_data::structures::{
    Structure, StructureKeys, StructurePlacementType, StructureSet, WeightedEntry,
};
use pumpkin_util::math::floor_div;
use pumpkin_util::random::{RandomImpl, get_large_feature_seed, legacy_rand::LegacyRand};

use crate::biome::BiomeSupplier;
use crate::generation::blender::{Blender, BlenderImpl};
use crate::generation::generator::VanillaGenerator;
use crate::generation::noise::router::multi_noise_sampler::MultiNoiseSampler;
use crate::generation::positions::chunk_pos;
use crate::generation::structure::height_sampler::NoiseHeightSampler;
use crate::generation::structure::lazily_generate_structure;
use crate::generation::structure::placement::{
    get_structure_chunk_in_region, should_generate_structure,
};
use crate::generation::structure::structures::ocean_monument;
use crate::generation::structure::structures::{
    HeightSampler, StructureGeneratorContext, StructurePiecesCollector, StructurePosition,
    create_chunk_random,
};

/// A structure start a chunk references: the key, the start chunk and the pieces.
pub type StructureReference = (
    StructureKeys,
    (i32, i32),
    Arc<Mutex<StructurePiecesCollector>>,
);

/// A start this many chunks away can still reach into a chunk (vanilla searches the
/// 17 x 17 chunks around it).
const REFERENCE_RANGE_CHUNKS: i32 = 8;

/// The structure starts whose adjusted bounding box reaches into the chunk
/// (`chunk_x`, `chunk_z`).
///
/// Only the structure sets `set_indices` (indices of [`StructureSet::ALL`]) are searched;
/// `min_y` and `height` bound the structures like the chunk's generation context does.
pub fn structure_references(
    generator: &VanillaGenerator,
    chunk_x: i32,
    chunk_z: i32,
    min_y: i32,
    height: u16,
    set_indices: impl IntoIterator<Item = usize>,
) -> Vec<StructureReference> {
    let random_config = &generator.random_config;
    let settings = generator.settings;
    let global_cache = &generator.global_structure_cache;

    let start_x = chunk_pos::start_block_x(chunk_x);
    let start_z = chunk_pos::start_block_z(chunk_z);
    let end_x = start_x + 15;
    let end_z = start_z + 15;

    let seed = random_config.structure_seed as i64;

    let base_supplier: &dyn BiomeSupplier = &generator.biome_supplier;
    let blender = Blender::empty();
    let biome_supplier = blender.get_biome_supplier(base_supplier);
    let mut multi_noise_sampler = MultiNoiseSampler::generate(&generator.base_router.multi_noise);
    let mut height_sampler = NoiseHeightSampler::new(generator);

    let mut references = Vec::new();
    for set_index in set_indices {
        let set = &StructureSet::ALL[set_index];
        for (candidate_chunk_x, candidate_chunk_z) in
            candidate_start_chunks(generator, set_index, chunk_x, chunk_z)
        {
            let entries = if set.structures.len() == 1 {
                set.structures.to_vec()
            } else {
                weighted_structure_order(
                    set.structures,
                    random_config.structure_seed,
                    candidate_chunk_x,
                    candidate_chunk_z,
                )
            };

            for entry in entries {
                let structure = Structure::get(&entry.structure);

                // A structure's placement depends only on its start chunk and the world
                // seed, so cache it: otherwise every surrounding chunk whose references
                // overlap it would re-run the (expensive) jigsaw expansion. `context` is
                // only built on a cache miss.
                let start_data = global_cache.get_or_compute_structure_start(
                    entry.structure,
                    candidate_chunk_x,
                    candidate_chunk_z,
                    || {
                        let context = StructureGeneratorContext {
                            seed,
                            chunk_x: candidate_chunk_x,
                            chunk_z: candidate_chunk_z,
                            random: create_chunk_random(seed, candidate_chunk_x, candidate_chunk_z),
                            sea_level: settings.sea_level,
                            min_y,
                            height,
                            height_sampler: Some(&mut height_sampler),
                            structure_key: Some(entry.structure),
                        };
                        lazily_generate_structure(
                            &entry.structure,
                            structure,
                            context,
                            &biome_supplier,
                            &mut multi_noise_sampler,
                        )
                    },
                );

                if let Some(start_data) = start_data {
                    if start_data
                        .get_adjusted_bounding_box(&entry.structure)
                        .intersects_raw_xz(start_x, start_z, end_x, end_z)
                    {
                        references.push((
                            entry.structure,
                            (candidate_chunk_x, candidate_chunk_z),
                            start_data.collector.clone(),
                        ));
                    }
                    break;
                }
            }
        }
    }
    references
}

/// The order in which vanilla visits a chunk's starts of one structure: indices into
/// `start_chunks`, the start chunks (x, z) of those starts.
///
/// Vanilla `StructureManager.startsForStructure` walks the chunk's reference set of the
/// structure, a fastutil `LongOpenHashSet` of packed start chunks that
/// `ChunkGenerator.createReferences` filled with x as the outer and z as the inner loop,
/// both ascending. Its hash order decides which start draws from the shared random first.
#[must_use]
pub fn vanilla_reference_order(start_chunks: &[(i32, i32)]) -> Vec<usize> {
    debug_assert!(
        start_chunks
            .iter()
            .enumerate()
            .all(|(index, chunk)| !start_chunks[..index].contains(chunk)),
        "a structure has at most one start per chunk"
    );
    let mut insertion_order: Vec<usize> = (0..start_chunks.len()).collect();
    insertion_order.sort_by_key(|&index| start_chunks[index]);

    let mut set = FastutilLongSet::new();
    for &index in &insertion_order {
        set.add(pack_chunk_pos(start_chunks[index]));
    }
    set.iter()
        .filter_map(|key| {
            start_chunks
                .iter()
                .position(|&chunk| pack_chunk_pos(chunk) == key)
        })
        .collect()
}

/// Vanilla `ChunkPos.pack`: x in the low and z in the high 32 bits.
const fn pack_chunk_pos((x, z): (i32, i32)) -> u64 {
    (x as u32 as u64) | ((z as u32 as u64) << 32)
}

/// fastutil `HashCommon.LONG_PHI`, the golden ratio multiplier of `HashCommon.mix`.
const LONG_PHI: u64 = 0x9E37_79B9_7F4A_7C15;
/// The table size of fastutil's default `LongOpenHashSet()`: `Hash.DEFAULT_INITIAL_SIZE`
/// (16) at `Hash.DEFAULT_LOAD_FACTOR`.
const DEFAULT_TABLE_SIZE: usize = 32;
/// fastutil `Hash.DEFAULT_LOAD_FACTOR` (0.75) as a fraction, which keeps the ceilings exact.
const LOAD_FACTOR_NUMERATOR: usize = 3;
const LOAD_FACTOR_DENOMINATOR: usize = 4;

/// The insertion and iteration order of fastutil's `LongOpenHashSet` (open addressing with
/// linear probing; 0 marks an empty slot, so the key 0 is kept aside).
struct FastutilLongSet {
    slots: Vec<u64>,
    contains_null: bool,
    size: usize,
    max_fill: usize,
}

impl FastutilLongSet {
    fn new() -> Self {
        Self {
            slots: vec![0; DEFAULT_TABLE_SIZE],
            contains_null: false,
            size: 0,
            max_fill: Self::max_fill(DEFAULT_TABLE_SIZE),
        }
    }

    /// fastutil `HashCommon.maxFill`: `min(ceil(n * f), n - 1)`.
    const fn max_fill(table_size: usize) -> usize {
        let filled = (table_size * LOAD_FACTOR_NUMERATOR).div_ceil(LOAD_FACTOR_DENOMINATOR);
        if filled < table_size - 1 {
            filled
        } else {
            table_size - 1
        }
    }

    /// fastutil `HashCommon.arraySize`: `max(2, nextPowerOfTwo(ceil(expected / f)))`.
    fn array_size(expected: usize) -> usize {
        (expected * LOAD_FACTOR_DENOMINATOR)
            .div_ceil(LOAD_FACTOR_NUMERATOR)
            .next_power_of_two()
            .max(2)
    }

    /// fastutil `HashCommon.mix`, cut to the start slot like `(int) mix(k) & mask`.
    const fn start_slot(key: u64, mask: usize) -> usize {
        let mut hash = key.wrapping_mul(LONG_PHI);
        hash ^= hash >> 32;
        hash ^= hash >> 16;
        (hash as u32 as usize) & mask
    }

    /// Stores `key` (not 0) in the first empty slot from its start slot on.
    const fn insert_into(slots: &mut [u64], key: u64) {
        let mask = slots.len() - 1;
        let mut pos = Self::start_slot(key, mask);
        while slots[pos] != 0 {
            pos = (pos + 1) & mask;
        }
        slots[pos] = key;
    }

    /// `LongOpenHashSet.add` for a key not in the set yet.
    fn add(&mut self, key: u64) {
        if key == 0 {
            self.contains_null = true;
        } else {
            Self::insert_into(&mut self.slots, key);
        }
        let old_size = self.size;
        self.size += 1;
        if old_size >= self.max_fill {
            self.rehash(Self::array_size(self.size + 1));
        }
    }

    /// `LongOpenHashSet.rehash`: the old table's keys from the last slot down into a new
    /// table of `table_size` slots.
    fn rehash(&mut self, table_size: usize) {
        let mut slots = vec![0; table_size];
        for &key in self.slots.iter().rev().filter(|&&key| key != 0) {
            Self::insert_into(&mut slots, key);
        }
        self.slots = slots;
        self.max_fill = Self::max_fill(table_size);
    }

    /// The iterator's order: the key 0 first, then the slots from the last down.
    fn iter(&self) -> impl Iterator<Item = u64> + '_ {
        self.contains_null
            .then_some(0)
            .into_iter()
            .chain(self.slots.iter().rev().copied().filter(|&key| key != 0))
    }
}

/// The start chunks of structure set `set_index` within reach of the chunk
/// (`chunk_x`, `chunk_z`) that pass the set's placement test.
fn candidate_start_chunks(
    generator: &VanillaGenerator,
    set_index: usize,
    chunk_x: i32,
    chunk_z: i32,
) -> Vec<(i32, i32)> {
    let set = &StructureSet::ALL[set_index];
    let global_cache = &generator.global_structure_cache;
    let in_range = |(candidate_x, candidate_z): &(i32, i32)| {
        (candidate_x - chunk_x).abs() <= REFERENCE_RANGE_CHUNKS
            && (candidate_z - chunk_z).abs() <= REFERENCE_RANGE_CHUNKS
    };
    let candidates: Vec<(i32, i32)> = match &set.placement.placement_type {
        StructurePlacementType::RandomSpread(spread) => {
            let seed = generator.random_config.structure_seed as i64;
            let min_rx = floor_div(chunk_x - REFERENCE_RANGE_CHUNKS, spread.spacing);
            let max_rx = floor_div(chunk_x + REFERENCE_RANGE_CHUNKS, spread.spacing);
            let min_rz = floor_div(chunk_z - REFERENCE_RANGE_CHUNKS, spread.spacing);
            let max_rz = floor_div(chunk_z + REFERENCE_RANGE_CHUNKS, spread.spacing);
            (min_rx..=max_rx)
                .flat_map(|rx| (min_rz..=max_rz).map(move |rz| (rx, rz)))
                .map(|(rx, rz)| {
                    get_structure_chunk_in_region(
                        spread,
                        seed,
                        rx,
                        rz,
                        generator.structure_salts[set_index],
                    )
                })
                .collect()
        }
        StructurePlacementType::ConcentricRings(_) => global_cache
            .get_stronghold_chunks()
            .iter()
            .copied()
            .filter(in_range)
            .collect(),
    };
    candidates
        .into_iter()
        .filter(|&(candidate_x, candidate_z)| {
            should_generate_structure(
                set_index,
                &generator.structure_calculator,
                &generator.structure_salts,
                candidate_x,
                candidate_z,
                global_cache,
            )
        })
        .filter(in_range)
        .collect()
}

/// The order in which a structure set's structures try to claim a start chunk: vanilla
/// draws them by weight with the chunk's large feature seed.
#[must_use]
pub fn weighted_structure_order(
    entries: &[WeightedEntry],
    seed: u64,
    chunk_x: i32,
    chunk_z: i32,
) -> Vec<WeightedEntry> {
    let mut candidates = entries.to_vec();
    let large_feature_seed = get_large_feature_seed(seed, chunk_x, chunk_z);
    let mut random = LegacyRand::from_seed(large_feature_seed);
    let mut total_weight: u32 = candidates.iter().map(|entry| entry.weight).sum();
    let mut ordered = Vec::with_capacity(candidates.len());

    while !candidates.is_empty() {
        let mut roll = random.next_bounded_i32(total_weight as i32);
        let mut selected_idx = 0;

        for (index, entry) in candidates.iter().enumerate() {
            roll -= entry.weight as i32;
            if roll < 0 {
                selected_idx = index;
                break;
            }
        }

        let selected = candidates.remove(selected_idx);
        total_weight -= selected.weight;
        ordered.push(selected);
    }

    ordered
}

/// Vanilla `ChunkGenerator.tryGenerateStructure`: the start `entry` places in the chunk,
/// `None` where the structure refuses it.
///
/// The start lands in the generator's start cache, so chunk generation and the structure
/// search resolve a chunk once and alike.
pub fn structure_start(
    generator: &VanillaGenerator,
    entry: &WeightedEntry,
    chunk_x: i32,
    chunk_z: i32,
    height_sampler: &mut dyn HeightSampler,
) -> Option<StructurePosition> {
    let sea_level = generator.settings.sea_level;
    if entry.structure == StructureKeys::Monument {
        let mut sampler = MultiNoiseSampler::generate(&generator.base_router.multi_noise);
        let center_x = chunk_pos::get_center_x(chunk_x);
        let center_z = chunk_pos::get_center_z(chunk_z);
        let start_y = height_sampler.estimate_ocean_floor_height(center_x, center_z);
        if !ocean_monument::has_valid_biomes(
            &generator.biome_supplier,
            &mut sampler,
            chunk_x,
            chunk_z,
            sea_level,
            start_y,
        ) {
            return None;
        }
    }
    // Vanilla `WorldGenerationContext`: the generator's shape clipped to the dimension.
    let dimension = &generator.dimension;
    let shape = generator.settings.shape.trim_height(
        dimension.min_y as i8,
        (dimension.min_y + dimension.height) as u16,
    );
    generator
        .global_structure_cache
        .get_or_compute_structure_start(entry.structure, chunk_x, chunk_z, || {
            let seed = generator.random_config.structure_seed as i64;
            let context = StructureGeneratorContext {
                seed,
                chunk_x,
                chunk_z,
                random: create_chunk_random(seed, chunk_x, chunk_z),
                sea_level,
                min_y: i32::from(shape.min_y).max(dimension.min_y),
                height: shape.height.min(dimension.height as u16),
                height_sampler: Some(height_sampler),
                structure_key: Some(entry.structure),
            };
            let mut sampler = MultiNoiseSampler::generate(&generator.base_router.multi_noise);
            lazily_generate_structure(
                &entry.structure,
                Structure::get(&entry.structure),
                context,
                &generator.biome_supplier,
                &mut sampler,
            )
        })
}

/// Vanilla `StructureCheck` for a chunk a structure set places in: the set's structure
/// that claims it.
///
/// That is the first of the weighted order with a valid start, the way chunk generation
/// decides it, so a search never names a structure another one of the set displaced.
pub fn claiming_structure_start(
    generator: &VanillaGenerator,
    set: &StructureSet,
    chunk_x: i32,
    chunk_z: i32,
    height_sampler: &mut dyn HeightSampler,
) -> Option<(StructureKeys, StructurePosition)> {
    let seed = generator.random_config.structure_seed;
    weighted_structure_order(set.structures, seed, chunk_x, chunk_z)
        .iter()
        .find_map(|entry| {
            structure_start(generator, entry, chunk_x, chunk_z, height_sampler)
                .map(|start| (entry.structure, start))
        })
}

#[cfg(test)]
mod tests {
    use super::vanilla_reference_order;

    /// The start chunks in the order `vanilla_reference_order` visits them.
    fn ordered(start_chunks: &[(i32, i32)]) -> Vec<(i32, i32)> {
        vanilla_reference_order(start_chunks)
            .into_iter()
            .map(|index| start_chunks[index])
            .collect()
    }

    // The expected orders a-d are reference arrays vanilla 26.3 saved for
    // `minecraft:mineshaft` (seed -4172144997902289642).

    #[test]
    fn two_starts_in_one_column() {
        assert_eq!(ordered(&[(-32, -30), (-32, -21)]), [(-32, -30), (-32, -21)]);
    }

    #[test]
    fn two_starts_against_insertion_order() {
        assert_eq!(ordered(&[(-36, 0), (-32, 7)]), [(-32, 7), (-36, 0)]);
    }

    #[test]
    fn three_starts_scrambled() {
        assert_eq!(
            ordered(&[(16, 22), (9, 27), (13, 27)]),
            [(13, 27), (9, 27), (16, 22)]
        );
    }

    #[test]
    fn three_starts_reversed() {
        assert_eq!(
            ordered(&[(-31, 13), (-33, 9), (-32, 7)]),
            [(-31, 13), (-32, 7), (-33, 9)]
        );
    }

    #[test]
    fn origin_chunk_comes_first() {
        assert_eq!(
            ordered(&[(4, -1), (0, 0), (-3, 2)]),
            [(0, 0), (4, -1), (-3, 2)]
        );
    }
}
