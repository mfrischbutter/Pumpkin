mod merge;
mod pickup;

use crate::server::Server;
use crate::world::World;
use core::f32;
use pumpkin_data::damage::DamageType;
use pumpkin_data::data_component_impl::{BundleContentsImpl, ContainerImpl, DamageResistantImpl};
use pumpkin_data::entity::EntityType;
use pumpkin_data::game_event::GameEvent;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::tag::Taggable;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_protocol::bedrock::client::CAddItemActor;
use pumpkin_protocol::bedrock::network_item::ItemStackWrapper;
use pumpkin_protocol::bedrock::server::actor_event::{ActorEventID, SActorEvent};
use pumpkin_protocol::codec::item_stack_seralizer::ItemStackSerializer;
use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_protocol::codec::var_long::VarLong;
use pumpkin_protocol::codec::var_ulong::VarULong;
use pumpkin_protocol::java::client::play::{CSetEntityMetadata, Metadata};
use pumpkin_util::math::atomic_f32::AtomicF32;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use std::sync::atomic::Ordering::{AcqRel, Relaxed};

use std::sync::{
    Arc, Mutex,
    atomic::{
        AtomicBool, AtomicI32,
        Ordering::{self},
    },
};

use super::{Entity, EntityBase, living::LivingEntity, player::Player};
use crate::block::blocks::bubble_column::SURFACE_UPWARD_ACCELERATION;
use crate::world::entity_islands::{self, AuditedReach};

pub struct ItemEntity {
    entity: Entity,
    /// Vanilla `age`, negative for an item given a longer life.
    item_age: AtomicI32,
    thrower: crossbeam::atomic::AtomicCell<Option<uuid::Uuid>>,
    /// Vanilla `target`, saved as `Owner`: the only player who may pick the item up.
    target: crossbeam::atomic::AtomicCell<Option<uuid::Uuid>>,
    // These cannot be atomic values because we mutate their state based on what they are; we run
    // into the ABA problem
    item_stack: Mutex<ItemStack>,
    /// Vanilla `pickupDelay`: saved as a short, and only exactly 0 allows a pickup.
    pickup_delay: AtomicI32,
    health: AtomicF32,
    never_despawn: AtomicBool,
    never_pickup: AtomicBool,
    merge_reserved: AtomicBool,
}

/// Vanilla `ItemEntity.LIFETIME`.
const LIFETIME: i32 = 6000; // 5 minutes in ticks
/// Vanilla `ItemEntity.INFINITE_PICKUP_DELAY`: the saved `PickupDelay` of a never-pickup item.
const INFINITE_PICKUP_DELAY: i16 = 32767;
/// Vanilla `ItemEntity.INFINITE_LIFETIME`: the saved `Age` of a never-despawn item.
const INFINITE_LIFETIME: i16 = -32768;
/// Vanilla `ItemEntity.merge(to, from, 64)`: cap on top of `getMaxStackSize`.
const MERGE_MAX_COUNT: u8 = 64;

/// Horizontal search distance of `mergeWithNeighbours`.
const MERGE_DISTANCE: f64 = 0.5;
/// Vanilla `FLOAT_HEIGHT`: how deep the item must be in a fluid before it floats up.
const FLOAT_HEIGHT: f32 = 0.1;
/// Vanilla `setUnderwaterMovement`.
const WATER_DRAG: f32 = 0.99;
/// Vanilla `setUnderLavaMovement`.
const LAVA_DRAG: f32 = 0.95;
/// Vanilla `setFluidMovement`: the rise, up to the speed where it stops.
const FLUID_RISE: f32 = 5.0e-4;
const FLUID_RISE_MAX_SPEED: f32 = 0.06;
/// Vanilla `getAirDrag`.
const AIR_DRAG: f32 = 0.98;

impl ItemEntity {
    pub const DEFAULT_PICKUP_DELAY: u8 = 10;

    pub fn new(entity: Entity, item_stack: ItemStack) -> Self {
        entity.velocity.store(Vector3::new(
            rand::random::<f64>().mul_add(0.2, -0.1),
            0.2,
            rand::random::<f64>().mul_add(0.2, -0.1),
        ));
        entity.yaw.store(rand::random::<f32>() * 360.0);
        Self::update_fire_immune(&entity, &item_stack);

        Self {
            entity,
            item_stack: Mutex::new(item_stack),
            item_age: AtomicI32::new(0),
            thrower: crossbeam::atomic::AtomicCell::new(None),
            target: crossbeam::atomic::AtomicCell::new(None),
            pickup_delay: AtomicI32::new(i32::from(Self::DEFAULT_PICKUP_DELAY)),
            health: AtomicF32::new(5.0),
            never_despawn: AtomicBool::new(false),
            never_pickup: AtomicBool::new(false),
            merge_reserved: AtomicBool::new(false),
        }
    }

    pub fn new_with_velocity(
        entity: Entity,
        item_stack: ItemStack,
        velocity: Vector3<f64>,
        pickup_delay: u8,
    ) -> Self {
        entity.velocity.store(velocity);
        entity.yaw.store(rand::random::<f32>() * 360.0);
        Self::update_fire_immune(&entity, &item_stack);

        Self {
            entity,
            item_stack: Mutex::new(item_stack),
            item_age: AtomicI32::new(0),
            thrower: crossbeam::atomic::AtomicCell::new(None),
            target: crossbeam::atomic::AtomicCell::new(None),
            pickup_delay: AtomicI32::new(i32::from(pickup_delay)),
            health: AtomicF32::new(5.0),
            never_despawn: AtomicBool::new(false),
            never_pickup: AtomicBool::new(false),
            merge_reserved: AtomicBool::new(false),
        }
    }

    /// Creates an `ItemEntity` for restoring from NBT without random velocity.
    /// The velocity and position will be set by `Entity::read_nbt_non_mut`.
    pub fn new_empty(entity: Entity) -> Self {
        Self {
            entity,
            item_stack: Mutex::new(ItemStack::new(1, &pumpkin_data::item::Item::AIR)),
            item_age: AtomicI32::new(0),
            thrower: crossbeam::atomic::AtomicCell::new(None),
            target: crossbeam::atomic::AtomicCell::new(None),
            pickup_delay: AtomicI32::new(0),
            health: AtomicF32::new(5.0),
            never_despawn: AtomicBool::new(false),
            never_pickup: AtomicBool::new(false),
            merge_reserved: AtomicBool::new(false),
        }
    }

    pub const fn get_item_stack(&self) -> &Mutex<ItemStack> {
        &self.item_stack
    }

    /// Vanilla `ItemEntity.setItem`: replaces the stack and resends it to tracking players.
    /// An empty stack removes the entity on its next tick.
    pub fn set_item_stack(&self, item_stack: ItemStack) {
        Self::update_fire_immune(&self.entity, &item_stack);
        *self
            .item_stack
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = item_stack;
        self.on_count_changed();
    }

    pub fn get_thrower(&self) -> Option<uuid::Uuid> {
        self.thrower.load()
    }

    pub fn set_thrower(&self, uuid: uuid::Uuid) {
        self.thrower.store(Some(uuid));
    }

    /// Vanilla `setTarget`: only this player may pick the item up.
    pub fn set_target(&self, target: Option<uuid::Uuid>) {
        self.target.store(target);
    }

    /// The delay as the plugin API knows it; a saved delay can be longer than this shows.
    pub fn get_pickup_delay(&self) -> u8 {
        self.pickup_delay
            .load(Ordering::Relaxed)
            .clamp(0, i32::from(u8::MAX)) as u8
    }

    /// Vanilla `!hasPickUpDelay() && isAlive()`, what mobs look for on the ground.
    #[must_use]
    pub fn can_be_picked_up_by_mob(&self) -> bool {
        !self.has_pickup_delay() && self.entity.is_alive()
    }

    pub fn set_pickup_delay(&self, pickup_delay: u8) {
        self.pickup_delay
            .store(i32::from(pickup_delay), Ordering::Relaxed);
    }

    /// Vanilla `ItemEntity.hasPickUpDelay`: a never-pickup item always has one.
    pub fn has_pickup_delay(&self) -> bool {
        self.never_pickup.load(Ordering::Relaxed) || self.pickup_delay.load(Ordering::Relaxed) > 0
    }

    pub fn is_never_pickup(&self) -> bool {
        self.never_pickup.load(Ordering::Relaxed)
    }

    /// Vanilla `ItemEntity.setNeverPickUp`: nothing can pick the item up while this is set.
    pub fn set_never_pickup(&self, never_pickup: bool) {
        self.never_pickup.store(never_pickup, Ordering::Relaxed);
    }

    pub fn is_never_despawn(&self) -> bool {
        self.never_despawn.load(Ordering::Relaxed)
    }

    /// Vanilla `ItemEntity.setUnlimitedLifetime`: the item does not age while this is set.
    pub fn set_never_despawn(&self, never_despawn: bool) {
        self.never_despawn.store(never_despawn, Ordering::Relaxed);
    }

    pub const fn get_entity(&self) -> &Entity {
        &self.entity
    }

    /// Vanilla `ItemStack.canBeHurtBy`: the `damage_resistant` tag blocks matching damage.
    fn can_be_hurt_by(stack: &ItemStack, damage_type: DamageType) -> bool {
        stack
            .get_data_component::<DamageResistantImpl>()
            .is_none_or(|res| damage_type.is_tagged_with(res.res_type.as_str()) != Some(true))
    }

    /// Vanilla `ItemEntity.fireImmune`, cached since the item only changes on load.
    fn update_fire_immune(entity: &Entity, stack: &ItemStack) {
        entity.fire_immune.store(
            !Self::can_be_hurt_by(stack, DamageType::IN_FIRE),
            Ordering::Relaxed,
        );
    }

    /// Vanilla `Item.onDestroyed`: containers (`BlockItem`, e.g. shulker boxes and bundles
    /// spill their contents where the item died, whatever the damage type was).
    fn on_destroyed(&self, world: &Arc<World>) {
        let contents = {
            let mut stack = self
                .item_stack
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let mut contents = Vec::new();
            if let Some(container) = stack.get_data_component::<ContainerImpl>()
                && !container.items.is_empty()
            {
                contents.extend(container.items.iter().map(|(_, item)| item.clone()));
                stack.set_data_component(ContainerImpl { items: Vec::new() });
            }
            if let Some(bundle) = stack.get_data_component::<BundleContentsImpl>()
                && !bundle.items.is_empty()
            {
                contents.extend(bundle.items.iter().cloned());
                stack.set_data_component(BundleContentsImpl { items: Vec::new() });
            }
            contents
        };

        // no pickup delay.
        let pos = self.entity.pos.load();
        for item in contents.into_iter().filter(|item| !item.is_empty()) {
            let spilled = Self::new(Entity::new(world.clone(), pos, &EntityType::ITEM), item);
            spilled.set_pickup_delay(0);
            world.spawn_entity(Arc::new(spilled));
        }
    }

    /// Resends the stack after its count changed. Bedrock item actors ignore the `ITEM`
    /// metadata, so they get the count as `UpdateStackSize`
    fn on_count_changed(&self) {
        self.init_data_tracker();

        let count = self
            .item_stack
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .item_count;
        let packet = SActorEvent {
            target_runtime_id: VarULong(self.entity.entity_id as u64),
            event_id: ActorEventID::UpdateStackSize,
            data: VarInt(i32::from(count)),
            fire_at_position: None,
        };
        self.entity
            .world
            .load()
            .send_to_tracking_players_bedrock(&self.entity, &packet);
    }

    fn decrement_pickup_delay(&self) {
        let delay = self.pickup_delay.load(Ordering::Relaxed);
        if delay > 0 {
            self.pickup_delay.store(delay - 1, Ordering::Relaxed);
        }
    }

    fn apply_fluid_drag_or_gravity(&self, mut velo: Vector3<f64>) -> Vector3<f64> {
        let entity = &self.entity;

        let float_height = f64::from(FLOAT_HEIGHT);
        let fluid_drag = if entity.touching_water.load(Ordering::SeqCst)
            && entity.water_height.load() > float_height
        {
            Some(WATER_DRAG)
        } else if entity.touching_lava.load(Ordering::SeqCst)
            && entity.lava_height.load() > float_height
        {
            Some(LAVA_DRAG)
        } else {
            None
        };
        if let Some(drag) = fluid_drag {
            // Vanilla `setFluidMovement`.
            velo.x *= f64::from(drag);
            velo.z *= f64::from(drag);
            if velo.y < f64::from(FLUID_RISE_MAX_SPEED) {
                velo.y += f64::from(FLUID_RISE);
            }
        } else {
            velo.y -= <Self as EntityBase>::get_gravity(self);
        }

        velo
    }

    fn update_no_physics_and_push_out(&self) {
        let entity = &self.entity;
        let pos = entity.pos.load();
        let bounding_box = entity.bounding_box.load();

        let no_physics = !entity
            .world
            .load()
            .is_space_empty(bounding_box.expand(-1.0e-7, -1.0e-7, -1.0e-7));

        entity.no_physics.store(no_physics, Ordering::Relaxed);

        if no_physics {
            entity.push_out_of_blocks(Vector3::new(
                pos.x,
                f64::midpoint(bounding_box.min.y, bounding_box.max.y),
                pos.z,
            ));
        }
    }

    /// a resting item only moves every 4th `tickCount`, offset by its id.
    fn should_tick_move(&self, move_velo: Vector3<f64>) -> bool {
        let entity = &self.entity;

        !entity.on_ground.load(Ordering::SeqCst)
            || move_velo.horizontal_length_squared() > f64::from(1.0e-5f32)
            || entity
                .tick_count
                .load(Ordering::Relaxed)
                .wrapping_add(entity.entity_id)
                % 4
                == 0
    }

    fn move_and_apply_friction(&self, caller: &dyn EntityBase, move_velo: Vector3<f64>) {
        let entity = &self.entity;

        entity.move_entity(caller, move_velo);
        entity.tick_block_collisions(caller);

        let mut friction = AIR_DRAG;
        let on_ground = entity.on_ground.load(Ordering::SeqCst);

        let mut velo = entity.velocity.load();
        if on_ground {
            let block_affecting_velo = entity.get_block_with_y_offset(f64::from(0.999_999f32)).1;
            friction *= block_affecting_velo.slipperiness;
        }

        velo = velo.multiply(f64::from(friction), f64::from(AIR_DRAG), f64::from(friction));

        // a landing item bounces back up with half its speed
        if on_ground && velo.y < 0.0 {
            velo.y *= -0.5;
        }

        entity.velocity.store(velo);
    }

    /// Vanilla `ItemEntity.tick`: the merge rate on `tickCount` is 2 while the item changes
    /// block cell, else 40.
    fn merge_if_due(&self) {
        let entity = &self.entity;
        let moved =
            BlockPos::floored_v(entity.last_pos.load()) != BlockPos::floored_v(entity.pos.load());
        let rate = if moved { 2 } else { 40 };
        if entity.tick_count.load(Ordering::Relaxed) % rate == 0 {
            self.merge_with_neighbours();
        }
    }

    /// Vanilla `ItemEntity.tick`: an item at the end of its life is discarded.
    fn despawn_if_old(&self) {
        let entity = &self.entity;
        if self.never_despawn.load(Ordering::Relaxed)
            || entity.is_removed()
            || self.item_age.load(Ordering::Relaxed) < LIFETIME
        {
            return;
        }
        let entity_id = entity.entity_id;
        let world = entity.world.load_full();
        let mut despawn_event =
            crate::plugin::api::events::entity::item_despawn::ItemDespawnEvent::new(entity_id);
        if let Some(server) = world.server.upgrade() {
            server
                .plugin_manager
                .fire_blocking(&server, &mut despawn_event);
        }
        if !despawn_event.cancelled {
            entity.remove();
        }
    }

    /// Vanilla `ItemEntity.tick` `needsSync`: fluid contact or velocity change.
    fn mark_needs_sync(&self, caller: &dyn EntityBase, original_velo: Vector3<f64>) {
        let entity = &self.entity;
        if entity.is_removed() {
            return;
        }

        // Vanilla runs `updateFluidInteraction` again here, but the fluid damage only
        // comes from the block effects, so it must not be applied a second time.
        let in_fluid = !entity.update_fluid_interaction(caller).is_empty();

        if in_fluid || entity.velocity.load().sub(&original_velo).length_squared() > 0.01 {
            entity.velocity_dirty.store(true, Ordering::SeqCst);
        }
    }
}

impl EntityBase for ItemEntity {
    fn dampens_vibrations(&self) -> bool {
        self.item_stack
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .item
            .has_tag(&pumpkin_data::tag::Item::MINECRAFT_DAMPENS_VIBRATIONS)
    }

    /// Vanilla `ItemEntity.getOwner`: the thrower.
    fn get_owner_entity(&self) -> Option<Arc<dyn EntityBase>> {
        let uuid = self.thrower.load()?;
        let world = self.entity.world.load();
        world.get_entity_by_uuid(uuid).or_else(|| {
            world
                .get_player_by_uuid(uuid)
                .map(|player| player as Arc<dyn EntityBase>)
        })
    }

    fn audited_reach(&self) -> Option<AuditedReach> {
        let entity = &self.entity;
        let world = entity.world.load();
        let pushes =
            super::MAX_PUSH_OUT_SPEED + super::max_fluid_push_speed(world.dimension.fast_lava);
        let velocity = entity.velocity.load();
        let horizontal = velocity.x.hypot(velocity.z) + pushes;
        let vertical = velocity.y.abs() + pushes + self.get_gravity() + SURFACE_UPWARD_ACCELERATION;
        let mut links = entity_islands::links_of(self);
        // Entering water splashes, wherever the item ends up.
        let moved = entity
            .bounding_box
            .load()
            .expand(horizontal, vertical, horizontal);
        super::mob::game_event_listener_links(&world, false, &moved, &mut links);
        let mut reach = AuditedReach {
            bounding_box: entity.bounding_box.load(),
            motion: (horizontal, vertical),
            entities: MERGE_DISTANCE,
            players: 0.0,
            block_reads: 1.0,
            route: None,
            changes_blocks: false,
            global: false,
            links,
        };
        reach.scan_contact_blocks(&world);
        Some(reach)
    }

    fn tick(&self, caller: &dyn EntityBase, server: &Server) {
        let entity = &self.entity;
        if self
            .item_stack
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_empty()
        {
            entity.remove();
            return;
        }

        // Vanilla `super.tick()`: `xo/yo/zo`, fluids, fire, portals and the void.
        entity.tick(caller, server);
        if entity.removed.load(Ordering::SeqCst) {
            return;
        }

        self.decrement_pickup_delay();

        let original_velo = entity.velocity.load();
        entity
            .velocity
            .store(self.apply_fluid_drag_or_gravity(original_velo));

        self.update_no_physics_and_push_out();

        let move_velo = entity.velocity.load(); // In case push_out_of_blocks modifies it

        if self.should_tick_move(move_velo) {
            self.move_and_apply_friction(caller, move_velo);
        } else {
            // Vanilla `applyEffectsFromBlocksForLastMovements`: a resting item still burns.
            entity.tick_block_collisions(caller);
        }

        self.merge_if_due();
        if !self.never_despawn.load(Ordering::Relaxed) {
            self.item_age.fetch_add(1, Ordering::Relaxed);
        }
        self.mark_needs_sync(caller, original_velo);
        self.despawn_if_old();
    }

    /// Vanilla `ItemEntity.teleport`.
    fn teleport(
        &self,
        position: Vector3<f64>,
        yaw: Option<f32>,
        pitch: Option<f32>,
        world: Arc<World>,
    ) {
        self.entity.teleport(position, yaw, pitch, &world);
        self.merge_with_neighbours();
    }

    /// Vanilla `ItemEntity.shouldPlayLavaHurtSound`.
    fn should_play_lava_hurt_sound(&self) -> bool {
        self.health.load(Relaxed) <= 0.0 || self.entity.tick_count.load(Relaxed) % 10 == 0
    }

    fn init_data_tracker(&self) {
        self.entity.set_synced_data(
            pumpkin_data::tracked_data::item::ITEM,
            ItemStackSerializer::from(
                self.item_stack
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .clone(),
            ),
        );
    }

    fn damage_with_context(
        &self,
        _caller: &dyn EntityBase,
        amount: f32,
        damage_type: DamageType,
        _position: Option<Vector3<f64>>,
        _source: Option<&dyn EntityBase>,
        cause: Option<&dyn EntityBase>,
    ) -> bool {
        // Vanilla `ItemEntity.hurtServer`.
        let entity = &self.entity;
        if entity.is_invulnerable_to(&damage_type, cause) {
            return false;
        }
        let world = entity.world.load_full();
        if !world.level_info.load().game_rules.mob_griefing
            && cause.is_some_and(|cause| cause.get_mob().is_some())
        {
            return false;
        }
        // E.g. netherite ignores fire, the nether star explosions.
        let can_be_hurt = Self::can_be_hurt_by(
            &self
                .item_stack
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            damage_type,
        );
        if !can_be_hurt {
            return false;
        }

        let mut event = crate::plugin::api::events::entity::entity_damage::EntityDamageEvent::new(
            entity.entity_id,
            damage_type,
            amount,
        );
        if let Some(server) = world.server.upgrade() {
            server.plugin_manager.fire_blocking(&server, &mut event);
        }
        if event.cancelled {
            return false;
        }

        // Vanilla `markHurt`: resend the motion.
        entity.velocity_dirty.store(true, Ordering::SeqCst);

        // Vanilla keeps item health as an int: `(int)(health - damage)`.
        let destroyed = loop {
            let current = self.health.load(Relaxed);
            let new = (current - event.damage).trunc();
            if self
                .health
                .compare_exchange(current, new, AcqRel, Relaxed)
                .is_ok()
            {
                break current > 0.0 && new <= 0.0;
            }
        };

        world.emit_game_event(GameEvent::EntityDamage.name(), entity.pos.load());
        if destroyed {
            self.on_destroyed(&world);
            entity.remove();
        }
        true
    }

    fn on_player_collision(&self, player: &Arc<Player>) {
        self.player_touch(player);
    }

    fn get_entity(&self) -> &Entity {
        &self.entity
    }

    fn get_living_entity(&self) -> Option<&LivingEntity> {
        None
    }

    fn get_item_entity(&self) -> Option<&ItemEntity> {
        Some(self)
    }

    fn get_gravity(&self) -> f64 {
        if self.entity.has_no_gravity() {
            0.0
        } else {
            0.04
        }
    }

    fn bedrock_y_offset(&self) -> f64 {
        0.125
    }

    fn write_custom_nbt(&self, nbt: &mut NbtCompound) {
        let item = self
            .item_stack
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut item_compound = NbtCompound::new();
        item.write_item_stack(&mut item_compound);
        nbt.put_compound("Item", item_compound);

        let age = if self.is_never_despawn() {
            INFINITE_LIFETIME
        } else {
            self.item_age.load(Ordering::Relaxed) as i16
        };
        nbt.put_short("Age", age);
        let pickup_delay = if self.is_never_pickup() {
            INFINITE_PICKUP_DELAY
        } else {
            self.pickup_delay.load(Ordering::Relaxed) as i16
        };
        nbt.put_short("PickupDelay", pickup_delay);
        nbt.put_short("Health", self.health.load(Relaxed) as i16);
        if let Some(thrower) = self.thrower.load() {
            nbt.put_uuid("Thrower", thrower);
        }
        if let Some(target) = self.target.load() {
            nbt.put_uuid("Owner", target);
        }
    }

    /// Vanilla `readAdditionalSaveData`. Missing keys keep their value, a fresh item already
    /// holds the defaults.
    fn read_custom_nbt(&self, nbt: &NbtCompound) {
        if nbt.has("Thrower") {
            self.thrower.store(nbt.get_uuid("Thrower"));
        }
        if nbt.has("Owner") {
            self.target.store(nbt.get_uuid("Owner"));
        }
        // Restore the item stack from the "Item" compound
        if let Some(item_compound) = nbt.get_compound("Item")
            && let Some(stack) = ItemStack::read_item_stack(item_compound)
        {
            // `new_empty` had no item yet to derive this from.
            Self::update_fire_immune(&self.entity, &stack);
            *self
                .item_stack
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = stack;
        }

        // Vanilla stores Age as a short: negative is a longer life, not an old item.
        if let Some(age) = nbt.get_numeric_short("Age") {
            let never_despawn = age == INFINITE_LIFETIME;
            self.set_never_despawn(never_despawn);
            if !never_despawn {
                self.item_age.store(i32::from(age), Ordering::Relaxed);
            }
        }

        // Vanilla stores PickupDelay as a short
        if let Some(delay) = nbt.get_numeric_short("PickupDelay") {
            let never_pickup = delay == INFINITE_PICKUP_DELAY;
            self.set_never_pickup(never_pickup);
            if !never_pickup {
                self.pickup_delay.store(i32::from(delay), Ordering::Relaxed);
            }
        }

        // Vanilla stores Health as a short
        if let Some(health) = nbt.get_numeric_short("Health") {
            self.health.store(f32::from(health), Relaxed);
        }
    }

    fn remove_custom_nbt(&self, removed: &[Box<str>]) {
        for key in removed {
            match &**key {
                "Thrower" => self.thrower.store(None),
                "Owner" => self.target.store(None),
                _ => {}
            }
        }
    }

    fn cast_any(&self) -> &dyn std::any::Any {
        self
    }

    fn send_bedrock_spawn_packet(&self, client: &crate::net::bedrock::BedrockClient) {
        let entity = &self.entity;
        let runtime_id = entity.entity_id as u64;
        let data = {
            let item_stack = self
                .item_stack
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let packet = CAddItemActor {
                target_actor_id: VarLong(runtime_id as i64),
                target_runtime_id: VarULong(runtime_id),
                item: ItemStackWrapper::from(&*item_stack),
                position: self.bedrock_pos().to_f32_lossy(),
                velocity: entity.velocity.load().to_f32_lossy(),
                entity_data: entity.bedrock_metadata(),
                is_from_fishing: false,
            };
            client.serialize_packet(&packet).ok()
        };
        if let Some(data) = data {
            client.try_enqueue_packet(data);
        }
    }

    fn send_java_spawn_packet(&self, client: &crate::net::java::JavaClient) {
        let spawn_packet = self.entity.create_spawn_packet();
        if let Ok(data) = client.serialize_packet(&spawn_packet) {
            client.try_enqueue_packet(data);
        }

        let metadata = Metadata::new(
            pumpkin_data::tracked_data::item::ITEM,
            ItemStackSerializer::from(
                self.item_stack
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .clone(),
            ),
        );
        let mut data = Vec::new();
        if metadata
            .write(&mut data, &pumpkin_data::packet::CURRENT_MC_VERSION)
            .is_ok()
        {
            data.push(255);
            let meta_packet = CSetEntityMetadata::new(self.entity.entity_id.into(), data.into());
            if let Ok(meta_data) = client.serialize_packet(&meta_packet) {
                client.try_enqueue_packet(meta_data);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ItemEntity;
    use pumpkin_data::data_component_impl::{CustomDataImpl, CustomNameImpl};
    use pumpkin_data::item::Item;
    use pumpkin_data::item_stack::ItemStack;
    use pumpkin_data::{Enchantment, damage::DamageType};
    use pumpkin_nbt::compound::NbtCompound;
    use pumpkin_util::text::TextComponent;

    #[test]
    fn different_counts_merge_up_to_max_stack() {
        let stone = |count| ItemStack::new(count, &Item::STONE);
        assert!(ItemEntity::are_mergeable(&stone(1), &stone(5)));
        assert!(ItemEntity::are_mergeable(&stone(32), &stone(32)));
        assert!(!ItemEntity::are_mergeable(&stone(33), &stone(32)));
    }

    #[test]
    fn unstackable_items_never_merge() {
        let sword = ItemStack::new(1, &Item::DIAMOND_SWORD);
        assert!(!ItemEntity::are_mergeable(&sword, &sword.clone()));
    }

    #[test]
    fn different_components_never_merge() {
        let plain = ItemStack::new(1, &Item::STONE);

        let mut named = plain.clone();
        named.set_data_component(CustomNameImpl {
            name: TextComponent::text("a"),
        });
        let mut renamed = plain.clone();
        renamed.set_data_component(CustomNameImpl {
            name: TextComponent::text("b"),
        });
        assert!(!ItemEntity::are_mergeable(&plain, &named));
        assert!(!ItemEntity::are_mergeable(&named, &renamed));
        assert!(ItemEntity::are_mergeable(&named, &named.clone()));

        let mut enchanted = plain.clone();
        enchanted.add_enchantment(&Enchantment::UNBREAKING, 1);
        let mut stronger = plain.clone();
        stronger.add_enchantment(&Enchantment::UNBREAKING, 2);
        assert!(!ItemEntity::are_mergeable(&plain, &enchanted));
        assert!(!ItemEntity::are_mergeable(&enchanted, &stronger));

        let mut data = NbtCompound::new();
        data.put_string("id", "a".to_string());
        let mut tagged = plain.clone();
        tagged.set_data_component(CustomDataImpl::new(data));
        assert!(!ItemEntity::are_mergeable(&plain, &tagged));
    }

    #[test]
    fn damage_resistant_items() {
        let sword = ItemStack::new(1, &Item::NETHERITE_SWORD);
        assert!(!ItemEntity::can_be_hurt_by(&sword, DamageType::LAVA));
        assert!(ItemEntity::can_be_hurt_by(&sword, DamageType::CACTUS));

        let star = ItemStack::new(1, &Item::NETHER_STAR);
        assert!(!ItemEntity::can_be_hurt_by(&star, DamageType::EXPLOSION));
        assert!(ItemEntity::can_be_hurt_by(&star, DamageType::LAVA));
    }
}
