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

use pumpkin_data::BlockStateId;
use pumpkin_data::Mirror;
use pumpkin_data::Rotation;
use pumpkin_data::fluid::Fluid;
use pumpkin_nbt::{compound::NbtCompound, tag::NbtTag};
use pumpkin_util::HeightMap;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_util::random::{RandomGenerator, RandomImpl, hash_block_pos, legacy_rand::LegacyRand};

use crate::ProtoChunk;

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
    fn add_block_entity(&mut self, nbt: NbtCompound);
    /// The first free y above the heightmap column, like vanilla `LevelReader.getHeight`, or
    /// `None` if this placer has no such heightmap.
    fn get_height(&self, heightmap: HeightMap, x: i32, z: i32) -> Option<i32>;
    /// Schedules a fluid tick, like vanilla's `scheduleTick`; a placer without ticks ignores it.
    fn schedule_fluid_tick(&mut self, _pos: &Vector3<i32>, _fluid: &'static Fluid, _delay: u8) {}
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

/// Places a template at a world origin with an un-rotated XZ offset.
///
/// All rotation is handled internally:
/// - The offset is rotated to position the template correctly
/// - Block positions within the template are rotated
/// - Directional block properties (facing, axis, etc.) are rotated
/// - Block entities are created from template NBT data
///
/// `origin` is the base world position (x, y, z).
/// `offset` is the un-rotated XZ offset from origin (`x_offset`, `z_offset`) - rotation is applied automatically.
#[allow(clippy::too_many_arguments)]
pub fn place_template(
    placer: &mut impl BlockPlacer,
    template: &StructureTemplate,
    origin: Vector3<i32>,
    offset: (i32, i32),
    rotation: Rotation,
    skip_air: bool,
    apply_waterlogging: bool,
    processors: &[StructureProcessor],
    chunk_box: Option<&pumpkin_util::math::block_box::BlockBox>,
) {
    place_template_with_options(
        placer,
        template,
        origin,
        offset,
        rotation,
        skip_air,
        apply_waterlogging,
        processors,
        chunk_box,
        false,
        &mut RandomGenerator::Legacy(LegacyRand::from_seed(hash_block_pos(
            origin.x, origin.y, origin.z,
        ) as u64)),
        TemplatePlacement {
            position: origin,
            level_seed: 0,
        },
    );
}

/// Where vanilla's `StructureTemplate.placeInWorld` happens: its `position`, at which a capped
/// processor's random is forked, and the level seed that random starts from.
#[derive(Clone, Copy, Debug)]
pub struct TemplatePlacement {
    pub position: Vector3<i32>,
    pub level_seed: i64,
}

/// One template block after the processors (vanilla's processed `StructureBlockInfo`).
struct ProcessedBlock {
    pos: Vector3<i32>,
    state: &'static BlockState,
    nbt: Option<NbtCompound>,
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
pub fn place_template_with_options(
    placer: &mut impl BlockPlacer,
    template: &StructureTemplate,
    origin: Vector3<i32>,
    offset: (i32, i32),
    rotation: Rotation,
    skip_air: bool,
    apply_waterlogging: bool,
    processors: &[StructureProcessor],
    chunk_box: Option<&pumpkin_util::math::block_box::BlockBox>,
    keep_jigsaws: bool,
    random: &mut RandomGenerator,
    placement: TemplatePlacement,
) {
    let (rotated_ox, rotated_oz) = rotation.rotate_offset(offset.0, offset.1);
    let world_x = origin.x + rotated_ox;
    let world_z = origin.z + rotated_oz;

    let mut context = processor::ProcessorContext::new(origin);

    // Vanilla `StructureTemplate.processBlockInfos`: every block of the template runs through the
    // processors, not only this chunk's, because a capped processor then picks among all of them.
    let mut processed = Vec::with_capacity(template.blocks.len());
    for block in &template.blocks {
        // Rotate block position within template bounds
        let local_pos = rotation.transform_pos(block.pos, template.size);

        let palette_entry = &template.palette[block.state as usize];

        // Structure blocks are data markers.
        if palette_entry.name == "minecraft:structure_block" {
            continue;
        }

        let mut block_entity_nbt = block.nbt.clone();
        let jigsaw_entry: PaletteEntry;
        let placed_entry = if !keep_jigsaws && palette_entry.name == "minecraft:jigsaw" {
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

        // Resolve block state with rotation applied to directional properties
        let Some(mut state) =
            BlockStateResolver::resolve(placed_entry, rotation, Mirror::default())
        else {
            continue;
        };

        let mut world_pos = Vector3::new(
            world_x + local_pos.x,
            origin.y + local_pos.y,
            world_z + local_pos.z,
        );

        // Apply processors; each one seeds its own random from the block's position, like
        // vanilla's processors (`RandomSource.create(Mth.getSeed(pos))`).
        let mut should_place = true;
        for processor in processors {
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

    for processor in processors {
        if let StructureProcessor::Capped { limit, delegate } = processor {
            finalize_capped(
                placer,
                &mut processed,
                limit,
                delegate,
                &mut context,
                placement,
            );
        }
    }

    // Vanilla's waterlogging: a block that holds a source locks it, a waterloggable block takes
    // the water source it replaces, and one that replaced no source fills from a neighbouring
    // source once the whole template stands.
    let mut locked_fluids = Vec::new();
    let mut to_fill = Vec::new();
    for ProcessedBlock {
        pos: world_pos,
        state,
        nbt: block_entity_nbt,
    } in processed
    {
        if chunk_box.is_some_and(|bbox| !bbox.contains_pos(&world_pos)) {
            continue;
        }
        // Legacy pool elements ignore air after jigsaw replacement and custom processors.
        if skip_air && state.id.to_block_id() == pumpkin_data::Block::AIR.id {
            continue;
        }

        let previous = apply_waterlogging.then(|| placer.get_block_state(&world_pos));
        placer.set_block_state(&world_pos, state);
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

            placer.add_block_entity(placed_nbt);
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
    placement: TemplatePlacement,
) {
    if limit.get_max() == 0 || processed.is_empty() {
        return;
    }
    let mut random = pumpkin_util::random::RandomDeriverImpl::split_pos(
        &LegacyRand::from_seed(placement.level_seed as u64).next_splitter(),
        placement.position.x,
        placement.position.y,
        placement.position.z,
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

pub(crate) fn place_template_entities(
    chunk: &mut ProtoChunk,
    template: &StructureTemplate,
    origin: Vector3<i32>,
    rotation: Rotation,
    chunk_box: &pumpkin_util::math::block_box::BlockBox,
    finalize_entities: bool,
) {
    for entity in &template.entities {
        let block_pos = rotation.transform_pos(entity.block_pos, template.size);
        let world_block_pos = Vector3::new(
            origin.x + block_pos.x,
            origin.y + block_pos.y,
            origin.z + block_pos.z,
        );
        if !chunk_box.contains_pos(&world_block_pos) {
            continue;
        }

        let pos = match rotation {
            Rotation::None => entity.pos,
            Rotation::Clockwise90 => Vector3::new(
                f64::from(template.size.z) - entity.pos.z,
                entity.pos.y,
                entity.pos.x,
            ),
            Rotation::Rotate180 => Vector3::new(
                f64::from(template.size.x) - entity.pos.x,
                entity.pos.y,
                f64::from(template.size.z) - entity.pos.z,
            ),
            Rotation::CounterClockwise90 => Vector3::new(
                entity.pos.z,
                entity.pos.y,
                f64::from(template.size.x) - entity.pos.x,
            ),
        };
        let mut nbt = entity.nbt.clone();
        nbt.put(
            "Pos",
            NbtTag::List(vec![
                (f64::from(origin.x) + pos.x).into(),
                (f64::from(origin.y) + pos.y).into(),
                (f64::from(origin.z) + pos.z).into(),
            ]),
        );
        nbt.child_tags.remove("UUID");

        if let Some(rotation_nbt) = nbt.get_list("Rotation")
            && rotation_nbt.len() == 2
        {
            let yaw = rotation_nbt[0].extract_float().unwrap_or_default()
                + match rotation {
                    Rotation::None => 0.0,
                    Rotation::Clockwise90 => 90.0,
                    Rotation::Rotate180 => 180.0,
                    Rotation::CounterClockwise90 => 270.0,
                };
            let pitch = rotation_nbt[1].extract_float().unwrap_or_default();
            nbt.put("Rotation", NbtTag::List(vec![yaw.into(), pitch.into()]));
        }

        chunk.add_structure_entity(nbt, finalize_entities);
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

        fn add_block_entity(&mut self, _nbt: NbtCompound) {}

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
            place_template(
                &mut placer,
                &template,
                Vector3::new(0, 0, 0),
                (0, 0),
                Rotation::None,
                false,
                false,
                &[],
                None,
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
}
