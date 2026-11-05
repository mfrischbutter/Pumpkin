use std::sync::Arc;

use pumpkin_data::{Mirror, Rotation};
use pumpkin_util::{
    HeightMap,
    math::{block_box::BlockBox, vector3::Vector3},
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
                StructurePiecesCollector, StructurePosition, WorldPortalExt, chunk_center_start,
            },
            template::{
                RegionPlacer, StructurePlaceSettings, StructureTemplate, get_template,
                place_template_piece, processor::StructureProcessor, set_block_entity_loot_table,
            },
        },
    },
};

const NUMBER_OF_BLOCKS_ALLOWED_IN_WORLD_GEN_REGION: i32 = 32;
pub const PIVOT: Vector3<i32> = Vector3::new(4, 0, 15);

const STRUCTURE_LOCATION_BEACHED: &[&str] = &[
    "shipwreck/with_mast",
    "shipwreck/sideways_full",
    "shipwreck/sideways_fronthalf",
    "shipwreck/sideways_backhalf",
    "shipwreck/rightsideup_full",
    "shipwreck/rightsideup_fronthalf",
    "shipwreck/rightsideup_backhalf",
    "shipwreck/with_mast_degraded",
    "shipwreck/rightsideup_full_degraded",
    "shipwreck/rightsideup_fronthalf_degraded",
    "shipwreck/rightsideup_backhalf_degraded",
];

const STRUCTURE_LOCATION_OCEAN: &[&str] = &[
    "shipwreck/with_mast",
    "shipwreck/upsidedown_full",
    "shipwreck/upsidedown_fronthalf",
    "shipwreck/upsidedown_backhalf",
    "shipwreck/sideways_full",
    "shipwreck/sideways_fronthalf",
    "shipwreck/sideways_backhalf",
    "shipwreck/rightsideup_full",
    "shipwreck/rightsideup_fronthalf",
    "shipwreck/rightsideup_backhalf",
    "shipwreck/with_mast_degraded",
    "shipwreck/upsidedown_full_degraded",
    "shipwreck/upsidedown_fronthalf_degraded",
    "shipwreck/upsidedown_backhalf_degraded",
    "shipwreck/sideways_full_degraded",
    "shipwreck/sideways_fronthalf_degraded",
    "shipwreck/sideways_backhalf_degraded",
    "shipwreck/rightsideup_full_degraded",
    "shipwreck/rightsideup_fronthalf_degraded",
    "shipwreck/rightsideup_backhalf_degraded",
];

const MARKERS_TO_LOOT: &[(&str, &str)] = &[
    ("map_chest", "minecraft:chests/shipwreck_map"),
    ("treasure_chest", "minecraft:chests/shipwreck_treasure"),
    ("supply_chest", "minecraft:chests/shipwreck_supply"),
];

pub struct ShipwreckGenerator {
    pub is_beached: bool,
}

impl StructureGenerator for ShipwreckGenerator {
    fn get_structure_position(
        &self,
        mut context: StructureGeneratorContext<'_>,
    ) -> Option<StructurePosition> {
        let fallback_y = if self.is_beached {
            64
        } else {
            context.sea_level
        };
        let start_pos = chunk_center_start(&mut context, !self.is_beached, fallback_y);

        let rotation_idx = context.random.next_bounded_i32(4) as u8;
        let rotation = Rotation::from_index(rotation_idx);

        let start_x = start_block_x(context.chunk_x);
        let start_z = start_block_z(context.chunk_z);
        let position = Vector3::new(start_x, 90, start_z);

        let pool = if self.is_beached {
            STRUCTURE_LOCATION_BEACHED
        } else {
            STRUCTURE_LOCATION_OCEAN
        };
        let template_idx = context.random.next_bounded_i32(pool.len() as i32) as usize;
        let template_name = pool[template_idx];
        let template = get_template(template_name)?;

        let mut piece = ShipwreckPiece::new(template, position, rotation, self.is_beached);

        if piece.is_too_big_to_fit_in_world_gen_region() {
            let bb = piece.piece.bounding_box;
            let height = if self.is_beached {
                let min_y = context.height_sampler.as_deref_mut().map_or(64, |sampler| {
                    let c0 = sampler.estimate_height(bb.min.x, bb.min.z);
                    let c1 = sampler.estimate_height(bb.min.x, bb.max.z);
                    let c2 = sampler.estimate_height(bb.max.x, bb.min.z);
                    let c3 = sampler.estimate_height(bb.max.x, bb.max.z);
                    c0.min(c1).min(c2).min(c3)
                });
                piece.calculate_beached_position(min_y, &mut context.random)
            } else if let Some(sampler) = context.height_sampler.as_deref_mut() {
                let c0 = sampler.estimate_height(bb.min.x, bb.min.z);
                let c1 = sampler.estimate_height(bb.min.x, bb.max.z);
                let c2 = sampler.estimate_height(bb.max.x, bb.min.z);
                let c3 = sampler.estimate_height(bb.max.x, bb.max.z);
                (c0 + c1 + c2 + c3) / 4
            } else {
                context.sea_level
            };

            piece.adjust_position_height(height);
        }

        let mut collector = StructurePiecesCollector::default();
        collector.add_piece(Box::new(piece));

        Some(StructurePosition {
            start_pos,
            collector: Arc::new(collector.into()),
        })
    }
}

pub struct ShipwreckPiece {
    piece: StructurePiece,
    template: Arc<StructureTemplate>,
    place_settings: StructurePlaceSettings,
    template_position: Vector3<i32>,
    is_beached: bool,
    height_adjusted: bool,
}

impl ShipwreckPiece {
    #[must_use]
    pub fn new(
        template: Arc<StructureTemplate>,
        position: Vector3<i32>,
        rotation: Rotation,
        is_beached: bool,
    ) -> Self {
        let place_settings = make_settings(rotation);
        let bounding_box = template.get_bounding_box(&place_settings, position);

        Self {
            piece: StructurePiece::new(StructurePieceType::Shipwreck, bounding_box, 0),
            template,
            place_settings,
            template_position: position,
            is_beached,
            height_adjusted: false,
        }
    }

    #[must_use]
    pub fn is_too_big_to_fit_in_world_gen_region(&self) -> bool {
        self.template.size.x > NUMBER_OF_BLOCKS_ALLOWED_IN_WORLD_GEN_REGION
            || self.template.size.y > NUMBER_OF_BLOCKS_ALLOWED_IN_WORLD_GEN_REGION
    }

    #[must_use]
    pub fn calculate_beached_position(&self, min_y: i32, random: &mut RandomGenerator) -> i32 {
        min_y - self.template.size.y / 2 - random.next_bounded_i32(3)
    }

    pub fn adjust_position_height(&mut self, new_height: i32) {
        self.height_adjusted = true;
        self.template_position.y = new_height;
        self.piece.bounding_box = self
            .template
            .get_bounding_box(&self.place_settings, self.template_position);
    }
}

impl StructurePieceBase for ShipwreckPiece {
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

    /// Vanilla's `ShipwreckPiece.postProcess`.
    fn place_in_region(
        &mut self,
        cache: &mut dyn GenerationCache,
        _block_registry: &dyn WorldPortalExt,
        random: &mut RandomGenerator,
        seed: i64,
        chunk_box: &BlockBox,
    ) {
        if !self.height_adjusted && !self.is_too_big_to_fit_in_world_gen_region() {
            let heightmap_type = if self.is_beached {
                HeightMap::WorldSurfaceWg
            } else {
                HeightMap::OceanFloorWg
            };

            let template_size = self.template.size;
            let base_size = template_size.x * template_size.z;

            let (mean, min_y) = if base_size == 0 {
                let h = cache.get_top_y(
                    &heightmap_type,
                    self.template_position.x,
                    self.template_position.z,
                );
                (h, h)
            } else {
                let mut min_y = i32::MAX;
                let mut total = 0;
                for x in self.template_position.x..self.template_position.x + template_size.x {
                    for z in self.template_position.z..self.template_position.z + template_size.z {
                        let h = cache.get_top_y(&heightmap_type, x, z);
                        total += h;
                        min_y = min_y.min(h);
                    }
                }
                (total / base_size, min_y)
            };

            let target_y = if self.is_beached {
                self.calculate_beached_position(min_y, random)
            } else {
                mean
            };

            self.adjust_position_height(target_y);
        }

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
                if let Some((_, loot_table)) = MARKERS_TO_LOOT.iter().find(|(id, _)| *id == marker)
                {
                    set_block_entity_loot_table(
                        placer.0.get_center_chunk_mut(),
                        random,
                        Vector3::new(position.x, position.y - 1, position.z),
                        loot_table,
                    );
                }
            },
        );
    }
}

/// Vanilla's `ShipwreckPiece.makeSettings`.
fn make_settings(rotation: Rotation) -> StructurePlaceSettings {
    StructurePlaceSettings::new()
        .set_rotation(rotation)
        .set_mirror(Mirror::None)
        .set_rotation_pivot(PIVOT)
        .set_apply_waterlogging(true)
        .add_processor(StructureProcessor::ignore_structure_blocks_and_air())
}

#[cfg(test)]
mod tests {
    use pumpkin_data::dimension::Dimension;
    use pumpkin_util::{random::legacy_rand::LegacyRand, world_seed::Seed};

    use crate::generation::{
        get_world_gen,
        positions::chunk_pos::{start_block_x, start_block_z},
        proto_chunk_test::stage_parity::TestBlockRegistry,
    };

    use super::*;

    /// The wreck's data markers give the chests below them their loot tables, in every chunk the
    /// wreck crosses.
    #[test]
    fn marker_chests_get_their_loot_tables() {
        let world_gen = get_world_gen(
            Seed(0),
            Dimension::OVERWORLD,
            false,
            Vec::new(),
            String::new(),
        );
        let template = get_template("shipwreck/rightsideup_full").unwrap();
        let mut expected = template
            .blocks
            .iter()
            .filter_map(|block| {
                let metadata = block.nbt.as_ref()?.get_string("metadata")?;
                MARKERS_TO_LOOT
                    .iter()
                    .find(|(id, _)| *id == metadata)
                    .map(|(_, loot_table)| (*loot_table).to_string())
            })
            .collect::<Vec<_>>();
        expected.sort();
        assert!(!expected.is_empty());

        let mut piece = ShipwreckPiece::new(
            template,
            Vector3::new(8, 90, -24),
            Rotation::Clockwise90,
            false,
        );
        piece.adjust_position_height(70);
        let bounding_box = piece.piece.bounding_box;
        let mut random = RandomGenerator::Legacy(LegacyRand::from_seed(0));
        let mut loot_tables = Vec::new();
        for chunk_x in (bounding_box.min.x >> 4)..=(bounding_box.max.x >> 4) {
            for chunk_z in (bounding_box.min.z >> 4)..=(bounding_box.max.z >> 4) {
                let mut chunk = ProtoChunk::new(chunk_x, chunk_z, &world_gen);
                let chunk_box = BlockBox::new(
                    start_block_x(chunk_x),
                    -63,
                    start_block_z(chunk_z),
                    start_block_x(chunk_x) + 15,
                    319,
                    start_block_z(chunk_z) + 15,
                );
                piece.place(&mut chunk, &TestBlockRegistry, &mut random, 0, &chunk_box);
                for nbt in chunk.take_pending_block_entities() {
                    if let Some(loot_table) = nbt.get_string("LootTable") {
                        assert!(nbt.get_long("LootTableSeed").is_some());
                        loot_tables.push(loot_table.to_string());
                    }
                }
            }
        }
        loot_tables.sort();
        assert_eq!(loot_tables, expected);
    }
}
