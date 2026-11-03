use crate::entity::mob::requires_custom_persistence_base;
use crate::world::World;
use crate::world::level_accessor::LevelAccessor;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicI32, AtomicU8, Ordering},
};
use uuid::Uuid;

use pumpkin_data::BlockState;
use pumpkin_data::{
    damage::DamageType,
    data_component_impl::{EquipmentSlot, FoodImpl},
    effect::StatusEffect,
    entity::{EntityStatus, EntityType},
    item::Item,
    item_stack::ItemStack,
    potion::Effect,
    sound::Sound,
    tag::{self, Taggable},
    zombie_nautilus_variant::ZombieNautilusVariant,
};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_protocol::bedrock::server::ActorEventID;
use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_util::math::boundingbox::EntityDimensions;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::wrap_degrees;
use rand::RngExt;

use super::nautilus_ai;
use crate::entity::ai::brain::memory::{PackedMemories, types};
use crate::entity::ai::brain::sensing::SensorType;
use crate::entity::ai::brain::{Brain, BrainProvider};
use crate::entity::ai::control::smooth_swimming_look_control::SmoothSwimmingLookControl;
use crate::entity::ai::control::smooth_swimming_move_control::SmoothSwimmingMoveControl;
use crate::entity::ai::pathfinder::{Navigator, node::PathType};
use crate::entity::mob::spawn::SpawnGroupData;
use crate::entity::passive::tamable::{TamableAnimal, TamableData};
use crate::entity::r#type::from_type;
use crate::entity::{
    Entity, EntityBase,
    ageable::{AgeableData, AgeableMob},
    custom_sound::CustomSound,
    mob::{Mob, MobEntity, equipment},
    passive::animal::Animal,
    player::Player,
};

/// Vanilla `AbstractNautilus.DASH_COOLDOWN_TICKS`.
const DASH_COOLDOWN_TICKS: i32 = 40;
/// Vanilla `AbstractNautilus.DASH_MINIMUM_DURATION_TICKS`.
const DASH_MINIMUM_DURATION_TICKS: i32 = 5;
/// Vanilla `SMALL_RESTRICTION_RADIUS`.
const SMALL_RESTRICTION_RADIUS: i32 = 16;
/// Vanilla `LARGE_RESTRICTION_RADIUS`.
const LARGE_RESTRICTION_RADIUS: i32 = 32;
/// Vanilla `RESTRICTION_RADIUS_BUFFER`.
const RESTRICTION_RADIUS_BUFFER: i32 = 8;
/// Vanilla `EFFECT_DURATION`.
const EFFECT_DURATION: i32 = 60;
/// Vanilla `EFFECT_REFRESH_RATE`.
const EFFECT_REFRESH_RATE: i64 = 40;
/// Vanilla `Nautilus.NAUTILUS_TOTAL_AIR_SUPPLY`.
const TOTAL_AIR_SUPPLY: i32 = 300;
/// Vanilla `Nautilus.handleAirSupply`.
const DRYING_OUT_AIR: i32 = -20;
/// Vanilla `tickRidden`.
const RIDDEN_TURN_SPEED: f32 = 0.5;
/// Vanilla `getRiddenRotation`.
const RIDDEN_PITCH_SCALE: f32 = 0.5;
/// Vanilla `Nautilus.BABY_DIMENSIONS`.
const BABY_SCALE: f32 = 0.5;

/// A nautilus or, as both share vanilla's `AbstractNautilus`, a zombie nautilus.
pub struct NautilusEntity {
    pub mob_entity: MobEntity,
    pub ageable_data: AgeableData,
    pub tamable_data: TamableData,
    pub is_dashing: AtomicBool,
    pub dash_cooldown: AtomicI32,
    /// Vanilla `ZombieNautilus.DATA_VARIANT_ID`.
    variant: AtomicU8,
    /// Who hurt it, kept for after the brain tick: the damage hook may not lock the brain.
    hurt_by: Mutex<Option<Uuid>>,
    /// What its size is set up for.
    sized_as_baby: AtomicBool,
}

impl NautilusEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let mut mob_entity = MobEntity::new(entity);
        Self::set_up_movement(&mut mob_entity);
        Arc::new(Self {
            mob_entity,
            ageable_data: AgeableData::default(),
            tamable_data: TamableData::default(),
            is_dashing: AtomicBool::new(false),
            dash_cooldown: AtomicI32::new(0),
            variant: AtomicU8::new(ZombieNautilusVariant::Temperate.id()),
            hurt_by: Mutex::new(None),
            sized_as_baby: AtomicBool::new(false),
        })
    }

    /// Vanilla `AbstractNautilus` constructor and `createNavigation`.
    fn set_up_movement(mob_entity: &mut MobEntity) {
        *mob_entity
            .move_control
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            Box::new(SmoothSwimmingMoveControl::new(85, 10, 0.011, 0.0, true));
        *mob_entity
            .look_control
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            Box::new(SmoothSwimmingLookControl::new(10));
        let navigator = mob_entity
            .navigator
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *navigator = Navigator::water_bound(false);
        navigator.set_pathfinding_malus(PathType::Water, 0.0);
    }

    fn is_zombie(&self) -> bool {
        self.get_entity().entity_type == &EntityType::ZOMBIE_NAUTILUS
    }

    fn is_in_water(&self) -> bool {
        self.get_entity().touching_water.load(Ordering::Relaxed)
    }

    pub fn is_dashing(&self) -> bool {
        self.is_dashing.load(Ordering::Relaxed)
    }

    pub fn set_dashing(&self, dashing: bool) {
        self.is_dashing.store(dashing, Ordering::Relaxed);
        self.get_entity()
            .set_synced_data(pumpkin_data::tracked_data::nautilus::DASH, dashing);
    }

    /// One sound for each of baby, adult and zombie, in water and out of it.
    fn sound(&self, baby: (Sound, Sound), adult: (Sound, Sound), zombie: (Sound, Sound)) -> Sound {
        let (wet, dry) = if self.is_zombie() {
            zombie
        } else if self.is_baby() {
            baby
        } else {
            adult
        };
        if self.is_in_water() { wet } else { dry }
    }

    fn dash_sound(&self) -> Sound {
        let adult = (Sound::EntityNautilusDash, Sound::EntityNautilusDashLand);
        self.sound(
            adult,
            adult,
            (
                Sound::EntityZombieNautilusDash,
                Sound::EntityZombieNautilusDashLand,
            ),
        )
    }

    fn dash_ready_sound(&self) -> Sound {
        let adult = (
            Sound::EntityNautilusDashReady,
            Sound::EntityNautilusDashReadyLand,
        );
        self.sound(
            adult,
            adult,
            (
                Sound::EntityZombieNautilusDashReady,
                Sound::EntityZombieNautilusDashReadyLand,
            ),
        )
    }

    fn eat_sound(&self) -> Sound {
        if self.is_zombie() {
            Sound::EntityZombieNautilusEat
        } else if self.is_baby() {
            Sound::EntityBabyNautilusEat
        } else {
            Sound::EntityNautilusEat
        }
    }

    /// Vanilla `Nautilus.handleAirSupply`; a zombie nautilus does not dry out.
    fn handle_air_supply(&self, caller: &dyn EntityBase) {
        let entity = self.get_entity();
        if entity.is_alive() && !self.is_in_water() {
            let air = entity.air_supply.load(Ordering::Relaxed) - 1;
            entity.set_air_supply(air);
            if air <= DRYING_OUT_AIR {
                entity.set_air_supply(0);
                caller.damage(caller, 2.0, DamageType::DRY_OUT);
            }
        } else {
            entity.set_air_supply(TOTAL_AIR_SUPPLY);
        }
    }

    /// Vanilla `getNautilusRestrictionRadius`.
    fn restriction_radius(&self) -> i32 {
        if !self.is_baby() && !self.is_saddled() {
            LARGE_RESTRICTION_RADIUS
        } else {
            SMALL_RESTRICTION_RADIUS
        }
    }

    /// Vanilla `checkRestriction`.
    fn check_restriction(&self) {
        let entity = self.get_entity();
        let has_rider = !entity
            .passengers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_empty();
        if entity.is_leashed() || has_rider || !self.is_tame() {
            return;
        }
        let radius = self.restriction_radius();
        let mob = &self.mob_entity;
        let pos = entity.block_pos.load();
        let margin = radius + RESTRICTION_RADIUS_BUFFER;
        if !mob.has_position_target()
            || mob.position_target.load().squared_distance(&pos) >= margin * margin
            || radius != mob.position_target_range.load(Ordering::Relaxed)
        {
            mob.set_home_to(pos, radius);
        }
    }

    /// Vanilla `applyEffects`: its rider breathes.
    fn apply_effects(&self, world: &World) {
        let Some(rider) = crate::entity::mob::saddle::first_passenger_player(self) else {
            return;
        };
        if !rider.has_effect(&StatusEffect::BREATH_OF_THE_NAUTILUS)
            || world.get_world_age() % EFFECT_REFRESH_RATE == 0
        {
            rider.add_effect(Effect {
                effect_type: &StatusEffect::BREATH_OF_THE_NAUTILUS,
                duration: EFFECT_DURATION,
                amplifier: 0,
                ambient: true,
                show_particles: true,
                show_icon: true,
                blend: true,
            });
        }
    }

    fn tick_dash(&self) {
        let cooldown = self.dash_cooldown.load(Ordering::Relaxed);
        if self.is_dashing() && cooldown < DASH_COOLDOWN_TICKS - DASH_MINIMUM_DURATION_TICKS {
            self.set_dashing(false);
        }
        if cooldown > 0 {
            self.dash_cooldown.store(cooldown - 1, Ordering::Relaxed);
            if cooldown == 1 {
                self.play_entity_sound(self.dash_ready_sound(), 1.0, 1.0);
            }
        }
    }

    /// Vanilla `tryToTame`: one in three.
    fn try_to_tame(&self, player: &Arc<Player>) {
        let entity = self.get_entity();
        let world = entity.world.load();
        if self.get_random().random_range(0..3) == 0 && self.approve_taming(player) {
            self.tame(player.gameprofile.id);
            self.mob_entity
                .navigator
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .stop();
            world.send_entity_status(
                entity,
                EntityStatus::TamingSucceeded,
                ActorEventID::TamingSucceeded,
            );
        } else {
            world.send_entity_status(
                entity,
                EntityStatus::TamingFailed,
                ActorEventID::TamingFailed,
            );
        }
        self.play_entity_sound(self.eat_sound(), 1.0, 1.0);
    }

    /// Vanilla `TamableAnimal.feed(player, hand, item, 2, 1)`.
    fn feed_to_heal(&self, player: &Player, stack: &mut ItemStack) {
        let heal = stack
            .get_data_component::<FoodImpl>()
            .map_or(1.0, |food| 2.0 * food.nutrition as f32);
        self.use_player_item(player, stack);
        self.mob_entity.living_entity.heal(heal);
        self.play_entity_sound(self.eat_sound(), 1.0, 1.0);
    }

    /// The equip part of vanilla `Equippable.equipOnInteract` for its armour.
    fn try_equip_armor(&self, player: &Player, stack: &mut ItemStack) -> bool {
        if !self.is_tame()
            || !equipment::is_equippable_in_slot(
                self.get_entity().entity_type,
                stack,
                &EquipmentSlot::BODY,
            )
            || !self
                .mob_entity
                .living_entity
                .entity_equipment
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .get(&EquipmentSlot::BODY)
                .is_empty()
        {
            return false;
        }
        self.mob_entity
            .set_item_slot_and_drop_when_killed(&EquipmentSlot::BODY, stack.copy_with_count(1));
        stack.decrement_unless_creative(player.gamemode.load(), 1);
        true
    }

    /// Vanilla `doPlayerRide`; it carries one rider.
    fn start_riding(&self, player: &Player) -> bool {
        let entity = self.get_entity();
        let has_rider = !entity
            .passengers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_empty();
        if has_rider {
            return false;
        }
        let world = player.world();
        if let Some(vehicle) = world.get_entity_by_id(entity.entity_id)
            && let Some(passenger) = world.get_player_by_id(player.entity_id())
        {
            entity.add_passenger(vehicle, passenger as Arc<dyn EntityBase>);
        }
        true
    }

    /// Vanilla `ZombieNautilusVariants`: the coral one in warm oceans.
    fn variant_for_biome(world: &World, pos: &BlockPos) -> ZombieNautilusVariant {
        if world
            .get_biome(pos)
            .has_tag(&tag::WorldgenBiome::MINECRAFT_SPAWNS_CORAL_VARIANT_ZOMBIE_NAUTILUS)
        {
            ZombieNautilusVariant::Warm
        } else {
            ZombieNautilusVariant::Temperate
        }
    }

    fn set_variant(&self, variant: ZombieNautilusVariant) {
        self.variant.store(variant.id(), Ordering::Relaxed);
        self.get_entity().set_synced_data(
            pumpkin_data::tracked_data::zombie_nautilus::DATA_VARIANT_ID,
            VarInt(i32::from(variant.id())),
        );
    }

    fn variant(&self) -> ZombieNautilusVariant {
        ZombieNautilusVariant::from_id(self.variant.load(Ordering::Relaxed))
            .unwrap_or(ZombieNautilusVariant::Temperate)
    }
}

impl AgeableMob for NautilusEntity {
    fn get_ageable_data(&self) -> &AgeableData {
        &self.ageable_data
    }

    /// Vanilla `ZombieNautilus.canBeABaby`.
    fn can_be_a_baby(&self) -> bool {
        !self.is_zombie()
    }
}

impl TamableAnimal for NautilusEntity {
    fn get_tamable_data(&self) -> &TamableData {
        &self.tamable_data
    }
}

impl Animal for NautilusEntity {
    /// Vanilla `AbstractNautilus.isFood`: taming first, then its food.
    fn is_food(&self, item_stack: &ItemStack) -> bool {
        let tag = if !self.is_tame() && !self.is_baby() {
            &tag::Item::MINECRAFT_NAUTILUS_TAMING_ITEMS
        } else {
            &tag::Item::MINECRAFT_NAUTILUS_FOOD
        };
        item_stack.item.has_tag(tag)
    }

    /// Vanilla `AbstractNautilus.usePlayerItem`: a bucket of fish leaves the water bucket.
    fn use_player_item(&self, player: &Player, item_stack: &mut ItemStack) {
        if item_stack
            .item
            .has_tag(&tag::Item::MINECRAFT_NAUTILUS_BUCKET_FOOD)
        {
            crate::item::items::bucket::fill_bucket_stack(item_stack, player, &Item::WATER_BUCKET);
        } else {
            item_stack.decrement_unless_creative(player.gamemode.load(), 1);
        }
    }

    /// Vanilla `Nautilus.getBreedOffspring`: a tame parent gives a tame baby.
    fn get_breed_offspring(&self, _partner: &dyn EntityBase) -> Arc<dyn EntityBase> {
        let entity = self.get_entity();
        let baby = from_type(
            entity.entity_type,
            entity.pos.load(),
            &entity.world.load(),
            Uuid::new_v4(),
        );
        if self.is_tame()
            && let Some(baby) = baby.cast_any().downcast_ref::<Self>()
        {
            baby.set_tame(true);
            baby.set_owner(self.get_owner());
        }
        baby
    }
}

impl CustomSound for NautilusEntity {
    fn hurt_sound(&self) -> Option<Sound> {
        Some(self.sound(
            (
                Sound::EntityBabyNautilusHurt,
                Sound::EntityBabyNautilusHurtLand,
            ),
            (Sound::EntityNautilusHurt, Sound::EntityNautilusHurtLand),
            (
                Sound::EntityZombieNautilusHurt,
                Sound::EntityZombieNautilusHurtLand,
            ),
        ))
    }

    fn death_sound(&self) -> Option<Sound> {
        Some(self.sound(
            (
                Sound::EntityBabyNautilusDeath,
                Sound::EntityBabyNautilusDeathLand,
            ),
            (Sound::EntityNautilusDeath, Sound::EntityNautilusDeathLand),
            (
                Sound::EntityZombieNautilusDeath,
                Sound::EntityZombieNautilusDeathLand,
            ),
        ))
    }
}

impl Mob for NautilusEntity {
    fn make_brain(&self, packed: &PackedMemories) -> Brain {
        BrainProvider {
            memory_types: &nautilus_ai::MEMORIES,
            sensor_types: &[
                SensorType::NearestLivingEntities,
                SensorType::NearestAdult,
                SensorType::NearestPlayers,
                SensorType::HurtBy,
                SensorType::NautilusTemptations,
            ],
            activities: if self.is_zombie() {
                nautilus_ai::zombie_activities
            } else {
                nautilus_ai::activities
            },
        }
        .make_brain(self, packed)
    }

    /// Vanilla `Nautilus.getDefaultDimensions` follows the age.
    fn before_mob_ai_step(&self) {
        let baby = self.is_baby();
        if self.sized_as_baby.swap(baby, Ordering::Relaxed) != baby {
            let entity = self.get_entity();
            let full = Entity::type_dimensions(entity.entity_type);
            entity.set_default_dimensions(if baby {
                EntityDimensions::new(
                    full.width * BABY_SCALE,
                    full.height * BABY_SCALE,
                    full.eye_height * BABY_SCALE,
                )
            } else {
                full
            });
        }
    }

    /// Vanilla `customServerAiStep` of both kinds, with `AbstractNautilus.hurtServer`'s grudge.
    fn custom_server_ai_step(&self, _caller: &dyn EntityBase) {
        self.check_restriction();
        self.mob_entity.tick_brain(self);
        let hurt_by = self
            .hurt_by
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        self.mob_entity.with_brain(self, |brain| {
            if let Some(attacker) = hurt_by {
                brain.erase(types::CANT_REACH_WALK_TARGET_SINCE.id());
                brain.set_with_expiry(types::ANGRY_AT, attacker, nautilus_ai::ANGER_DURATION);
            }
            nautilus_ai::update_activity(brain);
        });
    }

    /// Vanilla `AbstractNautilus.hurtServer` remembers who did it.
    fn on_damage(&self, _damage_type: DamageType, source: Option<&dyn EntityBase>) {
        if let Some(source) = source.filter(|source| source.get_living_entity().is_some()) {
            *self
                .hurt_by
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) =
                Some(source.get_entity().entity_uuid);
        }
    }

    /// Vanilla `AbstractNautilus.travelInWater`.
    fn travel_in_water(&self, caller: &dyn EntityBase, _gravity: f64, _falling: bool) {
        self.mob_entity.living_entity.travel_in_water_freely(caller);
    }

    /// Vanilla `AbstractNautilus.isPushedByFluid`.
    fn mob_is_pushed_by_fluids(&self) -> bool {
        false
    }

    /// Vanilla `Nautilus.getMaxAirSupply`, which is also every mob's default.
    fn mob_max_air_supply(&self) -> i32 {
        TOTAL_AIR_SUPPLY
    }

    fn mob_handles_own_air_supply(&self) -> bool {
        !self.is_zombie()
    }

    /// Vanilla `AbstractNautilus.getWalkTargetValue`: every spot is alike, light does not count.
    fn get_walk_target_value(&self, _pos: &BlockPos, _level: &dyn LevelAccessor) -> f32 {
        0.0
    }

    /// Vanilla `AbstractNautilus.checkSpawnObstruction`: only entities obstruct, the water does
    /// not.
    fn check_spawn_obstruction(&self, level: &dyn LevelAccessor) -> bool {
        level.is_unobstructed(&self.get_mob_entity().living_entity.entity)
    }

    /// Vanilla `AbstractNautilus.removeWhenFarAway`.
    fn remove_when_far_away(&self, _distance_sq: f64) -> bool {
        true
    }

    /// Vanilla `AbstractNautilus.requiresCustomPersistence`: a tame nautilus stays.
    fn requires_custom_persistence(&self) -> bool {
        requires_custom_persistence_base(self) || self.is_tame()
    }

    fn as_tamable(&self) -> Option<&dyn TamableAnimal> {
        Some(self)
    }

    fn as_ageable(&self) -> Option<&dyn AgeableMob> {
        Some(self)
    }

    fn get_ambient_sound(&self) -> Option<Sound> {
        Some(self.sound(
            (
                Sound::EntityBabyNautilusAmbient,
                Sound::EntityBabyNautilusAmbientLand,
            ),
            (
                Sound::EntityNautilusAmbient,
                Sound::EntityNautilusAmbientLand,
            ),
            (
                Sound::EntityZombieNautilusAmbient,
                Sound::EntityZombieNautilusAmbientLand,
            ),
        ))
    }

    fn mob_get_swim_sound(&self) -> Option<Sound> {
        Some(if self.is_zombie() {
            Sound::EntityZombieNautilusSwim
        } else if self.is_baby() {
            Sound::EntityBabyNautilusSwim
        } else {
            Sound::EntityNautilusSwim
        })
    }

    /// Vanilla `AbstractNautilus` steps silently.
    fn mob_play_step_sound(&self, _pos: &BlockPos, _state: &BlockState) {}

    fn as_custom_sound(&self) -> Option<&dyn CustomSound> {
        Some(self)
    }

    fn as_animal(&self) -> Option<&dyn Animal> {
        Some(self)
    }

    /// Vanilla `AbstractNautilus.finalizeSpawn` and, for a zombie nautilus, its variant.
    fn finalize_spawn(
        &self,
        world: &Arc<World>,
        group_data: Option<SpawnGroupData>,
    ) -> Option<SpawnGroupData> {
        if self.is_zombie() {
            self.set_variant(Self::variant_for_biome(
                world,
                &self.get_entity().block_pos.load(),
            ));
        }
        let cooldown = self
            .get_random()
            .random_range(nautilus_ai::TIME_BETWEEN_NON_PLAYER_ATTACKS);
        self.mob_entity.with_brain(self, |brain| {
            brain.set(types::ATTACK_TARGET_COOLDOWN, cooldown);
        });
        self.finalize_spawn_super(group_data)
    }

    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        self.write_tamable_nbt(nbt);
        if self.is_zombie() {
            nbt.put_string("variant", format!("minecraft:{}", self.variant().to_name()));
        }
    }

    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        self.read_tamable_nbt(nbt);
        if nbt.get_bool("IsTame") == Some(true) {
            // Older Pumpkin saves kept this as a flag.
            self.set_tame(true);
        }
        if nbt.get_bool("Saddled") == Some(true) && !self.is_saddled() {
            // Older Pumpkin saves kept the saddle as a flag.
            self.mob_entity.set_item_slot_and_drop_when_killed(
                &EquipmentSlot::SADDLE,
                ItemStack::new(1, &Item::SADDLE),
            );
        }
        if let Some(variant) = nbt.get_string("variant").and_then(|name| {
            ZombieNautilusVariant::from_name(name.trim_start_matches("minecraft:"))
        }) {
            self.set_variant(variant);
        }
    }

    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    fn mob_init_data_tracker(&self) {
        let entity = self.get_entity();
        entity.set_synced_data(
            pumpkin_data::tracked_data::nautilus::DASH,
            self.is_dashing(),
        );
        if self.is_zombie() {
            entity.set_synced_data(
                pumpkin_data::tracked_data::zombie_nautilus::DATA_VARIANT_ID,
                VarInt(i32::from(self.variant().id())),
            );
        }
    }

    /// Vanilla `AbstractNautilus.tick` and `Nautilus.baseTick`; the client draws the bubbles.
    fn mob_tick(&self, caller: &dyn EntityBase) {
        if self.mob_handles_own_air_supply() {
            self.handle_air_supply(caller);
        }
        self.apply_effects(&self.get_entity().world.load());
        self.tick_dash();
    }

    /// Vanilla `AbstractNautilus.mobInteract`.
    fn mob_interact(&self, player: &Arc<Player>, item_stack: &mut ItemStack) -> bool {
        self.mob_entity
            .persistence_required
            .store(true, Ordering::Relaxed);
        if self.is_baby() {
            return self.animal_interact(player, item_stack, self.eat_sound());
        }
        let sneaking = player.get_entity().is_sneaking();
        if self.is_tame() && sneaking {
            self.open_custom_inventory_screen(player);
            return true;
        }
        let is_food = self.is_food(item_stack);
        if !item_stack.is_empty() {
            if !self.is_tame() && is_food {
                self.use_player_item(player, item_stack);
                self.try_to_tame(player);
                return true;
            }
            let living = &self.mob_entity.living_entity;
            if is_food && living.health.load() < living.get_max_health() {
                self.feed_to_heal(player, item_stack);
                return true;
            }
            if self.is_tame()
                && (crate::entity::mob::saddle::try_equip_saddle(self, player, item_stack)
                    || self.try_equip_armor(player, item_stack))
            {
                return true;
            }
        }
        if self.is_tame() && !sneaking && !is_food {
            return self.start_riding(player);
        }
        !self.is_zombie() && self.animal_interact(player, item_stack, self.eat_sound())
    }

    /// Vanilla `AbstractNautilus.canUseSlot(SADDLE)`: a tamed adult.
    fn can_be_saddled(&self) -> bool {
        let living = &self.mob_entity.living_entity;
        living.entity.is_alive() && !living.is_baby() && self.is_tame()
    }

    /// Vanilla `AbstractNautilus.canUseSlot(BODY)`, the saddle's rule.
    fn can_use_body_slot(&self) -> bool {
        self.can_be_saddled()
    }

    /// Vanilla `AbstractNautilus.openCustomInventoryScreen`.
    fn open_custom_inventory_screen(&self, player: &Arc<Player>) -> bool {
        let entity = self.get_entity();
        if (!entity.has_passengers() || entity.has_passenger(player.entity_id())) && self.is_tame()
        {
            super::horse_inventory::open_nautilus_screen(self, player);
        }
        true
    }

    /// Vanilla `AbstractNautilus.getEquipSound`.
    fn saddle_sound(&self) -> Sound {
        if self.is_in_water() {
            Sound::ItemNautilusSaddleUnderwaterEquip
        } else {
            Sound::ItemNautilusSaddleEquip
        }
    }

    /// Vanilla `AbstractNautilus.getControllingPassenger`: a player in a saddle.
    fn mob_controlling_player(&self) -> Option<Arc<Player>> {
        if !self.is_saddled() {
            return None;
        }
        crate::entity::mob::saddle::first_passenger_player(self)
    }

    /// Vanilla `AbstractNautilus.tickRidden`.
    fn mob_tick_ridden(&self, controller: &Player) {
        let entity = self.get_entity();
        let rider = controller.get_entity();
        let yaw = entity.yaw.load();
        let turned = yaw + wrap_degrees(rider.yaw.load() - yaw) * RIDDEN_TURN_SPEED;
        entity.set_rotation(turned, rider.pitch.load() * RIDDEN_PITCH_SCALE);
        entity.body_yaw.store(turned);
        entity.head_yaw.store(turned);
    }

    /// Vanilla `AbstractNautilus.handleStartJump`: the rider's dash; the client pushes it.
    fn mob_start_rider_jump(&self, _strength: i32) {
        if !self.is_saddled() || self.dash_cooldown.load(Ordering::Relaxed) > 0 {
            return;
        }
        self.play_entity_sound(self.dash_sound(), 1.0, 1.0);
        let entity = self.get_entity();
        entity.world.load().emit_game_event(
            pumpkin_data::game_event::GameEvent::EntityAction.name(),
            entity.pos.load(),
        );
        self.dash_cooldown
            .store(DASH_COOLDOWN_TICKS, Ordering::Relaxed);
        self.set_dashing(true);
    }
}
