mod grid;
mod placer;

use std::sync::{Arc, Mutex};

use pumpkin_data::{Block, Mirror, Rotation};
use pumpkin_nbt::{compound::NbtCompound, tag::NbtTag};
use pumpkin_util::{
    math::{block_box::BlockBox, position::BlockPos, vector3::Vector3},
    random::{RandomDeriverImpl, RandomGenerator, RandomImpl, xoroshiro128::Xoroshiro},
};

use crate::{
    ProtoChunk,
    generation::{
        positions::chunk_pos::{start_block_x, start_block_z},
        structure::{
            piece::StructurePieceType,
            structures::{
                StructureGenerator, StructureGeneratorContext, StructurePiece, StructurePieceBase,
                StructurePiecesCollector, StructurePosition,
            },
            template::{
                BlockStateResolver, PaletteEntry, StructureTemplate, get_block_entity_id,
                get_template, is_randomizable_container,
            },
        },
    },
    world::WorldPortalExt,
};

use self::{
    grid::MansionGrid,
    placer::{MansionPiecePlacer, PieceDescriptor},
};

const LOOT_TABLE: &str = "minecraft:chests/woodland_mansion";
/// The data markers; vanilla's `BlockIgnoreProcessor.STRUCTURE_BLOCK` keeps them out of the world.
const STRUCTURE_BLOCK: &str = "minecraft:structure_block";

pub struct MansionGenerator;

impl StructureGenerator for MansionGenerator {
    fn get_structure_position(
        &self,
        mut context: StructureGeneratorContext<'_>,
    ) -> Option<StructurePosition> {
        let rotation = Rotation::from_index(context.random.next_bounded_i32(4) as u8);
        let x = context.chunk_x * 16 + 7;
        let z = context.chunk_z * 16 + 7;
        let (offset_x, offset_z) = match rotation {
            Rotation::None => (5, 5),
            Rotation::Clockwise90 => (-5, 5),
            Rotation::Rotate180 => (-5, -5),
            Rotation::CounterClockwise90 => (5, -5),
        };
        let y = {
            let sampler = context.height_sampler.as_deref_mut()?;
            [
                sampler.estimate_height(x, z) - 1,
                sampler.estimate_height(x, z + offset_z) - 1,
                sampler.estimate_height(x + offset_x, z) - 1,
                sampler.estimate_height(x + offset_x, z + offset_z) - 1,
            ]
            .into_iter()
            .min()?
        };
        if y < 60 {
            return None;
        }

        let origin = Vector3::new(x, y, z);
        let grid = MansionGrid::new(&mut context.random);
        let descriptors = MansionPiecePlacer::create(&mut context.random, origin, rotation, &grid);
        let mut collector = StructurePiecesCollector::default();
        for descriptor in descriptors {
            collector.add_piece(Box::new(MansionTemplatePiece::new(&descriptor)?));
        }

        Some(StructurePosition {
            start_pos: BlockPos::new(x, y, z),
            collector: Arc::new(Mutex::new(collector)),
        })
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum TemplateMirror {
    #[default]
    None,
    LeftRight,
    FrontBack,
}

impl TemplateMirror {
    const fn state_mirror(self) -> Mirror {
        match self {
            Self::None => Mirror::None,
            Self::LeftRight => Mirror::LeftRight,
            Self::FrontBack => Mirror::FrontBack,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Direction {
    North,
    South,
    West,
    East,
    Up,
}

impl Direction {
    const HORIZONTAL: [Self; 4] = [Self::North, Self::East, Self::South, Self::West];

    const fn from_2d_index(index: i32) -> Self {
        [Self::South, Self::West, Self::North, Self::East][index as usize]
    }

    fn from_name(name: &str) -> Option<Self> {
        match name {
            "north" => Some(Self::North),
            "south" => Some(Self::South),
            "west" => Some(Self::West),
            "east" => Some(Self::East),
            "up" => Some(Self::Up),
            _ => None,
        }
    }

    const fn step_x(self) -> i32 {
        match self {
            Self::West => -1,
            Self::East => 1,
            _ => 0,
        }
    }

    const fn step_z(self) -> i32 {
        match self {
            Self::North => -1,
            Self::South => 1,
            _ => 0,
        }
    }

    const fn name(self) -> &'static str {
        match self {
            Self::North => "north",
            Self::South => "south",
            Self::West => "west",
            Self::East => "east",
            Self::Up => "up",
        }
    }

    const fn clockwise(self) -> Self {
        match self {
            Self::North => Self::East,
            Self::East => Self::South,
            Self::South => Self::West,
            Self::West => Self::North,
            Self::Up => Self::Up,
        }
    }

    const fn counterclockwise(self) -> Self {
        match self {
            Self::North => Self::West,
            Self::West => Self::South,
            Self::South => Self::East,
            Self::East => Self::North,
            Self::Up => Self::Up,
        }
    }

    const fn opposite(self) -> Self {
        match self {
            Self::North => Self::South,
            Self::South => Self::North,
            Self::West => Self::East,
            Self::East => Self::West,
            Self::Up => Self::Up,
        }
    }

    const fn rotate(self, rotation: Rotation) -> Self {
        match rotation {
            Rotation::None => self,
            Rotation::Clockwise90 => self.clockwise(),
            Rotation::Rotate180 => self.opposite(),
            Rotation::CounterClockwise90 => self.counterclockwise(),
        }
    }
}

trait RotateDirection {
    fn rotate(self, direction: Direction) -> Direction;
}

impl RotateDirection for Rotation {
    fn rotate(self, direction: Direction) -> Direction {
        direction.rotate(self)
    }
}

const fn offset(position: Vector3<i32>, x: i32, y: i32, z: i32) -> Vector3<i32> {
    Vector3::new(position.x + x, position.y + y, position.z + z)
}

const fn relative(position: Vector3<i32>, direction: Direction, distance: i32) -> Vector3<i32> {
    offset(
        position,
        direction.step_x() * distance,
        0,
        direction.step_z() * distance,
    )
}

struct MansionTemplatePiece {
    piece: StructurePiece,
    template: Arc<StructureTemplate>,
    template_position: Vector3<i32>,
    rotation: Rotation,
    mirror: TemplateMirror,
}

impl MansionTemplatePiece {
    fn new(descriptor: &PieceDescriptor) -> Option<Self> {
        let template = get_template(&format!("woodland_mansion/{}", descriptor.template))?;
        let max = Vector3::new(
            template.size.x - 1,
            template.size.y - 1,
            template.size.z - 1,
        );
        let first = transform(
            Vector3::new(0, 0, 0),
            descriptor.mirror,
            descriptor.rotation,
        );
        let second = transform(max, descriptor.mirror, descriptor.rotation);
        let bounding_box = BlockBox::new(
            descriptor.position.x + first.x.min(second.x),
            descriptor.position.y,
            descriptor.position.z + first.z.min(second.z),
            descriptor.position.x + first.x.max(second.x),
            descriptor.position.y + max.y,
            descriptor.position.z + first.z.max(second.z),
        );
        Some(Self {
            piece: StructurePiece::new(StructurePieceType::WoodlandMansion, bounding_box, 0),
            template,
            template_position: descriptor.position,
            rotation: descriptor.rotation,
            mirror: descriptor.mirror,
        })
    }

    const fn world_position(&self, local: Vector3<i32>) -> Vector3<i32> {
        let transformed = transform(local, self.mirror, self.rotation);
        offset(
            self.template_position,
            transformed.x,
            transformed.y,
            transformed.z,
        )
    }

    /// Vanilla's `TemplateStructurePiece.postProcess`: the template's blocks, then its data
    /// markers.
    fn place_in_chunk(
        &self,
        chunk: &mut ProtoChunk,
        random: &mut RandomGenerator,
        region_random: &mut RandomGenerator,
        chunk_box: &BlockBox,
    ) {
        self.place_blocks(chunk, random, chunk_box);
        self.handle_data_markers(chunk, random, region_random, chunk_box);
    }

    /// Vanilla's `StructureTemplate.placeInWorld` with the mansion's settings: every block inside
    /// the chunk but the structure blocks.
    fn place_blocks(
        &self,
        chunk: &mut ProtoChunk,
        random: &mut RandomGenerator,
        chunk_box: &BlockBox,
    ) {
        for template_block in &self.template.blocks {
            let palette = &self.template.palette[template_block.state as usize];
            if palette.name == STRUCTURE_BLOCK {
                continue;
            }
            let position = self.world_position(template_block.pos);
            if !chunk_box.contains_pos(&position) {
                continue;
            }
            let Some(mut state) =
                BlockStateResolver::resolve(palette, self.rotation, self.mirror.state_mirror())
            else {
                continue;
            };
            if chunk.get_block_state(&position).to_block_id() == Block::WATER.id
                && let Some(waterlogged) = state.set_waterlogged(true)
            {
                state = waterlogged;
            }
            chunk.set_block_state(position.x, position.y, position.z, state);
            Self::place_block_entity(
                chunk,
                palette,
                template_block.nbt.as_ref(),
                position,
                random,
            );
        }
    }

    fn place_block_entity(
        chunk: &mut ProtoChunk,
        palette: &PaletteEntry,
        template_nbt: Option<&NbtCompound>,
        position: Vector3<i32>,
        random: &mut RandomGenerator,
    ) {
        let block_entity_id = get_block_entity_id(&palette.name);
        if template_nbt.is_none() && block_entity_id.is_none() {
            return;
        }
        let mut nbt = NbtCompound::new();
        nbt.put_string("id", block_entity_id.unwrap_or(&palette.name).to_string());
        nbt.put_int("x", position.x);
        nbt.put_int("y", position.y);
        nbt.put_int("z", position.z);
        if let Some(template_nbt) = template_nbt {
            for (key, value) in &template_nbt.child_tags {
                if !matches!(key.as_ref(), "x" | "y" | "z" | "id") {
                    nbt.child_tags.insert(key.clone(), value.clone());
                }
            }
            // Vanilla draws the seed for every container with template data, also for one
            // without a loot table to roll it.
            if block_entity_id.is_some_and(is_randomizable_container) {
                let seed = random.next_i64();
                if nbt.get_string("LootTable").is_some() {
                    nbt.put_long("LootTableSeed", seed);
                }
            }
        }
        chunk.add_block_entity(nbt);
    }

    /// The template's data markers inside the chunk, in the template's order.
    fn handle_data_markers(
        &self,
        chunk: &mut ProtoChunk,
        random: &mut RandomGenerator,
        region_random: &mut RandomGenerator,
        chunk_box: &BlockBox,
    ) {
        for template_block in &self.template.blocks {
            if self.template.palette[template_block.state as usize].name != STRUCTURE_BLOCK {
                continue;
            }
            let Some(nbt) = &template_block.nbt else {
                continue;
            };
            let position = self.world_position(template_block.pos);
            if nbt.get_string("mode") == Some("DATA") && chunk_box.contains_pos(&position) {
                self.handle_data_marker(
                    chunk,
                    random,
                    region_random,
                    nbt.get_string("metadata").unwrap_or_default(),
                    position,
                );
            }
        }
    }

    /// Vanilla's `WoodlandMansionPiece.handleDataMarker`.
    fn handle_data_marker(
        &self,
        chunk: &mut ProtoChunk,
        random: &mut RandomGenerator,
        region_random: &mut RandomGenerator,
        marker: &str,
        position: Vector3<i32>,
    ) {
        if let Some(side) = marker.strip_prefix("Chest") {
            self.create_chest(chunk, random, side, position);
            return;
        }

        let (entity, count) = match marker {
            "Mage" => ("minecraft:evoker", 1),
            "Warrior" => ("minecraft:vindicator", 1),
            "Group of Allays" => ("minecraft:allay", region_random.next_bounded_i32(3) + 1),
            _ => return,
        };
        chunk.set_block_state(position.x, position.y, position.z, Block::AIR.default_state);
        for _ in 0..count {
            let mut nbt = NbtCompound::new();
            nbt.put_string("id", entity.to_string());
            nbt.put_list(
                "Pos",
                vec![
                    (f64::from(position.x) + 0.5).into(),
                    f64::from(position.y).into(),
                    (f64::from(position.z) + 0.5).into(),
                ],
            );
            nbt.put(
                "Motion",
                NbtTag::List(vec![0.0.into(), 0.0.into(), 0.0.into()]),
            );
            nbt.put("Rotation", NbtTag::List(vec![0.0f32.into(), 0.0f32.into()]));
            nbt.put_bool("PersistenceRequired", true);
            chunk.add_structure_entity(nbt, true);
        }
    }

    /// Vanilla's `StructurePiece.createChest`: a chest facing the marker's side turned with the
    /// piece (an unknown side keeps the default facing), unless a chest stands there already.
    fn create_chest(
        &self,
        chunk: &mut ProtoChunk,
        random: &mut RandomGenerator,
        side: &str,
        position: Vector3<i32>,
    ) {
        use pumpkin_data::block_properties::{Facing, Property};

        if chunk.get_block_state(&position).to_block_id() == Block::CHEST.id {
            return;
        }
        let direction = match side {
            "West" => Some(Direction::West),
            "East" => Some(Direction::East),
            "South" => Some(Direction::South),
            "North" => Some(Direction::North),
            _ => None,
        };
        let facing = match direction.map(|direction| self.rotation.rotate(direction)) {
            Some(Direction::West) => Facing::West,
            Some(Direction::East) => Facing::East,
            Some(Direction::South) => Facing::South,
            Some(Direction::North | Direction::Up) | None => Facing::North,
        };
        let chest = PaletteEntry::with_properties(
            "minecraft:chest".to_string(),
            vec![Property::Facing(facing)],
        );
        if let Some(state) = BlockStateResolver::resolve_simple(&chest) {
            chunk.set_block_state(position.x, position.y, position.z, state);
        }
        let mut nbt = NbtCompound::new();
        nbt.put_string("id", "minecraft:chest".to_string());
        nbt.put_int("x", position.x);
        nbt.put_int("y", position.y);
        nbt.put_int("z", position.z);
        nbt.put_string("LootTable", LOOT_TABLE.to_string());
        nbt.put_long("LootTableSeed", random.next_i64());
        chunk.add_block_entity(nbt);
    }
}

impl StructurePieceBase for MansionTemplatePiece {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn get_structure_piece(&self) -> &StructurePiece {
        &self.piece
    }

    fn get_structure_piece_mut(&mut self) -> &mut StructurePiece {
        &mut self.piece
    }

    fn place(
        &mut self,
        chunk: &mut ProtoChunk,
        _block_registry: &dyn WorldPortalExt,
        random: &mut RandomGenerator,
        seed: i64,
        chunk_box: &BlockBox,
    ) {
        let mut region_random = region_random(seed, chunk);
        self.place_in_chunk(chunk, random, &mut region_random, chunk_box);
    }
}

/// Stand-in for vanilla's `level.getRandom()` while generating, the region's random, which the
/// allay count draws from: the level's `minecraft:worldgen_region_random` factory at the chunk.
fn region_random(seed: i64, chunk: &ProtoChunk) -> RandomGenerator {
    RandomGenerator::Xoroshiro(Xoroshiro::from_seed(seed as u64))
        .next_splitter()
        .split_string("minecraft:worldgen_region_random")
        .next_splitter()
        .split_pos(start_block_x(chunk.x), 0, start_block_z(chunk.z))
}

/// Vanilla's `WoodlandMansionStructure.afterPlace`, the mansion's cobblestone foundation.
///
/// Below each block of the mansion's bottom layer in the chunk that a piece covers and that is not
/// air, cobblestone replaces the air and fluids down to the ground.
pub fn after_place(
    collector: &StructurePiecesCollector,
    chunk: &mut ProtoChunk,
    chunk_box: &BlockBox,
) {
    let Some(bounding_box) =
        BlockBox::encompass_all(collector.pieces.iter().map(|piece| piece.bounding_box()))
    else {
        return;
    };
    let y_start = bounding_box.min.y;
    let min_y = i32::from(chunk.bottom_y());
    for x in chunk_box.min.x..=chunk_box.max.x {
        for z in chunk_box.min.z..=chunk_box.max.z {
            let position = Vector3::new(x, y_start, z);
            if chunk.get_block_state(&position).to_state().is_air()
                || !bounding_box.contains_pos(&position)
                || !collector
                    .pieces
                    .iter()
                    .any(|piece| piece.bounding_box().contains_pos(&position))
            {
                continue;
            }
            for y in (min_y + 1..y_start).rev() {
                let state = chunk.get_block_state(&Vector3::new(x, y, z)).to_state();
                if !state.is_air() && !state.is_liquid() {
                    break;
                }
                chunk.set_block_state(x, y, z, Block::COBBLESTONE.default_state);
            }
        }
    }
}

#[cfg(test)]
fn transformed_palette(
    palette: &PaletteEntry,
    mirror: TemplateMirror,
    rotation: Rotation,
) -> PaletteEntry {
    let properties = pumpkin_data::transform_block_properties(
        &palette.name,
        &palette.properties,
        rotation,
        mirror.state_mirror(),
    );
    PaletteEntry::with_properties(palette.name.clone(), properties)
}

#[cfg(test)]
fn property(palette: &PaletteEntry, name: &str) -> Option<&'static str> {
    palette
        .properties
        .iter()
        .find_map(|p| (p.key_str() == name).then_some(p.value_str()))
}

const fn transform(
    position: Vector3<i32>,
    mirror: TemplateMirror,
    rotation: Rotation,
) -> Vector3<i32> {
    let x = if matches!(mirror, TemplateMirror::FrontBack) {
        -position.x
    } else {
        position.x
    };
    let z = if matches!(mirror, TemplateMirror::LeftRight) {
        -position.z
    } else {
        position.z
    };
    let (x, z) = rotation.rotate_offset(x, z);
    Vector3::new(x, position.y, z)
}

#[cfg(test)]
mod tests {
    use pumpkin_data::dimension::Dimension;
    use pumpkin_util::{
        random::{get_decorator_seed, legacy_rand::LegacyRand, worldgen_random::WorldgenRandom},
        world_seed::Seed,
    };

    use crate::generation::{
        get_world_gen,
        structure::structures::{HeightSampler, create_chunk_random},
    };

    use super::*;

    fn palette(name: &str, properties: &[(&str, &str)]) -> PaletteEntry {
        PaletteEntry::with_properties(
            name.to_string(),
            properties
                .iter()
                .filter_map(|(key, value)| pumpkin_data::Property::from_str_pair(key, value))
                .collect(),
        )
    }

    #[test]
    fn mirrors_directional_shapes_like_java() {
        let stairs = transformed_palette(
            &palette(
                "minecraft:dark_oak_stairs",
                &[("facing", "north"), ("shape", "inner_left")],
            ),
            TemplateMirror::LeftRight,
            Rotation::Clockwise90,
        );
        assert_eq!(property(&stairs, "shape"), Some("inner_right"));

        let door = transformed_palette(
            &palette(
                "minecraft:dark_oak_door",
                &[("facing", "east"), ("hinge", "left")],
            ),
            TemplateMirror::FrontBack,
            Rotation::None,
        );
        assert_eq!(property(&door, "hinge"), Some("right"));

        let rail = transformed_palette(
            &palette("minecraft:rail", &[("shape", "north_east")]),
            TemplateMirror::LeftRight,
            Rotation::Clockwise90,
        );
        assert_eq!(property(&rail, "shape"), Some("south_west"));
    }

    struct RecordingHeightSampler {
        calls: Vec<(i32, i32)>,
        heights: std::vec::IntoIter<i32>,
    }

    impl HeightSampler for RecordingHeightSampler {
        fn estimate_height(&mut self, block_x: i32, block_z: i32) -> i32 {
            self.calls.push((block_x, block_z));
            self.heights.next().unwrap()
        }
    }

    #[test]
    fn start_position_uses_java_five_by_five_corner_sampling() {
        let seed = 42;
        let mut expected_random = RandomGenerator::Legacy(LegacyRand::from_seed(seed));
        let rotation = Rotation::from_index(expected_random.next_bounded_i32(4) as u8);
        let (offset_x, offset_z) = match rotation {
            Rotation::None => (5, 5),
            Rotation::Clockwise90 => (-5, 5),
            Rotation::Rotate180 => (-5, -5),
            Rotation::CounterClockwise90 => (5, -5),
        };
        let mut sampler = RecordingHeightSampler {
            calls: Vec::new(),
            heights: vec![74, 70, 72, 73].into_iter(),
        };
        let position = MansionGenerator
            .get_structure_position(StructureGeneratorContext {
                seed: seed as i64,
                chunk_x: 3,
                chunk_z: -2,
                random: RandomGenerator::Legacy(LegacyRand::from_seed(seed)),
                sea_level: 63,
                min_y: -64,
                height: 384,
                height_sampler: Some(&mut sampler),
                structure_key: None,
            })
            .unwrap();
        let x = 3 * 16 + 7;
        let z = -2 * 16 + 7;
        assert_eq!(position.start_pos, BlockPos::new(x, 69, z));
        assert_eq!(
            sampler.calls,
            [
                (x, z),
                (x, z + offset_z),
                (x + offset_x, z),
                (x + offset_x, z + offset_z),
            ]
        );
    }

    #[test]
    fn vanilla_zero_pivot_transform_is_not_bounds_normalized() {
        assert_eq!(
            transform(
                Vector3::new(3, 2, 5),
                TemplateMirror::LeftRight,
                Rotation::Clockwise90,
            ),
            Vector3::new(5, 2, 3)
        );
        assert_eq!(
            transform(
                Vector3::new(3, 2, 5),
                TemplateMirror::FrontBack,
                Rotation::None,
            ),
            Vector3::new(-3, 2, 5)
        );
    }

    /// Chests of the woodland mansion of seed 1657324430 (start chunk 37, 105) in a world vanilla
    /// 26.3 generated: a marker chest's seed comes after those of the chests in the templates
    /// placed before it in the chunk.
    #[test]
    fn chest_loot_seeds_match_vanilla() {
        const SEED: i64 = 1_657_324_430;
        // The mansion's place among the surface structures of vanilla 26.3's registry.
        const DECORATION_INDEX: u64 = 23;
        const SURFACE_STRUCTURES_STEP: u64 = 4;

        let world_gen = get_world_gen(
            Seed(SEED as u64),
            Dimension::OVERWORLD,
            false,
            Vec::new(),
            String::new(),
        );
        let mut random = create_chunk_random(SEED, 37, 105);
        let rotation = Rotation::from_index(random.next_bounded_i32(4) as u8);
        let grid = MansionGrid::new(&mut random);
        let pieces =
            MansionPiecePlacer::create(&mut random, Vector3::new(599, 62, 1687), rotation, &grid)
                .iter()
                .map(MansionTemplatePiece::new)
                .collect::<Option<Vec<_>>>()
                .unwrap();

        for (chest, loot_table_seed) in [
            (Vector3::new(583, 88, 1704), 1_469_095_020_799_506_374),
            (Vector3::new(599, 77, 1739), -1_914_885_221_393_064_794),
            (Vector3::new(618, 68, 1727), -4_821_547_239_914_805_796),
        ] {
            let (chunk_x, chunk_z) = (chest.x >> 4, chest.z >> 4);
            let mut chunk = ProtoChunk::new(chunk_x, chunk_z, &world_gen);
            let chunk_box = BlockBox::new(
                start_block_x(chunk_x),
                -63,
                start_block_z(chunk_z),
                start_block_x(chunk_x) + 15,
                319,
                start_block_z(chunk_z) + 15,
            );
            let population_seed = WorldgenRandom::get_population_seed(
                SEED as u64,
                start_block_x(chunk_x),
                start_block_z(chunk_z),
            );
            let mut random = RandomGenerator::Worldgen(WorldgenRandom::from_seed(
                get_decorator_seed(population_seed, DECORATION_INDEX, SURFACE_STRUCTURES_STEP),
            ));
            let mut region_random = RandomGenerator::Legacy(LegacyRand::from_seed(0));
            for piece in &pieces {
                if piece.piece.bounding_box.intersects(&chunk_box) {
                    piece.place_in_chunk(&mut chunk, &mut random, &mut region_random, &chunk_box);
                }
            }
            let seeds = chunk
                .take_pending_block_entities()
                .iter()
                .filter(|nbt| {
                    [nbt.get_int("x"), nbt.get_int("y"), nbt.get_int("z")]
                        == [Some(chest.x), Some(chest.y), Some(chest.z)]
                })
                .filter_map(|nbt| nbt.get_long("LootTableSeed"))
                .collect::<Vec<_>>();
            assert_eq!(seeds, [loot_table_seed], "chest at {chest:?}");
        }
    }

    #[test]
    fn foundation_fills_air_and_fluids_below_covered_blocks() {
        let world_gen = get_world_gen(
            Seed(0),
            Dimension::OVERWORLD,
            false,
            Vec::new(),
            String::new(),
        );
        let mut chunk = ProtoChunk::new(0, 0, &world_gen);
        let chunk_box = BlockBox::new(0, -63, 0, 15, 319, 15);
        for x in 0..16 {
            for z in 0..16 {
                chunk.set_block_state(x, 60, z, Block::STONE.default_state);
            }
        }
        for y in 61..65 {
            chunk.set_block_state(5, y, 5, Block::WATER.default_state);
        }
        let roof = MansionTemplatePiece::new(&PieceDescriptor {
            template: "roof".to_string(),
            position: Vector3::new(4, 70, 4),
            rotation: Rotation::None,
            mirror: TemplateMirror::None,
        })
        .unwrap();
        let mut random = RandomGenerator::Legacy(LegacyRand::from_seed(0));
        let mut region_random = RandomGenerator::Legacy(LegacyRand::from_seed(0));
        roof.place_in_chunk(&mut chunk, &mut random, &mut region_random, &chunk_box);
        chunk.set_block_state(6, 70, 6, Block::AIR.default_state);
        chunk.set_block_state(2, 70, 2, Block::DARK_OAK_PLANKS.default_state);
        let mut collector = StructurePiecesCollector::default();
        collector.add_piece(Box::new(roof));

        after_place(&collector, &mut chunk, &chunk_box);

        let block = |x, y, z| chunk.get_block_state(&Vector3::new(x, y, z)).to_block_id();
        assert!((61..70).all(|y| block(5, y, 5) == Block::COBBLESTONE.id));
        assert_eq!(block(5, 60, 5), Block::STONE.id);
        // An empty block of the bottom layer, or one outside every piece, gets no foundation.
        assert_eq!(block(6, 69, 6), Block::AIR.id);
        assert_eq!(block(2, 69, 2), Block::AIR.id);
    }
}
