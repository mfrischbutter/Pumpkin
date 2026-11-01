use pumpkin_data::Block;
use pumpkin_data::fluid::Fluid;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use std::sync::atomic::Ordering::Relaxed;

use super::{Controls, Goal};
use crate::entity::EntityBase;
use crate::entity::ai::pathfinder::{NavigatorGoal, node::PathComputationType};
use crate::entity::mob::Mob;

/// Below this much air the mob goes up for breath.
const LOW_AIR: i32 = 140;

/// Vanilla `BreathAirGoal`: with little air left, a mob that cannot stay under water swims
/// straight up to the nearest air above it, up to 8 blocks.
pub struct BreathAirGoal;

impl BreathAirGoal {
    /// Vanilla `givesAir`: no fluid, or a bubble column, that a land path can cross.
    fn gives_air(mob: &dyn Mob, pos: &BlockPos) -> bool {
        let world = mob.get_entity().world.load();
        let (block, state) = world.get_block_and_state(pos);
        (world.get_fluid(pos).id == Fluid::EMPTY.id || block == &Block::BUBBLE_COLUMN)
            && world
                .block_registry
                .is_pathfindable(block, state, PathComputationType::Land)
    }

    /// Vanilla `findAirPosition`: the first air in the column above, else 8 blocks up.
    fn find_air_position(mob: &dyn Mob) {
        let entity = mob.get_entity();
        let base = entity.block_pos.load();
        let destination = (0..=8)
            .map(|dy| base.up_height(dy))
            .find(|pos| Self::gives_air(mob, pos))
            .unwrap_or_else(|| {
                let pos = entity.pos.load();
                BlockPos::floored(pos.x, pos.y + 8.0, pos.z)
            });
        mob.get_mob_entity()
            .steered()
            .navigator()
            .set_progress(NavigatorGoal::new(
                entity.pos.load(),
                Vector3::new(
                    f64::from(destination.0.x),
                    f64::from(destination.0.y + 1),
                    f64::from(destination.0.z),
                ),
                1.0,
            ));
    }
}

impl Goal for BreathAirGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        mob.get_entity().air_supply.load(Relaxed) < LOW_AIR
    }

    fn start(&mut self, mob: &dyn Mob) {
        mob.get_mob_entity().steered().navigator().stop();
        Self::find_air_position(mob);
    }

    fn tick(&mut self, mob: &dyn Mob) {
        Self::find_air_position(mob);
        let living = &mob.get_mob_entity().living_entity;
        living
            .entity
            .update_velocity_from_input(living.movement_input.load(), 0.02);
        let caller: &dyn EntityBase = mob;
        living.make_move(caller);
    }

    fn can_stop(&self) -> bool {
        false
    }

    fn controls(&self) -> Controls {
        Controls::MOVE | Controls::LOOK
    }
}
