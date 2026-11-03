use super::super::BrainTick;
use super::super::memory::{MemoryModuleId, types};
use super::Sensor;
use crate::entity::mob::warden::WardenEntity;

const REQUIRES: [MemoryModuleId; 2] = [
    types::NEAREST_ATTACKABLE.id(),
    types::NEAREST_LIVING_ENTITIES.id(),
];

/// Vanilla `WardenEntitySensor`: the nearest it may target, players before anything else.
pub struct WardenEntitySensor;

impl Sensor for WardenEntitySensor {
    fn requires(&self) -> &'static [MemoryModuleId] {
        &REQUIRES
    }

    fn do_tick(&mut self, tick: &mut BrainTick<'_>) {
        let Some(warden) = tick.mob.cast_any().downcast_ref::<WardenEntity>() else {
            return;
        };
        let attackable = {
            let Some(nearby) = tick.state.get(types::NEAREST_LIVING_ENTITIES) else {
                return;
            };
            let mut targets = nearby
                .iter()
                .filter(|e| warden.can_target_entity(e.as_ref()));
            let first = targets.clone().find(|e| e.get_player().is_some());
            first.or_else(|| targets.next()).cloned()
        };
        tick.state
            .set_optional(types::NEAREST_ATTACKABLE, attackable);
    }
}
