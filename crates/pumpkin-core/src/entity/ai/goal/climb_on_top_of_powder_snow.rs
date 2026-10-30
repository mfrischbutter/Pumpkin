use std::sync::atomic::Ordering;

use pumpkin_data::Block;
use pumpkin_data::tag::{self, Taggable};

use super::{Controls, Goal, GoalReach};
use crate::entity::mob::Mob;

/// Vanilla `ClimbOnTopOfPowderSnowGoal`: a mob that can walk on powder snow jumps out of it.
pub struct ClimbOnTopOfPowderSnowGoal;

impl Goal for ClimbOnTopOfPowderSnowGoal {
    fn tick_reach(&self, _mob: &dyn Mob, _running: bool) -> Option<GoalReach> {
        Some(GoalReach::default())
    }

    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        let entity = mob.get_entity();
        let in_powder_snow = entity.was_in_powder_snow.load(Ordering::Relaxed)
            || entity.is_in_powder_snow.load(Ordering::Relaxed);
        if !in_powder_snow
            || !entity
                .entity_type
                .has_tag(&tag::EntityType::MINECRAFT_POWDER_SNOW_WALKABLE_MOBS)
        {
            return false;
        }
        let above = entity.block_pos.load().up();
        let (block, state) = entity.world.load().get_block_and_state(&above);
        block == &Block::POWDER_SNOW || state.get_block_collision_shapes().next().is_none()
    }

    fn should_run_every_tick(&self) -> bool {
        true
    }

    fn tick(&mut self, mob: &dyn Mob) {
        mob.get_mob_entity()
            .jump_control
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .jump();
    }

    fn controls(&self) -> Controls {
        Controls::JUMP
    }
}
