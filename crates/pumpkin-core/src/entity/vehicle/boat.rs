mod chest;
mod leash;
mod physics;
mod riders;

use std::f32::consts::{PI, TAU};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};

use crossbeam::atomic::AtomicCell;

use crate::entity::player::Player;
use crate::entity::{Entity, EntityBase, LeashTick, living::LivingEntity, push_apart};
use crate::server::Server;
use crate::world::game_event::GameEventContext;

use pumpkin_data::damage::DamageType;
use pumpkin_data::game_event::GameEvent;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::Sound;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::GameMode;
use pumpkin_util::math::boundingbox::BoundingBox;
use pumpkin_util::math::vector3::Vector3;

use crate::entity::vehicle::vehicle::VehicleEntity;
use chest::ChestBoat;
use physics::BoatStatus;

/// Vanilla `AbstractBoat.TIME_TO_EJECT`.
const TIME_TO_EJECT: f32 = 60.0;
/// Vanilla `AbstractBoat.PADDLE_SPEED`.
const PADDLE_SPEED: f32 = PI / 8.0;
/// Vanilla `AbstractBoat.PADDLE_SOUND_TIME`.
const PADDLE_SOUND_TIME: f32 = PI / 4.0;
/// Vanilla `AbstractBoat.PADDLE_LEFT`.
const PADDLE_LEFT: usize = 0;
/// Vanilla `AbstractBoat.PADDLE_RIGHT`.
const PADDLE_RIGHT: usize = 1;
/// Vanilla `AbstractBoat.getDefaultGravity`.
const DEFAULT_GRAVITY: f64 = 0.04;

/// Every wood's boat and chest boat, the bamboo raft and the chest raft: vanilla `AbstractBoat`.
pub struct BoatEntity {
    pub vehicle: VehicleEntity,
    /// Vanilla `AbstractChestBoat`, for the chest variants.
    chest: Option<ChestBoat>,
    status: AtomicCell<Option<BoatStatus>>,
    old_status: AtomicCell<Option<BoatStatus>>,
    out_of_control_ticks: AtomicCell<f32>,
    water_level: AtomicCell<f64>,
    land_friction: AtomicCell<f32>,
    last_yd: AtomicCell<f64>,
    fall_distance: AtomicCell<f64>,
    left_paddle_moving: AtomicBool,
    right_paddle_moving: AtomicBool,
    paddle_positions: [AtomicCell<f32>; 2],
    is_above_bubble_column: AtomicBool,
    bubble_column_direction_is_down: AtomicBool,
    bubble_time: AtomicI32,
    /// Vanilla `isRemoved` as `LivingEntity.dismountVehicle` reads it while the boat goes away.
    being_removed: AtomicBool,
}

/// The entity type names a chest boat or chest raft by, one name per wood in `pumpkin-data`.
fn is_chest_variant(resource_name: &str) -> bool {
    resource_name.contains("_chest_")
}

/// Vanilla `Raft.rideHeight` as against `Boat.rideHeight`.
fn is_raft_variant(resource_name: &str) -> bool {
    resource_name.ends_with("_raft")
}

impl BoatEntity {
    #[must_use]
    pub fn new(mut entity: Entity) -> Self {
        entity.blocks_building = true;
        let chest = is_chest_variant(entity.entity_type.resource_name).then(ChestBoat::new);
        Self {
            vehicle: VehicleEntity::new(entity),
            chest,
            status: AtomicCell::new(None),
            old_status: AtomicCell::new(None),
            out_of_control_ticks: AtomicCell::new(0.0),
            water_level: AtomicCell::new(0.0),
            land_friction: AtomicCell::new(0.0),
            last_yd: AtomicCell::new(0.0),
            fall_distance: AtomicCell::new(0.0),
            left_paddle_moving: AtomicBool::new(false),
            right_paddle_moving: AtomicBool::new(false),
            paddle_positions: [AtomicCell::new(0.0), AtomicCell::new(0.0)],
            is_above_bubble_column: AtomicBool::new(false),
            bubble_column_direction_is_down: AtomicBool::new(false),
            bubble_time: AtomicI32::new(0),
            being_removed: AtomicBool::new(false),
        }
    }

    /// Vanilla `AbstractBoat.setPaddleState`.
    pub fn set_paddles(&self, left: bool, right: bool) {
        let entity = &self.vehicle.entity;
        if self.left_paddle_moving.swap(left, Ordering::Relaxed) != left {
            entity.set_synced_data(pumpkin_data::tracked_data::boat::ID_PADDLE_LEFT, left);
        }
        if self.right_paddle_moving.swap(right, Ordering::Relaxed) != right {
            entity.set_synced_data(pumpkin_data::tracked_data::boat::ID_PADDLE_RIGHT, right);
        }
    }

    /// Vanilla `AbstractBoat.getPaddleState`.
    fn paddle_state(&self, side: usize) -> bool {
        let paddle = if side == PADDLE_LEFT {
            &self.left_paddle_moving
        } else {
            &self.right_paddle_moving
        };
        paddle.load(Ordering::Relaxed) && self.get_controlling_passenger().is_some()
    }

    /// Vanilla `isLocalInstanceAuthoritative` on the server, negated: a player in the first seat
    /// moves the boat from their client. The server moves it otherwise.
    fn is_client_authoritative(&self) -> bool {
        self.vehicle
            .entity
            .passengers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .first()
            .is_some_and(|passenger| passenger.get_player().is_some())
    }

    /// The first half of vanilla `AbstractBoat.tick`: being under water too long ejects everyone.
    fn tick_out_of_control(&self) {
        if self.is_under_water() {
            self.out_of_control_ticks
                .store(self.out_of_control_ticks.load() + 1.0);
        } else {
            self.out_of_control_ticks.store(0.0);
        }

        if self.out_of_control_ticks.load() >= TIME_TO_EJECT {
            self.eject_passengers();
        }
    }

    /// The part of vanilla `AbstractBoat.tick` that only the side with authority runs.
    fn tick_motion(&self, caller: &dyn EntityBase) {
        let entity = &self.vehicle.entity;
        if self.is_client_authoritative() {
            entity.velocity.store(Vector3::new(0.0, 0.0, 0.0));
            return;
        }

        self.set_paddles(false, false);
        self.float_boat();

        let velocity = entity.velocity.load();
        self.last_yd.store(velocity.y);
        let y_before = entity.pos.load().y;
        let motion = self.collide_with_entities(velocity);
        entity.move_entity(caller, motion);
        self.check_fall_damage(entity.pos.load().y - y_before);
    }

    /// Vanilla `AbstractBoat.getPaddleSound`.
    fn paddle_sound(&self) -> Option<Sound> {
        match self.compute_status() {
            BoatStatus::InWater | BoatStatus::UnderWater | BoatStatus::UnderFlowingWater => {
                Some(Sound::EntityBoatPaddleWater)
            }
            BoatStatus::OnLand => Some(Sound::EntityBoatPaddleLand),
            BoatStatus::InAir => None,
        }
    }

    /// The paddle loop of vanilla `AbstractBoat.tick`: each stroke sounds on its side of the boat.
    fn tick_paddles(&self) {
        let entity = &self.vehicle.entity;
        for side in [PADDLE_LEFT, PADDLE_RIGHT] {
            if !self.paddle_state(side) {
                self.paddle_positions[side].store(0.0);
                continue;
            }
            let position = self.paddle_positions[side].load();
            if !entity.is_silent()
                && position % TAU <= PADDLE_SOUND_TIME
                && (position + PADDLE_SPEED) % TAU >= PADDLE_SOUND_TIME
                && let Some(sound) = self.paddle_sound()
            {
                let view = entity.rotation();
                let (dx, dz) = if side == PADDLE_RIGHT {
                    (-view.z, view.x)
                } else {
                    (view.z, -view.x)
                };
                let pos = entity.pos.load();
                entity.world.load().play_sound_fine(
                    sound,
                    self.sound_source(),
                    &Vector3::new(pos.x + f64::from(dx), pos.y, pos.z + f64::from(dz)),
                    1.0,
                    rand::random::<f32>().mul_add(0.4, 0.8),
                );
            }
            self.paddle_positions[side].store(position + PADDLE_SPEED);
        }
    }

    /// Vanilla `Entity.kill` as the boats' `remove` overrides make it: contents and leash drop,
    /// riders get off.
    fn kill_boat(&self) {
        self.remove_destroying();
        self.emit_game_event(GameEvent::EntityDie, Some(self));
    }

    /// Vanilla `AbstractChestBoat.remove`, `AbstractBoat.remove` and `Entity.setRemoved`, for the
    /// removal reasons that `shouldDestroy`.
    fn remove_destroying(&self) {
        let entity = &self.vehicle.entity;
        if let Some(chest) = &self.chest {
            chest.drop_contents(entity);
        }
        if entity.is_leashed() {
            leash::drop_leash(entity);
        }
        self.being_removed.store(true, Ordering::Relaxed);
        if let Some(vehicle) = entity.get_vehicle() {
            vehicle.get_entity().remove_passenger(entity.entity_id);
        }
        self.eject_passengers();
        entity.remove();
    }

    /// Vanilla `VehicleEntity.destroy` and `AbstractChestBoat.destroy`: the boat item carries the
    /// custom name.
    fn destroy(&self, direct: Option<&dyn EntityBase>) {
        let entity = &self.vehicle.entity;
        self.kill_boat();

        let world = entity.world.load();
        if !world.level_info.load().game_rules.entity_drops {
            return;
        }
        if let Some(item) = Item::from_registry_key(entity.entity_type.resource_name) {
            let mut stack = ItemStack::new(1, item);
            if let Some(name) = entity.custom_name.load().as_ref() {
                stack.set_data_component(pumpkin_data::data_component_impl::CustomNameImpl {
                    name: name.clone(),
                });
            }
            entity.spawn_at_location(stack);
        }
        if self.chest.is_some() {
            ChestBoat::anger_piglins(entity, direct);
        }
    }

    /// Vanilla `Entity.gameEvent(event, source)`.
    fn emit_game_event(&self, event: GameEvent, source: Option<&dyn EntityBase>) {
        let entity = &self.vehicle.entity;
        entity.world.load().emit_game_event_with_context(
            event.name(),
            entity.pos.load(),
            &GameEventContext {
                source_entity: source,
            },
        );
    }
}

impl EntityBase for BoatEntity {
    fn write_custom_nbt(&self, nbt: &mut NbtCompound) {
        let entity = &self.vehicle.entity;
        entity.write_leash_data(nbt);
        nbt.put_double("fall_distance", self.fall_distance.load());
        if let Some(chest) = &self.chest {
            chest.write_nbt(nbt);
        }
    }

    fn read_custom_nbt(&self, nbt: &NbtCompound) {
        let entity = &self.vehicle.entity;
        entity.read_leash_data(nbt);
        self.fall_distance
            .store(nbt.get_numeric_double("fall_distance").unwrap_or(0.0));
        if let Some(chest) = &self.chest {
            chest.read_nbt(nbt);
        }
    }

    /// Vanilla `AbstractBoat.getControllingPassenger`: only a player steers.
    fn get_controlling_passenger(&self) -> Option<Arc<dyn EntityBase>> {
        self.get_entity()
            .passengers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .first()
            .filter(|passenger| passenger.get_player().is_some())
            .cloned()
    }

    fn get_entity(&self) -> &Entity {
        &self.vehicle.entity
    }

    fn get_living_entity(&self) -> Option<&LivingEntity> {
        None
    }

    fn get_gravity(&self) -> f64 {
        if self.vehicle.entity.has_no_gravity() {
            0.0
        } else {
            DEFAULT_GRAVITY
        }
    }

    /// Vanilla `AbstractBoat.tick`.
    fn tick(&self, caller: &dyn EntityBase, server: &Server) {
        let entity = &self.vehicle.entity;
        self.update_status();
        self.tick_out_of_control();
        // Hurt time and damage count down before the base tick, as in vanilla.
        self.vehicle.tick();

        entity.tick(caller, server);
        if entity.is_removed() {
            return;
        }
        if matches!(entity.tick_leash(caller), LeashTick::Elastic(_)) {
            self.check_fall_distance_accumulation();
        }

        self.tick_motion(caller);

        // Vanilla calls `applyEffectsFromBlocks` twice in a row, and the second call runs the
        // blocks' effects over again: a cactus hurts a boat twice a tick.
        entity.tick_block_collisions(caller);
        entity.tick_block_collisions(caller);
        if entity.is_removed() {
            return;
        }

        self.tick_bubble_column();
        self.tick_paddles();
        self.push_and_pull_entities();
    }

    fn init_data_tracker(&self) {}

    fn can_hit(&self) -> bool {
        self.vehicle.entity.is_alive()
    }

    fn is_collidable(&self, _entity: Option<Box<dyn EntityBase>>) -> bool {
        true
    }

    fn damage_with_context(
        &self,
        _caller: &dyn EntityBase,
        amount: f32,
        damage_type: DamageType,
        _position: Option<Vector3<f64>>,
        source: Option<&dyn EntityBase>,
        cause: Option<&dyn EntityBase>,
    ) -> bool {
        let entity = &self.vehicle.entity;
        // Vanilla's `source.getEntity()` is the shooter of an arrow, not the arrow.
        let attacker = cause.or(source);
        if entity.is_alive() && entity.is_invulnerable_to(&damage_type, attacker) {
            return false;
        }
        self.vehicle.hurt_server(
            amount,
            attacker,
            || self.remove_destroying(),
            || self.destroy(source),
        )
    }

    /// Vanilla `Entity.interact`, `AbstractBoat.interact` and `AbstractChestBoat.interact`.
    fn interact(&self, player: &Arc<Player>, item_stack: &mut ItemStack) -> bool {
        if self.interact_with_leash(player, item_stack) {
            return true;
        }

        let sneaking = player.get_entity().is_sneaking();
        if !sneaking
            && self.out_of_control_ticks.load() < TIME_TO_EJECT
            && self.start_riding(player.clone() as Arc<dyn EntityBase>)
        {
            return true;
        }

        let Some(chest) = &self.chest else {
            return false;
        };
        if self.can_add_passenger() && !sneaking {
            return false;
        }
        chest.open(&self.vehicle.entity, player, self.get_name())
    }

    fn set_paddle_state(&self, left: bool, right: bool) {
        self.set_paddles(left, right);
    }

    fn cast_any(&self) -> &dyn std::any::Any {
        self
    }

    fn is_pushable(&self) -> bool {
        true
    }

    /// Vanilla `AbstractBoat.push`: boats meet at any height below this one's top, anything else
    /// only from level or below.
    fn push(&self, other: &dyn EntityBase) {
        let entity = &self.vehicle.entity;
        let other_entity = other.get_entity();
        let own_box = entity.bounding_box.load();
        let other_box = other_entity.bounding_box.load();
        let reaches = if other.cast_any().is::<Self>() {
            other_box.min.y < own_box.max.y
        } else {
            other_box.min.y <= own_box.min.y
        };
        if reaches && !entity.is_passenger_of_same_vehicle(other_entity) {
            push_apart(self, other);
        }
    }

    fn position_rider(&self, passenger: &dyn EntityBase) {
        self.place_rider(passenger);
    }

    fn dismount_location_for_passenger(&self, passenger: &dyn EntityBase) -> Option<Vector3<f64>> {
        Some(self.dismount_location(passenger))
    }

    /// Vanilla `AbstractBoat.modifyPassengerFluidInteractionBox`: a boat afloat keeps its riders
    /// dry up to its rim.
    fn modify_passenger_fluid_interaction_box(
        &self,
        passenger_box: BoundingBox,
    ) -> Option<BoundingBox> {
        if self.is_under_water() {
            return Some(passenger_box);
        }
        let boat_top = self.vehicle.entity.bounding_box.load().max.y;
        if boat_top >= passenger_box.max.y {
            return None;
        }
        let mut wet_box = passenger_box;
        wet_box.min.y = passenger_box.min.y.max(boat_top);
        Some(wet_box)
    }

    fn on_above_bubble_column(&self, drag_down: bool) -> bool {
        self.note_above_bubble_column(drag_down);
        true
    }

    fn support_quad_leash(&self) -> bool {
        true
    }

    fn kill(&self, _caller: &dyn EntityBase) {
        self.kill_boat();
    }

    /// Vanilla `Entity.onBelowWorld`, a discard: the boat's `remove` overrides still run.
    fn tick_in_void(&self, _dyn_self: &dyn EntityBase) {
        self.remove_destroying();
    }
}

/// Whether `player` is in creative, vanilla `Player.hasInfiniteMaterials`.
fn has_infinite_materials(player: &Player) -> bool {
    player.gamemode.load() == GameMode::Creative
}

#[cfg(test)]
mod tests {
    use super::{is_chest_variant, is_raft_variant};
    use pumpkin_data::entity::EntityType;
    use pumpkin_data::item::Item;

    /// A boat that finds no item under its name drops nothing when it breaks.
    #[test]
    fn every_boat_type_names_its_item_and_variant() {
        for (entity_type, chest, raft) in [
            (&EntityType::OAK_BOAT, false, false),
            (&EntityType::OAK_CHEST_BOAT, true, false),
            (&EntityType::SPRUCE_BOAT, false, false),
            (&EntityType::SPRUCE_CHEST_BOAT, true, false),
            (&EntityType::BIRCH_BOAT, false, false),
            (&EntityType::BIRCH_CHEST_BOAT, true, false),
            (&EntityType::JUNGLE_BOAT, false, false),
            (&EntityType::JUNGLE_CHEST_BOAT, true, false),
            (&EntityType::ACACIA_BOAT, false, false),
            (&EntityType::ACACIA_CHEST_BOAT, true, false),
            (&EntityType::DARK_OAK_BOAT, false, false),
            (&EntityType::DARK_OAK_CHEST_BOAT, true, false),
            (&EntityType::MANGROVE_BOAT, false, false),
            (&EntityType::MANGROVE_CHEST_BOAT, true, false),
            (&EntityType::CHERRY_BOAT, false, false),
            (&EntityType::CHERRY_CHEST_BOAT, true, false),
            (&EntityType::PALE_OAK_BOAT, false, false),
            (&EntityType::PALE_OAK_CHEST_BOAT, true, false),
            (&EntityType::POPLAR_BOAT, false, false),
            (&EntityType::POPLAR_CHEST_BOAT, true, false),
            (&EntityType::BAMBOO_RAFT, false, true),
            (&EntityType::BAMBOO_CHEST_RAFT, true, true),
        ] {
            let name = entity_type.resource_name;
            assert!(Item::from_registry_key(name).is_some(), "{name}");
            assert_eq!(is_chest_variant(name), chest, "{name}");
            assert_eq!(is_raft_variant(name), raft, "{name}");
        }
    }
}
