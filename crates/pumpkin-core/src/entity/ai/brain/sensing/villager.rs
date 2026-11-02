//! The sensors of villagers: hostile mobs near, babies in sight, the secondary work blocks and
//! iron golems around.

use std::sync::atomic::Ordering::Relaxed;

use pumpkin_data::Block;
use pumpkin_data::entity::EntityType;
use pumpkin_util::math::position::BlockPos;

use super::super::behavior::poi::{HomeBatch, global_pos};
use super::super::memory::{MemoryModuleId, types};
use super::super::{BrainState, BrainTick};
use super::Sensor;

/// Vanilla `VillagerHostilesSensor.ACCEPTABLE_DISTANCE_FROM_HOSTILES`.
fn acceptable_distance(kind: &EntityType) -> Option<f64> {
    Some(match kind {
        kind if kind == &EntityType::DROWNED
            || kind == &EntityType::HUSK
            || kind == &EntityType::VEX
            || kind == &EntityType::ZOMBIE
            || kind == &EntityType::ZOMBIE_VILLAGER =>
        {
            8.0
        }
        kind if kind == &EntityType::VINDICATOR || kind == &EntityType::ZOGLIN => 10.0,
        kind if kind == &EntityType::EVOKER
            || kind == &EntityType::ILLUSIONER
            || kind == &EntityType::RAVAGER =>
        {
            12.0
        }
        kind if kind == &EntityType::PILLAGER => 15.0,
        _ => return None,
    })
}

const HOSTILES_REQUIRE: [MemoryModuleId; 2] = [
    types::NEAREST_HOSTILE.id(),
    types::NEAREST_VISIBLE_LIVING_ENTITIES.id(),
];

/// Vanilla `VillagerHostilesSensor`.
pub struct VillagerHostilesSensor;

impl Sensor for VillagerHostilesSensor {
    fn requires(&self) -> &'static [MemoryModuleId] {
        &HOSTILES_REQUIRE
    }

    fn do_tick(&mut self, tick: &mut BrainTick<'_>) {
        let position = tick.mob.get_entity().pos.load();
        let hostile = {
            let Some(visible) = tick.state.get(types::NEAREST_VISIBLE_LIVING_ENTITIES) else {
                return;
            };
            visible.find_closest(&tick.visibility(), |mob| {
                let entity = mob.get_entity();
                acceptable_distance(entity.entity_type).is_some_and(|distance| {
                    entity.pos.load().squared_distance_to_vec(&position) <= distance * distance
                })
            })
        };
        tick.state.set_optional(types::NEAREST_HOSTILE, hostile);
    }
}

const BABIES_REQUIRE: [MemoryModuleId; 1] = [types::VISIBLE_VILLAGER_BABIES.id()];

/// Vanilla `VillagerBabiesSensor`.
pub struct VillagerBabiesSensor;

impl Sensor for VillagerBabiesSensor {
    fn requires(&self) -> &'static [MemoryModuleId] {
        &BABIES_REQUIRE
    }

    fn do_tick(&mut self, tick: &mut BrainTick<'_>) {
        let babies = tick
            .state
            .get(types::NEAREST_VISIBLE_LIVING_ENTITIES)
            .map_or_else(Vec::new, |visible| {
                visible
                    .find_all(&tick.visibility(), |mob| {
                        let entity = mob.get_entity();
                        entity.entity_type == &EntityType::VILLAGER && entity.age.load(Relaxed) < 0
                    })
                    .cloned()
                    .collect()
            });
        tick.state.set(types::VISIBLE_VILLAGER_BABIES, babies);
    }
}

const SECONDARY_REQUIRE: [MemoryModuleId; 1] = [types::SECONDARY_JOB_SITE.id()];

/// Vanilla `SecondaryPoiSensor`: the blocks the mob's profession also works at, as the mob
/// names them.
pub struct SecondaryPoiSensor;

impl Sensor for SecondaryPoiSensor {
    fn requires(&self) -> &'static [MemoryModuleId] {
        &SECONDARY_REQUIRE
    }

    fn do_tick(&mut self, tick: &mut BrainTick<'_>) {
        let blocks: &[&Block] = tick.mob.mob_secondary_poi_blocks();
        let center = tick.mob.get_entity().block_pos.load();
        let mut sites = Vec::new();
        if !blocks.is_empty() {
            for pos in BlockPos::iterate(
                BlockPos::new(center.0.x - 4, center.0.y - 2, center.0.z - 4),
                BlockPos::new(center.0.x + 4, center.0.y + 2, center.0.z + 4),
            ) {
                if blocks.contains(&tick.world.get_block(&pos)) {
                    sites.push(global_pos(tick.world, pos));
                }
            }
        }
        if sites.is_empty() {
            tick.state.erase(types::SECONDARY_JOB_SITE.id());
        } else {
            tick.state.set(types::SECONDARY_JOB_SITE, sites);
        }
    }
}

/// Vanilla `GolemSensor.MEMORY_TIME_TO_LIVE`.
const GOLEM_MEMORY_TIME_TO_LIVE: i64 = 599;

const GOLEM_REQUIRE: [MemoryModuleId; 2] = [
    types::NEAREST_LIVING_ENTITIES.id(),
    types::GOLEM_DETECTED_RECENTLY.id(),
];

/// Vanilla `GolemSensor.golemDetected`.
pub fn golem_detected(state: &mut BrainState) {
    state.set_with_expiry(
        types::GOLEM_DETECTED_RECENTLY,
        true,
        GOLEM_MEMORY_TIME_TO_LIVE,
    );
}

/// Vanilla `GolemSensor`.
pub struct GolemSensor;

impl Sensor for GolemSensor {
    fn requires(&self) -> &'static [MemoryModuleId] {
        &GOLEM_REQUIRE
    }

    fn do_tick(&mut self, tick: &mut BrainTick<'_>) {
        let present = tick
            .state
            .get(types::NEAREST_LIVING_ENTITIES)
            .is_some_and(|mobs| {
                mobs.iter()
                    .any(|mob| mob.get_entity().entity_type == &EntityType::IRON_GOLEM)
            });
        if present {
            golem_detected(tick.state);
        }
    }
}

const BED_REQUIRE: [MemoryModuleId; 1] = [types::NEAREST_BED.id()];

/// Vanilla `NearestBedSensor`.
#[derive(Default)]
pub struct NearestBedSensor {
    batch: HomeBatch,
}

impl Sensor for NearestBedSensor {
    fn requires(&self) -> &'static [MemoryModuleId] {
        &BED_REQUIRE
    }

    fn do_tick(&mut self, tick: &mut BrainTick<'_>) {
        if tick.mob.get_entity().age.load(Relaxed) >= 0 {
            return;
        }
        if let Some(bed) = self
            .batch
            .find_reachable_home(tick.mob, tick.world, tick.time)
        {
            tick.state.set(types::NEAREST_BED, bed);
        }
    }
}
