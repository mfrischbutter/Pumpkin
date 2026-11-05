use pumpkin_data::{Block, Mirror, Rotation};
use pumpkin_util::{
    HeightMap,
    math::{block_box::BlockBox, position::BlockPos, vector3::Vector3},
    random::{RandomGenerator, RandomImpl},
};

use crate::generation::{
    proto_chunk::GenerationCache,
    structure::template::{
        RegionPlacer, StructurePlaceSettings, TemplatePlacement, get_template, load_processor_list,
        place_template_with_options,
    },
};
use crate::world::WorldPortalExt;

/// Vanilla's `FossilFeature`: a bone skeleton buried below the ocean floor and an ore overlay of
/// the same shape, both rotting away through their processor lists.
pub struct FossilFeature {
    pub fossil_structures: Vec<&'static str>,
    pub overlay_structures: Vec<&'static str>,
    pub fossil_processor: FossilProcessor,
    pub overlay_processor: FossilProcessor,
    pub max_empty_corners_allowed: u8,
}

/// The processor lists a fossil config names.
#[derive(Clone, Copy)]
pub enum FossilProcessor {
    FossilRot,
    Coal,
    Diamonds,
}

impl FossilProcessor {
    const fn list_name(self) -> &'static str {
        match self {
            Self::FossilRot => "minecraft:fossil_rot",
            Self::Coal => "minecraft:fossil_coal",
            Self::Diamonds => "minecraft:fossil_diamonds",
        }
    }
}

impl FossilFeature {
    pub fn generate<T: GenerationCache>(
        &self,
        chunk: &mut T,
        block_registry: &dyn WorldPortalExt,
        min_y: i8,
        height: u16,
        random: &mut RandomGenerator,
        pos: BlockPos,
    ) -> bool {
        if self.fossil_structures.is_empty()
            || self.fossil_structures.len() != self.overlay_structures.len()
        {
            return false;
        }

        let rotation = Rotation::from_index(random.next_bounded_i32(4) as u8);
        let index = random.next_bounded_i32(self.fossil_structures.len() as i32) as usize;
        let (Some(base), Some(overlay)) = (
            get_template(self.fossil_structures[index]),
            get_template(self.overlay_structures[index]),
        ) else {
            return false;
        };

        let chunk_min_x = (pos.0.x >> 4) << 4;
        let chunk_min_z = (pos.0.z >> 4) << 4;
        let mut settings = StructurePlaceSettings::new()
            .set_rotation(rotation)
            .set_bounding_box(Some(BlockBox::new(
                chunk_min_x - 16,
                i32::from(min_y),
                chunk_min_z - 16,
                chunk_min_x + 31,
                i32::from(min_y) + i32::from(height) - 1,
                chunk_min_z + 31,
            )))
            .set_apply_waterlogging(true)
            .set_shared_random(true);

        let size = base.get_size_with_rotation(rotation);
        let low_corner = Vector3::new(pos.0.x - size.x / 2, pos.0.y, pos.0.z - size.z / 2);
        let mut lowest_surface_y = pos.0.y;
        for x in 0..size.x {
            for z in 0..size.z {
                lowest_surface_y = lowest_surface_y.min(chunk.get_top_y(
                    &HeightMap::OceanFloorWg,
                    low_corner.x + x,
                    low_corner.z + z,
                ));
            }
        }
        let target_y =
            (lowest_surface_y - 15 - random.next_bounded_i32(10)).max(i32::from(min_y) + 10);
        let target = base.get_zero_position_with_transform(
            Vector3::new(low_corner.x, target_y, low_corner.z),
            Mirror::None,
            rotation,
        );

        if count_empty_corners(chunk, &base.get_bounding_box(&settings, target))
            > usize::from(self.max_empty_corners_allowed)
        {
            return false;
        }

        let placement = TemplatePlacement {
            reference: target,
            level_seed: block_registry.feature_seed() as i64,
            skip_air: false,
            keep_jigsaws: true,
        };
        let mut placer = RegionPlacer(chunk);
        settings.processors = load_processor_list(self.fossil_processor.list_name());
        place_template_with_options(&mut placer, &base, target, &settings, random, placement);
        settings.processors = load_processor_list(self.overlay_processor.list_name());
        place_template_with_options(&mut placer, &overlay, target, &settings, random, placement);

        true
    }
}

/// Vanilla's `countEmptyCorners`: the corners of the box that hold air, lava or water.
fn count_empty_corners<T: GenerationCache>(chunk: &T, bounding_box: &BlockBox) -> usize {
    let mut empty_corners = 0;

    for x in [bounding_box.min.x, bounding_box.max.x] {
        for y in [bounding_box.min.y, bounding_box.max.y] {
            for z in [bounding_box.min.z, bounding_box.max.z] {
                let state =
                    GenerationCache::get_block_state(chunk, &Vector3::new(x, y, z)).to_state();
                let block = Block::from_state_id(state.id);
                if state.is_air() || block == &Block::LAVA || block == &Block::WATER {
                    empty_corners += 1;
                }
            }
        }
    }

    empty_corners
}
