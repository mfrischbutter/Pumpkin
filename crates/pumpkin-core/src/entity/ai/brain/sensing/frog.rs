use super::super::BrainTick;
use super::super::memory::{MemoryModuleId, types};
use super::{Sensor, is_entity_attackable};
use crate::entity::passive::frog::can_eat;

const ATTACKABLES_REQUIRES: [MemoryModuleId; 3] = [
    types::NEAREST_ATTACKABLE.id(),
    types::NEAREST_VISIBLE_LIVING_ENTITIES.id(),
    types::UNREACHABLE_TONGUE_TARGETS.id(),
];

/// Vanilla `FrogAttackablesSensor.TARGET_DETECTION_DISTANCE`, squared.
const TARGET_DETECTION_DISTANCE_SQR: f64 = 100.0;

/// Vanilla `FrogAttackablesSensor`: the closest visible prey within 10 blocks that the tongue
/// has not failed on lately.
pub struct FrogAttackablesSensor;

impl Sensor for FrogAttackablesSensor {
    fn requires(&self) -> &'static [MemoryModuleId] {
        &ATTACKABLES_REQUIRES
    }

    fn do_tick(&mut self, tick: &mut BrainTick<'_>) {
        let position = tick.mob.get_entity().pos.load();
        let attackable = {
            let Some(visible) = tick.state.get(types::NEAREST_VISIBLE_LIVING_ENTITIES) else {
                return;
            };
            let unreachable = tick.state.get(types::UNREACHABLE_TONGUE_TARGETS);
            let ctx = tick.visibility();
            visible.find_closest(&ctx, |candidate| {
                let entity = candidate.get_entity();
                entity.pos.load().squared_distance_to_vec(&position)
                    <= TARGET_DETECTION_DISTANCE_SQR
                    && can_eat(candidate.as_ref())
                    && unreachable.is_none_or(|targets| !targets.contains(&entity.entity_uuid))
                    && is_entity_attackable(&ctx, candidate.as_ref())
            })
        };
        tick.state
            .set_optional(types::NEAREST_ATTACKABLE, attackable);
    }
}

const IN_WATER_REQUIRES: [MemoryModuleId; 1] = [types::IS_IN_WATER.id()];

/// Vanilla `IsInWaterSensor`.
pub struct IsInWaterSensor;

impl Sensor for IsInWaterSensor {
    fn requires(&self) -> &'static [MemoryModuleId] {
        &IN_WATER_REQUIRES
    }

    fn do_tick(&mut self, tick: &mut BrainTick<'_>) {
        if tick.mob.get_entity().is_in_water() {
            tick.state.set(types::IS_IN_WATER, ());
        } else {
            tick.state.erase(types::IS_IN_WATER.id());
        }
    }
}
