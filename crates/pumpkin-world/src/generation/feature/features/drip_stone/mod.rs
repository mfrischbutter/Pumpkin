use std::f32::consts::TAU;

use pumpkin_data::{
    Block, BlockDirection, BlockId, BlockStateId,
    block_properties::{PointedDripstoneLikeProperties, SpeleothemThickness, VerticalDirection},
    tag::{self, Taggable},
};
use pumpkin_util::math::{cos, position::BlockPos, sin};

use crate::generation::proto_chunk::GenerationCache;

pub mod cluster;
mod column;
pub mod large;
pub mod small;

/// Vanilla's `SpeleothemUtils.getSpeleothemHeight`: how far a large dripstone reaches at
/// `xz_distance_from_center` blocks from its axis.
fn get_speleothem_height(
    xz_distance_from_center: f64,
    speleothem_radius: f64,
    scale: f64,
    bluntness: f64,
) -> f64 {
    const CUTOFF: f64 = 0.384;
    let r = xz_distance_from_center.max(bluntness) / speleothem_radius * CUTOFF;
    let part1 = 0.75 * r.powf(4.0 / 3.0);
    let part2 = r.powf(2.0 / 3.0);
    let part3 = 1.0 / 3.0 * r.ln();
    let height_relative_to_max_radius = (scale * (part1 - part2 - part3)).max(0.0);
    height_relative_to_max_radius / CUTOFF * speleothem_radius
}

/// Vanilla's `SpeleothemUtils.isCircleMostlyEmbeddedInStone`: neither the center nor the points
/// of the circle around it, one every 6 blocks of arc, are air, water or lava.
fn is_circle_mostly_embedded_in_stone<T: GenerationCache>(
    chunk: &T,
    center: BlockPos,
    xz_radius: i32,
) -> bool {
    const ARC_LENGTH: f32 = 6.0;
    if is_empty_or_water_or_lava(GenerationCache::get_block_state(chunk, &center.0)) {
        return false;
    }
    let angle_increment = ARC_LENGTH / xz_radius as f32;
    let mut angle = 0.0;
    while angle < TAU {
        let dx = (cos(angle) * xz_radius as f32) as i32;
        let dz = (sin(angle) * xz_radius as f32) as i32;
        let pos = center.add(dx, 0, dz);
        if is_empty_or_water_or_lava(GenerationCache::get_block_state(chunk, &pos.0)) {
            return false;
        }
        angle += angle_increment;
    }
    true
}

/// Vanilla's `SpeleothemUtils.isEmptyOrWater`.
fn is_empty_or_water(state: BlockStateId) -> bool {
    state.to_state().is_air() || state.to_block_id() == BlockId::WATER
}

/// Vanilla's `SpeleothemUtils.isNeitherEmptyNorWater`.
fn is_neither_empty_nor_water(state: BlockStateId) -> bool {
    !is_empty_or_water(state)
}

/// Vanilla's `SpeleothemUtils.isEmptyOrWaterOrLava`.
fn is_empty_or_water_or_lava(state: BlockStateId) -> bool {
    is_empty_or_water(state) || state.to_block_id() == BlockId::LAVA
}

/// Vanilla's `SpeleothemUtils.isBase`: the base block itself or a block it may replace.
fn is_base(state: BlockStateId, base_block: &Block, replaceable_blocks: &[u16]) -> bool {
    let block = state.to_block_id();
    block == base_block.id || replaceable_blocks.contains(&block.as_u16())
}

/// Vanilla's `SpeleothemUtils.isBaseOrLava`.
fn is_base_or_lava(state: BlockStateId, base_block: &Block, replaceable_blocks: &[u16]) -> bool {
    is_base(state, base_block, replaceable_blocks) || state.to_block_id() == BlockId::LAVA
}

/// Vanilla's `LevelReader.isWaterAt`: the fluid at `pos` is water, also inside a waterlogged
/// block.
fn is_water_at<T: GenerationCache>(chunk: &T, pos: BlockPos) -> bool {
    let (fluid, _) = chunk.get_fluid_and_fluid_state(&pos.0);
    fluid.has_tag(&tag::Fluid::MINECRAFT_WATER)
}

/// Vanilla's `BlockPos.atY`.
const fn at_y(pos: BlockPos, y: i32) -> BlockPos {
    BlockPos::new(pos.0.x, y, pos.0.z)
}

/// Vanilla's `SpeleothemUtils.placeBaseBlockIfPossible`: turns a replaceable block into the base
/// block.
fn place_base_block_if_possible<T: GenerationCache>(
    chunk: &mut T,
    pos: BlockPos,
    base_block: &Block,
    replaceable_blocks: &[u16],
) -> bool {
    let block = GenerationCache::get_block_state(chunk, &pos.0).to_block_id();
    if !replaceable_blocks.contains(&block.as_u16()) {
        return false;
    }
    chunk.set_block_state(&pos.0, base_block.default_state);
    true
}

/// Vanilla's `SpeleothemUtils.buildBaseToTipColumn`: the thickness of each block of a speleothem
/// `length` blocks long, from its base to its tip.
fn build_base_to_tip_column(
    length: i32,
    merged_tip: bool,
) -> impl Iterator<Item = SpeleothemThickness> {
    (0..length).map(move |i| {
        if i == length - 1 {
            if merged_tip {
                SpeleothemThickness::TipMerge
            } else {
                SpeleothemThickness::Tip
            }
        } else if i == length - 2 {
            SpeleothemThickness::Frustum
        } else if i == 0 {
            SpeleothemThickness::Base
        } else {
            SpeleothemThickness::Middle
        }
    })
}

/// Vanilla's `SpeleothemUtils.growSpeleothem`: `height` pointed blocks from `start_pos` towards
/// `tip_direction`, when the block behind `start_pos` is a base.
#[expect(clippy::too_many_arguments)]
fn grow_speleothem<T: GenerationCache>(
    chunk: &mut T,
    start_pos: BlockPos,
    tip_direction: BlockDirection,
    height: i32,
    merged_tip: bool,
    base_block: &Block,
    pointed_block: &Block,
    replaceable_blocks: &[u16],
) {
    let behind_start = start_pos.offset(tip_direction.opposite().to_offset());
    if !is_base(
        GenerationCache::get_block_state(chunk, &behind_start.0),
        base_block,
        replaceable_blocks,
    ) {
        return;
    }
    let vertical_direction = if tip_direction == BlockDirection::Up {
        VerticalDirection::Up
    } else {
        VerticalDirection::Down
    };
    let mut pos = start_pos;
    for thickness in build_base_to_tip_column(height, merged_tip) {
        let state = PointedDripstoneLikeProperties {
            thickness,
            vertical_direction,
            waterlogged: is_water_at(chunk, pos),
        }
        .to_state_id(pointed_block);
        chunk.set_block_state(&pos.0, state.to_state());
        pos = pos.offset(tip_direction.to_offset());
    }
}
