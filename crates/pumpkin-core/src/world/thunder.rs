//! Vanilla `ServerLevel.tickThunder`: a lightning strike in a spawning chunk during a
//! thunderstorm, drawn to a lightning rod or a living entity under the sky, and the
//! skeleton trap horse it may leave.

use std::sync::{Arc, PoisonError};

use pumpkin_data::entity::EntityType;
use pumpkin_data::poi_type::PoiType;
use pumpkin_data::tag::Block::MINECRAFT_LIGHTNING_RODS;
use pumpkin_data::tag::Taggable;
use pumpkin_util::math::boundingbox::BoundingBox;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector2::Vector2;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_world::chunk::ChunkHeightmapType::{MotionBlocking, WorldSurface};
use pumpkin_world::poi::Occupancy;
use rand::{RngExt, rng};
use uuid::Uuid;

use super::World;
use crate::entity::ageable::AgeableMob;
use crate::entity::lightning::LightningBoltEntity;
use crate::entity::mob::equipment::RegionalDifficulty;
use crate::entity::passive::skeleton_horse::SkeletonHorseEntity;
use crate::entity::{Entity, EntityBase};

/// Vanilla rolls `nextInt(100000) == 0` per spawning chunk and tick.
const STRIKE_ODDS: i32 = 100_000;
/// The trap horse chance is the effective difficulty times this.
const TRAP_CHANCE_PER_DIFFICULTY: f64 = 0.01;
/// A lightning rod this close to the strike's column draws it.
const LIGHTNING_ROD_RANGE: i32 = 128;
/// Living entities this far around the column may be struck instead.
const TARGET_RANGE: f64 = 3.0;

impl World {
    /// Vanilla `ServerLevel.tickThunder` for one spawning chunk.
    pub(super) fn tick_thunder(self: &Arc<Self>, chunk_pos: Vector2<i32>) {
        if !self.is_raining() || !self.is_thundering() || rng().random_range(0..STRIKE_ODDS) != 0 {
            return;
        }
        let pos = self.find_lightning_target_around(block_random_pos(
            chunk_pos.x << 4,
            0,
            chunk_pos.y << 4,
        ));
        if !self.is_raining_at(&pos) {
            return;
        }
        let difficulty = RegionalDifficulty::at(self, pos.to_f64());
        let is_trap = self.level_info.load().game_rules.spawn_mobs
            && rng().random::<f64>()
                < f64::from(difficulty.effective_difficulty) * TRAP_CHANCE_PER_DIFFICULTY
            && !self
                .get_block(&pos.down())
                .has_tag(&MINECRAFT_LIGHTNING_RODS);
        if is_trap {
            let horse = SkeletonHorseEntity::new(Entity::from_uuid(
                Uuid::new_v4(),
                self.clone(),
                pos.to_f64(),
                &EntityType::SKELETON_HORSE,
            ));
            horse.set_trap(true);
            horse.set_baby(false);
            self.spawn_entity_non_save(horse);
        }
        let bolt = LightningBoltEntity::new(Entity::from_uuid(
            Uuid::new_v4(),
            self.clone(),
            pos.to_f64().add_raw(0.5, 0.0, 0.5),
            &EntityType::LIGHTNING_BOLT,
        ));
        bolt.set_visual_only(is_trap);
        self.spawn_entity_non_save(Arc::new(bolt));
    }

    /// Vanilla `ServerLevel.findLightningTargetAround`: a lightning rod near the column's
    /// surface, else a living entity under the sky around the column, else the surface.
    fn find_lightning_target_around(&self, pos: BlockPos) -> BlockPos {
        // `get_heightmap_height` is the top block, vanilla `getHeightmapPos` the air above it.
        let center = BlockPos::new(
            pos.0.x,
            self.get_heightmap_height(MotionBlocking, pos.0.x, pos.0.z) + 1,
            pos.0.z,
        );
        if let Some(rod) = self.find_lightning_rod(center) {
            return rod;
        }
        let top = self.min_y + self.dimension.height;
        let search = BoundingBox::new(
            Vector3::new(
                f64::from(center.0.x),
                f64::from(center.0.y),
                f64::from(center.0.z),
            ),
            Vector3::new(
                f64::from(center.0.x + 1),
                f64::from(top + 1),
                f64::from(center.0.z + 1),
            ),
        )
        .expand_all(TARGET_RANGE);
        let under_sky =
            |entity: &Entity| entity.is_alive() && self.can_see_sky(&entity.block_pos.load());
        let mut targets: Vec<BlockPos> = self
            .get_entities_at_box(&search)
            .iter()
            .filter(|entity| entity.get_living_entity().is_some())
            .map(|entity| entity.get_entity())
            .filter(|entity| under_sky(entity))
            .map(|entity| entity.block_pos.load())
            .collect();
        targets.extend(
            self.get_players_at_box(&search)
                .iter()
                .map(|player| player.get_entity())
                .filter(|entity| under_sky(entity))
                .map(|entity| entity.block_pos.load()),
        );
        if !targets.is_empty() {
            return targets[rng().random_range(0..targets.len())];
        }
        if center.0.y == self.min_y - 1 {
            return center.up().up();
        }
        center
    }

    /// Vanilla `ServerLevel.findLightningRod`: the closest lightning rod within 128 blocks
    /// that tops its column; the strike lands on the block above it.
    fn find_lightning_rod(&self, center: BlockPos) -> Option<BlockPos> {
        self.portal_poi
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .find_closest(
                center,
                LIGHTNING_ROD_RANGE,
                |kind| kind == &PoiType::LIGHTNING_ROD,
                |rod| rod.0.y == self.get_heightmap_height(WorldSurface, rod.0.x, rod.0.z),
                Occupancy::Any,
            )
            .map(|rod| rod.up())
    }
}

/// Vanilla `Level.getBlockRandomPos` with its mask of 15: a block of the chunk column from
/// `min_y` upwards (the random source differs).
fn block_random_pos(min_x: i32, min_y: i32, min_z: i32) -> BlockPos {
    let value = rng().random::<i32>() >> 2;
    BlockPos::new(
        min_x + (value & 15),
        min_y + (value >> 16 & 15),
        min_z + (value >> 8 & 15),
    )
}
