//! Vanilla `AbstractBoat` and `Entity`: who gets on a boat, where they sit, where they get off.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use pumpkin_data::entity::{EntityPose, EntityType};
use pumpkin_data::fluid::Fluid;
use pumpkin_data::game_event::GameEvent;
use pumpkin_data::tag::{self, Taggable};
use pumpkin_util::math::boundingbox::BoundingBox;
use pumpkin_util::math::clamp_angle;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;

use super::{BoatEntity, is_raft_variant, leash};
use crate::entity::living::is_allowed_by_team_rules;
use crate::entity::{Entity, EntityBase};
use crate::world::World;

/// Vanilla `AbstractChestBoat.getSinglePassengerXOffset`.
const CHEST_BOAT_SINGLE_OFFSET: f32 = 0.15;
const FRONT_SEAT_OFFSET: f32 = 0.2;
const BACK_SEAT_OFFSET: f32 = -0.6;
const ANIMAL_SEAT_OFFSET: f32 = 0.2;

/// Vanilla `DismountHelper.isBlockFloorValid`.
fn is_block_floor_valid(floor: f64) -> bool {
    !floor.is_infinite() && floor < 1.0
}

/// Vanilla `DismountHelper.canDismountTo`.
fn can_dismount_to(
    world: &World,
    location: Vector3<f64>,
    passenger: &Entity,
    pose: EntityPose,
) -> bool {
    let dimensions = passenger.get_dimensions(pose);
    let bbox = BoundingBox::new_from_pos(location.x, location.y, location.z, &dimensions);
    if !world.is_space_empty(bbox) {
        return false;
    }
    let border = world
        .worldborder
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    border.contains(bbox.min.x, bbox.min.z) && border.contains(bbox.max.x, bbox.max.z)
}

/// Vanilla `Entity.getCollisionHorizontalEscapeVector`: to the edge of the box around the
/// collider, in the direction the passenger faces.
fn collision_horizontal_escape_vector(
    collider_width: f64,
    colliding_width: f64,
    direction_degrees: f32,
) -> Vector3<f64> {
    let distance = (collider_width + colliding_width + f64::from(1.0E-5f32)) / 2.0;
    let (sin, cos) = direction_degrees.to_radians().sin_cos();
    let direction_x = f64::from(-sin);
    let direction_z = f64::from(cos);
    let scale = direction_x.abs().max(direction_z.abs());
    Vector3::new(
        direction_x * distance / scale,
        0.0,
        direction_z * distance / scale,
    )
}

/// Vanilla `passenger instanceof Animal`.
fn is_animal(passenger: &dyn EntityBase) -> bool {
    passenger
        .get_mob()
        .and_then(crate::entity::mob::Mob::as_animal)
        .is_some()
}

/// Vanilla `canRide` of the mobs that refuse every vehicle.
fn never_rides(entity_type: &EntityType) -> bool {
    entity_type == &EntityType::WARDEN
        || entity_type == &EntityType::WITHER
        || entity_type == &EntityType::ENDER_DRAGON
}

impl BoatEntity {
    /// Vanilla `AbstractBoat.getMaxPassengers` and `AbstractChestBoat.getMaxPassengers`.
    const fn max_passengers(&self) -> usize {
        if self.chest.is_some() { 1 } else { 2 }
    }

    fn passenger_count(&self) -> usize {
        self.vehicle
            .entity
            .passengers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len()
    }

    /// Vanilla `AbstractBoat.canAddPassenger`.
    pub(super) fn can_add_passenger(&self) -> bool {
        self.passenger_count() < self.max_passengers()
            && !self.vehicle.entity.is_submerged_in_water()
    }

    /// Vanilla `Entity.startRiding` with a boat to ride, and the leash `Mob.startRiding` drops.
    pub(super) fn start_riding(&self, passenger: &Arc<dyn EntityBase>) -> bool {
        let entity = &self.vehicle.entity;
        let rider = passenger.get_entity();
        let current = rider.get_vehicle();
        if current
            .as_ref()
            .is_some_and(|vehicle| vehicle.get_entity().entity_id == entity.entity_id)
            || rider.has_indirect_passenger(entity)
            || never_rides(rider.entity_type)
            || rider.is_sneaking()
            || rider.riding_cooldown.load(Ordering::Relaxed) > 0
            || !self.can_add_passenger()
        {
            return false;
        }
        let Some(boat) = entity.world.load().get_entity_by_id(entity.entity_id) else {
            return false;
        };

        if let Some(vehicle) = current {
            vehicle.get_entity().remove_passenger(rider.entity_id);
        }
        rider.set_pose(EntityPose::Standing);
        entity.add_passenger(boat, passenger.clone());
        // A plugin can refuse the mount.
        if !entity.has_passenger(rider.entity_id) {
            return false;
        }

        self.emit_game_event(GameEvent::EntityMount, Some(passenger.as_ref()));
        if passenger.get_mob().is_some() && rider.is_leashed() {
            leash::drop_leash(rider);
        }
        true
    }

    /// Vanilla `Entity.ejectPassengers`, last seat first.
    pub(super) fn eject_passengers(&self) {
        let entity = &self.vehicle.entity;
        let riders: Vec<i32> = entity
            .passengers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .rev()
            .map(|passenger| passenger.get_entity().entity_id)
            .collect();
        for rider in riders {
            entity.remove_passenger(rider);
        }
    }

    /// The end of vanilla `AbstractBoat.tick`: what touches a boat nobody steers gets on, within
    /// the seats and the tags; everything else gets pushed.
    pub(super) fn push_and_pull_entities(&self) {
        let entity = &self.vehicle.entity;
        let area = entity.bounding_box.load().expand(0.2, -0.01, 0.2);
        let candidates = entity.world.load().get_all_at_box(&area);
        if candidates.is_empty() {
            return;
        }

        // Vanilla `EntitySelector.pushableBy`.
        let own_team = self.get_team();
        let add_new_passengers = !self.is_client_authoritative();
        for other in candidates {
            let other_entity = other.get_entity();
            if other_entity.entity_id == entity.entity_id
                || other.is_spectator()
                || !other.is_pushable()
                || !is_allowed_by_team_rules(own_team.as_ref(), other.get_team().as_ref())
                || other_entity.has_passenger(entity.entity_id)
            {
                continue;
            }
            if add_new_passengers
                && self.passenger_count() < self.max_passengers()
                && !other.is_passenger()
                && other_entity.width() < entity.width()
                && other.get_living_entity().is_some()
                && !other_entity
                    .entity_type
                    .has_tag(&tag::EntityType::MINECRAFT_CANNOT_BE_PUSHED_ONTO_BOATS)
            {
                self.start_riding(&other);
            } else {
                self.push(other.as_ref());
            }
        }
    }

    /// Vanilla `Boat.rideHeight` and `Raft.rideHeight`.
    fn ride_height(&self) -> f64 {
        let entity = &self.vehicle.entity;
        let height = entity.height();
        f64::from(if is_raft_variant(entity.entity_type.resource_name) {
            height * 0.888_888_9f32
        } else {
            height / 3.0f32
        })
    }

    /// Vanilla `AbstractBoat.getPassengerAttachmentPoint`.
    fn passenger_attachment_point(&self, passenger: &dyn EntityBase) -> Vector3<f64> {
        let entity = &self.vehicle.entity;
        let passenger_id = passenger.get_entity().entity_id;
        let (count, first_id) = {
            let passengers = entity
                .passengers
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            (
                passengers.len(),
                passengers.first().map(|p| p.get_entity().entity_id),
            )
        };

        let mut offset = if self.chest.is_some() {
            CHEST_BOAT_SINGLE_OFFSET
        } else {
            0.0
        };
        if count > 1 {
            offset = if first_id == Some(passenger_id) {
                FRONT_SEAT_OFFSET
            } else {
                BACK_SEAT_OFFSET
            };
            if is_animal(passenger) {
                offset += ANIMAL_SEAT_OFFSET;
            }
        }

        let (sin, cos) = (-entity.yaw.load()).to_radians().sin_cos();
        let offset = f64::from(offset);
        Vector3::new(
            offset * f64::from(sin),
            self.ride_height(),
            offset * f64::from(cos),
        )
    }

    /// Vanilla `AbstractBoat.positionRider`. Rotation by `deltaRotation` is not here: only a
    /// client steering the boat turns it, and that client turns the rider as well.
    pub(super) fn place_rider(&self, passenger: &dyn EntityBase) {
        let seat = self
            .vehicle
            .entity
            .pos
            .load()
            .add(&self.passenger_attachment_point(passenger));
        passenger
            .get_entity()
            .set_pos(seat.sub(&passenger.vehicle_attachment_point()));
        if !passenger
            .get_entity()
            .entity_type
            .has_tag(&tag::EntityType::MINECRAFT_CAN_TURN_IN_BOATS)
        {
            self.clamp_rotation(passenger);
        }
    }

    /// Vanilla `AbstractBoat.calculatePassengerBodyYRot`: animals in a full boat sit sideways.
    fn passenger_body_y_rot(&self, passenger: &dyn EntityBase) -> f32 {
        let entity = &self.vehicle.entity;
        let yaw = entity.yaw.load();
        if is_animal(passenger) && self.passenger_count() == self.max_passengers() {
            let turn = if passenger.get_entity().entity_id % 2 == 0 {
                90.0
            } else {
                270.0
            };
            return yaw + turn;
        }
        yaw
    }

    /// Vanilla `AbstractBoat.clampRotation`. A player's own look stays theirs: their client
    /// clamps it and reports the result.
    fn clamp_rotation(&self, passenger: &dyn EntityBase) {
        let body_yaw = self.passenger_body_y_rot(passenger);
        let rider = passenger.get_entity();
        rider.body_yaw.store(body_yaw);
        if let Some(mob) = passenger.get_mob() {
            rider.yaw.store(body_yaw);
            // Vanilla `Mob.clampHeadRotationToBody`.
            rider.head_yaw.store(clamp_angle(
                rider.head_yaw.load(),
                body_yaw,
                mob.get_max_head_rotation(),
            ));
        }
    }

    /// Vanilla `AbstractBoat.getDismountLocationForPassenger`, and the branch of
    /// `LivingEntity.dismountVehicle` for a vehicle that is going away or stands in a portal.
    pub(super) fn dismount_location(&self, passenger: &dyn EntityBase) -> Vector3<f64> {
        let entity = &self.vehicle.entity;
        let rider = passenger.get_entity();
        let world = entity.world.load();
        let pos = entity.pos.load();

        if self.being_removed.load(Ordering::Relaxed)
            || world
                .get_block(&entity.block_pos.load())
                .has_tag(&tag::Block::MINECRAFT_PORTALS)
        {
            let rider_pos = rider.pos.load();
            return Vector3::new(rider_pos.x, rider_pos.y.max(pos.y), rider_pos.z);
        }

        let top = entity.bounding_box.load().max.y;
        let direction = collision_horizontal_escape_vector(
            f64::from(entity.width() * std::f32::consts::SQRT_2),
            f64::from(rider.width()),
            rider.yaw.load(),
        );
        let (target_x, target_z) = (pos.x + direction.x, pos.z + direction.z);
        let target_pos = BlockPos::floored(target_x, top, target_z);
        let below_pos = target_pos.down();

        if !world.get_fluid(&below_pos).matches_type(&Fluid::WATER) {
            let targets: Vec<Vector3<f64>> = [&target_pos, &below_pos]
                .into_iter()
                .filter_map(|block| {
                    let floor = world.get_dismount_height(block);
                    is_block_floor_valid(floor)
                        .then(|| Vector3::new(target_x, f64::from(block.0.y) + floor, target_z))
                })
                .collect();
            for &pose in Self::dismount_poses(passenger) {
                if let Some(target) = targets
                    .iter()
                    .find(|target| can_dismount_to(&world, **target, rider, pose))
                {
                    rider.set_pose(pose);
                    return *target;
                }
            }
        }

        Vector3::new(pos.x, top, pos.z)
    }

    /// Vanilla `LivingEntity.getDismountPoses` and `Player.getDismountPoses`.
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
}
