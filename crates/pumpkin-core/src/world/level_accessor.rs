//! Vanilla `ServerLevelAccessor` as far as spawning needs it.
//!
//! The live world, or the region around a chunk that is still being generated
//! (`WorldGenRegion`): the spawn placement rules, the collision test and a mob's own spawn
//! checks read blocks, fluids, biomes and light through it, so chunk generation spawns see
//! the chunk they spawn into.

use pumpkin_data::chunk::Biome;
use pumpkin_data::fluid::Fluid;
use pumpkin_data::{Block, BlockState, BlockStateId};
use pumpkin_util::math::boundingbox::BoundingBox;
use pumpkin_util::math::position::BlockPos;
use pumpkin_world::chunk::ChunkLight;
use pumpkin_world::chunk::format::LightContainer;
use pumpkin_world::chunk::palette::BlockPalette;
use pumpkin_world::generation::proto_chunk::GenerationCache;
use pumpkin_world::world::BlockAccessor;

use crate::entity::Entity;
use crate::world::{MAX_LIGHT_LEVEL, World, brightness};

pub trait LevelAccessor: BlockAccessor {
    /// Vanilla `ServerLevelAccessor.getLevel`: the world behind the accessor (difficulty,
    /// game rules, dimension, players, time).
    fn get_level(&self) -> &World;

    fn get_fluid(&self, pos: &BlockPos) -> &'static Fluid;

    /// Vanilla `LevelReader.getBiome`: the biome through the `BiomeManager` zoom.
    fn get_biome(&self, pos: &BlockPos) -> &'static Biome;

    /// Vanilla `getBrightness(LightLayer.SKY, pos)`.
    fn get_sky_light_level(&self, pos: &BlockPos) -> u8;

    /// Vanilla `getBrightness(LightLayer.BLOCK, pos)`.
    fn get_block_light_level(&self, pos: &BlockPos) -> u8;

    /// Vanilla `LevelReader.isUnobstructed(Entity)`: no other entity in the entity's box.
    fn is_unobstructed(&self, entity: &Entity) -> bool;

    /// Vanilla `CollisionGetter.noCollision(AABB)`: no block collision shape and no entity
    /// that can be collided with (boats, shulkers) meets the box.
    fn no_collision(&self, bounding_box: &BoundingBox) -> bool;

    /// Vanilla `LevelReader.getRawBrightness`.
    fn get_raw_brightness(&self, pos: &BlockPos, sky_darken: u8) -> u8 {
        self.get_sky_light_level(pos)
            .saturating_sub(sky_darken)
            .max(self.get_block_light_level(pos))
    }

    /// Vanilla `LevelReader.getMaxLocalRawBrightness`.
    fn get_max_local_raw_brightness(&self, pos: &BlockPos) -> u8 {
        self.get_raw_brightness(pos, self.get_level().get_sky_darken() as u8)
    }

    /// Vanilla `LevelReader.getLightLevelDependentMagicValue`.
    fn get_light_level_dependent_magic_value(&self, pos: &BlockPos) -> f32 {
        brightness::light_level_curve(
            self.get_max_local_raw_brightness(pos),
            self.get_level().dimension.ambient_light,
        )
    }

    /// Vanilla `LevelReader.getPathfindingCostFromLightLevels`.
    fn get_pathfinding_cost_from_light_levels(&self, pos: &BlockPos) -> f32 {
        self.get_light_level_dependent_magic_value(pos) - 0.5
    }

    /// Vanilla `LevelReader.canSeeSky`.
    fn can_see_sky(&self, pos: &BlockPos) -> bool {
        self.get_level().is_within_build_height(pos)
            && self.get_sky_light_level(pos) >= MAX_LIGHT_LEVEL
    }

    /// Vanilla `LevelReader.canSeeSkyFromBelowWater`: below the sea level the sky must be
    /// visible at the sea level, with only liquids and light-passing blocks in between.
    fn can_see_sky_from_below_water(&self, pos: &BlockPos) -> bool {
        let sea_level = self.get_level().sea_level;
        if pos.0.y >= sea_level {
            return self.can_see_sky(pos);
        }
        let scan_point = BlockPos::new(pos.0.x, sea_level, pos.0.z);
        if !self.can_see_sky(&scan_point) {
            return false;
        }
        let mut scan = scan_point.down();
        while scan.0.y > pos.0.y {
            let state = self.get_block_state(&scan);
            if state.opacity > 0 && !state.is_liquid() {
                return false;
            }
            scan = scan.down();
        }
        true
    }

    /// Vanilla `LevelReader.containsAnyLiquid`.
    fn contains_any_liquid(&self, bounding_box: BoundingBox) -> bool {
        let min_x = bounding_box.min.x.floor() as i32;
        let max_x = bounding_box.max.x.ceil() as i32;
        let min_y = bounding_box.min.y.floor() as i32;
        let max_y = bounding_box.max.y.ceil() as i32;
        let min_z = bounding_box.min.z.floor() as i32;
        let max_z = bounding_box.max.z.ceil() as i32;
        for x in min_x..max_x {
            for y in min_y..max_y {
                for z in min_z..max_z {
                    if self.get_fluid(&BlockPos::new(x, y, z)).id != Fluid::EMPTY.id {
                        return true;
                    }
                }
            }
        }
        false
    }
}

impl LevelAccessor for World {
    fn get_level(&self) -> &World {
        self
    }

    fn get_fluid(&self, pos: &BlockPos) -> &'static Fluid {
        Self::get_fluid(self, pos)
    }

    fn get_biome(&self, pos: &BlockPos) -> &'static Biome {
        Self::get_biome(self, pos)
    }

    fn get_sky_light_level(&self, pos: &BlockPos) -> u8 {
        Self::get_sky_light_level(self, pos)
    }

    fn get_block_light_level(&self, pos: &BlockPos) -> u8 {
        Self::get_block_light_level(self, pos).unwrap_or(0)
    }

    fn is_unobstructed(&self, entity: &Entity) -> bool {
        Self::is_unobstructed(self, entity)
    }

    fn no_collision(&self, bounding_box: &BoundingBox) -> bool {
        self.is_space_empty(*bounding_box) && !self.has_collidable_entity(bounding_box)
    }
}

/// Vanilla `WorldGenRegion` as far as spawning needs it.
///
/// The chunks a generation cache holds around the chunk being generated, with the world
/// behind them. A region holds no entities, and a chunk the cache does not hold is read from
/// the world.
pub struct GenerationRegion<'a> {
    world: &'a World,
    cache: &'a dyn GenerationCache,
}

impl<'a> GenerationRegion<'a> {
    #[must_use]
    pub fn new(world: &'a World, cache: &'a dyn GenerationCache) -> Self {
        Self { world, cache }
    }

    /// The chunks being generated, which the region reads before the world.
    #[must_use]
    pub fn cache(&self) -> &'a dyn GenerationCache {
        self.cache
    }

    /// The light of `layer` at `pos` from the cache's chunk, `None` when the cache does not
    /// hold that chunk as one being generated; `above_world` above the top section.
    fn light(
        &self,
        pos: &BlockPos,
        layer: impl Fn(&ChunkLight) -> &[LightContainer],
        above_world: u8,
    ) -> Option<u8> {
        let chunk = self.cache.get_chunk(pos.0.x >> 4, pos.0.z >> 4)?;
        let relative_y = pos.0.y - i32::from(chunk.bottom_y());
        let section = usize::try_from(relative_y.div_euclid(BlockPalette::SIZE as i32)).ok()?;
        let Some(container) = layer(&chunk.light).get(section) else {
            return Some(above_world);
        };
        Some(container.get(
            (pos.0.x & 15) as usize,
            relative_y.rem_euclid(BlockPalette::SIZE as i32) as usize,
            (pos.0.z & 15) as usize,
        ))
    }
}

impl BlockAccessor for GenerationRegion<'_> {
    fn get_block(&self, position: &BlockPos) -> &'static Block {
        BlockAccessor::get_block(self.cache, position)
    }

    fn get_block_state(&self, position: &BlockPos) -> &'static BlockState {
        BlockAccessor::get_block_state(self.cache, position)
    }

    fn get_block_state_id(&self, position: &BlockPos) -> BlockStateId {
        BlockAccessor::get_block_state_id(self.cache, position)
    }

    fn get_block_and_state(&self, position: &BlockPos) -> (&'static Block, &'static BlockState) {
        BlockAccessor::get_block_and_state(self.cache, position)
    }
}

impl LevelAccessor for GenerationRegion<'_> {
    fn get_level(&self) -> &World {
        self.world
    }

    fn get_fluid(&self, pos: &BlockPos) -> &'static Fluid {
        World::get_fluid_from_state_id(BlockAccessor::get_block_state_id(self.cache, pos))
    }

    fn get_biome(&self, pos: &BlockPos) -> &'static Biome {
        self.world.get_biome(pos)
    }

    fn get_sky_light_level(&self, pos: &BlockPos) -> u8 {
        self.light(pos, |light| &light.sky_light, MAX_LIGHT_LEVEL)
            .unwrap_or_else(|| self.world.get_sky_light_level(pos))
    }

    fn get_block_light_level(&self, pos: &BlockPos) -> u8 {
        self.light(pos, |light| &light.block_light, 0)
            .unwrap_or_else(|| self.world.get_block_light_level(pos).unwrap_or(0))
    }

    fn is_unobstructed(&self, _entity: &Entity) -> bool {
        true
    }

    fn no_collision(&self, bounding_box: &BoundingBox) -> bool {
        let min = bounding_box.min_block_pos();
        let max = bounding_box.max_block_pos();
        BlockPos::iterate(min, max).all(|pos| {
            let state = BlockAccessor::get_block_state(self.cache, &pos);
            !World::check_collision(bounding_box, pos, state, false, |_| ())
        })
    }
}
