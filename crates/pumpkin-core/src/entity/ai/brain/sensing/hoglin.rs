use std::sync::Arc;

use pumpkin_data::entity::EntityType;
use pumpkin_data::tag;
use pumpkin_data::tag::Taggable;
use pumpkin_util::math::position::{BlockPos, OutwardIterator};

use super::super::BrainTick;
use super::super::memory::{MemoryModuleId, types};
use super::Sensor;
use crate::entity::EntityBase;

const REQUIRES: [MemoryModuleId; 6] = [
    types::NEAREST_VISIBLE_LIVING_ENTITIES.id(),
    types::NEAREST_REPELLENT.id(),
    types::NEAREST_VISIBLE_ADULT_PIGLIN.id(),
    types::NEAREST_VISIBLE_ADULT_HOGLINS.id(),
    types::VISIBLE_ADULT_PIGLIN_COUNT.id(),
    types::VISIBLE_ADULT_HOGLIN_COUNT.id(),
];

/// Vanilla `HoglinAi.REPELLENT_DETECTION_RANGE_HORIZONTAL`.
const REPELLENT_RANGE_HORIZONTAL: i32 = 8;
/// Vanilla `HoglinAi.REPELLENT_DETECTION_RANGE_VERTICAL`.
const REPELLENT_RANGE_VERTICAL: i32 = 4;

/// Vanilla `HoglinSpecificSensor`.
pub struct HoglinSpecificSensor;

fn is_adult(entity: &Arc<dyn EntityBase>) -> bool {
    entity
        .get_living_entity()
        .is_none_or(|living| !living.is_baby())
}

impl Sensor for HoglinSpecificSensor {
    fn requires(&self) -> &'static [MemoryModuleId] {
        &REQUIRES
    }

    fn do_tick(&mut self, tick: &mut BrainTick<'_>) {
        let center: BlockPos = tick.mob.get_entity().block_pos.load();
        let repellent = OutwardIterator::new(
            center,
            REPELLENT_RANGE_HORIZONTAL,
            REPELLENT_RANGE_VERTICAL,
            REPELLENT_RANGE_HORIZONTAL,
        )
        .find(|pos| {
            tick.world
                .get_block(pos)
                .has_tag(&tag::Block::MINECRAFT_HOGLIN_REPELLENTS)
        });
        tick.state.set_optional(types::NEAREST_REPELLENT, repellent);

        let adults: Vec<Arc<dyn EntityBase>> = {
            let ctx = tick.visibility();
            tick.state
                .get(types::NEAREST_VISIBLE_LIVING_ENTITIES)
                .map_or_else(Vec::new, |visible| {
                    visible
                        .find_all(&ctx, |entity| {
                            let kind = entity.get_entity().entity_type;
                            is_adult(entity)
                                && (kind == &EntityType::PIGLIN || kind == &EntityType::HOGLIN)
                        })
                        .cloned()
                        .collect()
                })
        };
        let (piglins, hoglins): (Vec<_>, Vec<_>) = adults
            .into_iter()
            .partition(|entity| entity.get_entity().entity_type == &EntityType::PIGLIN);
        tick.state
            .set(types::VISIBLE_ADULT_PIGLIN_COUNT, piglins.len() as i32);
        tick.state
            .set(types::VISIBLE_ADULT_HOGLIN_COUNT, hoglins.len() as i32);
        tick.state.set_optional(
            types::NEAREST_VISIBLE_ADULT_PIGLIN,
            piglins.into_iter().next(),
        );
        tick.state
            .set(types::NEAREST_VISIBLE_ADULT_HOGLINS, hoglins);
    }
}
