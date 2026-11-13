use std::sync::Arc;

use pumpkin_data::{Block, BlockState};
use pumpkin_util::{
    BlockDirection, HeightMap,
    math::{block_box::BlockBox, vector3::Vector3},
    random::RandomGenerator,
};

use crate::{
    ProtoChunk,
    generation::structure::{
        piece::StructurePieceType,
        structures::{
            StructureGenerator, StructureGeneratorContext, StructurePiece, StructurePieceBase,
            StructurePiecesCollector, StructurePosition, WorldPortalExt, chunk_center_start,
        },
    },
};

pub struct BuriedTreasureGenerator;

impl StructureGenerator for BuriedTreasureGenerator {
    fn get_structure_position(
        &self,
        mut context: StructureGeneratorContext<'_>,
    ) -> Option<StructurePosition> {
        let x = (context.chunk_x << 4) + 9;
        let z = (context.chunk_z << 4) + 9;

        let bounding_box = BlockBox::new(x, 90, z, x, 90, z);

        let mut collector = StructurePiecesCollector::default();
        collector.add_piece(Box::new(BuriedTreasurePiece {
            piece: StructurePiece::new(StructurePieceType::BuriedTreasure, bounding_box, 0),
        }));

        Some(StructurePosition {
            start_pos: chunk_center_start(&mut context, true, 90),
            collector: Arc::new(collector.into()),
        })
    }
}

pub struct BuriedTreasurePiece {
    piece: StructurePiece,
}

/// Vanilla `BuriedTreasurePiece.isLiquid`: the block is water or lava.
fn is_liquid(state: &BlockState) -> bool {
    let block = Block::from_state_id(state.id);
    *block == Block::WATER || *block == Block::LAVA
}

impl StructurePieceBase for BuriedTreasurePiece {
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
        _seed: i64,
        chunk_box: &BlockBox,
    ) {
        let bb = self.bounding_box();
        let y = chunk.get_top_y(&HeightMap::OceanFloorWg, bb.min.x, bb.min.z);
        let min_y = chunk.bottom_y() as i32;
        let mut cur_y = y;

        while cur_y > min_y {
            let pos = Vector3::new(bb.min.x, cur_y, bb.min.z);
            let below_pos = Vector3::new(bb.min.x, cur_y - 1, bb.min.z);

            let current_state = chunk.get_block_state(&pos).to_state();
            let below_state = chunk.get_block_state(&below_pos).to_state();
            let below_block = Block::from_state_id(below_state.id);

            if *below_block == Block::SANDSTONE
                || *below_block == Block::STONE
                || *below_block == Block::ANDESITE
                || *below_block == Block::GRANITE
                || *below_block == Block::DIORITE
            {
                let soft_state = if !current_state.is_air() && !is_liquid(current_state) {
                    current_state
                } else {
                    Block::SAND.default_state
                };

                for dir in [
                    BlockDirection::Down,
                    BlockDirection::Up,
                    BlockDirection::North,
                    BlockDirection::South,
                    BlockDirection::West,
                    BlockDirection::East,
                ] {
                    let offset = dir.to_vector();
                    let rel_pos = pos + offset;
                    let rel_state = chunk.get_block_state(&rel_pos).to_state();

                    if rel_state.is_air() || is_liquid(rel_state) {
                        let below_rel_pos = rel_pos - Vector3::new(0, 1, 0);
                        let below_rel_state = chunk.get_block_state(&below_rel_pos).to_state();

                        if (below_rel_state.is_air() || is_liquid(below_rel_state))
                            && dir != BlockDirection::Up
                        {
                            chunk.set_block_state(rel_pos.x, rel_pos.y, rel_pos.z, below_state);
                        } else {
                            chunk.set_block_state(rel_pos.x, rel_pos.y, rel_pos.z, soft_state);
                        }
                    }
                }

                self.piece.bounding_box = BlockBox::new(pos.x, pos.y, pos.z, pos.x, pos.y, pos.z);

                // The piece has no facing, so its local coordinates are world coordinates.
                self.piece.create_chest(
                    chunk,
                    chunk_box,
                    random,
                    pos.x,
                    pos.y,
                    pos.z,
                    "minecraft:chests/buried_treasure",
                    None,
                );
                return;
            }

            cur_y -= 1;
        }
    }
}
