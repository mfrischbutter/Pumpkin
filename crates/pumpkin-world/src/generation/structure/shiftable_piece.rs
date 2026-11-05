use pumpkin_util::{
    HeightMap,
    math::{block_box::BlockBox, vector3::Axis},
};

use crate::{
    ProtoChunk,
    generation::structure::{piece::StructurePieceType, structures::StructurePiece},
};

pub const DEFAULT_H_POS: i32 = -1;

/// The average ground height under a piece, vanilla's `ScatteredFeaturePiece.updateAverageGroundHeight`.
///
/// It averages the exclusive top of the `MOTION_BLOCKING_NO_LEAVES` heightmap over the columns
/// of the piece's footprint that lie inside the chunk being placed, and is `None` when no
/// column lies inside.
#[must_use]
pub fn average_ground_height(
    chunk: &ProtoChunk,
    bounding_box: &BlockBox,
    chunk_box: &BlockBox,
) -> Option<i32> {
    let mut sum_y = 0;
    let mut count = 0;

    for z in bounding_box.min.z..=bounding_box.max.z {
        for x in bounding_box.min.x..=bounding_box.max.x {
            if chunk_box.contains(x, 64, z) {
                sum_y += chunk.get_top_y(&HeightMap::MotionBlockingNoLeaves, x, z);
                count += 1;
            }
        }
    }

    (count > 0).then(|| sum_y / count)
}

pub struct ShiftableStructurePiece {
    pub piece: StructurePiece,
    h_pos: i32,
    width: i32,
    height: i32,
}

impl ShiftableStructurePiece {
    #[expect(clippy::too_many_arguments)]
    #[must_use]
    pub fn new(
        r#type: StructurePieceType,
        x: i32,
        y: i32,
        z: i32,
        width: i32,
        height: i32,
        depth: i32,
        axis: Axis,
    ) -> Self {
        Self {
            piece: StructurePiece::new(
                r#type,
                BlockBox::create_box(x, y, z, axis, width, height, depth),
                0,
            ),
            h_pos: DEFAULT_H_POS,
            width,
            height,
        }
    }

    pub fn adjust_to_average_height(
        &mut self,
        chunk: &ProtoChunk,
        chunk_box: &BlockBox,
        offset: i32,
    ) -> bool {
        if self.h_pos >= 0 {
            return true;
        }

        let Some(h_pos) = average_ground_height(chunk, &self.piece.bounding_box, chunk_box) else {
            return false;
        };
        self.h_pos = h_pos;

        let shift_y = self.h_pos - self.piece.bounding_box.min.y + offset;
        self.piece.bounding_box.min.y += shift_y;
        self.piece.bounding_box.max.y += shift_y;

        true
    }

    pub fn adjust_to_min_height(&mut self, chunk: &ProtoChunk, y_offset: i32) -> bool {
        let bounding_box = self.piece.bounding_box;

        if self.h_pos >= 0 {
            return true;
        }

        let mut min_y = i32::MAX;
        let mut found_any = false;

        // Iterate over the X/Z footprint of the bounding box
        for z in bounding_box.min.z..=bounding_box.max.z {
            for x in bounding_box.min.x..=bounding_box.max.x {
                // Vanilla uses MOTION_BLOCKING_NO_LEAVES.
                // In Pumpkin/Standard generation, OceanFloorWg is usually the closest equivalent for terrain height.
                let y = chunk.get_top_y(&HeightMap::OceanFloorWg, x, z);

                if y < min_y {
                    min_y = y;
                }
                found_any = true;
            }
        }

        // Should only return false if the bounding box has no area (width or depth is 0)
        if !found_any {
            return false;
        }

        self.h_pos = min_y;

        // Calculate shift: (Target Height - Current Height) + Offset
        let current_y = bounding_box.min.y;
        let shift_y = (self.h_pos - current_y) + y_offset;

        self.piece.bounding_box.min.y += shift_y;
        self.piece.bounding_box.max.y += shift_y;
        true
    }
}

#[cfg(test)]
mod tests {
    use pumpkin_data::{Block, dimension::Dimension};
    use pumpkin_util::{HeightMap, math::block_box::BlockBox, world_seed::Seed};

    use super::average_ground_height;
    use crate::{
        ProtoChunk,
        generation::generator::{GeneratorInit, VanillaGenerator, WorldGenerator},
    };

    const FLOOR_TOP_Y: i32 = 55;
    const WATER_TOP_Y: i32 = 62;

    fn top_y(chunk: &ProtoChunk, x: i32, z: i32) -> i32 {
        chunk.get_top_y(&HeightMap::MotionBlockingNoLeaves, x, z)
    }

    fn chunk_with_floor(world_gen: &WorldGenerator) -> ProtoChunk {
        let mut chunk = ProtoChunk::new(0, 0, world_gen);
        for x in 0..16 {
            for z in 0..16 {
                chunk.set_block_state(x, FLOOR_TOP_Y, z, Block::STONE.default_state);
            }
        }
        chunk
    }

    #[test]
    fn average_ground_height_reads_the_water_surface_inside_the_chunk() {
        let world_gen = WorldGenerator::Noise(Box::new(VanillaGenerator::new(
            Seed(42),
            Dimension::OVERWORLD,
        )));

        // (a) Over a lake the piece stands on the water surface, not on the lake floor.
        let mut chunk = chunk_with_floor(&world_gen);
        let bottom_y = i32::from(chunk.bottom_y());
        let chunk_box = BlockBox::new(
            0,
            bottom_y + 1,
            0,
            15,
            bottom_y + i32::from(chunk.height()) - 1,
            15,
        );
        let footprint = BlockBox::new(2, 64, 3, 8, 70, 11);
        for x in 2..=8 {
            for z in 3..=11 {
                for y in FLOOR_TOP_Y + 1..=WATER_TOP_Y {
                    chunk.set_block_state(x, y, z, Block::WATER.default_state);
                }
            }
        }
        assert_eq!(top_y(&chunk, 0, 0), FLOOR_TOP_Y + 1, "test setup is wrong");
        assert_eq!(top_y(&chunk, 2, 3), WATER_TOP_Y + 1, "test setup is wrong");
        assert_eq!(top_y(&chunk, 8, 11), WATER_TOP_Y + 1, "test setup is wrong");
        assert_eq!(
            average_ground_height(&chunk, &footprint, &chunk_box),
            Some(WATER_TOP_Y + 1),
        );

        // (b) A footprint that sticks out of the chunk counts only the columns inside it; the
        // outside columns x 16..=19 would read the wrapped columns x 0..=3.
        let mut chunk = chunk_with_floor(&world_gen);
        let inside_top_y = 59;
        let wrapped_top_y = 99;
        for z in 0..=3 {
            for x in 12..=15 {
                chunk.set_block_state(x, inside_top_y, z, Block::STONE.default_state);
            }
            for x in 0..=3 {
                chunk.set_block_state(x, wrapped_top_y, z, Block::STONE.default_state);
            }
        }
        let footprint = BlockBox::new(12, 64, 0, 19, 70, 3);
        assert_eq!(
            top_y(&chunk, 12, 0),
            inside_top_y + 1,
            "test setup is wrong"
        );
        assert_eq!(
            top_y(&chunk, 15, 3),
            inside_top_y + 1,
            "test setup is wrong"
        );
        assert_eq!(
            top_y(&chunk, 16, 0),
            wrapped_top_y + 1,
            "test setup is wrong"
        );
        assert_eq!(
            top_y(&chunk, 19, 3),
            wrapped_top_y + 1,
            "test setup is wrong"
        );
        assert_eq!(
            average_ground_height(&chunk, &footprint, &chunk_box),
            Some(inside_top_y + 1),
        );
    }
}
