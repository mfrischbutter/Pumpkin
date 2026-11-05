use std::f64::consts::FRAC_PI_2;

use pumpkin_util::math::vector3::Vector3;
use pumpkin_world::poi::section_of;

use super::wander_around::WanderAroundGoal;
use crate::entity::ai::brain::behavior::village::{
    section_bottom_center, section_closest_to_village,
};
use crate::entity::ai::util::default_random_pos;
use crate::entity::mob::Mob;

const MAX_XZ_DIST: i32 = 10;
const MAX_Y_DIST: i32 = 7;
const INTERVAL: i32 = 10;
const SECTION_SEARCH_RADIUS: i32 = 2;

/// Vanilla `MoveBackToVillageGoal`: outside a village, a stroll towards the closest one.
#[must_use]
pub const fn move_back_to_village(speed: f64) -> WanderAroundGoal {
    WanderAroundGoal::with_interval(speed, INTERVAL)
        .with_start_condition(is_outside_village)
        .with_position(get_position, 0.0)
}

fn is_outside_village(mob: &dyn Mob) -> bool {
    let world = mob.get_entity().world.load();
    !world.is_village(&mob.get_entity().block_pos.load())
}

/// Vanilla `MoveBackToVillageGoal.getPosition`.
fn get_position(mob: &dyn Mob) -> Option<Vector3<f64>> {
    let world = mob.get_entity().world.load();
    let section = section_of(&mob.get_entity().block_pos.load());
    let target = section_closest_to_village(&world, section, SECTION_SEARCH_RADIUS)?;
    default_random_pos::get_pos_towards(
        mob,
        MAX_XZ_DIST,
        MAX_Y_DIST,
        section_bottom_center(target),
        FRAC_PI_2,
    )
}
