use std::f32::consts::PI;

use pumpkin_data::{Block, BlockId, tag};
use pumpkin_util::{
    HeightMap,
    math::{
        cos, float_provider::FloatProvider, int_provider::IntProvider, position::BlockPos, sin,
        vector3::Vector3,
    },
    random::{RandomGenerator, RandomImpl},
};

use super::{
    at_y, column::Column, get_speleothem_height, is_base_or_lava,
    is_circle_mostly_embedded_in_stone, is_empty_or_water, is_empty_or_water_or_lava,
};
use crate::generation::proto_chunk::GenerationCache;

/// Vanilla's `LargeDripstoneFeature`: a stalactite and a stalagmite of dripstone blocks between a
/// cave's ceiling and floor, both bent by the same wind when they are wide and blunt enough.
pub struct LargeDripstoneFeature {
    pub replaceable_blocks: &'static [u16],
    pub floor_to_ceiling_search_range: i32,
    pub column_radius: IntProvider,
    pub height_scale: FloatProvider,
    pub max_column_radius_to_cave_height_ratio: f32,
    pub stalactite_bluntness: FloatProvider,
    pub stalagmite_bluntness: FloatProvider,
    pub wind_speed: FloatProvider,
    pub min_radius_for_wind: i32,
    pub min_bluntness_for_wind: f32,
}

impl LargeDripstoneFeature {
    pub fn generate<T: GenerationCache>(
        &self,
        chunk: &mut T,
        random: &mut RandomGenerator,
        origin: BlockPos,
    ) -> bool {
        if !is_empty_or_water(GenerationCache::get_block_state(chunk, &origin.0)) {
            return false;
        }
        let Some(column) = Column::scan(
            chunk,
            origin,
            self.floor_to_ceiling_search_range,
            is_empty_or_water,
            |state| is_base_or_lava(state, &Block::DRIPSTONE_BLOCK, self.replaceable_blocks),
        ) else {
            return false;
        };
        let (Some(floor), Some(ceiling), Some(column_height)) =
            (column.floor, column.ceiling, column.height())
        else {
            return false;
        };
        if column_height < 4 {
            return false;
        }

        let max_column_radius_based_on_column_height =
            (column_height as f32 * self.max_column_radius_to_cave_height_ratio) as i32;
        let min_radius = self.column_radius.get_min();
        let max_column_radius = max_column_radius_based_on_column_height
            .max(min_radius)
            .min(self.column_radius.get_max());
        let radius = random.next_inbetween_i32(min_radius, max_column_radius);
        let mut stalactite = LargeDripstone::new(
            at_y(origin, ceiling - 1),
            false,
            random,
            radius,
            &self.stalactite_bluntness,
            &self.height_scale,
        );
        let mut stalagmite = LargeDripstone::new(
            at_y(origin, floor + 1),
            true,
            random,
            radius,
            &self.stalagmite_bluntness,
            &self.height_scale,
        );
        let wind = if stalactite
            .is_suitable_for_wind(self.min_radius_for_wind, self.min_bluntness_for_wind)
            && stalagmite
                .is_suitable_for_wind(self.min_radius_for_wind, self.min_bluntness_for_wind)
        {
            WindOffsetter::new(origin.0.y, random, &self.wind_speed, 16 - radius)
        } else {
            WindOffsetter::no_wind()
        };

        let stalactite_base_embedded_in_stone = stalactite
            .move_back_until_base_is_inside_stone_and_shrink_radius_if_necessary(chunk, &wind);
        let stalagmite_base_embedded_in_stone = stalagmite
            .move_back_until_base_is_inside_stone_and_shrink_radius_if_necessary(chunk, &wind);
        if stalactite_base_embedded_in_stone {
            stalactite.place_blocks(chunk, random, &wind);
        }
        if stalagmite_base_embedded_in_stone {
            stalagmite.place_blocks(chunk, random, &wind);
        }
        true
    }
}

struct LargeDripstone {
    root: BlockPos,
    pointing_up: bool,
    radius: i32,
    bluntness: f64,
    scale: f64,
}

impl LargeDripstone {
    /// Vanilla's `makeDripstone`, which samples the bluntness before the scale.
    fn new(
        root: BlockPos,
        pointing_up: bool,
        random: &mut RandomGenerator,
        radius: i32,
        bluntness: &FloatProvider,
        height_scale: &FloatProvider,
    ) -> Self {
        let bluntness = f64::from(bluntness.get(random));
        let scale = f64::from(height_scale.get(random));
        Self {
            root,
            pointing_up,
            radius,
            bluntness,
            scale,
        }
    }

    fn get_height(&self) -> i32 {
        self.get_height_at_radius(0.0)
    }

    fn move_back_until_base_is_inside_stone_and_shrink_radius_if_necessary<T: GenerationCache>(
        &mut self,
        chunk: &T,
        wind: &WindOffsetter,
    ) -> bool {
        while self.radius > 1 {
            let mut new_root = self.root;
            let max_tries = self.get_height().min(10);
            for _ in 0..max_tries {
                if GenerationCache::get_block_state(chunk, &new_root.0).to_block_id()
                    == BlockId::LAVA
                {
                    return false;
                }
                if is_circle_mostly_embedded_in_stone(chunk, wind.offset(new_root), self.radius) {
                    self.root = new_root;
                    return true;
                }
                new_root = if self.pointing_up {
                    new_root.down()
                } else {
                    new_root.up()
                };
            }
            self.radius /= 2;
        }
        false
    }

    fn get_height_at_radius(&self, check_radius: f32) -> i32 {
        get_speleothem_height(
            f64::from(check_radius),
            f64::from(self.radius),
            self.scale,
            self.bluntness,
        ) as i32
    }

    fn place_blocks<T: GenerationCache>(
        &self,
        chunk: &mut T,
        random: &mut RandomGenerator,
        wind: &WindOffsetter,
    ) {
        for dx in -self.radius..=self.radius {
            for dz in -self.radius..=self.radius {
                let current_radius = ((dx * dx + dz * dz) as f32).sqrt();
                if current_radius > self.radius as f32 {
                    continue;
                }
                let mut height = self.get_height_at_radius(current_radius);
                if height <= 0 {
                    continue;
                }
                // Vanilla compares the float roll with a double literal.
                if f64::from(random.next_f32()) < 0.2 {
                    height = (height as f32 * random.next_inbetween_f32(0.8, 1.0)) as i32;
                }
                let mut pos = self.root.add(dx, 0, dz);
                let mut has_been_out_of_stone = false;
                let max_y = if self.pointing_up {
                    chunk.get_top_y(&HeightMap::WorldSurfaceWg, pos.0.x, pos.0.z)
                } else {
                    i32::MAX
                };
                for _ in 0..height {
                    if pos.0.y >= max_y {
                        break;
                    }
                    let wind_adjusted_pos = wind.offset(pos);
                    let state = GenerationCache::get_block_state(chunk, &wind_adjusted_pos.0);
                    if is_empty_or_water_or_lava(state) {
                        has_been_out_of_stone = true;
                        chunk.set_block_state(
                            &wind_adjusted_pos.0,
                            Block::DRIPSTONE_BLOCK.default_state,
                        );
                    } else if has_been_out_of_stone
                        && state
                            .to_block_id()
                            .has_tag(tag::Block::MINECRAFT_BASE_STONE_OVERWORLD)
                    {
                        break;
                    }
                    pos = if self.pointing_up {
                        pos.up()
                    } else {
                        pos.down()
                    };
                }
            }
        }
    }

    fn is_suitable_for_wind(&self, min_radius_for_wind: i32, min_bluntness_for_wind: f32) -> bool {
        self.radius >= min_radius_for_wind && self.bluntness >= f64::from(min_bluntness_for_wind)
    }
}

struct WindOffsetter {
    origin_y: i32,
    wind_speed: Option<Vector3<f64>>,
    max_offset: i32,
}

impl WindOffsetter {
    fn new(
        origin_y: i32,
        random: &mut RandomGenerator,
        wind_speed_range: &FloatProvider,
        max_offset: i32,
    ) -> Self {
        let speed = wind_speed_range.get(random);
        let direction = random.next_inbetween_f32(0.0, PI);
        let wind_speed = Vector3::new(
            f64::from(cos(direction) * speed),
            0.0,
            f64::from(sin(direction) * speed),
        );
        Self {
            origin_y,
            wind_speed: Some(wind_speed),
            max_offset,
        }
    }

    const fn no_wind() -> Self {
        Self {
            origin_y: 0,
            wind_speed: None,
            max_offset: 0,
        }
    }

    fn offset(&self, pos: BlockPos) -> BlockPos {
        let Some(wind_speed) = self.wind_speed else {
            return pos;
        };
        let dy = f64::from(self.origin_y - pos.0.y);
        // Vanilla's `Mth.clamp`; `i32::clamp` would panic on a negative `max_offset`.
        let clamp = |value: f64| {
            (value.floor() as i32)
                .max(-self.max_offset)
                .min(self.max_offset)
        };
        pos.add(clamp(wind_speed.x * dy), 0, clamp(wind_speed.z * dy))
    }
}

#[cfg(test)]
mod tests {
    use pumpkin_util::math::position::BlockPos;

    use super::LargeDripstone;

    #[test]
    fn heights_match_vanilla() {
        // Vanilla's heights for radius 6, scale 1.5 and bluntness 0.5 at the distances
        // `sqrt(dx * dx + dz * dz)` that `placeBlocks` asks for, squared distance 0 to 36.
        const VANILLA: [i32; 37] = [
            24, 18, 14, 12, 11, 10, 9, 8, 7, 7, 6, 5, 5, 5, 4, 4, 4, 3, 3, 3, 2, 2, 2, 2, 1, 1, 1,
            1, 1, 1, 0, 0, 0, 0, 0, 0, 0,
        ];
        let dripstone = LargeDripstone {
            root: BlockPos::new(0, 0, 0),
            pointing_up: true,
            radius: 6,
            bluntness: f64::from(0.5f32),
            scale: f64::from(1.5f32),
        };
        let heights: Vec<i32> = (0..=36u16)
            .map(|distance_squared| {
                dripstone.get_height_at_radius(f32::from(distance_squared).sqrt())
            })
            .collect();
        assert_eq!(heights, VANILLA);
    }
}
