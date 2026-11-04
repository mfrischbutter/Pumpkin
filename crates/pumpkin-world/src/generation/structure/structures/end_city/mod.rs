mod layout;

use std::sync::{Arc, Mutex};

use pumpkin_data::{BlockDirection, Rotation, item::Item, item_stack::ItemStack};
use pumpkin_nbt::{compound::NbtCompound, tag::NbtTag};
use pumpkin_util::{
    math::{block_box::BlockBox, position::BlockPos, vector3::Vector3},
    random::{RandomGenerator, RandomImpl},
};

use crate::{
    ProtoChunk,
    generation::structure::{
        piece::StructurePieceType,
        structures::{
            StructureGenerator, StructureGeneratorContext, StructurePiece, StructurePieceBase,
            StructurePiecesCollector, StructurePosition,
        },
        template::{
            StructurePlaceSettings, place_template_piece, processor::StructureProcessor,
            set_block_entity_loot_table,
        },
    },
    world::WorldPortalExt,
};

use self::layout::{EndCityLayout, PieceDescriptor};

const LOOT_TABLE: &str = "minecraft:chests/end_city_treasure";

pub struct EndCityGenerator;

impl StructureGenerator for EndCityGenerator {
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

        let descriptors =
            EndCityLayout::create(Vector3::new(x, y, z), rotation, &mut context.random)?;
        let mut collector = StructurePiecesCollector::default();
        for descriptor in descriptors {
            collector.add_piece(Box::new(EndCityTemplatePiece::new(descriptor)));
        }
        Some(StructurePosition {
            start_pos: BlockPos::new(x, y, z),
            collector: Arc::new(Mutex::new(collector)),
        })
    }
}

struct EndCityTemplatePiece {
    piece: StructurePiece,
    descriptor: PieceDescriptor,
    place_settings: StructurePlaceSettings,
}

impl EndCityTemplatePiece {
    fn new(descriptor: PieceDescriptor) -> Self {
        Self {
            piece: StructurePiece::new(StructurePieceType::EndCity, descriptor.bounding_box, 0),
            place_settings: make_settings(descriptor.overwrite, descriptor.rotation),
            descriptor,
        }
    }

    /// Vanilla's `EndCityPiece.handleDataMarker`.
    fn handle_marker(
        &self,
        chunk: &mut ProtoChunk,
        random: &mut RandomGenerator,
        marker: &str,
        position: Vector3<i32>,
        chunk_box: &BlockBox,
    ) {
        if marker.starts_with("Chest") {
            let chest_position = Vector3::new(position.x, position.y - 1, position.z);
            if chunk_box.contains_pos(&chest_position) {
                set_block_entity_loot_table(chunk, random, chest_position, LOOT_TABLE);
            }
            return;
        }
        if !chunk_box.contains_pos(&position) {
            return;
        }
        if marker.starts_with("Sentry") {
            chunk.add_structure_entity(
                entity_nbt(
                    "minecraft:shulker",
                    Vector3::new(
                        f64::from(position.x) + 0.5,
                        f64::from(position.y),
                        f64::from(position.z) + 0.5,
                    ),
                ),
                false,
            );
        } else if marker.starts_with("Elytra") {
            chunk.add_structure_entity(self.item_frame_nbt(position), false);
        }
    }

    fn item_frame_nbt(&self, position: Vector3<i32>) -> NbtCompound {
        let facing = match self.descriptor.rotation {
            Rotation::None => BlockDirection::South,
            Rotation::Clockwise90 => BlockDirection::West,
            Rotation::Rotate180 => BlockDirection::North,
            Rotation::CounterClockwise90 => BlockDirection::East,
        };
        let offset = facing.to_offset();
        let mut nbt = entity_nbt(
            "minecraft:item_frame",
            Vector3::new(
                f64::from(position.x) + 0.5 - f64::from(offset.x) * 0.46875,
                f64::from(position.y) + 0.5,
                f64::from(position.z) + 0.5 - f64::from(offset.z) * 0.46875,
            ),
        );
        nbt.put_byte("Facing", facing.to_index() as i8);
        nbt.put(
            "block_pos",
            NbtTag::IntArray(vec![position.x, position.y, position.z]),
        );
        nbt.put_byte("ItemRotation", 0);
        nbt.child_tags.insert(
            "Rotation".into(),
            NbtTag::List(vec![
                match facing {
                    BlockDirection::South => 0.0f32,
                    BlockDirection::West => 90.0,
                    BlockDirection::North => 180.0,
                    BlockDirection::East => 270.0,
                    BlockDirection::Down | BlockDirection::Up => unreachable!(),
                }
                .into(),
                0.0f32.into(),
            ]),
        );
        let stack = ItemStack::new(1, &Item::ELYTRA);
        let mut item = NbtCompound::new();
        stack.write_item_stack(&mut item);
        nbt.put_compound("Item", item);
        nbt
    }
}

impl StructurePieceBase for EndCityTemplatePiece {
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
        self.place_settings.bounding_box = Some(*chunk_box);
        self.piece.bounding_box = self
            .descriptor
            .template
            .get_bounding_box(&self.place_settings, self.descriptor.template_position);
        place_template_piece(
            chunk,
            &self.descriptor.template,
            self.descriptor.template_position,
            &self.place_settings,
            random,
            seed,
            |chunk, random, marker, position| {
                self.handle_marker(chunk, random, marker, position, chunk_box);
            },
        );
    }
}

/// Vanilla's `EndCityPiece.makeSettings`.
fn make_settings(overwrite: bool, rotation: Rotation) -> StructurePlaceSettings {
    StructurePlaceSettings::new()
        .set_ignore_entities(true)
        .add_processor(if overwrite {
            StructureProcessor::ignore_structure_blocks()
        } else {
            StructureProcessor::ignore_structure_blocks_and_air()
        })
        .set_rotation(rotation)
        .set_apply_waterlogging(true)
}

fn entity_nbt(id: &str, position: Vector3<f64>) -> NbtCompound {
    let mut nbt = NbtCompound::new();
    nbt.put_string("id", id.to_string());
    nbt.put(
        "Pos",
        NbtTag::List(vec![
            position.x.into(),
            position.y.into(),
            position.z.into(),
        ]),
    );
    nbt.put(
        "Motion",
        NbtTag::List(vec![0.0.into(), 0.0.into(), 0.0.into()]),
    );
    nbt.put("Rotation", NbtTag::List(vec![0.0f32.into(), 0.0f32.into()]));
    nbt
}

#[cfg(test)]
mod tests {
    use pumpkin_util::random::legacy_rand::LegacyRand;

    use crate::generation::structure::{
        structures::HeightSampler,
        template::{get_block_entity_id, get_template},
    };

    use super::*;

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
        let position = EndCityGenerator
            .get_structure_position(StructureGeneratorContext {
                seed: seed as i64,
                chunk_x: 3,
                chunk_z: -2,
                random: RandomGenerator::Legacy(LegacyRand::from_seed(seed)),
                sea_level: 63,
                min_y: 0,
                height: 128,
                height_sampler: Some(&mut sampler),
                structure_key: None,
            })
            .unwrap();
        let x = 3 * 16 + 7;
        let z = -2 * 16 + 7;
        assert_eq!(
            sampler.calls,
            vec![
                (x, z),
                (x, z + offset_z),
                (x + offset_x, z),
                (x + offset_x, z + offset_z),
            ]
        );
        assert_eq!(position.start_pos, BlockPos::new(x, 69, z));
    }

    #[test]
    fn terrain_below_sixty_rejects_the_city() {
        let mut sampler = RecordingHeightSampler {
            calls: Vec::new(),
            heights: vec![70, 59, 70, 70].into_iter(),
        };
        assert!(
            EndCityGenerator
                .get_structure_position(StructureGeneratorContext {
                    seed: 0,
                    chunk_x: 0,
                    chunk_z: 0,
                    random: RandomGenerator::Legacy(LegacyRand::from_seed(0)),
                    sea_level: 63,
                    min_y: 0,
                    height: 128,
                    height_sampler: Some(&mut sampler),
                    structure_key: None,
                })
                .is_none()
        );
    }

    #[test]
    fn elytra_frame_matches_vanilla_ship_marker() {
        let descriptor = PieceDescriptor::new(
            "ship",
            Vector3::new(-4888, 123, -4393),
            Rotation::CounterClockwise90,
            true,
        )
        .unwrap();
        let nbt =
            EndCityTemplatePiece::new(descriptor).item_frame_nbt(Vector3::new(-4881, 128, -4399));
        assert_eq!(
            nbt.get_byte("Facing"),
            Some(BlockDirection::East.to_index() as i8)
        );
        assert_eq!(
            nbt.get_compound("Item")
                .and_then(|item| item.get_string("id")),
            Some("minecraft:elytra")
        );
        assert_eq!(
            nbt.get_int_array("block_pos"),
            Some(&[-4881, 128, -4399][..])
        );
        assert_eq!(
            nbt.get_list("Pos"),
            Some(&[(-4880.96875).into(), 128.5.into(), (-4398.5).into(),][..])
        );
        assert_eq!(
            nbt.get_list("Rotation")
                .and_then(|rotation| rotation[0].extract_float()),
            Some(270.0)
        );
    }

    #[test]
    fn ship_dragon_head_uses_the_skull_block_entity() {
        let ship = get_template("end_city/ship").unwrap();
        assert!(ship.palette.iter().any(|palette| {
            palette.name == "minecraft:dragon_wall_head"
                && get_block_entity_id(&palette.name) == Some("minecraft:skull")
        }));
    }
}
