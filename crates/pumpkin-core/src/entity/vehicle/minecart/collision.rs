//! Vanilla `AbstractMinecart.push` and the behaviours' `pushAndPickupEntities`.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use pumpkin_data::entity::{EntityPose, EntityType};
use pumpkin_data::game_event::GameEvent;
use pumpkin_util::math::boundingbox::BoundingBox;
use pumpkin_util::math::vector3::Vector3;

use crate::entity::living::is_allowed_by_team_rules;
use crate::entity::{Entity, EntityBase};

use super::MinecartEntity;
use super::movement::add_impulse;

const PUSH_MIN_DISTANCE_SQUARED: f32 = 1.0e-4;
/// Vanilla `xa *= 0.1F; xa *= 0.5`.
const PUSH_STRENGTH_A: f32 = 0.1;
const PUSH_STRENGTH_B: f64 = 0.5;
/// Below this facing, a cart ignores another cart in the old behaviour.
const MIN_FACING_DOT: f32 = 0.8;
/// Vanilla `pushAndPickupEntities`: a rideable cart picks up and pushes when this fast.
const OLD_PICKUP_MIN_SPEED_SQUARED: f64 = 0.01;
const OLD_HITBOX_INFLATE: f32 = 0.2;
const NEW_HITBOX_INFLATE: f64 = 0.2;
const NEW_PUSH_INFLATE: f64 = 1.0e-7;

/// Vanilla `EntitySelector.pushableBy`.
fn pushable_by(cart: &dyn EntityBase, other: &dyn EntityBase) -> bool {
    !other.is_spectator()
        && other.is_pushable()
        && is_allowed_by_team_rules(cart.get_team().as_ref(), other.get_team().as_ref())
}

/// Whether a rideable cart takes this entity on instead of pushing it.
fn can_board(other: &dyn EntityBase) -> bool {
    other.get_player().is_none()
        && other.get_entity().entity_type != &EntityType::IRON_GOLEM
        && !is_minecart(other)
        && !other.is_passenger()
}

fn is_minecart(entity: &dyn EntityBase) -> bool {
    entity.cast_any().is::<MinecartEntity>()
}

impl MinecartEntity {
    /// Vanilla `AbstractMinecart.push(Entity)`.
    pub(super) fn push_entity(&self, other: &dyn EntityBase) {
        let own = &self.vehicle.entity;
        let other_entity = other.get_entity();
        if other_entity.no_physics.load(Ordering::Relaxed)
            || own.no_physics.load(Ordering::Relaxed)
            || own.has_passenger(other_entity.entity_id)
        {
            return;
        }
        let own_pos = own.pos.load();
        let other_pos = other_entity.pos.load();
        let mut xa = other_pos.x - own_pos.x;
        let mut za = other_pos.z - own_pos.z;
        let distance_squared = xa * xa + za * za;
        if distance_squared < f64::from(PUSH_MIN_DISTANCE_SQUARED) {
            return;
        }
        let distance = distance_squared.sqrt();
        let pow = (1.0 / distance).min(1.0);
        let strength = pow * f64::from(PUSH_STRENGTH_A) * PUSH_STRENGTH_B / distance;
        xa *= strength;
        za *= strength;
        if let Some(other_minecart) = other.cast_any().downcast_ref::<Self>() {
            self.push_other_minecart(other_minecart, xa, za);
        } else {
            add_impulse(own, -xa, -za);
            add_impulse(other_entity, xa / 4.0, za / 4.0);
        }
    }

    /// Vanilla `AbstractMinecart.pushOtherMinecart`.
    fn push_other_minecart(&self, other: &Self, xa: f64, za: f64) {
        let own = &self.vehicle.entity;
        let other_entity = &other.vehicle.entity;
        if !self.experimental_movement && !self.is_facing(other_entity) {
            return;
        }
        let movement = own.velocity.load();
        let other_movement = other_entity.velocity.load();
        if other.is_furnace() && !self.is_furnace() {
            own.velocity.store(movement.multiply(0.2, 1.0, 0.2));
            add_impulse(own, other_movement.x - xa, other_movement.z - za);
            other_entity
                .velocity
                .store(other_movement.multiply(0.95, 1.0, 0.95));
        } else if !other.is_furnace() && self.is_furnace() {
            other_entity
                .velocity
                .store(other_movement.multiply(0.2, 1.0, 0.2));
            add_impulse(other_entity, movement.x + xa, movement.z + za);
            own.velocity.store(movement.multiply(0.95, 1.0, 0.95));
        } else {
            let xdd = f64::midpoint(other_movement.x, movement.x);
            let zdd = f64::midpoint(other_movement.z, movement.z);
            own.velocity.store(movement.multiply(0.2, 1.0, 0.2));
            add_impulse(own, xdd - xa, zdd - za);
            other_entity
                .velocity
                .store(other_movement.multiply(0.2, 1.0, 0.2));
            add_impulse(other_entity, xdd + xa, zdd + za);
        }
    }

    /// The old behaviour's facing test in `pushOtherMinecart`, on the line between the carts.
    fn is_facing(&self, other: &Entity) -> bool {
        let own = &self.vehicle.entity;
        let own_pos = own.pos.load();
        let other_pos = other.pos.load();
        let direction =
            Vector3::new(other_pos.x - own_pos.x, 0.0, other_pos.z - own_pos.z).normalize();
        let yaw = f64::from(own.yaw.load()).to_radians();
        let facing = Vector3::new(yaw.cos(), 0.0, yaw.sin()).normalize();
        direction.dot(&facing).abs() >= f64::from(MIN_FACING_DOT)
    }

    /// Vanilla `OldMinecartBehavior.pushAndPickupEntities`.
    pub(super) fn old_push_and_pickup_entities(&self, caller: &dyn EntityBase) {
        let own = &self.vehicle.entity;
        let inflate = f64::from(OLD_HITBOX_INFLATE);
        let hitbox = own.bounding_box.load().expand(inflate, 0.0, inflate);
        if self.is_rideable()
            && own.velocity.load().horizontal_length_squared() >= OLD_PICKUP_MIN_SPEED_SQUARED
        {
            for other in self.others_in(&hitbox, true) {
                if !pushable_by(caller, other.as_ref()) {
                    continue;
                }
                if can_board(other.as_ref()) && !own.has_passengers() {
                    self.start_riding(&other);
                } else {
                    other.push(caller);
                }
            }
        } else {
            for other in self.others_in(&hitbox, false) {
                if !own.has_passenger(other.get_entity().entity_id) && is_minecart(other.as_ref()) {
                    other.push(caller);
                }
            }
        }
    }

    /// Vanilla `NewMinecartBehavior.pushAndPickupEntities`: whether the cart should finish its
    /// move, which it does when it only picked something up.
    pub(super) fn new_push_and_pickup_entities(&self, caller: &dyn EntityBase) -> bool {
        let own = &self.vehicle.entity;
        let hitbox = own
            .bounding_box
            .load()
            .expand(NEW_HITBOX_INFLATE, 0.0, NEW_HITBOX_INFLATE);
        let picked_up = self.new_pickup_entities(caller, &hitbox);
        if !own.horizontal_collision.load(Ordering::SeqCst)
            && !own.vertical_collision.load(Ordering::SeqCst)
        {
            return false;
        }
        let pushed = self.new_push_entities(
            caller,
            &own.bounding_box.load().expand_all(NEW_PUSH_INFLATE),
        );
        picked_up && !pushed
    }

    /// Vanilla `NewMinecartBehavior.pickupEntities`.
    fn new_pickup_entities(&self, caller: &dyn EntityBase, hitbox: &BoundingBox) -> bool {
        if !self.is_rideable() || self.vehicle.entity.has_passengers() {
            return false;
        }
        for other in self.others_in(hitbox, true) {
            if pushable_by(caller, other.as_ref())
                && can_board(other.as_ref())
                && !self.vehicle.entity.has_passengers()
                && self.start_riding(&other)
            {
                return true;
            }
        }
        false
    }

    /// Vanilla `NewMinecartBehavior.pushEntities`.
    fn new_push_entities(&self, caller: &dyn EntityBase, hitbox: &BoundingBox) -> bool {
        let own = &self.vehicle.entity;
        let mut pushed = false;
        if self.is_rideable() {
            for other in self.others_in(hitbox, true) {
                if pushable_by(caller, other.as_ref())
                    && (!can_board(other.as_ref()) || own.has_passengers())
                {
                    other.push(caller);
                    pushed = true;
                }
            }
        } else {
            for other in self.others_in(hitbox, false) {
                if !own.has_passenger(other.get_entity().entity_id) && is_minecart(other.as_ref()) {
                    other.push(caller);
                    pushed = true;
                }
            }
        }
        pushed
    }

    fn others_in(&self, hitbox: &BoundingBox, with_players: bool) -> Vec<Arc<dyn EntityBase>> {
        let own = &self.vehicle.entity;
        let world = own.world.load();
        let mut others = if with_players {
            world.get_all_at_box(hitbox)
        } else {
            world.get_entities_at_box(hitbox)
        };
        others.retain(|other| other.get_entity().entity_id != own.entity_id);
        others
    }

    /// Vanilla `Entity.startRiding`, for `passenger` boarding this cart. Whether it boarded;
    /// a plugin can cancel it.
    pub(super) fn start_riding(&self, passenger: &Arc<dyn EntityBase>) -> bool {
        let own = &self.vehicle.entity;
        let passenger_entity = passenger.get_entity();
        // Vanilla `Entity.canRide` and `canAddPassenger`.
        if passenger_entity.is_sneaking()
            || passenger_entity.riding_cooldown.load(Ordering::Relaxed) > 0
            || own.has_passengers()
        {
            return false;
        }
        let Some(vehicle) = own.world.load().get_entity_by_id(own.entity_id) else {
            return false;
        };
        if let Some(old_vehicle) = passenger_entity.get_vehicle() {
            old_vehicle
                .get_entity()
                .remove_passenger_before_teleport(passenger_entity.entity_id);
        }
        passenger_entity.set_pose(EntityPose::Standing);
        own.add_passenger(vehicle, passenger.clone());
        if !own.has_passenger(passenger_entity.entity_id) {
            return false;
        }
        passenger_entity.emit_own_game_event(passenger.as_ref(), GameEvent::EntityMount);
        true
    }
}
