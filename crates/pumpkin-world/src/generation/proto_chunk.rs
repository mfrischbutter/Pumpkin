use std::sync::{Arc, LazyLock, Mutex};

use pumpkin_data::block_properties::is_air;
use pumpkin_data::chunk::DoublePerlinNoiseParameters;
use pumpkin_data::fluid::{Fluid, FluidState};
use pumpkin_data::structures::{Structure, StructureKeys, StructureSet};
use pumpkin_data::tag::RegistryKey;
use pumpkin_data::{Block, BlockState, block_properties::blocks_movement, chunk::Biome};
use pumpkin_data::{BlockId, BlockStateId, tag};
use pumpkin_util::random::xoroshiro128::XoroshiroSplitter;
use pumpkin_util::{
    HeightMap,
    math::{block_box::BlockBox, position::BlockPos, vector3::Vector3},
    random::{RandomGenerator, get_decorator_seed, worldgen_random::WorldgenRandom},
};
use rustc_hash::FxHashMap;

use super::{
    GlobalRandomConfig, biome_coords,
    feature::placed_features::PLACED_FEATURES,
    feature_order::select_features,
    noise::router::{
        multi_noise_sampler::MultiNoiseSampler, proto_noise_router::DoublePerlinNoiseBuilder,
        surface_height_sampler::SurfaceHeightEstimateSampler,
    },
    positions::chunk_pos::{start_block_x, start_block_z},
    section_coords,
    surface::{
        MaterialRuleContext, VeinContext, estimate_surface_height, terrain::SurfaceTerrainBuilder,
    },
};
use crate::biome::BiomeSupplier;
use crate::chunk::format::LightContainer;
use crate::chunk::palette::BlockPalette;
use crate::chunk::{ChunkData, ChunkHeightmapType, ChunkLight};
use crate::chunk_system::{StagedChunkEnum, generation_cache::SurfaceBiomeNeighborhood};
use crate::generation::height_limit::HeightLimitView;
use crate::generation::noise::aquifer_sampler::{FluidLevel, FluidLevelSamplerImpl};
use crate::generation::noise::perlin::DoublePerlinNoiseSampler;
use crate::generation::noise::router::{
    chunk_noise_router::ChunkVeinRouter,
    density_volume::{DensityBuffer, DensityVolume},
    surface_height_sampler::SurfaceHeightSamplerBuilderOptions,
};
use crate::generation::noise::{CHUNK_DIM, ChunkNoiseGenerator, LAVA_BLOCK, WATER_BLOCK};
use crate::generation::section_coords::section_to_block;
use crate::generation::structure::placement::should_generate_structure;
use crate::generation::structure::references::{claiming_structure_start, structure_references};
use crate::generation::structure::structures::{StructureInstance, StructurePiecesCollector};
use crate::generation::surface::rule::try_apply_material_rule;
use crate::{
    chunk::CHUNK_AREA,
    generation::{biome, positions::chunk_pos},
    world::{BlockAccessor, WorldPortalExt},
};
use pumpkin_data::tag::get_tag_ids;
use pumpkin_nbt::compound::NbtCompound;

use crate::generation::structure::template::BlockPlacer;
use crate::tick::{ScheduledTick, TickPriority};

pub trait GenerationCache: HeightLimitView + BlockAccessor {
    fn get_center_chunk_mut(&mut self) -> &mut ProtoChunk;
    fn get_center_chunk(&self) -> &ProtoChunk;

    fn get_chunk_mut(&mut self, chunk_x: i32, chunk_z: i32) -> Option<&mut ProtoChunk>;
    fn get_chunk(&self, chunk_x: i32, chunk_z: i32) -> Option<&ProtoChunk>;

    fn try_get_proto_chunk(&self, chunk_x: i32, chunk_z: i32) -> Option<&ProtoChunk>;

    fn get_block_state(&self, pos: &Vector3<i32>) -> BlockStateId;
    fn get_fluid_and_fluid_state(&self, position: &Vector3<i32>) -> (Fluid, FluidState);
    /// Sets the block and nothing else (vanilla's ore veins write this way, through
    /// `BulkSectionAccess`).
    fn set_block_state_raw(&mut self, pos: &Vector3<i32>, block_state: &BlockState);
    /// Vanilla's `WorldGenRegion.setBlock`: sets the block and marks the position the block asks
    /// post-processing for (`BlockBehaviour.getPostProcessPos`).
    fn set_block_state(&mut self, pos: &Vector3<i32>, block_state: &BlockState) {
        self.set_block_state_raw(pos, block_state);
        if let Some(nbt) =
            crate::generation::structure::template::dummy_block_entity(block_state, pos)
        {
            self.add_block_entity(pos, nbt);
        }
        if let Some(mark) = post_process_pos(block_state, pos) {
            self.mark_pos_for_post_processing(&mark);
        }
    }
    /// Marks a block of a chunk that is still generating for post-processing (vanilla
    /// `ChunkAccess.markPosForPostProcessing`); a full chunk ignores it, as in vanilla.
    fn mark_pos_for_post_processing(&mut self, pos: &Vector3<i32>) {
        if let Some(chunk) = self.get_chunk_mut(pos.x >> 4, pos.z >> 4) {
            chunk.mark_pos_for_post_processing(pos.x, pos.y, pos.z);
        }
    }
    /// Vanilla's `WorldGenRegion.scheduleTick` for a fluid: the tick waits in the chunk of the
    /// position until that chunk ticks, then runs `delay` ticks later.
    fn schedule_fluid_tick_at(&mut self, pos: &Vector3<i32>, fluid: &'static Fluid, delay: u8) {
        if let Some(chunk) = self.get_chunk_mut(pos.x >> 4, pos.z >> 4) {
            chunk.schedule_fluid_tick(pos.x, pos.y, pos.z, fluid, delay);
        }
    }
    /// [`Self::schedule_fluid_tick_at`] for a block.
    fn schedule_block_tick_at(&mut self, pos: &Vector3<i32>, block: &'static Block, delay: u8) {
        if let Some(chunk) = self.get_chunk_mut(pos.x >> 4, pos.z >> 4) {
            chunk.schedule_block_tick(pos.x, pos.y, pos.z, block, delay);
        }
    }
    /// Vanilla's `Feature.markAboveForPostProcessing`: the up to two blocks above that are not
    /// air, so what stands on a replaced block checks it can stay.
    fn mark_above_for_post_processing(&mut self, pos: &Vector3<i32>) {
        for dy in 1..=2 {
            let above = Vector3::new(pos.x, pos.y + dy, pos.z);
            if GenerationCache::get_block_state(self, &above)
                .to_state()
                .is_air()
            {
                return;
            }
            self.mark_pos_for_post_processing(&above);
        }
    }
    fn add_block_entity(&mut self, pos: &Vector3<i32>, nbt: NbtCompound);
    fn top_motion_blocking_block_height_exclusive(&self, x: i32, z: i32) -> i32;
    fn top_motion_blocking_block_no_leaves_height_exclusive(&self, x: i32, z: i32) -> i32;
    fn get_top_y(&self, heightmap: &HeightMap, x: i32, z: i32) -> i32;
    fn top_block_height_exclusive(&self, x: i32, z: i32) -> i32;
    fn ocean_floor_height_exclusive(&self, x: i32, z: i32) -> i32;
    fn top_block_wg_height_exclusive(&self, x: i32, z: i32) -> i32 {
        self.get_top_y(&HeightMap::WorldSurfaceWg, x, z)
    }
    fn ocean_floor_wg_height_exclusive(&self, x: i32, z: i32) -> i32 {
        self.get_top_y(&HeightMap::OceanFloorWg, x, z)
    }
    fn is_air(&self, local_pos: &Vector3<i32>) -> bool;
    fn get_biome_for_terrain_gen(&self, x: i32, y: i32, z: i32) -> &'static Biome;
    fn get_blending_data(
        &self,
        chunk_x: i32,
        chunk_z: i32,
    ) -> Option<&crate::generation::blender::blending_data::BlendingData>;
    fn get_sea_level(&self) -> i32 {
        self.get_center_chunk().get_sea_level()
    }
}

const AIR_BLOCK: Block = Block::AIR;

/// Vanilla's `BlockBehaviour.getPostProcessPos`: a mushroom checks it can stay where it was put,
/// magma and soul sand mark the block above, where a bubble column may start.
#[must_use]
pub const fn post_process_pos(state: &BlockState, pos: &Vector3<i32>) -> Option<Vector3<i32>> {
    match state.id.to_block_id() {
        pumpkin_data::BlockId::BROWN_MUSHROOM | pumpkin_data::BlockId::RED_MUSHROOM => Some(*pos),
        pumpkin_data::BlockId::MAGMA_BLOCK | pumpkin_data::BlockId::SOUL_SAND => {
            Some(Vector3::new(pos.x, pos.y + 1, pos.z))
        }
        _ => None,
    }
}

pub struct StandardChunkFluidLevelSampler {
    top_fluid: FluidLevel,
    bottom_fluid: FluidLevel,
    bottom_y: i32,
}

impl StandardChunkFluidLevelSampler {
    #[must_use]
    pub fn new(top_fluid: FluidLevel, bottom_fluid: FluidLevel) -> Self {
        let bottom_y = top_fluid
            .max_y_exclusive()
            .min(bottom_fluid.max_y_exclusive());
        Self {
            top_fluid,
            bottom_fluid,
            bottom_y,
        }
    }
}

impl FluidLevelSamplerImpl for StandardChunkFluidLevelSampler {
    fn get_fluid_level(&self, _x: i32, y: i32, _z: i32) -> &FluidLevel {
        if y < self.bottom_y {
            &self.bottom_fluid
        } else {
            &self.top_fluid
        }
    }
}

pub struct ProtoChunk {
    pub x: i32,
    pub z: i32,
    pub default_block: &'static BlockState,
    biome_mixer_seed: i64,
    pub(crate) flat_block_map: Vec<BlockStateId>,
    pub flat_biome_map: Box<[u8]>,
    pub flat_surface_height_map: [i16; CHUNK_AREA],
    pub flat_ocean_floor_height_map: [i16; CHUNK_AREA],
    pub flat_surface_wg_height_map: [i16; CHUNK_AREA],
    pub flat_ocean_floor_wg_height_map: [i16; CHUNK_AREA],
    pub flat_motion_blocking_height_map: [i16; CHUNK_AREA],
    pub flat_motion_blocking_no_leaves_height_map: [i16; CHUNK_AREA],
    pub(crate) structure_starts: FxHashMap<StructureKeys, StructureInstance>,
    pub(crate) structure_references: Vec<(StructureKeys, Arc<Mutex<StructurePiecesCollector>>)>,

    height: u16,
    bottom_y: i8,
    generation_height: u16,
    generation_bottom_y: i8,
    sea_level: i32,
    pub stage: StagedChunkEnum,
    pub light: ChunkLight,
    pub blending_data: Option<crate::generation::blender::blending_data::BlendingData>,
    pub pending_block_entities: Vec<NbtCompound>,
    pending_structure_entities: Vec<StructureEntity>,
    pub fluid_ticks: Vec<ScheduledTick<&'static Fluid>>,
    pub block_ticks: Vec<ScheduledTick<&'static Block>>,
    /// Blocks marked for post-processing, per section from the bottom, packed like vanilla
    /// (`x | y << 4 | z << 8` within the section).
    pub post_processing: Vec<Vec<u16>>,
    pub(crate) surface_height_cache: FxHashMap<u64, i32>,
    pub(crate) aquifer_sampler:
        Option<Box<crate::generation::noise::aquifer_sampler::WorldAquiferSampler>>,
    /// Vanilla's `WorldGenRegion` random (`level.getRandom()`) while the chunk's structures and
    /// features are placed.
    pub region_random: Option<RandomGenerator>,
    /// The heightmaps of the neighbouring chunks while the chunk's structures are placed.
    region_heights: Option<Box<RegionHeights>>,
}

/// The heightmaps a chunk's structure pieces read: vanilla's pieces measure the ground beyond
/// their chunk through the `WorldGenRegion`, so the neighbours' heights are taken from the cache.
pub(crate) struct RegionHeights {
    center_x: i32,
    center_z: i32,
    /// Per chunk of the 3x3 neighbourhood (x-major, the centre unused), per heightmap in
    /// [`REGION_HEIGHTMAPS`] order, per column.
    chunks: Box<[[[i32; CHUNK_AREA]; 6]]>,
}

const REGION_HEIGHTMAPS: [HeightMap; 6] = [
    HeightMap::WorldSurfaceWg,
    HeightMap::WorldSurface,
    HeightMap::OceanFloorWg,
    HeightMap::OceanFloor,
    HeightMap::MotionBlocking,
    HeightMap::MotionBlockingNoLeaves,
];

impl RegionHeights {
    fn snapshot<T: GenerationCache>(cache: &T, center_x: i32, center_z: i32) -> Self {
        let mut chunks = vec![[[0; CHUNK_AREA]; 6]; 9].into_boxed_slice();
        for (index, heights) in chunks.iter_mut().enumerate() {
            let (dx, dz) = (index as i32 / 3 - 1, index as i32 % 3 - 1);
            if dx == 0 && dz == 0 {
                continue;
            }
            let (start_x, start_z) = ((center_x + dx) << 4, (center_z + dz) << 4);
            for (map, columns) in REGION_HEIGHTMAPS.iter().zip(heights.iter_mut()) {
                for local_x in 0..16 {
                    for local_z in 0..16 {
                        columns[ProtoChunk::local_position_to_height_map_index(local_x, local_z)] =
                            cache.get_top_y(map, start_x + local_x, start_z + local_z);
                    }
                }
            }
        }
        Self {
            center_x,
            center_z,
            chunks,
        }
    }

    fn get(&self, map: HeightMap, x: i32, z: i32) -> Option<i32> {
        let (dx, dz) = ((x >> 4) - self.center_x, (z >> 4) - self.center_z);
        if dx.abs() > 1 || dz.abs() > 1 || (dx == 0 && dz == 0) {
            return None;
        }
        let map_index = REGION_HEIGHTMAPS.iter().position(|m| *m == map)?;
        Some(
            self.chunks[((dx + 1) * 3 + dz + 1) as usize][map_index]
                [ProtoChunk::local_position_to_height_map_index(x & 15, z & 15)],
        )
    }
}

/// An entity a structure placed, spawned once its chunk is generated.
pub struct StructureEntity {
    pub nbt: NbtCompound,
    /// Vanilla runs `finalizeSpawn` (reason `STRUCTURE`) on a mob spawned by a structure's
    /// data marker or jigsaw piece, but not on one from a plain template.
    pub finalize: bool,
}

pub struct TerrainCache {
    pub terrain_builder: SurfaceTerrainBuilder,
    pub surface_noise: DoublePerlinNoiseSampler,
    pub secondary_noise: DoublePerlinNoiseSampler,
}

impl TerrainCache {
    #[must_use]
    pub fn from_random(random_config: &GlobalRandomConfig) -> Self {
        let random = &random_config.base_random_deriver;
        let terrain_builder = SurfaceTerrainBuilder::new(random);
        let surface_noise = DoublePerlinNoiseBuilder::get_noise_sampler(
            &random_config.base_random_deriver,
            random_config.legacy_random_source,
            random_config.seed,
            &DoublePerlinNoiseParameters::SURFACE,
        );
        let secondary_noise = DoublePerlinNoiseBuilder::get_noise_sampler(
            &random_config.base_random_deriver,
            random_config.legacy_random_source,
            random_config.seed,
            &DoublePerlinNoiseParameters::SURFACE_SECONDARY,
        );
        Self {
            terrain_builder,
            surface_noise,
            secondary_noise,
        }
    }
}

impl ProtoChunk {
    #[must_use]
    pub const fn get_sea_level(&self) -> i32 {
        self.sea_level
    }

    #[cfg(test)]
    pub(crate) fn has_structure(&self, key: StructureKeys) -> bool {
        self.structure_starts.contains_key(&key)
            || self.structure_references.iter().any(|(k, _)| *k == key)
    }

    #[must_use]
    pub fn new(x: i32, z: i32, generator: &super::generator::WorldGenerator) -> Self {
        let dimension = generator.dimension();
        let height = dimension.height as u16;
        let bottom_y = dimension.min_y as i8;
        let section_count = (height as usize) / 16;

        let (generation_height, generation_bottom_y, sea_level) = match generator {
            super::generator::WorldGenerator::Noise(noise_gen) => {
                let shape = noise_gen
                    .settings
                    .shape
                    .trim_height(bottom_y, (dimension.min_y + dimension.height) as u16);
                (shape.height, shape.min_y, noise_gen.settings.sea_level)
            }
            super::generator::WorldGenerator::Flat(_)
            | super::generator::WorldGenerator::Custom(_) => (height, bottom_y, 63),
        };

        let default_block = match generator {
            super::generator::WorldGenerator::Noise(noise_gen) => noise_gen.default_block,
            super::generator::WorldGenerator::Flat(_) => Block::AIR.default_state,
            super::generator::WorldGenerator::Custom(custom_gen) => custom_gen.default_block(),
        };
        let biome_mixer_seed = match generator {
            super::generator::WorldGenerator::Noise(noise_gen) => noise_gen.biome_mixer_seed,
            super::generator::WorldGenerator::Flat(flat_gen) => {
                crate::biome::hash_seed(flat_gen.seed)
            }
            super::generator::WorldGenerator::Custom(custom_gen) => custom_gen.biome_mixer_seed(),
        };

        let default_heightmap = [(bottom_y as i16) - 1; CHUNK_AREA];
        let unprimed_wg_heightmap = [i16::MIN; CHUNK_AREA];
        Self {
            x,
            z,
            default_block,
            biome_mixer_seed,
            flat_block_map: Vec::new(),
            flat_biome_map: vec![
                Biome::PLAINS.id;
                biome_coords::from_block(CHUNK_DIM as i32) as usize
                    * biome_coords::from_block(CHUNK_DIM as i32) as usize
                    * biome_coords::from_block(height as i32) as usize
            ]
            .into_boxed_slice(),
            flat_surface_height_map: default_heightmap,
            flat_ocean_floor_height_map: default_heightmap,
            flat_surface_wg_height_map: unprimed_wg_heightmap,
            flat_ocean_floor_wg_height_map: unprimed_wg_heightmap,
            flat_motion_blocking_height_map: default_heightmap,
            flat_motion_blocking_no_leaves_height_map: default_heightmap,
            structure_starts: FxHashMap::default(),
            structure_references: Vec::new(),
            height,
            bottom_y,
            generation_height,
            generation_bottom_y,
            sea_level,
            stage: StagedChunkEnum::Empty,
            light: ChunkLight {
                sky_light: (0..section_count)
                    .map(|_| LightContainer::new_empty(0))
                    .collect(),
                block_light: (0..section_count)
                    .map(|_| LightContainer::new_empty(0))
                    .collect(),
            },
            blending_data: None,
            pending_block_entities: Vec::new(),
            pending_structure_entities: Vec::new(),
            fluid_ticks: Vec::new(),
            block_ticks: Vec::new(),
            post_processing: vec![Vec::new(); section_count],
            surface_height_cache: FxHashMap::default(),
            aquifer_sampler: None,
            region_random: None,
            region_heights: None,
        }
    }

    #[must_use]
    #[expect(clippy::too_many_lines)]
    pub fn from_chunk_data(
        chunk_data: &ChunkData,
        generator: &super::generator::WorldGenerator,
    ) -> Self {
        let mut proto_chunk = Self::new(chunk_data.x, chunk_data.z, generator);

        proto_chunk.light = chunk_data
            .light_engine
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        proto_chunk
            .blending_data
            .clone_from(&chunk_data.blending_data);

        // Chunks loaded from disk carry their block entities in the chunk data.
        // Keep them so the world can create the block entities when the chunk
        // becomes active, and so saving the chunk again does not drop them.
        proto_chunk.pending_block_entities = chunk_data
            .pending_block_entities
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .values()
            .cloned()
            .collect();
        for (section, offsets) in chunk_data
            .post_processing
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .enumerate()
        {
            if let Some(target) = proto_chunk.post_processing.get_mut(section) {
                target.extend_from_slice(offsets);
            }
        }

        let section_data = &chunk_data.section;
        let heightmap_data = chunk_data
            .heightmap
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        let block_sections_guard = section_data
            .block_sections
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let biome_sections_guard = section_data
            .biome_sections
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        for (section_idx, block_palette) in block_sections_guard.iter().enumerate() {
            let section_base_y = section_idx as i32 * 16;

            if section_base_y >= proto_chunk.height() as i32 {
                continue;
            }

            for x in 0..16 {
                for y in 0..16 {
                    for z in 0..16 {
                        let block_state_id = block_palette.get(x, y, z);
                        let block_state = BlockState::from_id(block_state_id);
                        let absolute_y = section_base_y + y as i32 + section_data.min_y;

                        proto_chunk.set_block_state(x as i32, absolute_y, z as i32, block_state);
                    }
                }
            }

            if let Some(biome_palette) = biome_sections_guard.get(section_idx) {
                for x in 0..4 {
                    for y in 0..4 {
                        for z in 0..4 {
                            let biome_id = biome_palette.get(x, y, z);
                            let biome_y_idx = (section_idx * 4) + y;
                            let index = proto_chunk.local_biome_pos_to_biome_index(
                                x as i32,
                                biome_y_idx as i32,
                                z as i32,
                            );
                            proto_chunk.flat_biome_map[index] = biome_id;
                        }
                    }
                }
            }
        }
        drop(block_sections_guard);
        drop(biome_sections_guard);

        for z in 0..16 {
            for x in 0..16 {
                let index = Self::local_position_to_height_map_index(x, z);

                proto_chunk.flat_motion_blocking_height_map[index] = heightmap_data.get(
                    ChunkHeightmapType::MotionBlocking,
                    x,
                    z,
                    section_data.min_y,
                ) as i16;

                proto_chunk.flat_motion_blocking_no_leaves_height_map[index] = heightmap_data.get(
                    ChunkHeightmapType::MotionBlockingNoLeaves,
                    x,
                    z,
                    section_data.min_y,
                )
                    as i16;

                let surface_height =
                    heightmap_data.get(ChunkHeightmapType::WorldSurface, x, z, section_data.min_y)
                        as i16;
                proto_chunk.flat_surface_height_map[index] = surface_height;
                proto_chunk.flat_surface_wg_height_map[index] = surface_height;
                proto_chunk.flat_ocean_floor_wg_height_map[index] =
                    proto_chunk.flat_ocean_floor_height_map[index];
            }
        }

        let saved_stage = StagedChunkEnum::from(chunk_data.status);
        proto_chunk.stage = saved_stage;
        if let super::generator::WorldGenerator::Noise(generator) = generator
            && (StagedChunkEnum::StructureStart..StagedChunkEnum::Features).contains(&saved_stage)
        {
            // Structure starts and references are currently transient proto-chunk data.
            // Rebuild them when resuming a partially generated chunk so structures that
            // cross chunk boundaries are not truncated at the unload boundary.
            proto_chunk.stage = StagedChunkEnum::Biomes;
            proto_chunk.set_structure_starts(generator);
            if saved_stage >= StagedChunkEnum::StructureReferences {
                proto_chunk.set_structure_references(generator);
            }
            proto_chunk.stage = saved_stage;
        }
        proto_chunk
    }

    #[inline]
    #[must_use]
    pub const fn stage_id(&self) -> u8 {
        self.stage as u8
    }

    #[must_use]
    pub const fn height(&self) -> u16 {
        self.height
    }

    #[must_use]
    pub const fn bottom_y(&self) -> i8 {
        self.bottom_y
    }

    #[must_use]
    pub const fn generation_height(&self) -> u16 {
        self.generation_height
    }

    #[must_use]
    pub const fn generation_bottom_y(&self) -> i8 {
        self.generation_bottom_y
    }

    pub fn add_block_entity(&mut self, nbt: NbtCompound) {
        self.pending_block_entities.push(nbt);
    }

    pub fn take_pending_block_entities(&mut self) -> Vec<NbtCompound> {
        std::mem::take(&mut self.pending_block_entities)
    }

    pub fn add_structure_entity(&mut self, nbt: NbtCompound, finalize: bool) {
        self.pending_structure_entities
            .push(StructureEntity { nbt, finalize });
    }

    pub fn take_pending_structure_entities(&mut self) -> Vec<StructureEntity> {
        std::mem::take(&mut self.pending_structure_entities)
    }

    /// Vanilla's `ProtoChunk.markPosForPostProcessing`: the block runs its fluid tick and its shape
    /// update once the chunk starts ticking. Positions outside the build height are ignored.
    pub fn mark_pos_for_post_processing(&mut self, x: i32, y: i32, z: i32) {
        let relative_y = y - i32::from(self.bottom_y);
        if relative_y < 0 || relative_y >= i32::from(self.height) {
            return;
        }
        if let Some(offsets) = self.post_processing.get_mut((relative_y >> 4) as usize) {
            offsets.push(((x & 15) | ((y & 15) << 4) | ((z & 15) << 8)) as u16);
        }
    }

    pub fn schedule_fluid_tick(
        &mut self,
        x: i32,
        y: i32,
        z: i32,
        fluid: &'static Fluid,
        delay: u8,
    ) {
        self.fluid_ticks.push(ScheduledTick {
            delay,
            priority: TickPriority::Normal,
            position: BlockPos::new(x, y, z),
            value: fluid,
        });
    }

    pub fn schedule_block_tick(
        &mut self,
        x: i32,
        y: i32,
        z: i32,
        block: &'static Block,
        delay: u8,
    ) {
        self.block_ticks.push(ScheduledTick {
            delay,
            priority: TickPriority::Normal,
            position: BlockPos::new(x, y, z),
            value: block,
        });
    }

    fn update_height_map_column(
        &mut self,
        hm_index: usize,
        local_x: i32,
        local_y: i32,
        local_z: i32,
        y: i16,
        block_state: &BlockState,
    ) {
        if y < self.flat_ocean_floor_height_map[hm_index]
            .min(self.flat_motion_blocking_no_leaves_height_map[hm_index])
        {
            return;
        }
        let is_not_air = !block_state.is_air();
        let (is_ocean_floor, is_liquid, is_leaves) = if is_not_air {
            let block = BlockId::from_state_id(block_state.id);
            (
                blocks_movement(block_state, block),
                block_state.is_liquid(),
                block.has_tag(tag::Block::MINECRAFT_LEAVES),
            )
        } else {
            (false, false, false)
        };
        let motion_blocking = is_ocean_floor || is_liquid;
        let motion_blocking_no_leaves = motion_blocking && !is_leaves;

        if is_not_air && y > self.flat_surface_height_map[hm_index] {
            self.flat_surface_height_map[hm_index] = y;
        }
        if is_ocean_floor && y > self.flat_ocean_floor_height_map[hm_index] {
            self.flat_ocean_floor_height_map[hm_index] = y;
        }
        if motion_blocking && y > self.flat_motion_blocking_height_map[hm_index] {
            self.flat_motion_blocking_height_map[hm_index] = y;
        }
        if motion_blocking_no_leaves && y > self.flat_motion_blocking_no_leaves_height_map[hm_index]
        {
            self.flat_motion_blocking_no_leaves_height_map[hm_index] = y;
        }

        let mut lower_surface = !is_not_air && y == self.flat_surface_height_map[hm_index];
        let mut lower_ocean = !is_ocean_floor && y == self.flat_ocean_floor_height_map[hm_index];
        let mut lower_mb = !motion_blocking && y == self.flat_motion_blocking_height_map[hm_index];
        let mut lower_mb_no_leaves = !motion_blocking_no_leaves
            && y == self.flat_motion_blocking_no_leaves_height_map[hm_index];

        if lower_surface || lower_ocean || lower_mb || lower_mb_no_leaves {
            let bottom_y = self.bottom_y() as i32;
            let default_y = (bottom_y - 1) as i16;
            if lower_surface {
                self.flat_surface_height_map[hm_index] = default_y;
            }
            if lower_ocean {
                self.flat_ocean_floor_height_map[hm_index] = default_y;
            }
            if lower_mb {
                self.flat_motion_blocking_height_map[hm_index] = default_y;
            }
            if lower_mb_no_leaves {
                self.flat_motion_blocking_no_leaves_height_map[hm_index] = default_y;
            }

            for scan_local_y in (0..local_y).rev() {
                let idx = Self::local_pos_to_block_index(local_x, scan_local_y, local_z);
                let Some(&state_id) = self.flat_block_map.get(idx) else {
                    continue;
                };
                if state_id == BlockStateId::AIR {
                    continue;
                }
                let state = state_id.to_state();
                if state.is_air() {
                    continue;
                }
                let scan_y = (bottom_y + scan_local_y) as i16;
                if lower_surface {
                    self.flat_surface_height_map[hm_index] = scan_y;
                    lower_surface = false;
                }
                let block = BlockId::from_state_id(state_id);
                let bm = blocks_movement(state, block);
                if lower_ocean && bm {
                    self.flat_ocean_floor_height_map[hm_index] = scan_y;
                    lower_ocean = false;
                }
                let mb = bm || state.is_liquid();
                if lower_mb && mb {
                    self.flat_motion_blocking_height_map[hm_index] = scan_y;
                    lower_mb = false;
                }
                if lower_mb_no_leaves && mb && !block.has_tag(tag::Block::MINECRAFT_LEAVES) {
                    self.flat_motion_blocking_no_leaves_height_map[hm_index] = scan_y;
                    lower_mb_no_leaves = false;
                }
                if !(lower_surface || lower_ocean || lower_mb || lower_mb_no_leaves) {
                    break;
                }
            }
        }
    }

    #[must_use]
    pub fn get_top_y(&self, heightmap: &HeightMap, x: i32, z: i32) -> i32 {
        match heightmap {
            HeightMap::WorldSurfaceWg => self.top_block_wg_height_exclusive(x, z),
            HeightMap::WorldSurface => self.top_block_height_exclusive(x, z),
            HeightMap::OceanFloorWg => self.ocean_floor_wg_height_exclusive(x, z),
            HeightMap::OceanFloor => self.ocean_floor_height_exclusive(x, z),
            HeightMap::MotionBlocking => self.top_motion_blocking_block_height_exclusive(x, z),
            HeightMap::MotionBlockingNoLeaves => {
                self.top_motion_blocking_block_no_leaves_height_exclusive(x, z)
            }
        }
    }

    pub fn prime_heightmaps(&mut self) {
        let bottom_y = self.generation_bottom_y() as i32;
        let max_populated_y = bottom_y + (self.flat_block_map.len() / CHUNK_AREA) as i32 - 1;
        let top_y = (bottom_y + self.generation_height() as i32 - 1).min(max_populated_y);
        let default_y = (self.bottom_y() as i32 - 1) as i16;

        for lx in 0..16 {
            for lz in 0..16 {
                let index = Self::local_position_to_height_map_index(lx, lz);
                let prev_surface = self.flat_surface_height_map[index];
                let col_top_y = if prev_surface == i16::MIN {
                    top_y
                } else {
                    (prev_surface as i32).min(top_y)
                };
                let mut surface_found = false;
                let mut ocean_floor_found = false;
                let mut motion_blocking_found = false;
                let mut motion_blocking_no_leaves_found = false;

                self.flat_surface_height_map[index] = default_y;
                self.flat_ocean_floor_height_map[index] = default_y;
                self.flat_motion_blocking_height_map[index] = default_y;
                self.flat_motion_blocking_no_leaves_height_map[index] = default_y;

                for y in (bottom_y..=col_top_y).rev() {
                    let block_index = Self::local_pos_to_block_index(lx, y - bottom_y, lz);
                    let Some(&state_id) = self.flat_block_map.get(block_index) else {
                        continue;
                    };
                    if state_id == BlockStateId::AIR {
                        continue;
                    }
                    let state = state_id.to_state();
                    if !state.is_air() {
                        if !surface_found {
                            self.flat_surface_height_map[index] = y as i16;
                            surface_found = true;
                        }
                        let block = BlockId::from_state_id(state_id);
                        let blocks_movement = blocks_movement(state, block);
                        if blocks_movement && !ocean_floor_found {
                            self.flat_ocean_floor_height_map[index] = y as i16;
                            ocean_floor_found = true;
                        }
                        let is_liquid = state.is_liquid();
                        if (blocks_movement || is_liquid) && !motion_blocking_found {
                            self.flat_motion_blocking_height_map[index] = y as i16;
                            motion_blocking_found = true;
                        }
                        if (blocks_movement || is_liquid)
                            && !block.has_tag(tag::Block::MINECRAFT_LEAVES)
                            && !motion_blocking_no_leaves_found
                        {
                            self.flat_motion_blocking_no_leaves_height_map[index] = y as i16;
                            motion_blocking_no_leaves_found = true;
                        }
                    }
                    if surface_found
                        && ocean_floor_found
                        && motion_blocking_found
                        && motion_blocking_no_leaves_found
                    {
                        break;
                    }
                }
            }
        }
    }

    #[inline]
    pub const fn snapshot_wg_heightmaps(&mut self) {
        self.flat_surface_wg_height_map = self.flat_surface_height_map;
        self.flat_ocean_floor_wg_height_map = self.flat_ocean_floor_height_map;
    }

    #[must_use]
    pub fn top_block_height_exclusive(&self, x: i32, z: i32) -> i32 {
        if let Some(height) = self.region_height(HeightMap::WorldSurface, x, z) {
            return height;
        }
        let index = Self::local_position_to_height_map_index(x & 15, z & 15);
        self.flat_surface_height_map[index] as i32 + 1
    }

    #[must_use]
    pub fn top_block_wg_height_exclusive(&self, x: i32, z: i32) -> i32 {
        if let Some(height) = self.region_height(HeightMap::WorldSurfaceWg, x, z) {
            return height;
        }
        let index = Self::local_position_to_height_map_index(x & 15, z & 15);
        self.flat_surface_wg_height_map[index] as i32 + 1
    }

    #[must_use]
    pub fn ocean_floor_height_exclusive(&self, x: i32, z: i32) -> i32 {
        if let Some(height) = self.region_height(HeightMap::OceanFloor, x, z) {
            return height;
        }
        let index = Self::local_position_to_height_map_index(x & 15, z & 15);
        self.flat_ocean_floor_height_map[index] as i32 + 1
    }

    #[must_use]
    pub fn ocean_floor_wg_height_exclusive(&self, x: i32, z: i32) -> i32 {
        if let Some(height) = self.region_height(HeightMap::OceanFloorWg, x, z) {
            return height;
        }
        let index = Self::local_position_to_height_map_index(x & 15, z & 15);
        self.flat_ocean_floor_wg_height_map[index] as i32 + 1
    }

    #[must_use]
    pub fn top_motion_blocking_block_height_exclusive(&self, x: i32, z: i32) -> i32 {
        if let Some(height) = self.region_height(HeightMap::MotionBlocking, x, z) {
            return height;
        }
        let index = Self::local_position_to_height_map_index(x & 15, z & 15);
        self.flat_motion_blocking_height_map[index] as i32 + 1
    }

    #[must_use]
    pub fn top_motion_blocking_block_no_leaves_height_exclusive(&self, x: i32, z: i32) -> i32 {
        if let Some(height) = self.region_height(HeightMap::MotionBlockingNoLeaves, x, z) {
            return height;
        }
        let index = Self::local_position_to_height_map_index(x & 15, z & 15);
        self.flat_motion_blocking_no_leaves_height_map[index] as i32 + 1
    }

    /// A neighbour's height while this chunk's structures are placed.
    fn region_height(&self, map: HeightMap, x: i32, z: i32) -> Option<i32> {
        if x >> 4 == self.x && z >> 4 == self.z {
            return None;
        }
        self.region_heights.as_ref()?.get(map, x, z)
    }

    #[inline]
    pub(crate) const fn local_position_to_height_map_index(x: i32, z: i32) -> usize {
        x as usize * CHUNK_DIM as usize + z as usize
    }

    #[inline]
    const fn local_pos_to_block_index(x: i32, y: i32, z: i32) -> usize {
        ((y as usize) >> 4) * BlockPalette::VOLUME
            + (x as usize) * CHUNK_AREA
            + ((y as usize) & 15) * CHUNK_DIM as usize
            + z as usize
    }

    #[inline]
    #[must_use]
    pub const fn local_biome_pos_to_biome_index(&self, x: i32, y: i32, z: i32) -> usize {
        let biome_height = self.height() as usize >> 2;
        biome_height * biome_coords::from_block(CHUNK_DIM as i32) as usize * x as usize
            + biome_coords::from_block(CHUNK_DIM as i32) as usize * y as usize
            + z as usize
    }

    #[inline]
    #[must_use]
    pub fn is_air(&self, local_pos: &Vector3<i32>) -> bool {
        is_air(self.get_block_state(local_pos))
    }

    #[inline]
    #[must_use]
    pub fn get_block_state_raw(&self, x: i32, y: i32, z: i32) -> BlockStateId {
        debug_assert!((0..i32::from(self.height())).contains(&y));
        self.flat_block_map
            .get(Self::local_pos_to_block_index(x, y, z))
            .copied()
            .unwrap_or(BlockStateId::AIR)
    }

    #[inline]
    #[must_use]
    pub fn get_block_state(&self, local_pos: &Vector3<i32>) -> BlockStateId {
        let local_y = local_pos.y - self.bottom_y() as i32;
        if local_y < 0 || local_y >= self.height() as i32 {
            return Block::VOID_AIR.default_state.id;
        }
        self.get_block_state_raw(local_pos.x & 15, local_y, local_pos.z & 15)
    }

    pub fn set_block_state(&mut self, x: i32, y: i32, z: i32, block_state: &BlockState) {
        let local_x = x & 15;
        let local_y = y - self.bottom_y() as i32;
        let local_z = z & 15;

        if local_y < 0 || local_y >= self.height() as i32 {
            return;
        }

        let index = Self::local_pos_to_block_index(local_x, local_y, local_z);
        if index >= self.flat_block_map.len() {
            if block_state.id == BlockStateId::AIR {
                return;
            }
            let section_end = ((local_y as usize >> 4) + 1) * BlockPalette::VOLUME;
            self.flat_block_map
                .reserve_exact(section_end - self.flat_block_map.len());
            self.flat_block_map.resize(section_end, BlockStateId::AIR);
        } else if self.flat_block_map[index] == block_state.id {
            return;
        }
        self.flat_block_map[index] = block_state.id;

        let hm_index = Self::local_position_to_height_map_index(local_x, local_z);
        self.update_height_map_column(hm_index, local_x, local_y, local_z, y as i16, block_state);
    }

    #[inline]
    #[must_use]
    pub fn get_biome(&self, x: i32, y: i32, z: i32) -> &'static Biome {
        Biome::from_id(self.get_biome_id(x, y, z)).unwrap_or(&Biome::PLAINS)
    }

    #[inline]
    #[must_use]
    pub fn get_biome_id(&self, x: i32, y: i32, z: i32) -> u8 {
        let index = self.local_biome_pos_to_biome_index(
            x & 3,
            y - biome_coords::from_block(self.bottom_y() as i32),
            z & 3,
        );
        self.flat_biome_map[index]
    }

    pub fn step_to_biomes(&mut self, generator: &super::generator::VanillaGenerator) {
        debug_assert_eq!(self.stage, StagedChunkEnum::Empty);
        let mut multi_noise_sampler =
            MultiNoiseSampler::generate(&generator.base_router.multi_noise);
        self.populate_biomes(generator, &mut multi_noise_sampler);
        self.stage = StagedChunkEnum::Biomes;
    }

    #[expect(clippy::too_many_lines)]
    pub fn step_to_noise(&mut self, generator: &super::generator::VanillaGenerator) {
        debug_assert_eq!(self.stage, StagedChunkEnum::StructureReferences);
        let settings = generator.settings;
        let generation_shape = &settings.shape;
        let start_x = start_block_x(self.x);
        let start_z = start_block_z(self.z);

        let sampler = StandardChunkFluidLevelSampler::new(
            FluidLevel::new(
                settings.sea_level,
                Block::from_state_id(settings.default_fluid.id),
            ),
            FluidLevel::new(-54, &Block::LAVA),
        );

        let mut beardifier_structures = Vec::new();
        let mut beardifier_junctions = Vec::new();
        let mut any_piece_bounding_box: Option<BlockBox> = None;

        let chunk_start_x = self.start_block_x();
        let chunk_start_z = self.start_block_z();
        let mut seen_collectors: Vec<&Arc<Mutex<StructurePiecesCollector>>> = Vec::new();

        for (key, collector) in self
            .structure_starts
            .iter()
            .map(|(k, inst)| {
                let col = match inst {
                    StructureInstance::Start(pos) => &pos.collector,
                    StructureInstance::Reference(collector) => collector,
                };
                (k, col)
            })
            .chain(self.structure_references.iter().map(|(k, col)| (k, col)))
        {
            if seen_collectors
                .iter()
                .any(|seen| Arc::ptr_eq(seen, collector))
            {
                continue;
            }
            seen_collectors.push(collector);
            let structure = pumpkin_data::structures::Structure::get(key);
            let terrain_adaptation = structure.terrain_adaptation;

            // Vanilla strictly skips filtering Beardifier parts if adaptation is None early-on
            if terrain_adaptation == pumpkin_data::structures::TerrainAdaptation::None {
                continue;
            }

            let collector = collector
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            for piece in &collector.pieces {
                let bounding_box = piece.get_structure_piece().bounding_box;

                // Match `piece.isCloseToChunk(chunkPos, 12)`
                // Validates if an expansion 12 blocks out covers the chunk borders
                if !bounding_box.intersects_raw_xz(
                    chunk_start_x - 12,
                    chunk_start_z - 12,
                    chunk_start_x + 15 + 12,
                    chunk_start_z + 15 + 12,
                ) {
                    continue;
                }

                let mut ground_level_delta = 0;

                if let Some(jigsaw_piece) = piece.as_any().downcast_ref::<crate::generation::structure::structures::jigsaw::PoolElementStructurePiece>() {
                    // Java only adds to rigids if projection is RIGID
                    if jigsaw_piece.projection == crate::generation::structure::structures::jigsaw::JigsawProjection::Rigid {
                        ground_level_delta = jigsaw_piece.ground_level_delta;
                        any_piece_bounding_box = any_piece_bounding_box.map_or(Some(bounding_box), |mut b| {
                            b.encompass(&bounding_box);
                            Some(b)
                        });

                        beardifier_structures.push(
                            crate::generation::noise::router::density_function::beardifier::BeardifierStructure {
                                bounding_box,
                                terrain_adaptation,
                                ground_level_delta,
                            },
                        );
                    }

                    for j in &jigsaw_piece.junctions {
                        let j_x = j.source_x;
                        let j_z = j.source_z;
                        // Junction bounds filter (match vanilla proximity checks)
                        if j_x > chunk_start_x - 12
                            && j_z > chunk_start_z - 12
                            && j_x < chunk_start_x + 15 + 12
                            && j_z < chunk_start_z + 15 + 12
                        {
                            beardifier_junctions.push(
                                crate::generation::noise::router::density_function::beardifier::BeardifierJunction {
                                    x: j_x,
                                    ground_y: j.source_ground_y,
                                    z: j_z,
                                },
                            );
                            let junction_box = BlockBox::from_pos(BlockPos::new(j_x, j.source_ground_y, j_z));
                            any_piece_bounding_box = any_piece_bounding_box.map_or(Some(junction_box), |mut b| {
                                b.encompass(&junction_box);
                                Some(b)
                            });
                        }
                    }
                } else {
                    any_piece_bounding_box = any_piece_bounding_box.map_or(Some(bounding_box), |mut b| {
                        b.encompass(&bounding_box);
                        Some(b)
                    });

                    beardifier_structures.push(
                        crate::generation::noise::router::density_function::beardifier::BeardifierStructure {
                            bounding_box,
                            terrain_adaptation,
                            ground_level_delta,
                        },
                    );
                }
            }
        }

        let affected_box = any_piece_bounding_box.map(|b| b.expand(24, 24, 24));

        // Passed the newly mapped beardifier structures & junctions arrays independently!
        let mut noise_sampler = ChunkNoiseGenerator::new(
            &generator.base_router.noise,
            &generator.random_config,
            DensityVolume::with_block_step(
                CHUNK_DIM as usize,
                generation_shape.height as usize,
                CHUNK_DIM as usize,
                start_x,
                i32::from(generation_shape.min_y),
                start_z,
            ),
            generation_shape,
            sampler,
            settings.aquifers_enabled,
            false,
            beardifier_structures,
            beardifier_junctions,
            affected_box,
        );

        let surface_config = SurfaceHeightSamplerBuilderOptions::new(
            generation_shape.min_y as i32,
            generation_shape.max_y() as i32,
            generation_shape.vertical_cell_block_count() as usize,
        );
        let mut surface_height_estimate_sampler = SurfaceHeightEstimateSampler::generate_with_cache(
            &generator.base_router.surface_estimator,
            &surface_config,
            std::mem::take(&mut self.surface_height_cache),
        );
        self.populate_noise(
            generator,
            &mut noise_sampler,
            &generator.random_config.ore_random_deriver,
            &mut surface_height_estimate_sampler,
        );
        self.surface_height_cache = surface_height_estimate_sampler.into_cache();
        self.aquifer_sampler = noise_sampler.into_aquifer();

        self.stage = StagedChunkEnum::Noise;
    }

    pub(crate) fn step_to_surface(
        &mut self,
        generator: &super::generator::VanillaGenerator,
        surface_biomes: &SurfaceBiomeNeighborhood,
    ) {
        debug_assert_eq!(self.stage, StagedChunkEnum::Noise);
        let generation_shape = &generator.settings.shape;
        let surface_config = SurfaceHeightSamplerBuilderOptions::new(
            generation_shape.min_y as i32,
            generation_shape.max_y() as i32,
            generation_shape.vertical_cell_block_count() as usize,
        );
        let mut surface_height_estimate_sampler = SurfaceHeightEstimateSampler::generate_with_cache(
            &generator.base_router.surface_estimator,
            &surface_config,
            std::mem::take(&mut self.surface_height_cache),
        );

        self.build_surface(
            generator,
            surface_biomes,
            &mut surface_height_estimate_sampler,
        );
        self.surface_height_cache = surface_height_estimate_sampler.into_cache();
        self.stage = StagedChunkEnum::Surface;
    }

    pub fn step_to_carvers(&mut self, generator: &super::generator::VanillaGenerator) {
        debug_assert_eq!(self.stage, StagedChunkEnum::Surface);
        super::carver::carve(self, generator);

        self.prime_heightmaps();
        self.snapshot_wg_heightmaps();
        self.stage = StagedChunkEnum::Carvers;
    }

    pub fn populate_biomes(
        &mut self,
        generator: &super::generator::VanillaGenerator,
        multi_noise_sampler: &mut MultiNoiseSampler,
    ) {
        let start_block_x = start_block_x(self.x);
        let start_block_z = start_block_z(self.z);

        let start_biome_x = biome_coords::from_block(start_block_x);
        let start_biome_z = biome_coords::from_block(start_block_z);

        match generator.biome_supplier {
            crate::biome::ActiveBiomeSupplier::Fixed(fixed) => {
                self.flat_biome_map.fill(fixed.biome.id);
            }
            crate::biome::ActiveBiomeSupplier::End(end) => {
                let biome = end.biome(start_biome_x, 0, start_biome_z, multi_noise_sampler);
                self.flat_biome_map.fill(biome.id);
            }
            crate::biome::ActiveBiomeSupplier::MultiNoise(multi) => {
                let min_y = self.bottom_y();
                let bottom_section = section_coords::block_to_section(min_y as i32);
                let top_section =
                    section_coords::block_to_section(min_y as i32 + self.height() as i32 - 1);
                let min_biome_y = biome_coords::from_block(min_y as i32);

                let biome_step = biome_coords::to_block(1);
                multi_noise_sampler.fill_volume(DensityVolume::new(
                    biome_coords::from_block(CHUNK_DIM as i32) as usize,
                    biome_coords::from_block(self.height() as i32) as usize,
                    biome_coords::from_block(CHUNK_DIM as i32) as usize,
                    start_block_x,
                    min_y as i32,
                    start_block_z,
                    biome_step,
                    biome_step,
                    biome_step,
                ));

                let biomes_per_section = biome_coords::from_block(CHUNK_DIM as i32);
                for i in bottom_section..=top_section {
                    let start_block_y = section_coords::section_to_block(i);
                    let start_biome_y = biome_coords::from_block(start_block_y);
                    for x in 0..biomes_per_section {
                        for y in 0..biomes_per_section {
                            for z in 0..biomes_per_section {
                                let biome = multi.biome(
                                    start_biome_x + x,
                                    start_biome_y + y,
                                    start_biome_z + z,
                                    multi_noise_sampler,
                                );
                                let index = self.local_biome_pos_to_biome_index(
                                    x,
                                    start_biome_y + y - min_biome_y,
                                    z,
                                );
                                self.flat_biome_map[index] = biome.id;
                            }
                        }
                    }
                }
            }
        }
    }

    pub fn populate_noise(
        &mut self,
        generator: &super::generator::VanillaGenerator,
        noise_sampler: &mut ChunkNoiseGenerator,
        ore_random_deriver: &XoroshiroSplitter,
        surface_height_estimate_sampler: &mut SurfaceHeightEstimateSampler,
    ) {
        let volume = *noise_sampler.volume();
        let densities = noise_sampler.sample_density();

        for z in 0..volume.size_z {
            let block_z = volume.block_z(z);
            for x in 0..volume.size_x {
                let block_x = volume.block_x(x);
                for y in (0..volume.size_y).rev() {
                    let block_y = volume.block_y(y);
                    let index = volume.index_unchecked(x, y, z);
                    let block_state = noise_sampler
                        .sample_block_state(
                            ore_random_deriver,
                            &Vector3::new(block_x, block_y, block_z),
                            densities.density[index],
                            densities.vein_sample(index).as_ref(),
                            surface_height_estimate_sampler,
                        )
                        .unwrap_or(generator.default_block);
                    self.set_block_state(block_x, block_y, block_z, block_state);
                    if noise_sampler.should_schedule_fluid_update() && block_state.is_liquid() {
                        self.mark_pos_for_post_processing(block_x, block_y, block_z);
                    }
                }
            }
        }
    }

    pub fn spawn_mobs<T: GenerationCache>(cache: &mut T, block_registry: &dyn WorldPortalExt) {
        let chunk = cache.get_center_chunk();
        if chunk.stage >= StagedChunkEnum::Spawn {
            return;
        }
        debug_assert_eq!(chunk.stage, StagedChunkEnum::Lighting);

        let biome = chunk.get_terrain_gen_biome(
            section_to_block(chunk.x),
            chunk.bottom_y() as i32 + chunk.height() as i32 - 1,
            section_to_block(chunk.z),
        );
        let x = chunk.x;
        let z = chunk.z;

        block_registry.spawn_mobs_for_chunk_generation(cache, biome, x, z);

        let entities = cache
            .get_center_chunk_mut()
            .take_pending_structure_entities();
        block_registry.spawn_structure_entities(entities);

        cache.get_center_chunk_mut().stage = StagedChunkEnum::Spawn;
    }

    #[must_use]
    pub fn get_terrain_gen_biome_id(&self, x: i32, y: i32, z: i32) -> u8 {
        let biome_pos = self.get_terrain_gen_biome_pos(x, y, z);

        self.get_biome_id(biome_pos.x, biome_pos.y, biome_pos.z)
    }

    #[must_use]
    pub(crate) fn get_terrain_gen_biome_pos(&self, x: i32, y: i32, z: i32) -> Vector3<i32> {
        biome::get_biome_blend(
            self.bottom_y(),
            self.height(),
            self.biome_mixer_seed,
            x,
            y,
            z,
        )
    }

    pub(crate) fn get_terrain_gen_biome_id_from_neighborhood(
        &self,
        surface_biomes: &SurfaceBiomeNeighborhood,
        x: i32,
        y: i32,
        z: i32,
    ) -> Option<u8> {
        let biome_pos = self.get_terrain_gen_biome_pos(x, y, z);

        if biome_pos.x >> 2 == self.x && biome_pos.z >> 2 == self.z {
            return Some(self.get_biome_id(biome_pos.x, biome_pos.y, biome_pos.z));
        }

        surface_biomes.get_biome_id(biome_pos.x, biome_pos.y, biome_pos.z)
    }

    #[must_use]
    pub fn get_terrain_gen_biome(&self, x: i32, y: i32, z: i32) -> &'static Biome {
        Biome::from_id(self.get_terrain_gen_biome_id(x, y, z)).unwrap_or(&Biome::PLAINS)
    }

    #[expect(clippy::too_many_lines)]
    #[expect(
        clippy::panic,
        reason = "surface scheduling guarantees a complete 3x3 biome neighborhood"
    )]
    pub(crate) fn build_surface(
        &mut self,
        generator: &super::generator::VanillaGenerator,
        surface_biomes: &SurfaceBiomeNeighborhood,
        surface_height_estimate_sampler: &mut SurfaceHeightEstimateSampler,
    ) {
        let start_x = chunk_pos::start_block_x(self.x);
        let start_z = chunk_pos::start_block_z(self.z);
        let min_y = self.bottom_y();

        let settings = generator.settings;
        let random_config = &generator.random_config;
        let terrain_cache = &generator.terrain_cache;

        let mut context = MaterialRuleContext::new(
            self.generation_bottom_y(),
            self.generation_height(),
            random_config,
            &terrain_cache.terrain_builder,
            &terrain_cache.surface_noise,
            &terrain_cache.secondary_noise,
            settings.sea_level,
        );
        context.surface_biomes = Some(surface_biomes);
        let mut vein_context = settings.ore_veins_enabled.then(|| {
            let chunk_min_y = i32::from(min_y);
            let chunk_max_y = chunk_min_y + i32::from(self.height());
            let vein_min_y = (-60).max(chunk_min_y);
            let vein_max_y = 52.min(chunk_max_y);
            let vein_height = (vein_max_y - vein_min_y).max(0) as usize;
            let volume =
                DensityVolume::with_block_step(16, vein_height, 16, start_x, vein_min_y, start_z);
            let mut router = ChunkVeinRouter::generate(&generator.base_router.vein);
            let mut vein_toggle = DensityBuffer::acquire(&volume);
            router.vein_toggle_volume(&mut vein_toggle, &volume);
            VeinContext {
                volume,
                vein_toggle,
                vein_ridged: None,
                router,
                ore_random_deriver: &random_config.ore_random_deriver,
            }
        });
        context.vein_context = vein_context.as_mut();
        let has_eroded_badlands = surface_biomes.contains_biome(Biome::ERODED_BADLANDS.id);
        let has_frozen_ocean = surface_biomes.contains_biome(Biome::FROZEN_OCEAN.id)
            || surface_biomes.contains_biome(Biome::DEEP_FROZEN_OCEAN.id);
        let check_column_biome = has_eroded_badlands || has_frozen_ocean;

        for local_x in 0..16 {
            for local_z in 0..16 {
                let x = start_x + local_x;
                let z = start_z + local_z;

                let top_block = self.top_block_height_exclusive(local_x, local_z);

                let biome_y = if settings.legacy_random_source {
                    0
                } else {
                    top_block
                };

                let this_biome = check_column_biome.then(|| {
                    let Some(id) = self.get_terrain_gen_biome_id_from_neighborhood(
                        surface_biomes,
                        x,
                        biome_y,
                        z,
                    ) else {
                        panic!("surface biome neighborhood must cover fuzzy biome lookup");
                    };
                    if id == Biome::ERODED_BADLANDS {
                        terrain_cache
                            .terrain_builder
                            .place_badlands_pillar(self, x, z, top_block);
                    }
                    id
                });

                let top_block = self.top_block_height_exclusive(local_x, local_z);

                context.init_horizontal(x, z);

                let mut stone_depth_above = 0;
                let mut min = i32::MAX;
                let mut fluid_height = i32::MIN;
                for y in (min_y as i32..=top_block).rev() {
                    let pos = Vector3::new(x, y, z);
                    let state_id = self.get_block_state(&pos);
                    if state_id == BlockStateId::AIR {
                        stone_depth_above = 0;
                        fluid_height = i32::MIN;
                        continue;
                    }
                    let state = state_id.to_state();
                    if state.is_air() {
                        stone_depth_above = 0;
                        fluid_height = i32::MIN;
                        continue;
                    }
                    if state.is_liquid() {
                        if fluid_height == i32::MIN {
                            fluid_height = y + 1;
                        }
                        continue;
                    }
                    if min >= y {
                        let shift = min_y << 4;
                        min = shift as i32;

                        for search_y in ((min_y as i32 - 1)..y).rev() {
                            if search_y < min_y as i32 {
                                min = search_y + 1;
                                break;
                            }

                            let block_id = self
                                .get_block_state(&Vector3::new(local_x, search_y, local_z))
                                .to_block_id();

                            if !(block_id != AIR_BLOCK
                                && block_id != WATER_BLOCK
                                && block_id != LAVA_BLOCK)
                            {
                                min = search_y + 1;
                                break;
                            }
                        }
                    }

                    stone_depth_above += 1;
                    let stone_depth_below = y - min + 1;
                    context.init_vertical(stone_depth_above, stone_depth_below, y, fluid_height);

                    let new_state = try_apply_material_rule(
                        generator.surface_rule,
                        self,
                        &mut context,
                        surface_height_estimate_sampler,
                    );

                    if let Some(state) = new_state {
                        self.set_block_state(x, y, z, state);
                        // Vanilla's surface column marks a fluid it places (swamp puddles, holes
                        // in frozen oceans, Nether lava) for post-processing.
                        if state.is_liquid() {
                            self.mark_pos_for_post_processing(x, y, z);
                        }
                    }
                }
                if let Some(this_biome) = this_biome
                    && (this_biome == Biome::FROZEN_OCEAN || this_biome == Biome::DEEP_FROZEN_OCEAN)
                {
                    let surface_estimate =
                        estimate_surface_height(&mut context, surface_height_estimate_sampler);

                    terrain_cache.terrain_builder.place_iceberg(
                        self,
                        Biome::from_id(this_biome).unwrap_or(&Biome::PLAINS),
                        x,
                        z,
                        surface_estimate,
                        top_block,
                        settings.sea_level,
                        &random_config.base_random_deriver,
                    );
                }
            }
        }
    }

    pub fn generate_features_and_structure<T: GenerationCache>(
        cache: &mut T,
        block_registry: &dyn WorldPortalExt,
        random_config: &GlobalRandomConfig,
    ) {
        let (center_x, center_z, min_y, generation_min_y, generation_height) = {
            let chunk = cache.get_center_chunk();
            (
                chunk.x,
                chunk.z,
                chunk.bottom_y() as i32,
                chunk.generation_bottom_y(),
                chunk.generation_height(),
            )
        };

        // Vanilla gathers every biome stored in the 3x3 chunk neighborhood before selecting the
        // globally ordered feature set.
        let mut possible_biomes = Vec::new();
        for chunk_x in center_x - 1..=center_x + 1 {
            for chunk_z in center_z - 1..=center_z + 1 {
                if let Some(chunk) = cache.get_chunk(chunk_x, chunk_z) {
                    for &biome_id in &chunk.flat_biome_map {
                        if !possible_biomes.contains(&biome_id) {
                            possible_biomes.push(biome_id);
                        }
                    }
                }
            }
        }

        let start_block_x = chunk_pos::start_block_x(center_x);
        let start_block_z = chunk_pos::start_block_z(center_z);
        let origin_pos = BlockPos::new(start_block_x, min_y, start_block_z);

        let population_seed = WorldgenRandom::get_population_seed(
            random_config.feature_seed,
            start_block_x,
            start_block_z,
        );
        cache.get_center_chunk_mut().region_random = Some(RandomGenerator::Xoroshiro(
            random_config
                .base_random_deriver
                .split_string("minecraft:worldgen_region_random")
                .next_splitter()
                .split_pos(start_block_x, 0, start_block_z),
        ));

        for chunk_x in center_x - 1..=center_x + 1 {
            for chunk_z in center_z - 1..=center_z + 1 {
                if let Some(chunk) = cache.get_chunk_mut(chunk_x, chunk_z)
                    && chunk.flat_surface_wg_height_map[0] == i16::MIN
                {
                    chunk.prime_heightmaps();
                    chunk.snapshot_wg_heightmaps();
                }
            }
        }

        for step in 0..11 {
            Self::generate_structure_step(
                cache,
                block_registry,
                step,
                population_seed,
                random_config.structure_seed as i64,
            );

            let selected = select_features(&possible_biomes, step);

            for (global_index, feature_enum) in selected {
                if let Some(feature) = PLACED_FEATURES.get(&feature_enum) {
                    let decorator_seed =
                        get_decorator_seed(population_seed, global_index as u64, step as u64);
                    let mut random =
                        RandomGenerator::Worldgen(WorldgenRandom::from_seed(decorator_seed));

                    feature.generate(
                        cache,
                        block_registry,
                        generation_min_y,
                        generation_height,
                        feature_enum,
                        &mut random,
                        origin_pos,
                    );
                }
            }
        }

        let center = cache.get_center_chunk_mut();
        center.region_random = None;
        center.stage = StagedChunkEnum::Features;
    }

    fn generate_structure_step<T: GenerationCache>(
        cache: &mut T,
        block_registry: &dyn WorldPortalExt,
        step: usize,
        population_seed: u64,
        world_seed: i64,
    ) {
        static STRUCTURES_PER_STEP: LazyLock<[Vec<StructureKeys>; 11]> = LazyLock::new(|| {
            let mut steps: [Vec<StructureKeys>; 11] = Default::default();
            for name in StructureKeys::all_names() {
                if let Some(key) = StructureKeys::from_name(name) {
                    let s = Structure::get(&key);
                    steps[s.step.ordinal()].push(key);
                }
            }
            steps
        });

        let (center_x, center_z) = {
            let center_chunk = cache.get_center_chunk();
            (center_chunk.x, center_chunk.z)
        };

        let structures = &STRUCTURES_PER_STEP[step];
        let mut region_heights = None;
        for (index, &structure_key) in structures.iter().enumerate() {
            let mut tasks = Vec::new();
            {
                let center_chunk = cache.get_center_chunk();

                for (id, instance) in &center_chunk.structure_starts {
                    if *id == structure_key {
                        match instance {
                            StructureInstance::Start(pos) => tasks.push(pos.collector.clone()),
                            StructureInstance::Reference(collector) => {
                                let collector_arc = collector.clone();
                                if !tasks.iter().any(|t| Arc::ptr_eq(t, &collector_arc)) {
                                    tasks.push(collector_arc);
                                }
                            }
                        }
                    }
                }

                for (id, collector_arc) in &center_chunk.structure_references {
                    if *id == structure_key && !tasks.iter().any(|t| Arc::ptr_eq(t, collector_arc))
                    {
                        tasks.push(collector_arc.clone());
                    }
                }

                let start_x = chunk_pos::start_block_x(center_x);
                let start_z = chunk_pos::start_block_z(center_z);
                let end_x = start_x + 15;
                let end_z = start_z + 15;

                let radius = 8;
                for dx in -radius..=radius {
                    for dz in -radius..=radius {
                        if dx == 0 && dz == 0 {
                            continue;
                        }

                        let neighbor_x = center_x + dx;
                        let neighbor_z = center_z + dz;

                        if let Some(neighbor) = cache.try_get_proto_chunk(neighbor_x, neighbor_z) {
                            for (id, instance) in &neighbor.structure_starts {
                                if *id == structure_key
                                    && let StructureInstance::Start(pos) = instance
                                    && pos
                                        .get_bounding_box()
                                        .intersects_raw_xz(start_x, start_z, end_x, end_z)
                                {
                                    let collector_arc = pos.collector.clone();
                                    if !tasks.iter().any(|t| Arc::ptr_eq(t, &collector_arc)) {
                                        tasks.push(collector_arc);
                                    }
                                }
                            }
                        }
                    }
                }
            }

            if tasks.is_empty() {
                continue;
            }

            let decorator_seed = get_decorator_seed(population_seed, index as u64, step as u64);
            let mut random = RandomGenerator::Worldgen(WorldgenRandom::from_seed(decorator_seed));

            // Only this chunk changes while its structures are placed, so one snapshot of the
            // neighbours' heights serves the whole step.
            let heights = region_heights
                .take()
                .unwrap_or_else(|| Box::new(RegionHeights::snapshot(cache, center_x, center_z)));
            let chunk = cache.get_center_chunk_mut();
            chunk.region_heights = Some(heights);
            for collector_arc in tasks {
                let mut collector = collector_arc
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                collector.generate_in_chunk(chunk, block_registry, &mut random, world_seed);
            }
            region_heights = chunk.region_heights.take();
        }
    }

    /// The biomes a structure may start in (vanilla `Structure.biomes()`).
    #[must_use]
    pub fn structure_biomes(key: StructureKeys) -> &'static [u16] {
        let structure = Structure::get(&key);
        get_tag_ids(
            RegistryKey::WorldgenBiome,
            structure
                .biomes
                .strip_prefix('#')
                .unwrap_or(structure.biomes),
        )
        .unwrap_or_default()
    }

    /// Returns every biome allowed by at least one structure in the set.
    #[must_use]
    pub fn get_allowed_biomes(set: &StructureSet) -> Vec<u16> {
        let mut allowed_biomes = Vec::new();
        for entry in set.structures {
            allowed_biomes.extend_from_slice(Self::structure_biomes(entry.structure));
        }
        allowed_biomes
    }

    /// Creates the structure starts owned by this chunk.
    pub fn set_structure_starts(&mut self, generator: &super::generator::VanillaGenerator) {
        debug_assert_eq!(self.stage, StagedChunkEnum::Biomes);
        let mut height_sampler =
            crate::generation::structure::height_sampler::NoiseHeightSampler::new(generator);

        for (i, set) in StructureSet::ALL.iter().enumerate() {
            if !generator.enabled_structure_sets.contains(&i)
                || !should_generate_structure(
                    i,
                    &generator.structure_calculator,
                    &generator.structure_salts,
                    self.x,
                    self.z,
                    &generator.global_structure_cache,
                )
            {
                continue;
            }
            if let Some((key, start)) =
                claiming_structure_start(generator, set, self.x, self.z, &mut height_sampler)
            {
                self.structure_starts
                    .insert(key, StructureInstance::Start(start));
            }
        }
        self.stage = StagedChunkEnum::StructureStart;
    }

    /// Adds references to nearby structure starts that intersect this chunk.
    pub fn set_structure_references(&mut self, generator: &super::generator::VanillaGenerator) {
        debug_assert_eq!(self.stage, StagedChunkEnum::StructureStart);
        // Matches vanilla WorldGenerationContext:
        // minY = Math.max(heightAccessor.getMinY(), generator.getMinY())
        // height = Math.min(heightAccessor.getHeight(), generator.getGenDepth())
        let chunk_min_y = (self.generation_bottom_y() as i32).max(self.bottom_y() as i32);
        let chunk_height = self.generation_height().min(self.height());
        let enabled_sets = (0..StructureSet::ALL.len())
            .filter(|set_index| generator.enabled_structure_sets.contains(set_index));
        let references = structure_references(
            generator,
            self.x,
            self.z,
            chunk_min_y,
            chunk_height,
            enabled_sets,
        );

        for (key, pos) in references {
            self.structure_starts
                .entry(key)
                .or_insert_with(|| StructureInstance::Reference(pos.clone()));
            self.structure_references.push((key, pos));
        }

        self.stage = StagedChunkEnum::StructureReferences;
    }

    const fn start_cell_x(&self, horizontal_cell_block_count: i32) -> i32 {
        self.start_block_x() / horizontal_cell_block_count
    }

    const fn start_cell_z(&self, horizontal_cell_block_count: i32) -> i32 {
        self.start_block_z() / horizontal_cell_block_count
    }

    const fn start_block_x(&self) -> i32 {
        start_block_x(self.x)
    }

    const fn start_block_z(&self) -> i32 {
        start_block_z(self.z)
    }
}

impl BlockAccessor for ProtoChunk {
    fn get_block(&self, position: &BlockPos) -> &'static Block {
        self.get_block_state(&position.0).to_block()
    }

    fn get_block_state(&self, position: &BlockPos) -> &'static BlockState {
        self.get_block_state(&position.0).to_state()
    }

    fn get_block_state_id(&self, position: &BlockPos) -> BlockStateId {
        self.get_block_state(&position.0)
    }

    fn get_block_and_state(&self, position: &BlockPos) -> (&'static Block, &'static BlockState) {
        let id = self.get_block_state(&position.0);
        BlockState::from_id_with_block(id)
    }
}

impl BlockPlacer for ProtoChunk {
    fn get_block_state(&self, pos: &Vector3<i32>) -> BlockStateId {
        self.get_block_state(pos)
    }

    fn set_block_state(&mut self, pos: &Vector3<i32>, state: &BlockState) {
        Self::set_block_state(self, pos.x, pos.y, pos.z, state);
    }

    fn add_block_entity(&mut self, nbt: NbtCompound) {
        self.add_block_entity(nbt);
    }

    fn get_height(&self, heightmap: HeightMap, x: i32, z: i32) -> Option<i32> {
        Some(self.get_top_y(&heightmap, x, z))
    }
}

impl GenerationCache for ProtoChunk {
    fn get_center_chunk_mut(&mut self) -> &mut ProtoChunk {
        self
    }
    fn get_center_chunk(&self) -> &ProtoChunk {
        self
    }
    fn get_chunk_mut(&mut self, cx: i32, cz: i32) -> Option<&mut ProtoChunk> {
        (cx == self.x && cz == self.z).then_some(self)
    }
    fn get_chunk(&self, cx: i32, cz: i32) -> Option<&ProtoChunk> {
        (cx == self.x && cz == self.z).then_some(self)
    }
    fn try_get_proto_chunk(&self, cx: i32, cz: i32) -> Option<&ProtoChunk> {
        self.get_chunk(cx, cz)
    }
    fn get_block_state(&self, pos: &Vector3<i32>) -> BlockStateId {
        Self::get_block_state(self, pos)
    }
    fn get_fluid_and_fluid_state(&self, pos: &Vector3<i32>) -> (Fluid, FluidState) {
        let id = GenerationCache::get_block_state(self, pos);
        let Some(fluid) = Fluid::from_state_id(id) else {
            let fluid = if id.is_waterlogged()
                || matches!(
                    id.to_block_id(),
                    pumpkin_data::BlockId::KELP
                        | pumpkin_data::BlockId::KELP_PLANT
                        | pumpkin_data::BlockId::SEAGRASS
                        | pumpkin_data::BlockId::TALL_SEAGRASS
                        | pumpkin_data::BlockId::BUBBLE_COLUMN
                ) {
                Fluid::WATER
            } else {
                Fluid::EMPTY
            };
            let state = fluid.states[0].clone();
            return (fluid, state);
        };
        let state = fluid.states[0].clone();
        (fluid.clone(), state)
    }
    fn set_block_state_raw(&mut self, pos: &Vector3<i32>, block_state: &BlockState) {
        Self::set_block_state(self, pos.x, pos.y, pos.z, block_state);
    }
    fn add_block_entity(&mut self, _pos: &Vector3<i32>, nbt: NbtCompound) {
        self.add_block_entity(nbt);
    }
    fn top_motion_blocking_block_height_exclusive(&self, x: i32, z: i32) -> i32 {
        Self::top_motion_blocking_block_height_exclusive(self, x, z)
    }
    fn top_motion_blocking_block_no_leaves_height_exclusive(&self, x: i32, z: i32) -> i32 {
        Self::top_motion_blocking_block_no_leaves_height_exclusive(self, x, z)
    }
    fn get_top_y(&self, heightmap: &HeightMap, x: i32, z: i32) -> i32 {
        Self::get_top_y(self, heightmap, x, z)
    }
    fn top_block_height_exclusive(&self, x: i32, z: i32) -> i32 {
        Self::top_block_height_exclusive(self, x, z)
    }
    fn top_block_wg_height_exclusive(&self, x: i32, z: i32) -> i32 {
        Self::top_block_wg_height_exclusive(self, x, z)
    }
    fn ocean_floor_height_exclusive(&self, x: i32, z: i32) -> i32 {
        Self::ocean_floor_height_exclusive(self, x, z)
    }
    fn ocean_floor_wg_height_exclusive(&self, x: i32, z: i32) -> i32 {
        Self::ocean_floor_wg_height_exclusive(self, x, z)
    }
    fn is_air(&self, local_pos: &Vector3<i32>) -> bool {
        self.is_air(local_pos)
    }
    fn get_biome_for_terrain_gen(&self, x: i32, y: i32, z: i32) -> &'static Biome {
        let biome_pos = self.get_terrain_gen_biome_pos(x, y, z);
        Self::get_biome(self, biome_pos.x, biome_pos.y, biome_pos.z)
    }
    fn get_blending_data(
        &self,
        _cx: i32,
        _cz: i32,
    ) -> Option<&crate::generation::blender::blending_data::BlendingData> {
        None
    }
    fn get_sea_level(&self) -> i32 {
        Self::get_sea_level(self)
    }
}
