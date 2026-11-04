use std::sync::Arc;

use pumpkin_data::{Block, BlockState, Mirror, Rotation};
use pumpkin_util::{
    math::{block_box::BlockBox, position::BlockPos, vector3::Vector3},
    random::{RandomGenerator, RandomImpl},
};

use crate::{
    ProtoChunk,
    generation::{
        positions::chunk_pos::{start_block_x, start_block_z},
        proto_chunk::GenerationCache,
        structure::{
            piece::StructurePieceType,
            structures::{
                StructureGenerator, StructureGeneratorContext, StructurePiece, StructurePieceBase,
                StructurePiecesCollector, StructurePosition, WorldPortalExt,
            },
            template::{
                RegionPlacer, StructurePlaceSettings, StructureTemplate, block_entity_id,
                get_template, place_template_piece, processor::StructureProcessor, set_loot_table,
            },
        },
    },
};

pub const GENERATION_HEIGHT: i32 = 90;
pub const STRUCTURE_LOCATION_IGLOO: &str = "igloo/top";
pub const STRUCTURE_LOCATION_LADDER: &str = "igloo/middle";
pub const STRUCTURE_LOCATION_LABORATORY: &str = "igloo/bottom";
const LOOT_TABLE: &str = "minecraft:chests/igloo_chest";

#[must_use]
pub const fn get_pivot(location: &str) -> Vector3<i32> {
    if const_str_eq(location, STRUCTURE_LOCATION_IGLOO) {
        Vector3::new(3, 5, 5)
    } else if const_str_eq(location, STRUCTURE_LOCATION_LADDER) {
        Vector3::new(1, 3, 1)
    } else if const_str_eq(location, STRUCTURE_LOCATION_LABORATORY) {
        Vector3::new(3, 6, 7)
    } else {
        Vector3::new(0, 0, 0)
    }
}

#[must_use]
pub const fn get_offset(location: &str) -> Vector3<i32> {
    if const_str_eq(location, STRUCTURE_LOCATION_IGLOO) {
        Vector3::new(0, 0, 0)
    } else if const_str_eq(location, STRUCTURE_LOCATION_LADDER) {
        Vector3::new(2, -3, 4)
    } else if const_str_eq(location, STRUCTURE_LOCATION_LABORATORY) {
        Vector3::new(0, -3, -2)
    } else {
        Vector3::new(0, 0, 0)
    }
}

const fn const_str_eq(a: &str, b: &str) -> bool {
    let a = a.as_bytes();
    let b = b.as_bytes();
    if a.len() != b.len() {
        return false;
    }
    let mut i = 0;
    while i < a.len() {
        if a[i] != b[i] {
            return false;
        }
        i += 1;
    }
    true
}

pub fn add_pieces(
    collector: &mut StructurePiecesCollector,
    position: Vector3<i32>,
    rotation: Rotation,
    random: &mut RandomGenerator,
) {
    if random.next_f64() < 0.5 {
        let depth = random.next_bounded_i32(8) + 4;
        if let Some(template) = get_template(STRUCTURE_LOCATION_LABORATORY) {
            collector.add_piece(Box::new(IglooPiece::new(
                template,
                STRUCTURE_LOCATION_LABORATORY.to_string(),
                position,
                rotation,
                depth * 3,
            )));
        }

        for i in 0..(depth - 1) {
            if let Some(template) = get_template(STRUCTURE_LOCATION_LADDER) {
                collector.add_piece(Box::new(IglooPiece::new(
                    template,
                    STRUCTURE_LOCATION_LADDER.to_string(),
                    position,
                    rotation,
                    i * 3,
                )));
            }
        }
    }

    if let Some(template) = get_template(STRUCTURE_LOCATION_IGLOO) {
        collector.add_piece(Box::new(IglooPiece::new(
            template,
            STRUCTURE_LOCATION_IGLOO.to_string(),
            position,
            rotation,
            0,
        )));
    }
}

pub struct IglooGenerator;

impl StructureGenerator for IglooGenerator {
    fn get_structure_position(
        &self,
        mut context: StructureGeneratorContext<'_>,
    ) -> Option<StructurePosition> {
        let block_pos = Vector3::new(
            start_block_x(context.chunk_x),
            GENERATION_HEIGHT,
            start_block_z(context.chunk_z),
        );
        let rotation_idx = context.random.next_bounded_i32(4) as u8;
        let rotation = Rotation::from_index(rotation_idx);

        let mut collector = StructurePiecesCollector::default();
        add_pieces(&mut collector, block_pos, rotation, &mut context.random);

        Some(StructurePosition {
            start_pos: BlockPos::new(block_pos.x, block_pos.y, block_pos.z),
            collector: Arc::new(collector.into()),
        })
    }
}

pub struct IglooPiece {
    pub piece: StructurePiece,
    pub template: Arc<StructureTemplate>,
    pub template_name: String,
    pub place_settings: StructurePlaceSettings,
    pub template_position: Vector3<i32>,
}

impl IglooPiece {
    #[must_use]
    pub fn new(
        template: Arc<StructureTemplate>,
        template_name: String,
        position: Vector3<i32>,
        rotation: Rotation,
        depth: i32,
    ) -> Self {
        let place_settings = make_settings(rotation, &template_name);
        let template_position = make_position(&template_name, position, depth);
        let bounding_box = template.get_bounding_box(&place_settings, template_position);

        Self {
            piece: StructurePiece::new(StructurePieceType::Igloo, bounding_box, 0),
            template,
            template_name,
            place_settings,
            template_position,
        }
    }
}

impl StructurePieceBase for IglooPiece {
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
        block_registry: &dyn WorldPortalExt,
        random: &mut RandomGenerator,
        seed: i64,
        chunk_box: &BlockBox,
    ) {
        self.place_in_region(chunk, block_registry, random, seed, chunk_box);
    }

    /// Vanilla's `IglooPiece.postProcess`.
    fn place_in_region(
        &mut self,
        cache: &mut dyn GenerationCache,
        _block_registry: &dyn WorldPortalExt,
        random: &mut RandomGenerator,
        seed: i64,
        chunk_box: &BlockBox,
    ) {
        let settings = make_settings(self.place_settings.get_rotation(), &self.template_name);
        let offset = get_offset(&self.template_name);
        let entrance_rel = StructureTemplate::calculate_relative_position(
            &settings,
            Vector3::new(3 - offset.x, 0, -offset.z),
        );
        let entrance_pos = self.template_position + entrance_rel;
        let height = cache.get_top_y(
            &pumpkin_util::HeightMap::WorldSurfaceWg,
            entrance_pos.x,
            entrance_pos.z,
        );

        let old_template_pos = self.template_position;
        self.template_position.y += height - GENERATION_HEIGHT - 1;
        self.place_settings.bounding_box = Some(*chunk_box);
        self.piece.bounding_box = self
            .template
            .get_bounding_box(&self.place_settings, self.template_position);
        place_template_piece(
            &mut RegionPlacer(cache),
            &self.template,
            self.template_position,
            &self.place_settings,
            random,
            seed,
            |placer, random, marker, position| {
                if marker != "chest" {
                    return;
                }
                placer
                    .0
                    .set_block_state(&position, Block::AIR.default_state);
                let chunk = placer.0.get_center_chunk_mut();
                let below = Vector3::new(position.x, position.y - 1, position.z);
                // `ChestBlockEntity`, which the trapped chest's block entity extends.
                if matches!(
                    block_entity_id(Block::from_state_id(chunk.get_block_state(&below))),
                    Some("minecraft:chest" | "minecraft:trapped_chest")
                ) {
                    set_loot_table(chunk, below, LOOT_TABLE, random.next_i64());
                }
            },
        );

        if self.template_name == STRUCTURE_LOCATION_IGLOO {
            let trap_door_rel =
                StructureTemplate::calculate_relative_position(&settings, Vector3::new(3, 0, 5));
            let trap_door_pos = self.template_position + trap_door_rel;
            let below_pos = trap_door_pos - Vector3::new(0, 1, 0);
            let below_state =
                BlockState::from_id(GenerationCache::get_block_state(cache, &below_pos));
            if !below_state.is_air() && below_state.id.to_block_id() != Block::LADDER.id {
                cache.set_block_state(&trap_door_pos, Block::SNOW_BLOCK.default_state);
            }
        }

        self.template_position = old_template_pos;
    }
}

/// Vanilla's `IglooPiece.makeSettings`.
fn make_settings(rotation: Rotation, template_location: &str) -> StructurePlaceSettings {
    StructurePlaceSettings::new()
        .set_rotation(rotation)
        .set_mirror(Mirror::None)
        .set_rotation_pivot(get_pivot(template_location))
        .add_processor(StructureProcessor::ignore_structure_blocks())
        .set_apply_waterlogging(false)
}

fn make_position(template_location: &str, position: Vector3<i32>, depth: i32) -> Vector3<i32> {
    position + get_offset(template_location) - Vector3::new(0, depth, 0)
}
