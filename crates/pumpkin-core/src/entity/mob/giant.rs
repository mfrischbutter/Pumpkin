use crate::world::level_accessor::LevelAccessor;
use pumpkin_util::math::position::BlockPos;
use std::sync::Arc;

use crate::entity::{
    Entity,
    mob::{Mob, MobEntity},
};

pub struct GiantEntity {
    pub mob_entity: MobEntity,
}

impl GiantEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let mob_entity = MobEntity::new(entity);
        {
            let mut attributes = mob_entity
                .living_entity
                .attributes
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(health) =
                attributes.get_mut(&pumpkin_data::attributes::Attributes::MAX_HEALTH.id)
            {
                health.base_value = 100.0;
                health
                    .dirty
                    .store(true, std::sync::atomic::Ordering::Relaxed);
            }
            if let Some(speed) =
                attributes.get_mut(&pumpkin_data::attributes::Attributes::MOVEMENT_SPEED.id)
            {
                speed.base_value = 0.5;
                speed
                    .dirty
                    .store(true, std::sync::atomic::Ordering::Relaxed);
            }
            if let Some(damage) =
                attributes.get_mut(&pumpkin_data::attributes::Attributes::ATTACK_DAMAGE.id)
            {
                damage.base_value = 50.0;
                damage
                    .dirty
                    .store(true, std::sync::atomic::Ordering::Relaxed);
            }
        }
        mob_entity.living_entity.health.store(100.0);

        // Vanilla `Giant` registers no goals: it stands where it is.
        Arc::new(Self { mob_entity })
    }
}

impl Mob for GiantEntity {
    /// Vanilla `Giant.getWalkTargetValue`: the light cost itself, the brighter the better.
    fn get_walk_target_value(&self, pos: &BlockPos, level: &dyn LevelAccessor) -> f32 {
        level.get_pathfinding_cost_from_light_levels(pos)
    }

    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }
}
