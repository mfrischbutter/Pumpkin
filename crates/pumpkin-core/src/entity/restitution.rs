//! Vanilla `Entity.restituteMovementAfterCollisions`, for the `bounciness` attribute.
//!
//! Stopping at a collision and the bounce of a block under a mob without bounciness stay with
//! `Entity::move_entity` and `update_entity_movement_after_fall_on`.

use std::sync::atomic::Ordering::Relaxed;

use pumpkin_data::Block;
use pumpkin_data::attributes::Attributes;
use pumpkin_data::game_event::GameEvent;
use pumpkin_data::tag::{self, Taggable};
use pumpkin_util::math::lerp;
use pumpkin_util::math::vector3::Vector3;

use crate::entity::EntityBase;
use crate::entity::living::LivingEntity;

/// Vanilla `Mth.equal`: closer than this counts as the same.
const EQUAL_EPSILON: f32 = 1.0E-5;
/// Vanilla `Blocks.SLIME_BLOCK.bounceRestitution`; `pumpkin-data` has no such block property yet.
const SLIME_BLOCK_BOUNCE_RESTITUTION: f64 = 1.0;
/// Vanilla `bounceRestitution` of the beds and the shelf mushroom.
const BED_BOUNCE_RESTITUTION: f64 = 0.75;

fn mth_equal(a: f64, b: f64) -> bool {
    (b - a).abs() < f64::from(EQUAL_EPSILON)
}

/// Vanilla `Block.getBounceRestitution`, for a living entity.
fn block_bounciness(block: &Block) -> f64 {
    if block == &Block::SLIME_BLOCK {
        SLIME_BLOCK_BOUNCE_RESTITUTION
    } else if block == &Block::SHELF_MUSHROOM || block.has_tag(&tag::Block::MINECRAFT_BEDS) {
        BED_BOUNCE_RESTITUTION
    } else {
        0.0
    }
}

/// Bounces a living entity with `bounciness` off what it collided with. Nothing happens without
/// it, so the cost for every other mob is one attribute read on a collision.
///
/// `current` is the velocity before the move, `motion` the wanted move and `moved` the move
/// after collisions.
pub(super) fn restitute_movement_after_collisions(
    caller: &dyn EntityBase,
    current: Vector3<f64>,
    motion: Vector3<f64>,
    moved: Vector3<f64>,
    effect_block: &Block,
    speed_factor: f64,
) {
    let x_collision = !mth_equal(motion.x, moved.x);
    let z_collision = !mth_equal(motion.z, moved.z);
    let vertical_collision = motion.y - moved.y != 0.0;
    if !(x_collision || z_collision || vertical_collision) {
        return;
    }
    let Some(living) = caller.get_living_entity() else {
        return;
    };
    let entity = &living.entity;
    let suppressing_bounce = entity.is_sneaking();
    let bounciness = if suppressing_bounce {
        0.0
    } else {
        living.get_attribute_value(&Attributes::BOUNCINESS)
    };
    if bounciness <= 0.0 {
        return;
    }

    let mut after = current;
    if x_collision {
        after.x = -current.x * bounciness;
    }
    if z_collision {
        after.z = -current.z * bounciness;
    }
    let mut bounced = x_collision || z_collision;
    if vertical_collision {
        let gravity = living.get_effective_gravity(caller);
        let restitution = if motion.y >= 0.0 {
            bounciness
        } else if -current.y <= gravity
            || suppressing_bounce
            || effect_block.has_tag(&tag::Block::MINECRAFT_SUPPRESSES_BOUNCE)
        {
            0.0
        } else {
            bounciness.max(block_bounciness(effect_block))
        };
        let (gravity_compensation, effective_drag) = if restitution > 0.0 {
            bounced = true;
            let portion_with_movement = moved.y / current.y;
            (
                portion_with_movement * gravity,
                lerp(portion_with_movement, 1.0, air_drag(caller, living)),
            )
        } else {
            (0.0, 1.0)
        };
        after.y = (gravity_compensation - current.y) * effective_drag * restitution;
    }

    entity
        .velocity
        .store(after.multiply(speed_factor, 1.0, speed_factor));
    if bounced {
        entity
            .world
            .load()
            .emit_game_event(GameEvent::Bounce.name(), entity.pos.load());
        entity.velocity_dirty.store(true, Relaxed);
    }
}

/// Vanilla `LivingEntity.getAirDrag`.
fn air_drag(caller: &dyn EntityBase, living: &LivingEntity) -> f64 {
    LivingEntity::compute_modified_friction(
        if caller.is_flutterer() { 0.91 } else { 0.98 },
        living.get_attribute_value(&Attributes::AIR_DRAG_MODIFIER) as f32,
    )
}
