#![allow(dead_code)]

mod biome;
pub mod blender;
pub mod block_predicate;
pub mod block_state_provider;
pub mod carver;
pub mod feature;
mod feature_order;
pub mod generator;
pub mod height_limit;
pub mod height_provider;
pub mod noise;
pub mod positions;
pub mod proto_chunk;
pub mod proto_chunk_test;
pub mod rule;
pub mod secret;
pub mod structure;
mod surface;

use generator::VanillaGenerator;
use pumpkin_data::dimension::Dimension;
use pumpkin_util::{
    random::xoroshiro128::{Xoroshiro, XoroshiroSplitter},
    world_seed::Seed,
};
use secret::{GenerationSecret, part};

#[must_use]
pub fn get_world_gen(
    seed: Seed,
    dimension: Dimension,
    is_flat: bool,
    flat_layers: Vec<generator::FlatLayer>,
    flat_biome: String,
) -> Box<generator::WorldGenerator> {
    get_world_gen_with_settings(seed, dimension, is_flat, flat_layers, flat_biome, None)
}

#[must_use]
pub fn get_world_gen_with_settings(
    seed: Seed,
    dimension: Dimension,
    is_flat: bool,
    flat_layers: Vec<generator::FlatLayer>,
    flat_biome: String,
    generator_settings: Option<&str>,
) -> Box<generator::WorldGenerator> {
    get_world_gen_with_all_settings(
        seed,
        dimension,
        is_flat,
        flat_layers,
        flat_biome,
        generator_settings,
        None,
        None,
        GenerationSecret::NONE,
    )
}

#[expect(clippy::too_many_arguments)]
#[must_use]
pub fn get_world_gen_with_all_settings(
    seed: Seed,
    dimension: Dimension,
    is_flat: bool,
    flat_layers: Vec<generator::FlatLayer>,
    flat_biome: String,
    generator_settings: Option<&str>,
    biome_source: Option<&crate::world_info::BiomeSource>,
    structure_overrides: Option<&[String]>,
    secret: GenerationSecret,
) -> Box<generator::WorldGenerator> {
    if is_flat {
        Box::new(generator::WorldGenerator::Flat(Box::new(
            generator::flat::FlatGenerator::new(seed, dimension, flat_layers, flat_biome),
        )))
    } else {
        Box::new(generator::WorldGenerator::Noise(Box::new(
            VanillaGenerator::new_with_all_settings(
                seed,
                dimension,
                generator_settings,
                biome_source,
                structure_overrides,
                secret,
            ),
        )))
    }
}

pub struct GlobalRandomConfig {
    /// The world seed: terrain, biome, aquifer and ore vein noise.
    pub seed: u64,
    pub legacy_random_source: bool,
    pub base_random_deriver: XoroshiroSplitter,
    aquifer_random_deriver: XoroshiroSplitter,
    pub ore_random_deriver: XoroshiroSplitter,
    /// The seeds the generation secret ([`secret`]) derives for the parts a
    /// client could recover the seed from. Without a secret every one of them
    /// is the world seed, as in vanilla.
    pub structure_seed: u64,
    pub stronghold_seed: u64,
    pub feature_seed: u64,
    pub carver_seed: u64,
    pub surface_seed: u64,
    pub surface_random_deriver: XoroshiroSplitter,
}

impl GlobalRandomConfig {
    #[must_use]
    pub fn new(seed: u64, legacy_random_source: bool) -> Self {
        Self::with_secret(seed, legacy_random_source, GenerationSecret::NONE)
    }

    #[must_use]
    pub fn with_secret(seed: u64, legacy_random_source: bool, secret: GenerationSecret) -> Self {
        let random_deriver = Xoroshiro::from_seed(seed).next_splitter();

        let aquifer_deriver = random_deriver
            .split_string("minecraft:aquifer")
            .next_splitter();
        let ore_deriver = random_deriver.split_string("minecraft:ore").next_splitter();
        let surface_seed = secret.seed_for(part::SURFACE, seed);
        Self {
            seed,
            legacy_random_source,
            base_random_deriver: random_deriver,
            aquifer_random_deriver: aquifer_deriver,
            ore_random_deriver: ore_deriver,
            structure_seed: secret.seed_for(part::STRUCTURES, seed),
            stronghold_seed: secret.seed_for(part::STRONGHOLDS, seed),
            feature_seed: secret.seed_for(part::FEATURES, seed),
            carver_seed: secret.seed_for(part::CARVERS, seed),
            surface_seed,
            surface_random_deriver: Xoroshiro::from_seed(surface_seed).next_splitter(),
        }
    }

    #[must_use]
    pub const fn seed(&self) -> u64 {
        self.seed
    }
}

pub mod section_coords {
    #[inline]
    #[must_use]
    pub const fn block_to_section(coord: i32) -> i32 {
        coord >> 4
    }

    #[must_use]
    pub const fn get_offset_pos(chunk_coord: i32, offset: i32) -> i32 {
        section_to_block(chunk_coord) + offset
    }

    #[inline]
    #[must_use]
    pub const fn section_to_block(coord: i32) -> i32 {
        coord << 4
    }
}

pub mod biome_coords {
    #[inline]
    #[must_use]
    pub const fn from_block(coord: i32) -> i32 {
        coord >> 2
    }

    #[inline]
    #[must_use]
    pub const fn to_block(coord: i32) -> i32 {
        coord << 2
    }

    #[inline]
    #[must_use]
    pub const fn from_chunk(coord: i32) -> i32 {
        coord << 2
    }

    #[inline]
    #[must_use]
    pub const fn to_chunk(coord: i32) -> i32 {
        coord >> 2
    }
}

#[derive(PartialEq, Eq)]
pub enum Direction {
    North,
    NorthEast,
    East,
    SouthEast,
    South,
    SouthWest,
    West,
    NorthWest,
}
