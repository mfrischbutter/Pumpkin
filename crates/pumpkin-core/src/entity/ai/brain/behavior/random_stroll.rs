use pumpkin_data::fluid::Fluid;
use pumpkin_util::math::vector3::Vector3;

use super::super::memory::{MemoryStatus, types, walk_target::WalkTarget};
use super::OneShot;
use super::utils::get_random_swimmable_pos;
use crate::entity::ai::util::{air_and_water_random_pos, goal_utils, land_random_pos};
use crate::entity::mob::Mob;

const MAX_XZ_DIST: i32 = 10;
const MAX_Y_DIST: i32 = 7;
const SWIM_XY_DISTANCE_TIERS: [(i32, i32); 6] = [(1, 1), (3, 3), (5, 5), (6, 5), (7, 7), (10, 7)];

/// Vanilla `RandomStroll.stroll`: a walk to a random spot on land, 10 blocks around.
#[must_use]
pub fn stroll(speed_modifier: f32) -> OneShot {
    stroll_within(speed_modifier, MAX_XZ_DIST, MAX_Y_DIST)
}

/// Vanilla `RandomStroll.stroll(speed, maxHorizontal, maxVertical)`.
#[must_use]
pub fn stroll_within(speed_modifier: f32, max_xz: i32, max_y: i32) -> OneShot {
    OneShot::new(
        "RandomStroll",
        vec![(types::WALK_TARGET.id(), MemoryStatus::ValueAbsent)],
        move |tick| {
            let target = land_random_pos::get_pos(tick.mob, max_xz, max_y);
            tick.state.set_optional(
                types::WALK_TARGET,
                target.map(|pos| WalkTarget::from_vec(pos, speed_modifier, 0)),
            );
            true
        },
    )
}

/// Vanilla `RandomStroll.swim`: a swim to a random spot in the fluid.
#[must_use]
pub fn swim(speed_modifier: f32) -> OneShot {
    OneShot::new(
        "RandomStroll",
        vec![(types::WALK_TARGET.id(), MemoryStatus::ValueAbsent)],
        move |tick| {
            if !tick.mob.get_entity().is_in_water() {
                return false;
            }
            let target = get_target_swim_pos(tick.mob);
            tick.state.set_optional(
                types::WALK_TARGET,
                target.map(|pos| WalkTarget::from_vec(pos, speed_modifier, 0)),
            );
            true
        },
    )
}

#[must_use]
pub fn fly(speed_modifier: f32) -> OneShot {
    OneShot::new(
        "RandomStroll",
        vec![(types::WALK_TARGET.id(), MemoryStatus::ValueAbsent)],
        move |tick| {
            let target = get_target_fly_pos(tick.mob, MAX_XZ_DIST, MAX_Y_DIST);
            tick.state.set_optional(
                types::WALK_TARGET,
                target.map(|pos| WalkTarget::from_vec(pos, speed_modifier, 0)),
            );
            true
        },
    )
}

/// Vanilla `RandomStroll.getTargetSwimPos`: ever farther spots along the first direction, as
/// long as they stay in the fluid.
fn get_target_swim_pos(mob: &dyn Mob) -> Option<Vector3<f64>> {
    let world = mob.get_entity().world.load();
    let position = mob.get_entity().pos.load();
    let mut fallback: Option<Vector3<f64>> = None;
    let mut target = None;
    for (horizontal, vertical) in SWIM_XY_DISTANCE_TIERS {
        target = fallback.map_or_else(
            || get_random_swimmable_pos(mob, horizontal, vertical),
            |fallback| {
                let direction = (fallback - position).normalize();
                Some(Vector3::new(
                    position.x + direction.x * f64::from(horizontal),
                    position.y + direction.y * f64::from(vertical),
                    position.z + direction.z * f64::from(horizontal),
                ))
            },
        );
        let restrict = goal_utils::mob_restricted(mob, f64::from(horizontal));
        let Some(pos) = target else {
            return fallback;
        };
        let block_pos = pos.to_block_pos();
        if world.get_fluid(&block_pos).id == Fluid::EMPTY.id
            || goal_utils::is_restricted(restrict, mob, &block_pos)
        {
            return fallback;
        }
        fallback = Some(pos);
    }
    target
}

fn get_target_fly_pos(
    mob: &dyn Mob,
    max_horizontal_distance: i32,
    max_vertical_distance: i32,
) -> Option<Vector3<f64>> {
    let entity = mob.get_entity();
    let wander_direction =
        Vector3::rotation_vector(f64::from(entity.pitch.load()), f64::from(entity.yaw.load()));
    air_and_water_random_pos::get_pos(
        mob,
        max_horizontal_distance,
        max_vertical_distance,
        -2,
        wander_direction.x,
        wander_direction.z,
        f64::from(std::f32::consts::FRAC_PI_2),
    )
}
