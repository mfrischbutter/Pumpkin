use pumpkin_data::tag::{self, Taggable};

use super::super::BrainTick;
use super::super::memory::{MemoryModuleId, types};
use super::{Sensor, is_entity_attackable};

const REQUIRES: [MemoryModuleId; 3] = [
    types::NEAREST_ATTACKABLE.id(),
    types::NEAREST_VISIBLE_LIVING_ENTITIES.id(),
    types::HAS_HUNTING_COOLDOWN.id(),
];

/// Vanilla `AxolotlAttackablesSensor.TARGET_DETECTION_DISTANCE`, squared.
const TARGET_DETECTION_DISTANCE_SQR: f64 = 64.0;

/// Vanilla `AxolotlAttackablesSensor`: the closest visible mob in the water within 8 blocks
/// that is always hostile to axolotls, or is prey while the axolotl may hunt.
pub struct AxolotlAttackablesSensor;

impl Sensor for AxolotlAttackablesSensor {
    fn requires(&self) -> &'static [MemoryModuleId] {
        &REQUIRES
    }

    fn do_tick(&mut self, tick: &mut BrainTick<'_>) {
        let position = tick.mob.get_entity().pos.load();
        let may_hunt = !tick
            .state
            .has_memory_value(types::HAS_HUNTING_COOLDOWN.id());
        let attackable = {
            let Some(visible) = tick.state.get(types::NEAREST_VISIBLE_LIVING_ENTITIES) else {
                return;
            };
            let ctx = tick.visibility();
            visible.find_closest(&ctx, |candidate| {
                let entity = candidate.get_entity();
                let kind = entity.entity_type;
                entity.pos.load().squared_distance_to_vec(&position)
                    <= TARGET_DETECTION_DISTANCE_SQR
                    && entity.is_in_water()
                    && (kind.has_tag(&tag::EntityType::MINECRAFT_AXOLOTL_ALWAYS_HOSTILES)
                        || (may_hunt
                            && kind.has_tag(&tag::EntityType::MINECRAFT_AXOLOTL_HUNT_TARGETS)))
                    && is_entity_attackable(&ctx, candidate.as_ref())
            })
        };
        tick.state
            .set_optional(types::NEAREST_ATTACKABLE, attackable);
    }
}
