//! NBT Structure Template System
//!
//! This module provides functionality for loading and placing Minecraft structure
//! templates from `.nbt` files. This enables exact vanilla structure matching and
//! dramatically simplifies implementing structures like igloos, shipwrecks, villages, etc.
//!
//! # Architecture
//!
//! - [`StructureTemplate`]: Represents a loaded NBT template with size, palette, and blocks
//! - [`TemplatePiece`]: A structure piece that places blocks from a template
//! - [`Rotation`] and [`Mirror`]: Transform positions and block properties
//! - [`TemplateCache`]: Lazy-loading cache for embedded template files
//!
//! # Example Usage
//!
//! ```ignore
//! use pumpkin_world::generation::structure::template::{TemplateCache, TemplatePiece};
//! use pumpkin_data::Rotation;
//!
//! // Load a template from the cache
//! let template = TemplateCache::get("igloo/top").expect("Template not found");
//!
//! // Create a piece to place the template
//! let piece = TemplatePiece::new(template, rotation, mirror, position);
//! ```

mod block_state_resolver;
mod cache;
pub mod processor;
mod structure_template;
mod template_piece;

use pumpkin_data::BlockDirection;
use pumpkin_data::BlockStateId;
use pumpkin_data::Mirror;
use pumpkin_data::Rotation;
use pumpkin_data::fluid::Fluid;
use pumpkin_nbt::{compound::NbtCompound, tag::NbtTag};
use pumpkin_util::HeightMap;
use pumpkin_util::math::{vector3::Vector3, wrap_degrees};
use pumpkin_util::random::{RandomGenerator, RandomImpl, hash_block_pos, legacy_rand::LegacyRand};

use rustc_hash::FxHashSet;

use crate::generation::proto_chunk::{GenerationCache, post_process_pos};

pub use block_state_resolver::BlockStateResolver;
pub use cache::{
    TemplateCache, all_embedded_datapack_names, all_pool_names, all_structure_names,
    all_template_names, get_template, global_cache,
};
pub use processor::{
    StructureProcessor, clear_dynamic_processors, load_processor_list,
    register_dynamic_processor_list,
};
pub use pumpkin_data::{BlockState, Mirror as BlockMirror, Rotation as BlockRotation};
pub use structure_template::{
    JigsawBlockInfo, Palette, PaletteEntry, SimplePalette, StructureBlockInfo, StructureEntityInfo,
    StructurePlaceSettings, StructureTemplate, TemplateBlock, TemplateEntity, random_palette_index,
};
pub use template_piece::TemplatePiece;

/// Abstraction over block placement, implemented by both [`ProtoChunk`] (worldgen) and
/// [`WorldBlockPlacer`] (live `/place template` command).
pub trait BlockPlacer {
    fn get_block_state(&self, pos: &Vector3<i32>) -> BlockStateId;
    fn set_block_state(&mut self, pos: &Vector3<i32>, state: &BlockState);
    fn add_block_entity(&mut self, pos: &Vector3<i32>, nbt: NbtCompound);
    /// The first free y above the heightmap column, like vanilla `LevelReader.getHeight`, or
    /// `None` if this placer has no such heightmap.
    fn get_height(&self, heightmap: HeightMap, x: i32, z: i32) -> Option<i32>;
    /// Schedules a fluid tick, like vanilla's `scheduleTick`; a placer without ticks ignores it.
    fn schedule_fluid_tick(&mut self, _pos: &Vector3<i32>, _fluid: &'static Fluid, _delay: u8) {}
    /// Adds an entity of the template, which runs `finalizeSpawn` when `finalize` is set; a placer
    /// without entities ignores it.
    fn add_entity(&mut self, _nbt: NbtCompound, _finalize: bool) {}
    /// Vanilla's `ChunkAccess.markPosForPostProcessing`: once its chunk ticks, the block takes the
    /// shape its neighbours give it; a placer without generation post-processing ignores it.
    fn mark_pos_for_post_processing(&mut self, _pos: &Vector3<i32>) {}
}

/// A [`BlockPlacer`] over the generation region around a chunk, for the pieces vanilla lets write
/// into the neighbouring chunks.
pub(crate) struct RegionPlacer<'a>(pub &'a mut dyn GenerationCache);

impl BlockPlacer for RegionPlacer<'_> {
    fn get_block_state(&self, pos: &Vector3<i32>) -> BlockStateId {
        GenerationCache::get_block_state(self.0, pos)
    }

    fn set_block_state(&mut self, pos: &Vector3<i32>, state: &BlockState) {
        self.0.set_block_state_raw(pos, state);
    }

    fn add_block_entity(&mut self, pos: &Vector3<i32>, nbt: NbtCompound) {
        self.0.add_block_entity(pos, nbt);
    }

    fn get_height(&self, heightmap: HeightMap, x: i32, z: i32) -> Option<i32> {
        Some(self.0.get_top_y(&heightmap, x, z))
    }

    fn schedule_fluid_tick(&mut self, pos: &Vector3<i32>, fluid: &'static Fluid, delay: u8) {
        self.0.schedule_fluid_tick_at(pos, fluid, delay);
    }

    fn add_entity(&mut self, nbt: NbtCompound, finalize: bool) {
        self.0
            .get_center_chunk_mut()
            .add_structure_entity(nbt, finalize);
    }

    fn mark_pos_for_post_processing(&mut self, pos: &Vector3<i32>) {
        self.0.mark_pos_for_post_processing(pos);
    }
}

/// Whether the block holds a water source: still water or a waterlogged block.
fn holds_water_source(state_id: BlockStateId) -> bool {
    state_id == pumpkin_data::Block::WATER.default_state.id || state_id.is_waterlogged()
}

/// Whether the block holds a fluid source of any kind.
fn holds_fluid_source(state_id: BlockStateId) -> bool {
    holds_water_source(state_id) || state_id == pumpkin_data::Block::LAVA.default_state.id
}

/// Vanilla's `SimpleWaterloggedBlock.placeLiquid` for water from a source: the block becomes
/// waterlogged and the water ticks.
fn waterlog(placer: &mut impl BlockPlacer, pos: &Vector3<i32>, state: &'static BlockState) {
    if let Some(waterlogged) = state.set_waterlogged(true) {
        placer.set_block_state(pos, waterlogged);
        let water = &Fluid::WATER;
        placer.schedule_fluid_tick(pos, water, water.flow_speed as u8);
    }
}

/// Where vanilla's `StructureTemplate.placeInWorld` happens besides its settings.
///
/// The `referencePos` the processors' position rules measure from, the level seed a capped
/// processor's random starts from, and the flags of a jigsaw pool element.
#[derive(Clone, Copy, Debug)]
pub struct TemplatePlacement {
    pub reference: Vector3<i32>,
    pub level_seed: i64,
    /// Legacy pool elements ignore air after every processor.
    pub skip_air: bool,
    /// Jigsaw blocks stay instead of turning into their final state.
    pub keep_jigsaws: bool,
}

impl TemplatePlacement {
    /// The placement of a template structure piece, which turns its jigsaw blocks into their final
    /// state (vanilla's `TemplateStructurePiece.postProcess`).
    #[must_use]
    pub const fn piece(reference: Vector3<i32>, level_seed: i64) -> Self {
        Self {
            reference,
            level_seed,
            skip_air: false,
            keep_jigsaws: false,
        }
    }
}

/// One template block after the processors (vanilla's processed `StructureBlockInfo`).
struct ProcessedBlock {
    pos: Vector3<i32>,
    state: &'static BlockState,
    nbt: Option<NbtCompound>,
}

/// Vanilla's `StructureTemplate.placeInWorld`.
///
/// The blocks of the palette the position picks, moved to `position` with the settings' mirror,
/// rotation and pivot, run through the processors and kept to the settings' bounding box, then the
/// template's entities unless the settings ignore them. Returns `false`, like vanilla, when the
/// template has nothing to place.
#[expect(clippy::too_many_lines)]
pub fn place_template_with_options(
    placer: &mut impl BlockPlacer,
    template: &StructureTemplate,
    position: Vector3<i32>,
    settings: &StructurePlaceSettings,
    random: &mut RandomGenerator,
    placement: TemplatePlacement,
) -> bool {
    if template.palettes.is_empty() {
        return false;
    }
    let blocks = settings
        .get_random_palette(&template.palettes, position)
        .blocks();
    if (blocks.is_empty() && (settings.ignore_entities || template.entities.is_empty()))
        || template.size.x < 1
        || template.size.y < 1
        || template.size.z < 1
    {
        return false;
    }
    let (mirror, rotation, pivot) = (settings.mirror, settings.rotation, settings.rotation_pivot);

    let mut context = processor::ProcessorContext::new(placement.reference);

    // Vanilla `StructureTemplate.processBlockInfos`: every block of the template runs through the
    // processors, not only this chunk's, because a capped processor then picks among all of them.
    let mut processed = Vec::with_capacity(blocks.len());
    for block in blocks {
        let palette_entry = &block.state;

        // Structure blocks are data markers.
        if palette_entry.name == "minecraft:structure_block" {
            continue;
        }

        let mut block_entity_nbt = block.nbt.clone();
        let jigsaw_entry: PaletteEntry;
        let placed_entry = if !placement.keep_jigsaws && palette_entry.name == "minecraft:jigsaw" {
            // Jigsaw blocks are replaced during template processing, before block entities are
            // collected. Keeping this in the placement pipeline avoids stale jigsaw entities.
            let final_state = block_entity_nbt
                .as_ref()
                .and_then(|nbt| nbt.get_string("final_state"))
                .unwrap_or("minecraft:air");
            jigsaw_entry = PaletteEntry::from_string(final_state);
            block_entity_nbt = None;
            &jigsaw_entry
        } else {
            palette_entry
        };

        // Structure void preserves the existing block, both from the palette itself and from a
        // jigsaw final state.
        if placed_entry.name == "minecraft:structure_void" {
            continue;
        }

        // Vanilla's processors see the template's own state; the placement turns it afterwards.
        let Some(mut state) = BlockStateResolver::resolve_simple(placed_entry) else {
            continue;
        };

        let mut world_pos =
            position + StructureTemplate::transform_block_pos(block.pos, mirror, rotation, pivot);

        // Apply processors; each one seeds its own random from the block's position, like
        // vanilla's processors (`RandomSource.create(Mth.getSeed(pos))`).
        let mut should_place = true;
        for processor in settings.get_processors() {
            match processor {
                // Mirrors `GravityProcessor`: the block keeps its template y on top of the column.
                StructureProcessor::Gravity { heightmap, offset } => {
                    if let Some(height) = placer.get_height(*heightmap, world_pos.x, world_pos.z) {
                        world_pos.y = height + offset + block.pos.y;
                    }
                    continue;
                }
                // Vanilla runs a capped processor's delegate in `finalizeProcessing`, below.
                StructureProcessor::Capped { .. } => continue,
                _ => {}
            }
            let mut rng =
                LegacyRand::from_seed(hash_block_pos(world_pos.x, world_pos.y, world_pos.z) as u64);
            let Some(processed_state) = processor.process_with_context(
                placer,
                world_pos,
                state,
                &mut block_entity_nbt,
                &mut context,
                &mut rng,
            ) else {
                should_place = false;
                break;
            };
            state = processed_state;
        }
        if should_place {
            processed.push(ProcessedBlock {
                pos: world_pos,
                state,
                nbt: block_entity_nbt,
            });
        }
    }

    for processor in settings.get_processors() {
        if let StructureProcessor::Capped { limit, delegate } = processor {
            finalize_capped(
                placer,
                &mut processed,
                limit,
                delegate,
                &mut context,
                position,
                placement.level_seed,
            );
        }
    }

    // Vanilla's waterlogging: a block that holds a source locks it, a waterloggable block takes
    // the water source it replaces, and one that replaced no source fills from a neighbouring
    // source once the whole template stands.
    let mut locked_fluids = Vec::new();
    let mut to_fill = Vec::new();
    let mut placed = Vec::new();
    for ProcessedBlock {
        pos: world_pos,
        state,
        nbt: block_entity_nbt,
    } in processed
    {
        if settings
            .bounding_box
            .is_some_and(|bbox| !bbox.contains_pos(&world_pos))
        {
            continue;
        }
        // Legacy pool elements ignore air after jigsaw replacement and custom processors.
        if placement.skip_air && state.id.to_block_id() == pumpkin_data::Block::AIR.id {
            continue;
        }
        let state = state.mirror(mirror).rotate(rotation);

        let previous = settings
            .apply_waterlogging
            .then(|| placer.get_block_state(&world_pos));
        placer.set_block_state(&world_pos, state);
        if !settings.known_shape {
            // Vanilla's `WorldGenRegion.setBlock` without `UPDATE_KNOWN_SHAPE`: the block marks
            // its `getPostProcessPos`.
            if let Some(mark) = post_process_pos(state, &world_pos) {
                placer.mark_pos_for_post_processing(&mark);
            }
            placed.push(world_pos);
        }
        if let Some(previous) = previous {
            if holds_fluid_source(state.id) {
                locked_fluids.push(world_pos);
            } else if state.set_waterlogged(true).is_some() {
                if holds_water_source(previous) {
                    waterlog(placer, &world_pos, state);
                }
                if !holds_fluid_source(previous) {
                    to_fill.push(world_pos);
                }
            }
        }

        // Create block entities for interactive blocks (furnaces, chests, etc.)
        let final_block = pumpkin_data::Block::from_id(state.id.to_block_id());
        let block_entity_id = get_block_entity_id(final_block.name);
        if block_entity_nbt.is_some() || block_entity_id.is_some() {
            let fallback_id = block_entity_id.unwrap_or(final_block.name);
            let mut placed_nbt = NbtCompound::new();

            placed_nbt.put_string("id", fallback_id.to_string());
            placed_nbt.put_int("x", world_pos.x);
            placed_nbt.put_int("y", world_pos.y);
            placed_nbt.put_int("z", world_pos.z);

            if let Some(template_nbt) = &block_entity_nbt {
                for (key, value) in &template_nbt.child_tags {
                    if key.as_ref() != "x" && key.as_ref() != "y" && key.as_ref() != "z" {
                        placed_nbt.child_tags.insert(key.clone(), value.clone());
                    }
                }
            }

            // Vanilla draws a loot seed for every container the template gives data, from the
            // placement's random in placement order; one without a loot table does not keep it.
            if block_entity_nbt.is_some() && block_entity_id.is_some_and(is_randomizable_container)
            {
                let seed = random.next_i64();
                if placed_nbt.get_string("LootTable").is_some() {
                    placed_nbt.put_long("LootTableSeed", seed);
                }
            }

            placer.add_block_entity(&world_pos, placed_nbt);
        }
    }

    let mut filled = true;
    while filled && !to_fill.is_empty() {
        filled = false;
        to_fill.retain(|pos: &Vector3<i32>| {
            let current = placer.get_block_state(pos);
            let mut source = holds_fluid_source(current).then_some(current);
            for offset in [(0, 1, 0), (0, 0, -1), (1, 0, 0), (0, 0, 1), (-1, 0, 0)] {
                if source.is_some() {
                    break;
                }
                let neighbor = Vector3::new(pos.x + offset.0, pos.y + offset.1, pos.z + offset.2);
                let neighbor_state = placer.get_block_state(&neighbor);
                if holds_fluid_source(neighbor_state) && !locked_fluids.contains(&neighbor) {
                    source = Some(neighbor_state);
                }
            }
            let Some(source) = source else {
                return true;
            };
            if holds_water_source(source) {
                waterlog(placer, pos, current.to_state());
            }
            filled = true;
            false
        });
    }

    if !settings.known_shape {
        mark_shape_updates(placer, &placed);
    }

    if !settings.ignore_entities {
        place_entities(placer, template, position, settings);
    }
    true
}

/// Vanilla's shape updates of a placement whose shape is not known: `updateShapeAtEdge` on the
/// faces between the placed blocks and the rest of the world, then
/// `Block.updateFromNeighbourShapes` on every placed block.
///
/// The block behaviours need the running world, so the neighbours across those faces and then the
/// placed blocks are marked for post-processing, which updates their shapes once their chunk
/// ticks. A neighbour takes the shape all its neighbours give it, not only the placed one.
fn mark_shape_updates(placer: &mut impl BlockPlacer, placed: &[Vector3<i32>]) {
    let placed_set: FxHashSet<Vector3<i32>> = placed.iter().copied().collect();
    let mut edges = FxHashSet::default();
    for pos in placed {
        for direction in BlockDirection::all() {
            let neighbor = *pos + direction.to_offset();
            if !placed_set.contains(&neighbor) && edges.insert(neighbor) {
                placer.mark_pos_for_post_processing(&neighbor);
            }
        }
    }
    for pos in placed {
        placer.mark_pos_for_post_processing(pos);
    }
}

/// Vanilla's `TemplateStructurePiece.postProcess` once the piece has moved its template: the
/// template inside the settings' bounding box, the chunk, then each data marker there, which
/// `handle_data_marker` gets with its metadata and position.
pub(crate) fn place_template_piece<P: BlockPlacer>(
    placer: &mut P,
    template: &StructureTemplate,
    position: Vector3<i32>,
    settings: &StructurePlaceSettings,
    random: &mut RandomGenerator,
    level_seed: i64,
    mut handle_data_marker: impl FnMut(&mut P, &mut RandomGenerator, &str, Vector3<i32>),
) {
    if !place_template_with_options(
        placer,
        template,
        position,
        settings,
        random,
        TemplatePlacement::piece(position, level_seed),
    ) {
        return;
    }
    for marker in template.filter_blocks(position, settings, "minecraft:structure_block") {
        if let Some(nbt) = &marker.nbt
            && nbt.get_string("mode") == Some("DATA")
        {
            handle_data_marker(
                placer,
                random,
                nbt.get_string("metadata").unwrap_or_default(),
                marker.pos,
            );
        }
    }
}

/// Vanilla's `RandomizableContainer.setBlockEntityLootTable`: the container at the position, if
/// there is one, gets the loot table and a seed from the random.
pub(crate) fn set_block_entity_loot_table(
    chunk: &mut crate::ProtoChunk,
    random: &mut RandomGenerator,
    pos: Vector3<i32>,
    loot_table: &str,
) {
    let block = pumpkin_data::Block::from_state_id(chunk.get_block_state(&pos));
    if block_entity_id(block).is_some_and(is_randomizable_container) {
        set_loot_table(chunk, pos, loot_table, random.next_i64());
    }
}

/// Vanilla's `RandomizableContainer.setLootTable` on the block entity at the position: its last
/// pending entry, or a new one.
pub(crate) fn set_loot_table(
    chunk: &mut crate::ProtoChunk,
    pos: Vector3<i32>,
    loot_table: &str,
    seed: i64,
) {
    let at_pos = |nbt: &NbtCompound| {
        [nbt.get_int("x"), nbt.get_int("y"), nbt.get_int("z")]
            == [Some(pos.x), Some(pos.y), Some(pos.z)]
    };
    if let Some(nbt) = chunk
        .pending_block_entities
        .iter_mut()
        .rev()
        .find(|nbt| at_pos(nbt))
    {
        nbt.put_string("LootTable", loot_table.to_string());
        nbt.put_long("LootTableSeed", seed);
        return;
    }
    let block = pumpkin_data::Block::from_state_id(chunk.get_block_state(&pos));
    let Some(id) = block_entity_id(block) else {
        return;
    };
    let mut nbt = NbtCompound::new();
    nbt.put_string("id", id.to_string());
    nbt.put_int("x", pos.x);
    nbt.put_int("y", pos.y);
    nbt.put_int("z", pos.z);
    nbt.put_string("LootTable", loot_table.to_string());
    nbt.put_long("LootTableSeed", seed);
    chunk.add_block_entity(nbt);
}

/// Vanilla's `CappedProcessor.finalizeProcessing`: a random forked from the level seed at the
/// template's position draws the limit and shuffles every processed block; the delegate runs on
/// them in that order until it changed `limit` of them.
fn finalize_capped(
    placer: &impl BlockPlacer,
    processed: &mut [ProcessedBlock],
    limit: &pumpkin_util::math::int_provider::IntProvider,
    delegate: &StructureProcessor,
    context: &mut processor::ProcessorContext,
    position: Vector3<i32>,
    level_seed: i64,
) {
    if limit.get_max() == 0 || processed.is_empty() {
        return;
    }
    let mut random = pumpkin_util::random::RandomDeriverImpl::split_pos(
        &LegacyRand::from_seed(level_seed as u64).next_splitter(),
        position.x,
        position.y,
        position.z,
    );
    let max_to_replace = limit.get(&mut random).min(processed.len() as i32);
    if max_to_replace < 1 {
        return;
    }
    let mut indices: Vec<usize> = (0..processed.len()).collect();
    for i in (2..=indices.len()).rev() {
        let swap_to = random.next_bounded_i32(i as i32) as usize;
        indices.swap(i - 1, swap_to);
    }
    let mut replaced = 0;
    for index in indices {
        if replaced >= max_to_replace {
            break;
        }
        let block = &mut processed[index];
        let mut nbt = block.nbt.clone();
        let mut rng =
            LegacyRand::from_seed(hash_block_pos(block.pos.x, block.pos.y, block.pos.z) as u64);
        if let Some(state) = delegate.process_with_context(
            placer,
            block.pos,
            block.state,
            &mut nbt,
            context,
            &mut rng,
        ) && (state.id != block.state.id || nbt != block.nbt)
        {
            replaced += 1;
            block.state = state;
            block.nbt = nbt;
        }
    }
}

/// Vanilla's `StructureTemplate.placeEntities`: every entity whose block lies in the settings'
/// bounding box, moved, mirrored and turned with the template.
fn place_entities(
    placer: &mut impl BlockPlacer,
    template: &StructureTemplate,
    position: Vector3<i32>,
    settings: &StructurePlaceSettings,
) {
    let (mirror, rotation, pivot) = (settings.mirror, settings.rotation, settings.rotation_pivot);
    for entity in &template.entities {
        let block_pos = position
            + StructureTemplate::transform_block_pos(entity.block_pos, mirror, rotation, pivot);
        if settings
            .bounding_box
            .is_some_and(|bbox| !bbox.contains_pos(&block_pos))
        {
            continue;
        }

        let pos = StructureTemplate::transform_vec3(entity.pos, mirror, rotation, pivot)
            + position.to_f64();
        let mut nbt = entity.nbt.clone();
        nbt.put(
            "Pos",
            NbtTag::List(vec![pos.x.into(), pos.y.into(), pos.z.into()]),
        );
        nbt.child_tags.remove("UUID");

        // `Entity.rotate` and `Entity.mirror` on the template's yaw.
        if let Some(rotation_nbt) = nbt.get_list("Rotation")
            && rotation_nbt.len() == 2
        {
            let yaw = rotation_nbt[0].extract_float().unwrap_or_default();
            let wrapped = wrap_degrees(yaw);
            let rotated = wrapped
                + match rotation {
                    Rotation::None => 0.0,
                    Rotation::Clockwise90 => 90.0,
                    Rotation::Rotate180 => 180.0,
                    Rotation::CounterClockwise90 => 270.0,
                };
            let mirrored = match mirror {
                Mirror::None => wrapped,
                Mirror::FrontBack => -wrapped,
                Mirror::LeftRight => 180.0 - wrapped,
            };
            let pitch = rotation_nbt[1].extract_float().unwrap_or_default();
            nbt.put(
                "Rotation",
                NbtTag::List(vec![(rotated + mirrored - yaw).into(), pitch.into()]),
            );
        }

        placer.add_entity(nbt, settings.finalize_entities);
    }
}

/// The block entity a block owns, as the id its NBT carries (`minecraft:<type>`): from the block's
/// state data like the server's `block_entity_name`. Beds have none in vanilla 26.3, though
/// the server gives a placed bed one for Bedrock and older Java clients.
pub(crate) fn get_block_entity_id(block_name: &str) -> Option<&'static str> {
    let name = block_name.strip_prefix("minecraft:").unwrap_or(block_name);
    block_entity_id(pumpkin_data::Block::from_name(name)?)
}

/// [`get_block_entity_id`] for a block.
pub(crate) fn block_entity_id(block: &pumpkin_data::Block) -> Option<&'static str> {
    static IDS: std::sync::LazyLock<Box<[String]>> = std::sync::LazyLock::new(|| {
        pumpkin_data::block_properties::BLOCK_ENTITY_TYPES
            .iter()
            .map(|name| format!("minecraft:{name}"))
            .collect()
    });
    IDS.get(usize::from(block.default_state.block_entity_type))
        .map(String::as_str)
}

/// Whether the block entity is a vanilla `RandomizableContainer`, which draws a loot table seed
/// from the structure's random when a template places it with its data.
pub(crate) fn is_randomizable_container(block_entity_id: &str) -> bool {
    matches!(
        block_entity_id,
        "minecraft:barrel"
            | "minecraft:chest"
            | "minecraft:crafter"
            | "minecraft:decorated_pot"
            | "minecraft:dispenser"
            | "minecraft:dropper"
            | "minecraft:hopper"
            | "minecraft:shulker_box"
            | "minecraft:trapped_chest"
    )
}

/// Vanilla's `DUMMY` block entity: a block entity block set during generation gets an empty
/// block entity, which the chunk turns into the block's default one. A caller with real data
/// adds it afterwards, and the last entry for a position wins.
#[must_use]
pub(crate) fn dummy_block_entity(
    state: &pumpkin_data::BlockState,
    pos: &Vector3<i32>,
) -> Option<NbtCompound> {
    let id = block_entity_id(pumpkin_data::Block::from_state_id(state.id))?;
    let mut nbt = NbtCompound::new();
    nbt.put_string("id", id.to_string());
    nbt.put_int("x", pos.x);
    nbt.put_int("y", pos.y);
    nbt.put_int("z", pos.z);
    Some(nbt)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pumpkin_data::Block;

    struct CollectingPlacer(Vec<BlockStateId>);

    impl BlockPlacer for CollectingPlacer {
        fn get_block_state(&self, _pos: &Vector3<i32>) -> BlockStateId {
            Block::AIR.default_state.id
        }

        fn set_block_state(&mut self, _pos: &Vector3<i32>, state: &BlockState) {
            self.0.push(state.id);
        }

        fn add_block_entity(&mut self, _pos: &Vector3<i32>, _nbt: NbtCompound) {}

        fn get_height(&self, _heightmap: HeightMap, _x: i32, _z: i32) -> Option<i32> {
            None
        }
    }

    #[test]
    fn structure_void_is_never_placed() {
        let mut placed_templates = 0;

        for name in all_template_names() {
            let Some(template) = get_template(name) else {
                continue;
            };

            let mut placer = CollectingPlacer(Vec::new());
            place_template_with_options(
                &mut placer,
                &template,
                Vector3::new(0, 0, 0),
                &StructurePlaceSettings::new(),
                &mut RandomGenerator::Legacy(LegacyRand::from_seed(0)),
                TemplatePlacement::piece(Vector3::new(0, 0, 0), 0),
            );

            for state_id in &placer.0 {
                assert_ne!(
                    state_id.to_block_id(),
                    Block::STRUCTURE_VOID.id,
                    "{name} placed a structure void"
                );
            }

            placed_templates += 1;
        }

        assert!(placed_templates > 0);
    }

    /// A world of still water that records what a placement writes and marks.
    struct WaterPlacer {
        blocks: rustc_hash::FxHashMap<Vector3<i32>, BlockStateId>,
        marks: FxHashSet<Vector3<i32>>,
    }

    impl WaterPlacer {
        fn new() -> Self {
            Self {
                blocks: rustc_hash::FxHashMap::default(),
                marks: FxHashSet::default(),
            }
        }
    }

    impl BlockPlacer for WaterPlacer {
        fn get_block_state(&self, pos: &Vector3<i32>) -> BlockStateId {
            self.blocks
                .get(pos)
                .copied()
                .unwrap_or(Block::WATER.default_state.id)
        }

        fn set_block_state(&mut self, pos: &Vector3<i32>, state: &BlockState) {
            self.blocks.insert(*pos, state.id);
        }

        fn add_block_entity(&mut self, _pos: &Vector3<i32>, _nbt: NbtCompound) {}

        fn get_height(&self, _heightmap: HeightMap, _x: i32, _z: i32) -> Option<i32> {
            None
        }

        fn mark_pos_for_post_processing(&mut self, pos: &Vector3<i32>) {
            self.marks.insert(*pos);
        }
    }

    /// The shipwreck of the report: its template keeps dry `outer_right` stairs. Vanilla's
    /// placement waterlogs the ones that replace water and, as the shape is not known, updates the
    /// shape of every placed block and of the blocks around it, which post-processing does here.
    #[test]
    fn unknown_shapes_are_waterlogged_and_marked_for_post_processing() {
        let template = get_template("shipwreck/rightsideup_full").unwrap();
        let position = Vector3::new(3, 40, -7);
        let settings = StructurePlaceSettings::new()
            .set_rotation(Rotation::Clockwise90)
            .set_rotation_pivot(Vector3::new(4, 0, 15))
            .set_apply_waterlogging(true)
            .add_processor(StructureProcessor::ignore_structure_blocks_and_air());
        let place = |settings: &StructurePlaceSettings| {
            let mut placer = WaterPlacer::new();
            assert!(place_template_with_options(
                &mut placer,
                &template,
                position,
                settings,
                &mut RandomGenerator::Legacy(LegacyRand::from_seed(0)),
                TemplatePlacement::piece(position, 0),
            ));
            placer
        };

        let placer = place(&settings);
        let stairs = placer
            .blocks
            .values()
            .filter(|id| {
                id.to_block_id()
                    .has_tag(pumpkin_data::tag::Block::MINECRAFT_STAIRS)
            })
            .collect::<Vec<_>>();
        assert!(!stairs.is_empty());
        assert!(stairs.iter().all(|id| id.is_waterlogged()));
        assert!(placer.blocks.keys().all(|pos| placer.marks.contains(pos)));
        assert!(
            placer
                .marks
                .iter()
                .any(|pos| !placer.blocks.contains_key(pos))
        );
        assert!(placer.marks.iter().all(|pos| {
            placer.blocks.contains_key(pos)
                || BlockDirection::all()
                    .iter()
                    .any(|direction| placer.blocks.contains_key(&(*pos + direction.to_offset())))
        }));

        let known = place(&settings.clone().set_known_shape(true));
        assert!(known.marks.is_empty());
    }
}
