use std::sync::Arc;
use std::sync::atomic::{AtomicI32, AtomicI64, AtomicU8, Ordering};

pub use crate::entity::decoration::armor_stand_pose::PackedRotation;
use crate::entity::decoration::armor_stand_pose::rotations;
use crate::entity::decoration::block_attached::{copy_custom_name, emit_game_event, may_interact};
use crate::entity::living::prevents_equipment_drop;
use crate::entity::player::Player;
use crate::entity::vehicle::minecart::MinecartEntity;
use crate::entity::{Entity, EntityBase, living::LivingEntity};
use crate::plugin::api::events::player::player_armor_stand_manipulate::PlayerArmorStandManipulateEvent;
use crate::server::Server;
use crossbeam::atomic::AtomicCell;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::{
    Block,
    attributes::Attributes,
    damage::DamageType,
    data_component_impl::{EquipmentSlot, EquipmentType, EquippableImpl},
    entity::EntityStatus,
    game_event::GameEvent,
    item::Item,
    particle::Particle,
    sound::Sound,
    tag::{self, Taggable},
    tracked_data::armor_stand,
};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_protocol::bedrock::server::actor_event::ActorEventID;
use pumpkin_protocol::java::client::play::ParticleOptions;
use pumpkin_util::math::boundingbox::EntityDimensions;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;

/// Vanilla `ArmorStand.WOBBLE_TIME`: the ticks in which a second hit breaks the stand.
const WOBBLE_TIME: i64 = 5;
/// Vanilla `DISABLE_TAKING_OFFSET`.
const DISABLE_TAKING_OFFSET: i32 = 8;
/// Vanilla `DISABLE_PUTTING_OFFSET`.
const DISABLE_PUTTING_OFFSET: i32 = 16;
/// Vanilla `FEET_OFFSET`.
const FEET_OFFSET: f64 = 0.1;
/// Vanilla `CHEST_OFFSET`.
const CHEST_OFFSET: f64 = 0.9;
/// Vanilla `LEGS_OFFSET`.
const LEGS_OFFSET: f64 = 0.4;
/// Vanilla `HEAD_OFFSET`.
const HEAD_OFFSET: f64 = 1.6;
/// Vanilla `BABY_DIMENSIONS`: half the size, with this eye height.
const SMALL_EYE_HEIGHT: f32 = 0.9875;
/// Vanilla `getAgeScale` of a baby, which a small stand counts as.
const SMALL_AGE_SCALE: f64 = 0.5;
/// Vanilla `hurtServer` and `causeDamage`: at or below this health fire breaks the stand.
const BURN_OUT_HEALTH: f32 = 0.5;
/// Vanilla `hurtServer`: damage of being in fire while already burning.
const IGNITED_DAMAGE: f32 = 0.15;
/// Vanilla `hurtServer`: damage of burning.
const BURN_DAMAGE: f32 = 4.0;
/// Vanilla `hurtServer`: seconds of burning from standing in fire.
const IGNITE_SECONDS: f32 = 5.0;
/// Vanilla `pushEntities`: a rideable minecart closer than this, squared, is pushed.
const MINECART_PUSH_DISTANCE_SQUARED: f64 = 0.2;

pub struct ArmorStandEntity {
    living_entity: LivingEntity,

    armor_stand_flags: AtomicU8,
    last_hit_time: AtomicI64,
    disabled_slots: AtomicI32,

    rotation: AtomicCell<PackedRotation>,
}

impl ArmorStandEntity {
    #[must_use]
    pub fn new(entity: Entity) -> Self {
        let packed_rotation = PackedRotation::default();
        // Vanilla `defineSynchedData`: the client assumes these until told otherwise.
        let data = &entity.synched_data;
        data.define(armor_stand::DATA_CLIENT_FLAGS, 0u8);
        data.define(armor_stand::DATA_HEAD_POSE, rotations(packed_rotation.head));
        data.define(armor_stand::DATA_BODY_POSE, rotations(packed_rotation.body));
        data.define(
            armor_stand::DATA_LEFT_ARM_POSE,
            rotations(packed_rotation.left_arm),
        );
        data.define(
            armor_stand::DATA_RIGHT_ARM_POSE,
            rotations(packed_rotation.right_arm),
        );
        data.define(
            armor_stand::DATA_LEFT_LEG_POSE,
            rotations(packed_rotation.left_leg),
        );
        data.define(
            armor_stand::DATA_RIGHT_LEG_POSE,
            rotations(packed_rotation.right_leg),
        );

        Self {
            living_entity: LivingEntity::new(entity),
            armor_stand_flags: AtomicU8::new(0),
            last_hit_time: AtomicI64::new(0),
            disabled_slots: AtomicI32::new(0),
            rotation: AtomicCell::new(packed_rotation),
        }
    }

    pub fn set_small(&self, small: bool) {
        self.set_bit_field(ArmorStandFlags::Small, small);
    }

    #[must_use]
    pub fn is_small(&self) -> bool {
        (self.armor_stand_flags.load(Ordering::Relaxed) & ArmorStandFlags::Small as u8) != 0
    }

    pub fn set_show_arms(&self, show_arms: bool) {
        self.set_bit_field(ArmorStandFlags::ShowArms, show_arms);
    }

    #[must_use]
    pub fn should_show_arms(&self) -> bool {
        (self.armor_stand_flags.load(Ordering::Relaxed) & ArmorStandFlags::ShowArms as u8) != 0
    }

    pub fn set_hide_base_plate(&self, hide_base_plate: bool) {
        self.set_bit_field(ArmorStandFlags::HideBasePlate, hide_base_plate);
    }

    #[must_use]
    pub fn should_show_base_plate(&self) -> bool {
        (self.armor_stand_flags.load(Ordering::Relaxed) & ArmorStandFlags::HideBasePlate as u8) == 0
    }

    pub fn set_marker(&self, marker: bool) {
        self.set_bit_field(ArmorStandFlags::Marker, marker);
    }

    #[must_use]
    pub fn is_marker(&self) -> bool {
        (self.armor_stand_flags.load(Ordering::Relaxed) & ArmorStandFlags::Marker as u8) != 0
    }

    fn set_bit_field(&self, bit_field: ArmorStandFlags, set: bool) {
        let current = self.armor_stand_flags.load(Ordering::Relaxed);
        let new_value = if set {
            current | bit_field as u8
        } else {
            current & !(bit_field as u8)
        };
        self.store_flags(new_value);
    }

    /// Vanilla `onSyncedDataUpdated` for the client flags.
    fn store_flags(&self, flags: u8) {
        self.armor_stand_flags.store(flags, Ordering::Relaxed);
        self.get_entity()
            .set_synced_data(armor_stand::DATA_CLIENT_FLAGS, flags);
        self.refresh_dimensions();
    }

    /// Vanilla `getDimensionsMarker`.
    fn dimensions(&self) -> EntityDimensions {
        if self.is_marker() {
            return EntityDimensions::new(0.0, 0.0, 0.0);
        }
        let dimensions = Entity::type_dimensions(self.get_entity().entity_type);
        if self.is_small() {
            EntityDimensions::new(
                dimensions.width / 2.0,
                dimensions.height / 2.0,
                SMALL_EYE_HEIGHT,
            )
        } else {
            dimensions
        }
    }

    /// Vanilla `ArmorStand.refreshDimensions`: resizing never moves the stand.
    fn refresh_dimensions(&self) {
        let entity = self.get_entity();
        let pos = entity.pos.load();
        entity.set_default_dimensions(self.dimensions());
        entity.set_pos(pos);
    }

    /// Vanilla `hasPhysics`.
    fn has_physics(&self) -> bool {
        !self.is_marker() && !self.get_entity().has_no_gravity()
    }

    /// Vanilla `canUseSlot`.
    #[must_use]
    pub fn can_use_slot(&self, slot: &EquipmentSlot) -> bool {
        !matches!(slot, EquipmentSlot::Body(_) | EquipmentSlot::Saddle(_))
            && !self.is_slot_disabled(slot)
    }

    /// One bit of vanilla `disabledSlots`, `1 << slot.getFilterBit(offset)`.
    fn disabled_bit(&self, slot: &EquipmentSlot, offset: i32) -> bool {
        self.disabled_slots.load(Ordering::Relaxed) & (1 << (slot.get_slot_index() + offset)) != 0
    }

    /// Vanilla `isDisabled`.
    #[must_use]
    pub fn is_slot_disabled(&self, slot: &EquipmentSlot) -> bool {
        self.disabled_bit(slot, 0)
            || (slot.slot_type() == EquipmentType::Hand && !self.should_show_arms())
    }

    pub fn set_slot_disabled(&self, slot: &EquipmentSlot, disabled: bool) {
        let slot_bit = 1 << slot.get_slot_index();
        let current = self.disabled_slots.load(Ordering::Relaxed);

        let new_val = if disabled {
            current | slot_bit
        } else {
            current & !slot_bit
        };

        self.disabled_slots.store(new_val, Ordering::Relaxed);
    }

    #[must_use]
    pub fn is_invisible(&self) -> bool {
        self.get_entity().invisible.load(Ordering::Relaxed)
    }

    #[must_use]
    pub fn pack_rotation(&self) -> PackedRotation {
        self.rotation.load()
    }

    /// Vanilla `setArmorStandPose`.
    pub fn unpack_rotation(&self, packed: &PackedRotation) {
        self.rotation.store(packed.to_owned());
        let entity = self.get_entity();
        entity.set_synced_data(armor_stand::DATA_HEAD_POSE, rotations(packed.head));
        entity.set_synced_data(armor_stand::DATA_BODY_POSE, rotations(packed.body));
        entity.set_synced_data(armor_stand::DATA_LEFT_ARM_POSE, rotations(packed.left_arm));
        entity.set_synced_data(armor_stand::DATA_RIGHT_ARM_POSE, rotations(packed.right_arm));
        entity.set_synced_data(armor_stand::DATA_LEFT_LEG_POSE, rotations(packed.left_leg));
        entity.set_synced_data(armor_stand::DATA_RIGHT_LEG_POSE, rotations(packed.right_leg));
    }

    fn get_item_by_slot(&self, slot: &EquipmentSlot) -> ItemStack {
        self.living_entity
            .entity_equipment
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(slot)
    }

    /// Vanilla `hasItemInSlot`.
    fn has_item_in_slot(&self, slot: &EquipmentSlot) -> bool {
        !self.get_item_by_slot(slot).is_empty()
    }

    /// Vanilla `LivingEntity.setItemSlot`.
    fn set_item_slot(&self, slot: &EquipmentSlot, stack: ItemStack) {
        let previous = self
            .living_entity
            .entity_equipment
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .put(slot, stack.clone());
        self.living_entity
            .send_equipment_changes(&[(slot.clone(), stack.clone())]);
        self.on_equip_item(slot, &previous, &stack);
    }

    /// Vanilla `LivingEntity.onEquipItem`.
    fn on_equip_item(&self, slot: &EquipmentSlot, old: &ItemStack, new: &ItemStack) {
        let entity = self.get_entity();
        if old.are_items_and_components_equal(new) || entity.first_tick.load(Ordering::Relaxed) {
            return;
        }
        let equippable = new.get_data_component::<EquippableImpl>();
        if let Some(equippable) = equippable
            && !entity.is_silent()
            && *equippable.slot == *slot
        {
            entity.world.load().play_sound_event(
                &equippable.equip_sound,
                self.sound_source(),
                &entity.pos.load(),
            );
        }
        let event = if equippable.is_some() {
            GameEvent::Equip
        } else {
            GameEvent::Unequip
        };
        emit_game_event(entity, event, None);
    }

    /// Vanilla `LivingEntity.getEquipmentSlotForItem`.
    fn get_equipment_slot_for_item(&self, stack: &ItemStack) -> EquipmentSlot {
        stack
            .get_data_component::<EquippableImpl>()
            .filter(|equippable| self.can_use_slot(equippable.slot))
            .map_or(EquipmentSlot::MAIN_HAND, |equippable| {
                equippable.slot.clone()
            })
    }

    /// Vanilla `getClickedSlot`. Without a location, as from Bedrock, the hands are it.
    fn get_clicked_slot(&self, location: Option<Vector3<f64>>) -> EquipmentSlot {
        let small = self.is_small();
        let age_scale = if small { SMALL_AGE_SCALE } else { 1.0 };
        let scale = f64::from(self.get_entity().scale.load()) * age_scale;
        let click_y = location.map_or(-1.0, |location| location.y / scale);

        let (feet_size, chest_start, chest_end, legs_size) = if small {
            (0.8, 0.3, 1.0, 1.0)
        } else {
            (0.45, 0.0, 0.7, 0.8)
        };
        let feet = FEET_OFFSET..FEET_OFFSET + feet_size;
        let chest = CHEST_OFFSET + chest_start..CHEST_OFFSET + chest_end;
        let legs = LEGS_OFFSET..LEGS_OFFSET + legs_size;

        if feet.contains(&click_y) && self.has_item_in_slot(&EquipmentSlot::FEET) {
            EquipmentSlot::FEET
        } else if chest.contains(&click_y) && self.has_item_in_slot(&EquipmentSlot::CHEST) {
            EquipmentSlot::CHEST
        } else if legs.contains(&click_y) && self.has_item_in_slot(&EquipmentSlot::LEGS) {
            EquipmentSlot::LEGS
        } else if click_y >= HEAD_OFFSET && self.has_item_in_slot(&EquipmentSlot::HEAD) {
            EquipmentSlot::HEAD
        } else if !self.has_item_in_slot(&EquipmentSlot::MAIN_HAND)
            && self.has_item_in_slot(&EquipmentSlot::OFF_HAND)
        {
            EquipmentSlot::OFF_HAND
        } else {
            EquipmentSlot::MAIN_HAND
        }
    }

    /// Vanilla `swapItem`, with the plugin event once the swap is certain to happen.
    fn swap_item(
        &self,
        player: &Arc<Player>,
        slot: &EquipmentSlot,
        player_stack: &mut ItemStack,
    ) -> bool {
        let current = self.get_item_by_slot(slot);
        let infinite_materials = player.is_creative();
        let offset = if current.is_empty() {
            DISABLE_PUTTING_OFFSET
        } else {
            DISABLE_TAKING_OFFSET
        };
        if self.disabled_bit(slot, offset) {
            return false;
        }
        let fills_from_infinite =
            infinite_materials && current.is_empty() && !player_stack.is_empty();
        let swaps = player_stack.is_empty() || player_stack.item_count <= 1;
        if !fills_from_infinite && !swaps && !current.is_empty() {
            return false;
        }
        if self.manipulation_cancelled(player, slot) {
            return false;
        }

        if fills_from_infinite {
            self.set_item_slot(slot, player_stack.copy_with_count(1));
        } else if swaps {
            self.set_item_slot(slot, player_stack.clone());
            *player_stack = current;
        } else {
            self.set_item_slot(slot, player_stack.split(1));
        }
        true
    }

    fn manipulation_cancelled(&self, player: &Arc<Player>, slot: &EquipmentSlot) -> bool {
        let Some(server) = self.get_entity().get_server() else {
            return false;
        };
        let mut event = PlayerArmorStandManipulateEvent {
            player: player.clone(),
            armor_stand_id: self.get_entity().entity_id,
            slot: slot.get_slot_index() as u8,
            cancelled: false,
        };
        server.plugin_manager.fire_blocking(&server, &mut event);
        event.cancelled
    }

    /// Vanilla `kill(level, attributedTo)`.
    fn kill_attributed(&self, attributed_to: Option<&dyn EntityBase>) {
        let entity = self.get_entity();
        entity.remove();
        emit_game_event(
            entity,
            GameEvent::EntityDie,
            attributed_to.or(Some(self as &dyn EntityBase)),
        );
    }

    /// Vanilla `Block.popResource`.
    fn pop_resource(&self, pos: &BlockPos, stack: ItemStack) {
        let world = self.get_entity().world.load();
        if world.level_info.load().game_rules.block_drops {
            world.drop_stack(pos, stack);
        }
    }

    /// Vanilla `brokenByPlayer`.
    fn broken_by_player(&self) {
        let entity = self.get_entity();
        let mut result = ItemStack::new(1, &Item::ARMOR_STAND);
        copy_custom_name(entity, &mut result);
        self.pop_resource(&entity.block_pos.load(), result);
        self.broken_by_anything();
    }

    /// Vanilla `brokenByAnything`.
    fn broken_by_anything(&self) {
        self.play_broken_sound();
        let drop_pos = self.get_entity().block_pos.load().up();
        for slot in (0..=7).filter_map(EquipmentSlot::from_slot_index) {
            let stack = self
                .living_entity
                .entity_equipment
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .put(slot, ItemStack::EMPTY.clone());
            if !stack.is_empty() && !prevents_equipment_drop(&stack) {
                self.pop_resource(&drop_pos, stack);
            }
        }
    }

    /// Vanilla `playBrokenSound`, which ignores the silent tag.
    fn play_broken_sound(&self) {
        let entity = self.get_entity();
        entity.world.load().play_sound(
            Sound::EntityArmorStandBreak,
            self.sound_source(),
            &entity.pos.load(),
        );
    }

    /// Vanilla `showBreakingParticles`.
    fn show_breaking_particles(&self) {
        let entity = self.get_entity();
        let pos = entity.pos.load();
        let (width, height) = (entity.width(), entity.height());
        entity.world.load().spawn_particle_with_options(
            Vector3::new(pos.x, pos.y + f64::from(height) * (2.0 / 3.0), pos.z),
            Vector3::new(width / 4.0, height / 4.0, width / 4.0),
            0.05,
            10,
            &ParticleOptions::Block {
                particle: Particle::Block,
                state: Block::OAK_PLANKS.default_state.id,
            },
        );
    }

    /// Vanilla `causeDamage`.
    fn cause_damage(&self, cause: Option<&dyn EntityBase>, damage: f32) {
        let health = self.living_entity.health.load() - damage;
        if health <= BURN_OUT_HEALTH {
            self.broken_by_anything();
            self.kill_attributed(cause);
        } else {
            self.living_entity.set_health(health);
            emit_game_event(self.get_entity(), GameEvent::EntityDamage, cause);
        }
    }

    /// The end of vanilla `hurtServer`: what a hit does that can break the stand.
    fn hurt_by_attack(&self, damage_type: DamageType, cause: Option<&dyn EntityBase>) -> bool {
        let can_break = damage_type.has_tag(&tag::DamageType::MINECRAFT_CAN_BREAK_ARMOR_STAND);
        let always_kills =
            damage_type.has_tag(&tag::DamageType::MINECRAFT_ALWAYS_KILLS_ARMOR_STANDS);
        if !can_break && !always_kills {
            return false;
        }
        if let Some(player) = cause.and_then(EntityBase::get_player) {
            if !player.may_build() {
                return false;
            }
            if player.is_creative() {
                self.play_broken_sound();
                self.show_breaking_particles();
                self.kill_attributed(cause);
                return true;
            }
        }

        let entity = self.get_entity();
        let world = entity.world.load();
        let time = world
            .level_time
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .query_gametime();

        if time - self.last_hit_time.load(Ordering::Relaxed) > WOBBLE_TIME && !always_kills {
            // The client plays the hit sound for this event.
            world.send_entity_status(entity, EntityStatus::ArmorstandWobble, ActorEventID::None);
            emit_game_event(entity, GameEvent::EntityDamage, cause);
            self.last_hit_time.store(time, Ordering::Relaxed);
        } else {
            self.broken_by_player();
            self.show_breaking_particles();
            self.kill_attributed(cause);
        }
        true
    }
}

impl EntityBase for ArmorStandEntity {
    fn can_be_seen_by_anyone(&self) -> bool {
        !self.is_invisible() && !self.is_marker()
    }

    fn write_custom_nbt(&self, nbt: &mut NbtCompound) {
        nbt.put_bool("Invisible", self.is_invisible());
        nbt.put_bool("Small", self.is_small());
        nbt.put_bool("ShowArms", self.should_show_arms());
        nbt.put_int("DisabledSlots", self.disabled_slots.load(Ordering::Relaxed));
        nbt.put_bool("NoBasePlate", !self.should_show_base_plate());
        if self.is_marker() {
            nbt.put_bool("Marker", true);
        }

        nbt.put("Pose", self.pack_rotation());
    }

    fn read_custom_nbt(&self, nbt: &NbtCompound) {
        let entity = self.get_entity();
        entity.set_invisible(nbt.get_bool("Invisible").unwrap_or(false));
        self.disabled_slots
            .store(nbt.get_int("DisabledSlots").unwrap_or(0), Ordering::Relaxed);

        let mut flags = 0u8;
        for (key, flag) in [
            ("Small", ArmorStandFlags::Small),
            ("ShowArms", ArmorStandFlags::ShowArms),
            ("NoBasePlate", ArmorStandFlags::HideBasePlate),
            ("Marker", ArmorStandFlags::Marker),
        ] {
            if nbt.get_bool(key).unwrap_or(false) {
                flags |= flag as u8;
            }
        }
        self.store_flags(flags);
        entity
            .no_physics
            .store(!self.has_physics(), Ordering::Relaxed);

        if let Some(pose_tag) = nbt.get("Pose") {
            let packed: PackedRotation = pose_tag.clone().into();
            self.unpack_rotation(&packed);
        }
    }

    fn get_entity(&self) -> &Entity {
        &self.living_entity.entity
    }

    fn get_living_entity(&self) -> Option<&LivingEntity> {
        Some(&self.living_entity)
    }

    /// With vanilla `tickHeadTurn`: the body follows the stand's yaw.
    fn tick(&self, caller: &dyn EntityBase, server: &Server) {
        self.living_entity.tick(caller, server);
        let entity = self.get_entity();
        entity.body_yaw.store(entity.yaw.load());
    }

    /// Vanilla `LivingEntity.kill` runs `hurtServer` with a kill, attributed to nobody.
    fn kill(&self, _caller: &dyn EntityBase) {
        self.kill_attributed(None);
    }

    fn interact_with_hand(
        &self,
        player: &Arc<Player>,
        item_stack: &mut ItemStack,
        hand: pumpkin_util::Hand,
    ) -> Option<bool> {
        self.interact_at(player, item_stack, hand, None)
    }

    /// Vanilla `ArmorStand.interact`.
    fn interact_at(
        &self,
        player: &Arc<Player>,
        item_stack: &mut ItemStack,
        _hand: pumpkin_util::Hand,
        location: Option<Vector3<f64>>,
    ) -> Option<bool> {
        if self.is_marker() || item_stack.item.id == Item::NAME_TAG.id {
            return Some(false);
        }

        let item_in_hand_slot = self.get_equipment_slot_for_item(item_stack);
        if item_stack.is_empty() {
            let clicked_slot = self.get_clicked_slot(location);
            let target_slot = if self.is_slot_disabled(&clicked_slot) {
                item_in_hand_slot
            } else {
                clicked_slot
            };
            return Some(
                self.has_item_in_slot(&target_slot)
                    && self.swap_item(player, &target_slot, item_stack),
            );
        }

        if self.is_slot_disabled(&item_in_hand_slot)
            || (item_in_hand_slot.slot_type() == EquipmentType::Hand && !self.should_show_arms())
        {
            return Some(false);
        }
        Some(self.swap_item(player, &item_in_hand_slot, item_stack))
    }

    /// Vanilla `ArmorStand.hurtServer`.
    fn damage_with_context(
        &self,
        _caller: &dyn EntityBase,
        _amount: f32,
        damage_type: DamageType,
        _position: Option<Vector3<f64>>,
        _source: Option<&dyn EntityBase>,
        cause: Option<&dyn EntityBase>,
    ) -> bool {
        let entity = self.get_entity();
        if entity.is_removed() {
            return false;
        }

        let griefing = entity.world.load().level_info.load().game_rules.mob_griefing;
        // Vanilla: only mob attackers are stopped by `mobGriefing`.
        if !griefing && cause.is_some_and(|cause| cause.get_mob().is_some()) {
            return false;
        }

        if damage_type.has_tag(&tag::DamageType::MINECRAFT_BYPASSES_INVULNERABILITY) {
            self.kill_attributed(cause);
            return false;
        }

        if entity.is_invulnerable_to(&damage_type, cause) || self.is_invisible() || self.is_marker()
        {
            return false;
        }

        if damage_type.has_tag(&tag::DamageType::MINECRAFT_IS_EXPLOSION) {
            self.broken_by_anything();
            self.kill_attributed(cause);
            return false;
        }

        if damage_type.has_tag(&tag::DamageType::MINECRAFT_IGNITES_ARMOR_STANDS) {
            if entity.is_on_fire() {
                self.cause_damage(cause, IGNITED_DAMAGE);
            } else {
                self.set_on_fire_for(IGNITE_SECONDS);
            }
            return false;
        }

        if damage_type.has_tag(&tag::DamageType::MINECRAFT_BURNS_ARMOR_STANDS)
            && self.living_entity.health.load() > BURN_OUT_HEALTH
        {
            self.cause_damage(cause, BURN_DAMAGE);
            return false;
        }

        self.hurt_by_attack(damage_type, cause)
    }

    fn get_gravity(&self) -> f64 {
        self.living_entity
            .get_attribute_value(&Attributes::GRAVITY)
    }

    /// Vanilla `isEffectiveAi`.
    fn is_effective_ai(&self) -> bool {
        self.has_physics()
    }

    /// Vanilla `pushEntities`: only rideable minecarts get pushed.
    fn push_entities_itself(&self) -> bool {
        let entity = self.get_entity();
        let pos = entity.pos.load();
        for other in entity
            .world
            .load()
            .get_entities_at_box(&entity.bounding_box.load())
        {
            if other
                .cast_any()
                .downcast_ref::<MinecartEntity>()
                .is_some_and(MinecartEntity::is_rideable)
                && other.get_entity().pos.load().squared_distance_to_vec(&pos)
                    <= MINECART_PUSH_DISTANCE_SQUARED
            {
                other.push(self);
            }
        }
        true
    }

    /// Vanilla `onSyncedDataUpdated`: a marker does not get in the way of building.
    fn blocks_building(&self) -> bool {
        !self.is_marker()
    }

    /// Vanilla `ignoreExplosion`.
    fn ignore_explosion(&self, affects_blocklike_entities: bool, _source_in_water: bool) -> bool {
        !affects_blocklike_entities || self.is_invisible()
    }

    /// Vanilla `skipAttackInteraction`.
    fn skip_attack_interaction(&self, player: &Player) -> bool {
        let entity = self.get_entity();
        !may_interact(&entity.world.load(), player, &entity.block_pos.load())
    }

    /// Vanilla `isPickable`.
    fn can_hit(&self) -> bool {
        self.living_entity.is_alive() && !self.is_marker()
    }

    /// Vanilla `canBeHitByProjectile` follows `isPickable`.
    fn can_be_hit_by_projectile(&self) -> bool {
        !self.is_marker()
    }

    /// Vanilla `getFallSounds`.
    fn get_fall_sounds(&self) -> (Sound, Sound) {
        (Sound::EntityArmorStandFall, Sound::EntityArmorStandFall)
    }

    /// Vanilla `thunderHit` does nothing.
    fn on_lightning_strike(
        &self,
        _caller: &dyn EntityBase,
        _lightning: &crate::entity::lightning::LightningBoltEntity,
    ) {
    }

    fn cast_any(&self) -> &dyn std::any::Any {
        self
    }
}

pub enum ArmorStandFlags {
    /// Small armor stand Flag
    Small = 1,
    /// Show arms Flag
    ShowArms = 4,
    /// Hide base plate fLag
    HideBasePlate = 8,
    /// Marker Flag
    Marker = 16,
}
