use pumpkin_data::entity::EntityType;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_world::poi::{Occupancy, section_of};
use rand::RngExt;

use super::wander_around::WanderAroundGoal;
use crate::entity::EntityBase;
use crate::entity::ai::util::land_random_pos;
use crate::entity::mob::Mob;
use crate::entity::passive::villager::VillagerEntity;

const POI_SECTION_SCAN_RADIUS: i32 = 2;
const VILLAGER_SCAN_RADIUS: f64 = 32.0;
const RANDOM_POS_XY_DISTANCE: i32 = 10;
const RANDOM_POS_Y_DISTANCE: i32 = 7;
const POI_SEARCH_RADIUS: i32 = 8;
const INTERVAL: i32 = 240;

/// Vanilla `GolemRandomStrollInVillageGoal`: strolls towards a villager that wants a golem or
/// an occupied point of interest of a village section nearby.
#[must_use]
pub const fn golem_random_stroll_in_village(speed: f64) -> WanderAroundGoal {
    WanderAroundGoal::with_interval(speed, INTERVAL)
        .with_position(get_position, VILLAGER_SCAN_RADIUS)
}

// Vanilla draws these from the level's random, which Pumpkin does not have; the mob's is used.
fn get_position(mob: &dyn Mob) -> Option<Vector3<f64>> {
    let mut random = mob.get_random();
    if random.random::<f32>() < 0.3 {
        return get_position_towards_anywhere(mob);
    }
    let target = if random.random::<f32>() < 0.7 {
        get_position_towards_villager_who_wants_golem(mob).or_else(|| get_position_towards_poi(mob))
    } else {
        get_position_towards_poi(mob).or_else(|| get_position_towards_villager_who_wants_golem(mob))
    };
    target.or_else(|| get_position_towards_anywhere(mob))
}

fn get_position_towards_anywhere(mob: &dyn Mob) -> Option<Vector3<f64>> {
    land_random_pos::get_pos(mob, RANDOM_POS_XY_DISTANCE, RANDOM_POS_Y_DISTANCE)
}

fn get_position_towards_villager_who_wants_golem(mob: &dyn Mob) -> Option<Vector3<f64>> {
    let world = mob.get_entity().world.load();
    let game_time = world.get_world_age();
    let search_box = mob.get_entity().bounding_box.load().expand(
        VILLAGER_SCAN_RADIUS,
        VILLAGER_SCAN_RADIUS,
        VILLAGER_SCAN_RADIUS,
    );
    let villagers: Vec<_> = world
        .get_entities_at_box(&search_box)
        .into_iter()
        .filter(|entity| entity.get_entity().entity_type == &EntityType::VILLAGER)
        .filter(|entity| does_villager_want_golem(entity.as_ref(), game_time))
        .collect();
    if villagers.is_empty() {
        return None;
    }
    let villager = &villagers[mob.get_random().random_range(0..villagers.len())];
    land_random_pos::get_pos_towards(
        mob,
        RANDOM_POS_XY_DISTANCE,
        RANDOM_POS_Y_DISTANCE,
        villager.get_entity().pos.load(),
    )
}

/// Vanilla `GolemRandomStrollInVillageGoal.doesVillagerWantGolem`.
fn does_villager_want_golem(entity: &dyn EntityBase, game_time: i64) -> bool {
    entity
        .cast_any()
        .downcast_ref::<VillagerEntity>()
        .is_some_and(|villager| villager.wants_to_spawn_golem_at(game_time))
}

fn get_position_towards_poi(mob: &dyn Mob) -> Option<Vector3<f64>> {
    let target_section = get_random_village_section(mob)?;
    let target_pos = get_random_poi_within_section(mob, target_section)?;
    land_random_pos::get_pos_towards(
        mob,
        RANDOM_POS_XY_DISTANCE,
        RANDOM_POS_Y_DISTANCE,
        Vector3::new(
            f64::from(target_pos.0.x) + 0.5,
            f64::from(target_pos.0.y),
            f64::from(target_pos.0.z) + 0.5,
        ),
    )
}

/// The village sections around the golem, in vanilla's `SectionPos.cube` order (x fastest).
fn get_random_village_section(mob: &dyn Mob) -> Option<(i32, i32, i32)> {
    let world = mob.get_entity().world.load();
    let center = section_of(&mob.get_entity().block_pos.load());
    let radius = POI_SECTION_SCAN_RADIUS;
    let mut village_sections = Vec::new();
    for z in -radius..=radius {
        for y in -radius..=radius {
            for x in -radius..=radius {
                let section = (center.0 + x, center.1 + y, center.2 + z);
                if world.sections_to_village(section) == 0 {
                    village_sections.push(section);
                }
            }
        }
    }
    if village_sections.is_empty() {
        return None;
    }
    Some(village_sections[mob.get_random().random_range(0..village_sections.len())])
}

fn get_random_poi_within_section(mob: &dyn Mob, (x, y, z): (i32, i32, i32)) -> Option<BlockPos> {
    let world = mob.get_entity().world.load();
    let center = BlockPos::new((x << 4) + 8, (y << 4) + 8, (z << 4) + 8);
    let pois = world.pois_in_range(center, POI_SEARCH_RADIUS, |_| true, Occupancy::IsOccupied);
    if pois.is_empty() {
        return None;
    }
    Some(pois[mob.get_random().random_range(0..pois.len())].0)
}
