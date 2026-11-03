//! Vanilla `AbstractMinecart` and the seven minecart types.
//!
//! Vanilla's class hierarchy is the [`MinecartKind`] enum here. Which of vanilla's two
//! `MinecartBehavior`s a cart uses is decided once, when it is created, by the
//! `minecart_improvements` feature flag.

mod chest;
mod collision;
mod command_block;
pub(super) mod container;
mod damage;
mod dismount;
mod furnace;
mod hopper;
mod movement;
mod new_behavior;
mod old_behavior;
mod rails;
mod rideable;
mod spawner;
mod tnt;

use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::{Arc, Mutex};

use crossbeam::atomic::AtomicCell;
use pumpkin_data::BlockStateId;
use pumpkin_data::damage::DamageType;
use pumpkin_data::data_component_impl::CustomNameImpl;
use pumpkin_data::entity::{EntityType, MovementEmission};
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::Sound;
use pumpkin_data::tag::{self, Taggable};
use pumpkin_data::tracked_data::abstract_minecart::{ID_CUSTOM_DISPLAY_BLOCK, ID_DISPLAY_OFFSET};
use pumpkin_inventory::Inventory;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_protocol::java::client::play::MinecartStep;
use pumpkin_util::math::vector3::Vector3;
use rand::RngExt;

use crate::entity::mob::enderman::{block_state_from_nbt, block_state_to_nbt};
use crate::entity::vehicle::vehicle::VehicleEntity;
use crate::entity::{Entity, EntityBase, living::LivingEntity, player::Player};
use crate::server::Server;
use crate::world::loot::LootContextParameters;
use chest::ChestMinecart;
use command_block::CommandBlockMinecart;
use container::MinecartInventory;
use furnace::FurnaceMinecart;
use hopper::HopperMinecart;
use rideable::RideableMinecart;
use spawner::SpawnerMinecart;
use tnt::TntMinecart;

pub(crate) use container::redstone_signal_from_container;

/// Vanilla `getDefaultGravity` in and out of water.
const GRAVITY: f64 = 0.04;
const GRAVITY_IN_WATER: f64 = 0.005;
/// Vanilla `WATER_SLOWDOWN_FACTOR`.
const WATER_SLOWDOWN_FACTOR: f32 = 0.95;
/// The minecart types' `passengerAttachments` in `EntityTypes`, not in the extracted entity data.
const PASSENGER_ATTACHMENT_Y: f64 = 0.1875;
/// Vanilla `getDefaultDisplayOffset`, with the chest and hopper overrides.
const DEFAULT_DISPLAY_OFFSET: i32 = 6;
const CHEST_DISPLAY_OFFSET: i32 = 8;
const HOPPER_DISPLAY_OFFSET: i32 = 1;
/// Vanilla `Entity.lavaHurt` and `lavaIgnite`.
const LAVA_DAMAGE: f32 = 4.0;
const LAVA_FIRE_SECONDS: f32 = 15.0;

pub struct MinecartEntity {
    pub vehicle: VehicleEntity,
    kind: MinecartKind,
    /// Vanilla `useExperimentalMovement`: this cart has a `NewMinecartBehavior`.
    experimental_movement: bool,
    /// Vanilla `NewMinecartBehavior.lerpSteps`, emptied by the entity tracker.
    lerp_steps: Mutex<Vec<MinecartStep>>,
    on_rails: AtomicBool,
    /// Vanilla `flipped`.
    flipped: AtomicBool,
    /// Vanilla `Entity.fallDistance`, which only living entities have in Pumpkin.
    fall_distance: AtomicCell<f64>,
    custom_display_block: AtomicCell<Option<BlockStateId>>,
    display_offset: AtomicI32,
}

enum MinecartKind {
    Rideable(RideableMinecart),
    Chest(ChestMinecart),
    Furnace(FurnaceMinecart),
    Hopper(HopperMinecart),
    Tnt(TntMinecart),
    Spawner(SpawnerMinecart),
    CommandBlock(CommandBlockMinecart),
}

impl MinecartEntity {
    pub fn new(mut entity: Entity) -> Self {
        entity.blocks_building = true;
        let kind = match entity.entity_type.id {
            id if id == EntityType::CHEST_MINECART.id => MinecartKind::Chest(ChestMinecart::new()),
            id if id == EntityType::FURNACE_MINECART.id => {
                MinecartKind::Furnace(FurnaceMinecart::new())
            }
            id if id == EntityType::HOPPER_MINECART.id => {
                MinecartKind::Hopper(HopperMinecart::new())
            }
            id if id == EntityType::TNT_MINECART.id => MinecartKind::Tnt(TntMinecart::new()),
            id if id == EntityType::SPAWNER_MINECART.id => {
                MinecartKind::Spawner(SpawnerMinecart::new())
            }
            id if id == EntityType::COMMAND_BLOCK_MINECART.id => {
                MinecartKind::CommandBlock(CommandBlockMinecart::new())
            }
            _ => MinecartKind::Rideable(RideableMinecart),
        };
        let experimental_movement = entity
            .world
            .load()
            .server
            .upgrade()
            .is_some_and(|server| server.is_feature_enabled("minecraft:minecart_improvements"));
        let display_offset = Self::default_display_offset_of(&kind);
        Self {
            vehicle: VehicleEntity::new(entity),
            kind,
            experimental_movement,
            lerp_steps: Mutex::new(Vec::new()),
            on_rails: AtomicBool::new(false),
            flipped: AtomicBool::new(false),
            fall_distance: AtomicCell::new(0.0),
            custom_display_block: AtomicCell::new(None),
            display_offset: AtomicI32::new(display_offset),
        }
    }

    /// Vanilla `AbstractMinecart.createMinecart`'s setup from the item: its custom name, and for
    /// the new behaviour the snap onto the rail.
    pub fn apply_item_stack(&self, stack: &ItemStack) {
        let entity = &self.vehicle.entity;
        if let Some(custom_name) = stack.get_data_component::<CustomNameImpl>() {
            entity.set_custom_name(custom_name.name.clone());
        }
        if self.experimental_movement {
            let pos = self.current_block_pos_or_rail_below();
            let (block, state_id) = entity.world.load().get_block_and_state_id(&pos);
            self.adjust_to_rails(pos, block, state_id, true);
        }
    }

    /// Whether this cart has vanilla's `NewMinecartBehavior`.
    #[must_use]
    pub const fn uses_new_behavior(&self) -> bool {
        self.experimental_movement
    }

    /// The steps vanilla `ServerEntity.handleMinecartPosRot` sends and clears.
    pub(crate) fn take_lerp_steps(&self) -> Vec<MinecartStep> {
        std::mem::take(
            &mut *self
                .lerp_steps
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        )
    }

    const fn default_display_offset_of(kind: &MinecartKind) -> i32 {
        match kind {
            MinecartKind::Chest(_) => CHEST_DISPLAY_OFFSET,
            MinecartKind::Hopper(_) => HOPPER_DISPLAY_OFFSET,
            _ => DEFAULT_DISPLAY_OFFSET,
        }
    }

    const fn container(&self) -> Option<&Arc<MinecartInventory>> {
        match &self.kind {
            MinecartKind::Chest(minecart) => Some(minecart.inventory()),
            MinecartKind::Hopper(minecart) => Some(minecart.inventory()),
            _ => None,
        }
    }

    /// Vanilla `getDropItem`.
    const fn drop_item(&self) -> &'static Item {
        match &self.kind {
            MinecartKind::Chest(_) => &Item::CHEST_MINECART,
            MinecartKind::Furnace(_) => &Item::FURNACE_MINECART,
            MinecartKind::Hopper(_) => &Item::HOPPER_MINECART,
            MinecartKind::Tnt(_) => &Item::TNT_MINECART,
            MinecartKind::Rideable(_) | MinecartKind::Spawner(_) | MinecartKind::CommandBlock(_) => {
                &Item::MINECART
            }
        }
    }

    /// Vanilla `isRideable`.
    const fn is_rideable(&self) -> bool {
        matches!(self.kind, MinecartKind::Rideable(_))
    }

    /// Vanilla `isFurnace`.
    const fn is_furnace(&self) -> bool {
        matches!(self.kind, MinecartKind::Furnace(_))
    }

    /// Vanilla `getMaxSpeed`, with the behaviours and `MinecartFurnace`'s override.
    fn max_speed(&self) -> f64 {
        let entity = &self.vehicle.entity;
        let speed = if self.experimental_movement {
            self.new_max_speed()
        } else if entity.is_in_water() {
            old_behavior::MAX_SPEED_IN_WATER
        } else {
            old_behavior::MAX_SPEED_ON_LAND
        };
        if self.is_furnace() {
            speed * FurnaceMinecart::max_speed_factor(entity)
        } else {
            speed
        }
    }

    /// Vanilla `AbstractMinecart.applyNaturalSlowdown`, with the furnace and container overrides.
    fn apply_natural_slowdown(&self, delta: Vector3<f64>) -> Vector3<f64> {
        let entity = &self.vehicle.entity;
        match &self.kind {
            MinecartKind::Furnace(minecart) => {
                self.base_natural_slowdown(minecart.apply_push(entity, delta))
            }
            MinecartKind::Chest(_) | MinecartKind::Hopper(_) => self
                .container()
                .map_or(delta, |inventory| container::natural_slowdown(entity, inventory, delta)),
            _ => self.base_natural_slowdown(delta),
        }
    }

    fn base_natural_slowdown(&self, delta: Vector3<f64>) -> Vector3<f64> {
        let entity = &self.vehicle.entity;
        let factor = if entity.has_passengers() {
            old_behavior::SLOWDOWN_WITH_PASSENGER
        } else if self.experimental_movement {
            new_behavior::SLOWDOWN_EMPTY
        } else {
            old_behavior::SLOWDOWN_EMPTY
        };
        let slowed = delta.multiply(factor, 0.0, factor);
        if entity.is_in_water() {
            slowed * f64::from(WATER_SLOWDOWN_FACTOR)
        } else {
            slowed
        }
    }

    /// Vanilla `activateMinecart`.
    fn activate_minecart(&self, powered: bool) {
        let entity = &self.vehicle.entity;
        match &self.kind {
            MinecartKind::Rideable(_) => RideableMinecart::activate(self, powered),
            MinecartKind::Hopper(minecart) => minecart.activate(powered),
            MinecartKind::Tnt(minecart) => minecart.activate(entity, powered),
            MinecartKind::CommandBlock(minecart) => minecart.activate(entity, powered),
            MinecartKind::Chest(_) | MinecartKind::Furnace(_) | MinecartKind::Spawner(_) => {}
        }
    }

    /// Vanilla `Entity.causeFallDamage` with `MinecartTNT`'s override: passengers take the fall.
    fn cause_fall_damage(&self, fall_distance: f64) {
        let entity = &self.vehicle.entity;
        if let MinecartKind::Tnt(minecart) = &self.kind {
            minecart.cause_fall_damage(entity, fall_distance);
        }
        if entity
            .entity_type
            .has_tag(&tag::EntityType::MINECRAFT_FALL_DAMAGE_IMMUNE)
        {
            return;
        }
        for passenger in self.passengers() {
            if let Some(living) = passenger.get_living_entity() {
                living.handle_fall_damage(passenger.as_ref(), fall_distance as f32, 1.0);
            }
        }
    }

    fn passengers(&self) -> Vec<Arc<dyn EntityBase>> {
        self.vehicle
            .entity
            .passengers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Vanilla `getFirstPassenger`.
    fn first_passenger(&self) -> Option<Arc<dyn EntityBase>> {
        self.vehicle
            .entity
            .passengers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .first()
            .cloned()
    }

    /// Vanilla `Entity.ejectPassengers`, last passenger first.
    fn eject_passengers(&self) {
        let entity = &self.vehicle.entity;
        for passenger in self.passengers().iter().rev() {
            entity.remove_passenger(passenger.get_entity().entity_id);
        }
    }

    /// Vanilla `ContainerEntity.unpackChestVehicleLootTable` without a player.
    fn unpack_container_loot(&self, container: &Arc<MinecartInventory>) {
        let entity = &self.vehicle.entity;
        let world = entity.world.load();
        container.unpack_loot(
            Some(entity),
            None,
            &LootContextParameters {
                world: Some(&world),
                position: Some(entity.pos.load()),
                ..Default::default()
            },
        );
    }

    /// Whether this cart is a vanilla `Container`.
    #[must_use]
    pub const fn has_container(&self) -> bool {
        self.container().is_some()
    }

    /// This cart as a vanilla `Container`. Like vanilla's slot access, this rolls a pending loot
    /// table first.
    #[must_use]
    pub fn get_container(&self) -> Option<Arc<dyn Inventory>> {
        let container = self.container()?;
        self.unpack_container_loot(container);
        Some(container.clone() as Arc<dyn Inventory>)
    }

    /// Vanilla `BaseCommandBlock.getSuccessCount` of a command block cart.
    #[must_use]
    pub fn command_success_count(&self) -> Option<u32> {
        if let MinecartKind::CommandBlock(minecart) = &self.kind {
            Some(minecart.success_count())
        } else {
            None
        }
    }

    /// Vanilla `handleSetCommandMinecart`'s writes. Whether this is a command block cart.
    pub fn set_command(&self, command: &str, track_output: bool, sync: bool) -> bool {
        let MinecartKind::CommandBlock(minecart) = &self.kind else {
            return false;
        };
        minecart.set_command(command, track_output);
        if sync {
            minecart.sync_entity_data(&self.vehicle.entity);
        }
        true
    }

    /// Whether a tick of this cart can reach further than its surroundings: a blast, a command or
    /// a spawn anywhere.
    #[must_use]
    pub fn has_global_tick_reach(&self) -> bool {
        match &self.kind {
            MinecartKind::Tnt(minecart) => {
                minecart.may_explode_soon(&self.vehicle.entity, self.fall_distance.load())
            }
            MinecartKind::Spawner(_) | MinecartKind::CommandBlock(_) => true,
            _ => false,
        }
    }

    fn set_custom_display_block(&self, state: Option<BlockStateId>) {
        self.custom_display_block.store(state);
        self.vehicle.entity.set_synced_data(
            ID_CUSTOM_DISPLAY_BLOCK,
            state.map_or(VarInt(0), |id| VarInt(i32::from(id.as_u16()))),
        );
    }

    fn set_display_offset(&self, offset: i32) {
        self.display_offset.store(offset, Ordering::Relaxed);
        self.vehicle
            .entity
            .set_synced_data(ID_DISPLAY_OFFSET, VarInt(offset));
    }

    /// Vanilla `Entity.lavaIgnite` and `lavaHurt`, which `AbstractMinecart.tick` runs itself.
    fn tick_in_lava(&self, caller: &dyn EntityBase) {
        let entity = &self.vehicle.entity;
        if !entity.is_fire_immune() {
            caller.set_on_fire_for(LAVA_FIRE_SECONDS);
            if caller.damage(caller, LAVA_DAMAGE, DamageType::LAVA) {
                caller.play_entity_sound(
                    Sound::EntityGenericBurn,
                    0.4,
                    2.0 + rand::rng().random::<f32>() * 0.4,
                );
            }
        }
        self.fall_distance.store(self.fall_distance.load() * 0.5);
    }
}

impl EntityBase for MinecartEntity {
    /// Vanilla `AbstractMinecart.addAdditionalSaveData` and its overrides.
    fn write_custom_nbt(&self, nbt: &mut NbtCompound) {
        if let Some(state) = self.custom_display_block.load() {
            nbt.put("DisplayState", block_state_to_nbt(state));
        }
        let display_offset = self.display_offset.load(Ordering::Relaxed);
        if display_offset != Self::default_display_offset_of(&self.kind) {
            nbt.put_int("DisplayOffset", display_offset);
        }
        nbt.put_bool("FlippedRotation", self.flipped.load(Ordering::Relaxed));
        // Vanilla stores `firstTick` under this name.
        nbt.put_bool(
            "HasTicked",
            self.vehicle.entity.first_tick.load(Ordering::Relaxed),
        );
        nbt.put_double("fall_distance", self.fall_distance.load());
        match &self.kind {
            MinecartKind::Chest(minecart) => minecart.write_nbt(nbt),
            MinecartKind::Furnace(minecart) => minecart.write_nbt(nbt),
            MinecartKind::Hopper(minecart) => minecart.write_nbt(nbt),
            MinecartKind::Tnt(minecart) => minecart.write_nbt(nbt),
            MinecartKind::Spawner(minecart) => minecart.write_nbt(nbt),
            MinecartKind::CommandBlock(minecart) => minecart.write_nbt(nbt),
            MinecartKind::Rideable(_) => {}
        }
    }

    /// Vanilla `AbstractMinecart.readAdditionalSaveData` and its overrides.
    fn read_custom_nbt(&self, nbt: &NbtCompound) {
        if let Some(state) = nbt.get("DisplayState").and_then(block_state_from_nbt) {
            self.set_custom_display_block(Some(state));
        }
        if let Some(offset) = nbt.get_int("DisplayOffset") {
            self.set_display_offset(offset);
        }
        self.flipped.store(
            nbt.get_bool("FlippedRotation").unwrap_or(false),
            Ordering::Relaxed,
        );
        self.vehicle.entity.first_tick.store(
            nbt.get_bool("HasTicked").unwrap_or(false),
            Ordering::Relaxed,
        );
        self.fall_distance
            .store(nbt.get_double("fall_distance").unwrap_or(0.0));
        match &self.kind {
            MinecartKind::Chest(minecart) => minecart.read_nbt(nbt),
            MinecartKind::Furnace(minecart) => minecart.read_nbt(nbt),
            MinecartKind::Hopper(minecart) => minecart.read_nbt(nbt),
            MinecartKind::Tnt(minecart) => minecart.read_nbt(nbt),
            MinecartKind::Spawner(minecart) => minecart.read_nbt(nbt),
            MinecartKind::CommandBlock(minecart) => {
                minecart.read_nbt(&self.vehicle.entity, nbt);
            }
            MinecartKind::Rideable(_) => {}
        }
    }

    /// Vanilla `Entity.positionRider` with `AbstractMinecart.getPassengerAttachmentPoint`:
    /// villagers and wandering traders sit lower.
    fn position_rider(&self, passenger: &dyn EntityBase) {
        let passenger_entity = passenger.get_entity();
        let attachment_y = if passenger_entity.entity_type == &EntityType::VILLAGER
            || passenger_entity.entity_type == &EntityType::WANDERING_TRADER
        {
            0.0
        } else {
            PASSENGER_ATTACHMENT_Y
        };
        let seat = self.vehicle.entity.pos.load() + Vector3::new(0.0, attachment_y, 0.0);
        passenger_entity.set_pos(seat - passenger.vehicle_attachment_point());
    }

    /// Vanilla `AbstractMinecart.tick`, which replaces `Entity.baseTick`: a burning cart never
    /// burns down, and it does not drown or freeze.
    fn tick(&self, caller: &dyn EntityBase, _server: &Server) {
        let entity = &self.vehicle.entity;
        // Vanilla `ServerLevel.tickNonPassenger`'s `setOldPosAndRot`.
        entity.update_last_pos();
        let y_rot_o = entity.yaw.load();
        if let MinecartKind::Hopper(minecart) = &self.kind {
            minecart.begin_tick();
        }

        self.vehicle.tick();
        entity.check_out_of_world(caller);
        entity.tick_portal(caller);
        if self.experimental_movement {
            self.new_tick(caller);
        } else {
            self.old_tick(caller, y_rot_o);
        }
        entity.update_fluid_interaction(caller);
        if entity.is_in_water() {
            self.fall_distance.store(0.0);
        }
        if !entity.first_tick.load(Ordering::Relaxed) && entity.touching_lava.load(Ordering::SeqCst)
        {
            self.tick_in_lava(caller);
        }
        entity.first_tick.store(false, Ordering::Relaxed);

        match &self.kind {
            MinecartKind::Furnace(minecart) => minecart.tick(entity),
            MinecartKind::Hopper(minecart) => minecart.try_consume_items(entity),
            MinecartKind::Tnt(minecart) => minecart.tick(entity),
            MinecartKind::Spawner(minecart) => minecart.tick(entity),
            MinecartKind::Rideable(_) | MinecartKind::Chest(_) | MinecartKind::CommandBlock(_) => {}
        }
    }

    fn tick_in_void(&self, _dyn_self: &dyn EntityBase) {
        self.discard();
    }

    fn kill(&self, _caller: &dyn EntityBase) {
        self.kill_minecart();
    }

    fn get_entity(&self) -> &Entity {
        &self.vehicle.entity
    }

    fn get_living_entity(&self) -> Option<&LivingEntity> {
        None
    }

    fn is_pushable(&self) -> bool {
        true
    }

    fn push(&self, entity: &dyn EntityBase) {
        self.push_entity(entity);
    }

    fn get_gravity(&self) -> f64 {
        let entity = &self.vehicle.entity;
        if entity.has_no_gravity() {
            0.0
        } else if entity.is_in_water() {
            GRAVITY_IN_WATER
        } else {
            GRAVITY
        }
    }

    /// Vanilla `AbstractMinecart.getBlockSpeedFactor`.
    fn get_block_speed_factor(&self) -> f32 {
        let entity = &self.vehicle.entity;
        if entity
            .world
            .load()
            .get_block(&entity.block_pos.load())
            .has_tag(&tag::Block::MINECRAFT_RAILS)
        {
            1.0
        } else {
            entity.get_block_speed_factor()
        }
    }

    /// Vanilla `AbstractMinecart.getMovementEmission`.
    fn movement_emission(&self) -> MovementEmission {
        MovementEmission::Events
    }

    fn is_on_rails(&self) -> bool {
        self.on_rails.load(Ordering::Relaxed)
    }

    fn init_data_tracker(&self) {
        self.vehicle.send_wobble_metadata();
        if let MinecartKind::Furnace(minecart) = &self.kind {
            minecart.init_data_tracker(&self.vehicle.entity);
        }
    }

    /// Vanilla `AbstractMinecart.isPickable`.
    fn can_hit(&self) -> bool {
        self.vehicle.entity.is_alive()
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
        self.hurt_server(amount, damage_type, source, cause)
    }

    fn interact(&self, player: &Arc<Player>, item_stack: &mut ItemStack) -> bool {
        let entity = &self.vehicle.entity;
        match &self.kind {
            MinecartKind::Chest(minecart) => minecart.interact(entity, player),
            MinecartKind::Furnace(minecart) => minecart.interact(entity, player, item_stack),
            MinecartKind::Hopper(minecart) => {
                minecart.interact(entity, player);
                true
            }
            MinecartKind::Rideable(_) => RideableMinecart::interact(self, player),
            // Vanilla `MinecartCommandBlock.interact`: the client opens the screen itself.
            MinecartKind::CommandBlock(_) => player.can_use_game_master_blocks(),
            MinecartKind::Tnt(_) | MinecartKind::Spawner(_) => false,
        }
    }

    fn move_entity(&self, caller: &dyn EntityBase, motion: Vector3<f64>) {
        self.cart_move(caller, motion);
    }

    fn cast_any(&self) -> &dyn std::any::Any {
        self
    }
}
