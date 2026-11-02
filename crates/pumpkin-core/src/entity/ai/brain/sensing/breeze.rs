use super::super::BrainTick;
use super::super::memory::{MemoryModuleId, types};
use super::{Sensor, is_entity_attackable};

const REQUIRES: [MemoryModuleId; 2] = [
    types::NEAREST_ATTACKABLE.id(),
    types::NEAREST_LIVING_ENTITIES.id(),
];

/// Vanilla `BreezeAttackEntitySensor`: the nearest it can attack, creative players left out.
pub struct BreezeAttackEntitySensor;

impl Sensor for BreezeAttackEntitySensor {
    fn requires(&self) -> &'static [MemoryModuleId] {
        &REQUIRES
    }

    fn do_tick(&mut self, tick: &mut BrainTick<'_>) {
        let attackable = {
            let Some(nearby) = tick.state.get(types::NEAREST_LIVING_ENTITIES) else {
                return;
            };
            let ctx = tick.visibility();
            nearby
                .iter()
                .find(|candidate| {
                    candidate.get_player().is_none_or(|player| {
                        !player.is_spectator()
                            && player.gamemode.load() != pumpkin_util::GameMode::Creative
                    }) && is_entity_attackable(&ctx, candidate.as_ref())
                })
                .cloned()
        };
        tick.state
            .set_optional(types::NEAREST_ATTACKABLE, attackable);
    }
}
