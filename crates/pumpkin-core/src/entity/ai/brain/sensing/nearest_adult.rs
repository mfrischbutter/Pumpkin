use super::super::BrainTick;
use super::super::memory::{MemoryModuleId, types};
use super::Sensor;

const REQUIRES: [MemoryModuleId; 2] = [
    types::NEAREST_VISIBLE_ADULT.id(),
    types::NEAREST_VISIBLE_LIVING_ENTITIES.id(),
];

/// Vanilla `AdultSensor` (`NEAREST_ADULT`): the closest visible grown-up of the mob's type.
pub struct AdultSensor;

impl Sensor for AdultSensor {
    fn requires(&self) -> &'static [MemoryModuleId] {
        &REQUIRES
    }

    fn do_tick(&mut self, tick: &mut BrainTick<'_>) {
        let kind = tick.mob.get_entity().entity_type;
        let adult = {
            let Some(visible) = tick.state.get(types::NEAREST_VISIBLE_LIVING_ENTITIES) else {
                return;
            };
            let ctx = tick.visibility();
            visible.find_closest(&ctx, |entity| {
                entity.get_entity().entity_type == kind
                    && entity
                        .get_entity()
                        .age
                        .load(std::sync::atomic::Ordering::Relaxed)
                        >= 0
            })
        };
        tick.state.set_optional(types::NEAREST_VISIBLE_ADULT, adult);
    }
}
