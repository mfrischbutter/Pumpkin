//! Vanilla `AbstractMinecart.getDismountLocationForPassenger` and the `DismountHelper` it uses.

use std::sync::atomic::Ordering;

use pumpkin_data::block_properties::OakTrapdoorLikeProperties;
use pumpkin_data::entity::EntityPose;
use pumpkin_data::tag::{self, Taggable};
use pumpkin_util::math::boundingbox::BoundingBox;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;

use crate::entity::EntityBase;
use crate::world::World;

use super::MinecartEntity;

/// Vanilla `POSE_DISMOUNT_HEIGHTS`.
const fn dismount_heights(pose: EntityPose) -> &'static [i32] {
    match pose {
        EntityPose::Swimming => &[0, 1],
        _ => &[0, 1, -1],
    }
}

/// Vanilla `LivingEntity.getDismountPoses`, with `Player`'s override.
fn dismount_poses(passenger: &dyn EntityBase) -> &'static [EntityPose] {
    if passenger.get_player().is_some() {
        &[
            EntityPose::Standing,
            EntityPose::Crouching,
            EntityPose::Swimming,
        ]
    } else {
        &[EntityPose::Standing]
    }
}

/// The top of vanilla `DismountHelper.nonClimbableShape`, `None` for an empty shape.
fn non_climbable_top(world: &World, pos: &BlockPos) -> Option<f64> {
    let (block, state) = world.get_block_and_state(pos);
    if block.has_tag(&tag::Block::MINECRAFT_CLIMBABLE)
        || (block.has_tag(&tag::Block::MINECRAFT_TRAPDOORS)
            && OakTrapdoorLikeProperties::from_state_id(state.id).open)
    {
        return None;
    }
    state
        .get_block_collision_shapes_at(pos)
        .map(|shape| shape.max.y)
        .reduce(f64::max)
}

/// Vanilla `CollisionGetter.getBlockFloorHeight` on the non-climbable shapes.
fn block_floor_height(world: &World, pos: &BlockPos) -> f64 {
    non_climbable_top(world, pos).unwrap_or_else(|| {
        non_climbable_top(world, &pos.down())
            .filter(|below| *below >= 1.0)
            .map_or(f64::NEG_INFINITY, |below| below - 1.0)
    })
}

/// Vanilla `DismountHelper.canDismountTo`.
fn can_dismount_to(world: &World, dismount_box: BoundingBox) -> bool {
    if !world.is_space_empty(dismount_box) {
        return false;
    }
    let border = world
        .worldborder
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    border.contains(dismount_box.min.x, dismount_box.min.z)
        && border.contains(dismount_box.max.x - 1.0e-5, dismount_box.max.z - 1.0e-5)
}

/// Vanilla `DismountHelper.offsetsForDirection`.
const fn offsets_for_direction((fx, fz): (i32, i32)) -> [(i32, i32); 8] {
    // `Direction.getClockWise`: north to east, east to south.
    let (rx, rz) = (-fz, fx);
    let (lx, lz) = (-rx, -rz);
    let (bx, bz) = (-fx, -fz);
    [
        (rx, rz),
        (lx, lz),
        (bx + rx, bz + rz),
        (bx + lx, bz + lz),
        (fx + rx, fz + rz),
        (fx + lx, fz + lz),
        (bx, bz),
        (fx, fz),
    ]
}

impl MinecartEntity {
    /// Vanilla `getMotionDirection` as a horizontal step.
    fn motion_direction(&self) -> (i32, i32) {
        // Vanilla `Direction.fromYRot`: south, west, north, east.
        const STEPS: [(i32, i32); 4] = [(0, 1), (-1, 0), (0, -1), (1, 0)];
        let yaw = self.vehicle.entity.yaw.load();
        let (x, z) = STEPS[(((yaw / 90.0) + 0.5).floor() as i32 & 3) as usize];
        if self.experimental_movement {
            return (x, z);
        }
        // Old behaviour: the yaw lies across the track.
        if self.flipped.load(Ordering::Relaxed) {
            (z, -x)
        } else {
            (-z, x)
        }
    }

    /// Vanilla `AbstractMinecart.getDismountLocationForPassenger`.
    #[must_use]
    pub fn dismount_location_for_passenger(&self, passenger: &dyn EntityBase) -> Vector3<f64> {
        let entity = &self.vehicle.entity;
        let world = entity.world.load();
        let offsets = offsets_for_direction(self.motion_direction());
        let vehicle_pos = entity.block_pos.load().0;
        let passenger_entity = passenger.get_entity();
        for &pose in dismount_poses(passenger) {
            let dimensions = passenger_entity.get_dimensions(pose);
            let reach = f64::from(dimensions.width.min(1.0)) / 2.0;
            for &offset_y in dismount_heights(pose) {
                for &(offset_x, offset_z) in &offsets {
                    let target = BlockPos::new(
                        vehicle_pos.x + offset_x,
                        vehicle_pos.y + offset_y,
                        vehicle_pos.z + offset_z,
                    );
                    let floor = block_floor_height(&world, &target);
                    if !floor.is_finite() || floor >= 1.0 {
                        continue;
                    }
                    let location = Vector3::new(
                        f64::from(target.0.x) + 0.5,
                        f64::from(target.0.y) + floor,
                        f64::from(target.0.z) + 0.5,
                    );
                    let dismount_box = BoundingBox::new(
                        location.add_raw(-reach, 0.0, -reach),
                        location.add_raw(reach, f64::from(dimensions.height), reach),
                    );
                    if can_dismount_to(&world, dismount_box) {
                        passenger_entity.set_pose(pose);
                        return location;
                    }
                }
            }
        }

        let position = entity.pos.load();
        let vehicle_top = entity.bounding_box.load().max.y;
        let target = BlockPos::floored(
            f64::from(vehicle_pos.x),
            vehicle_top,
            f64::from(vehicle_pos.z),
        );
        for &pose in dismount_poses(passenger) {
            let height = f64::from(passenger_entity.get_dimensions(pose).height);
            let block_coverage = (vehicle_top - f64::from(target.0.y) + height).ceil() as i32;
            if vehicle_top + height <= find_ceiling_from(&world, &target, block_coverage) {
                passenger_entity.set_pose(pose);
                break;
            }
        }
        Vector3::new(position.x, vehicle_top, position.z)
    }
}

/// Vanilla `DismountHelper.findCeilingFrom`.
fn find_ceiling_from(world: &World, pos: &BlockPos, blocks: i32) -> f64 {
    for y in 0..blocks {
        let cursor = pos.up_height(y);
        let bottom = world
            .get_block_state(&cursor)
            .get_block_collision_shapes_at(&cursor)
            .map(|shape| shape.min.y)
            .reduce(f64::min);
        if let Some(bottom) = bottom {
            return f64::from(pos.0.y + y) + bottom;
        }
    }
    f64::INFINITY
}
