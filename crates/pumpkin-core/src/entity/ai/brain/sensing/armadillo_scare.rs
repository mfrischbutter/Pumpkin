use super::super::BrainTick;
use super::super::memory::{MemoryModuleId, types};
use super::Sensor;
use crate::entity::passive::armadillo::ArmadilloEntity;

const REQUIRES: [MemoryModuleId; 1] = [types::NEAREST_LIVING_ENTITIES.id()];

/// Vanilla `ARMADILLO_SCARE_DETECTED`, a `MobSensor`: while an armadillo could stay rolled up,
/// a scary mob nearby keeps `DANGER_DETECTED_RECENTLY` for 80 ticks.
pub struct ArmadilloScareSensor;

impl Sensor for ArmadilloScareSensor {
    fn requires(&self) -> &'static [MemoryModuleId] {
        &REQUIRES
    }

    fn do_tick(&mut self, tick: &mut BrainTick<'_>) {
        let Some(armadillo) = tick.mob.cast_any().downcast_ref::<ArmadilloEntity>() else {
            return;
        };
        if !armadillo.can_stay_rolled_up() {
            tick.state.erase(types::DANGER_DETECTED_RECENTLY.id());
            return;
        }
        let scared = tick
            .state
            .get(types::NEAREST_LIVING_ENTITIES)
            .is_some_and(|nearby| {
                nearby
                    .iter()
                    .any(|entity| armadillo.is_scared_by(entity.as_ref()))
            });
        if scared {
            tick.state
                .set_with_expiry(types::DANGER_DETECTED_RECENTLY, true, 80);
        }
    }
}
