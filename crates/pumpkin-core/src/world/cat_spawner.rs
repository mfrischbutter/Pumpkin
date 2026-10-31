//! Vanilla `CatSpawner`: cats for villages with claimed beds and for swamp huts, one of the
//! overworld's custom spawners.

use std::sync::{Arc, LazyLock, PoisonError};

use pumpkin_data::entity::EntityType;
use pumpkin_data::poi_type::PoiType;
use pumpkin_data::structures::StructureKeys;
use pumpkin_data::tag::WorldgenStructure::MINECRAFT_CATS_SPAWN_IN;
use pumpkin_util::math::boundingbox::BoundingBox;
use pumpkin_util::math::position::BlockPos;
use pumpkin_world::poi::Occupancy;
use rand::RngExt;
use uuid::Uuid;

use crate::entity::EntityBase;
use crate::entity::mob::spawn::finalize_spawn;
use crate::entity::r#type::from_type;
use crate::world::World;
use crate::world::natural_spawner::is_spawn_position_ok;

/// Vanilla `TICK_DELAY`.
const TICK_DELAY: i32 = 1_200;
/// The spot is 8 to 31 blocks from the player on each axis.
const MIN_OFFSET: i32 = 8;
const OFFSET_RANGE: i32 = 24;
/// The chunks this far around the spot must be loaded.
const LOADED_RANGE: i32 = 10;
/// Vanilla `isCloseToVillage(pos, 2)`.
const VILLAGE_SECTION_DISTANCE: i32 = 2;
/// A village cat needs more than four claimed beds within 48 blocks and fewer than five
/// cats in the 48 by 8 by 48 box.
const VILLAGE_RANGE: i32 = 48;
const MIN_CLAIMED_BEDS: usize = 4;
const MAX_VILLAGE_CATS: usize = 5;
/// A hut cat needs no cat in the 16 by 8 by 16 box.
const HUT_RANGE: f64 = 16.0;
const CAT_BOX_HEIGHT: f64 = 8.0;

/// The structures of the `cats_spawn_in` tag.
static CATS_SPAWN_IN: LazyLock<Vec<StructureKeys>> = LazyLock::new(|| {
    MINECRAFT_CATS_SPAWN_IN
        .0
        .iter()
        .filter_map(|name| StructureKeys::from_name(name))
        .collect()
});

#[derive(Default)]
pub struct CatSpawner {
    next_tick: i32,
}

impl CatSpawner {
    /// Vanilla `CatSpawner.tick`: every minute a spot near a random living player, a cat
    /// when it lies in a village or a swamp hut.
    pub fn tick(&mut self, world: &Arc<World>, _spawn_enemies: bool) {
        self.next_tick -= 1;
        if self.next_tick > 0 {
            return;
        }
        self.next_tick = TICK_DELAY;
        let mut rng = rand::rng();
        let players = world.players.load();
        let alive: Vec<_> = players
            .iter()
            .filter(|player| player.get_entity().is_alive())
            .collect();
        if alive.is_empty() {
            return;
        }
        let player = alive[rng.random_range(0..alive.len())];
        let dx = (MIN_OFFSET + rng.random_range(0..OFFSET_RANGE)) * random_sign(&mut rng);
        let dz = (MIN_OFFSET + rng.random_range(0..OFFSET_RANGE)) * random_sign(&mut rng);
        let spawn_pos = player.get_entity().block_pos.load().add(dx, 0, dz);
        if !world.has_chunks_at(
            spawn_pos.0.x - LOADED_RANGE,
            spawn_pos.0.z - LOADED_RANGE,
            spawn_pos.0.x + LOADED_RANGE,
            spawn_pos.0.z + LOADED_RANGE,
        ) || !is_spawn_position_ok(world, &spawn_pos, &EntityType::CAT)
        {
            return;
        }
        if world.is_close_to_village(&spawn_pos, VILLAGE_SECTION_DISTANCE) {
            spawn_in_village(world, spawn_pos);
        } else if world.structure_with_piece_at(&CATS_SPAWN_IN, &spawn_pos) {
            spawn_in_hut(world, spawn_pos);
        }
    }
}

/// Vanilla `random.nextBoolean() ? -1 : 1`.
fn random_sign<R: rand::Rng + ?Sized>(rng: &mut R) -> i32 {
    if rng.random::<bool>() { -1 } else { 1 }
}

/// Vanilla `CatSpawner.spawnInVillage`.
fn spawn_in_village(world: &Arc<World>, spawn_pos: BlockPos) {
    let claimed_beds = world
        .portal_poi
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .count_in_range(
            spawn_pos,
            VILLAGE_RANGE,
            |kind| kind == &PoiType::HOME,
            Occupancy::IsOccupied,
        );
    if claimed_beds > MIN_CLAIMED_BEDS
        && cats_within(world, spawn_pos, f64::from(VILLAGE_RANGE)) < MAX_VILLAGE_CATS
    {
        spawn_cat(world, spawn_pos, false);
    }
}

/// Vanilla `CatSpawner.spawnInHut`.
fn spawn_in_hut(world: &Arc<World>, spawn_pos: BlockPos) {
    if cats_within(world, spawn_pos, HUT_RANGE) == 0 {
        spawn_cat(world, spawn_pos, true);
    }
}

/// The cats whose box meets the block's box grown by `range` sideways and 8 up and down.
fn cats_within(world: &World, center: BlockPos, range: f64) -> usize {
    let search = BoundingBox::from_block(&center).expand(range, CAT_BOX_HEIGHT, range);
    world
        .get_entities_at_box(&search)
        .iter()
        .filter(|entity| entity.get_entity().entity_type == &EntityType::CAT)
        .count()
}

/// Vanilla `CatSpawner.spawnCat`: a finalized cat at the block's corner, persistent in a hut.
fn spawn_cat(world: &Arc<World>, spawn_pos: BlockPos, make_persistent: bool) {
    let cat = from_type(&EntityType::CAT, spawn_pos.to_f64(), world, Uuid::new_v4());
    finalize_spawn(&cat, world, None);
    if make_persistent && let Some(mob) = cat.get_mob() {
        mob.get_mob_entity().set_persistence_required();
    }
    world.spawn_entity_non_save(cat);
}
