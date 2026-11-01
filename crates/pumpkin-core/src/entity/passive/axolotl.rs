use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicI32, Ordering},
};

use pumpkin_data::damage::DamageType;
use pumpkin_data::data_component_impl::{AxolotlVariantImpl, BucketEntityDataImpl};
use pumpkin_data::effect::StatusEffect;
use pumpkin_data::entity::EntityType;
use pumpkin_data::environment_attribute::Activity;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::potion::Effect;
use pumpkin_data::sound::Sound;
use pumpkin_data::tag;
use pumpkin_data::tag::Taggable;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use rand::{Rng, RngExt};

use crate::entity::ai::brain::{
    Brain, BrainProvider, BrainState, BrainTick,
    activity::ActivityData,
    behavior::{
        AnimalMakeLove, CountDownCooldownTicks, FollowTemptation, GateBehavior, LookAtTargetSink,
        MoveToTargetSink, OneShot, OrderPolicy, RunningPolicy, Timed, baby_follow_adult,
        erase_memory_if, melee_attack, one_shot::trigger_if, random_stroll, run_one,
        set_entity_look_target_sometimes, set_walk_target_from_attack_target,
        set_walk_target_from_look_target, start_attacking, stop_attacking_if_target_invalid,
        timed::Behavior, try_find_liquid, utils::is_breeding,
    },
    memory::{
        MemoryModuleId, MemoryStatus, PackedMemories, position_tracker::PositionTracker, types,
    },
    sensing::SensorType,
};
use crate::entity::ai::control::{
    Control, LookControlTrait, MoveControlTrait,
    smooth_swimming_look_control::SmoothSwimmingLookControl,
    smooth_swimming_move_control::SmoothSwimmingMoveControl,
};
use crate::entity::ai::goal::try_find_water::TryFindWaterGoal;
use crate::entity::ai::pathfinder::{Navigator, node::PathType};
use crate::entity::bucketable::{
    Bucketable, bucket_mob_pickup, load_age_from_bucket_tag, save_age_to_bucket_tag,
};
use crate::entity::mob::{requires_custom_persistence_base, spawn::SpawnGroupData};
use crate::entity::{
    Entity, EntityBase,
    ageable::{AgeableData, AgeableMob},
    mob::{Mob, MobEntity},
    passive::animal::Animal,
    player::Player,
};
use crate::world::World;
use crate::world::level_accessor::LevelAccessor;

/// Vanilla `Axolotl.TOTAL_PLAYDEAD_TIME`.
const TOTAL_PLAYDEAD_TIME: i32 = 200;
/// Vanilla `Axolotl.AXOLOTL_TOTAL_AIR_SUPPLY`.
const TOTAL_AIR_SUPPLY: i32 = 6000;
/// Vanilla `Axolotl.REHYDRATE_AIR_SUPPLY`.
const REHYDRATE_AIR_SUPPLY: i32 = 1800;
/// Vanilla `Axolotl.RARE_VARIANT_CHANCE`.
const RARE_VARIANT_CHANCE: i32 = 1200;
/// Vanilla `Axolotl.PLAYER_REGEN_DETECTION_RANGE`.
const PLAYER_REGEN_DETECTION_RANGE: f64 = 20.0;
/// Vanilla `Axolotl.REGEN_BUFF_BASE_DURATION`.
const REGEN_BUFF_BASE_DURATION: i32 = 100;
/// Vanilla `Axolotl.REGEN_BUFF_MAX_DURATION`.
const REGEN_BUFF_MAX_DURATION: i32 = 2400;
/// Vanilla `AxolotlAi`: two minutes without hunting after a fight.
const HUNTING_COOLDOWN: i64 = 2400;
/// Air at which a mob out of water takes damage (vanilla `shouldTakeDrowningDamage`).
const DRYING_OUT_AIR: i32 = -20;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(i32)]
pub enum AxolotlVariant {
    #[default]
    Lucy = 0,
    Wild = 1,
    Gold = 2,
    Cyan = 3,
    Blue = 4,
}

impl AxolotlVariant {
    #[must_use]
    pub const fn from_id(id: i32) -> Self {
        match id {
            1 => Self::Wild,
            2 => Self::Gold,
            3 => Self::Cyan,
            4 => Self::Blue,
            _ => Self::Lucy,
        }
    }

    #[must_use]
    pub const fn id(self) -> i32 {
        self as i32
    }

    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Lucy => "lucy",
            Self::Wild => "wild",
            Self::Gold => "gold",
            Self::Cyan => "cyan",
            Self::Blue => "blue",
        }
    }

    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        let name = name.strip_prefix("minecraft:").unwrap_or(name);
        [Self::Lucy, Self::Wild, Self::Gold, Self::Cyan, Self::Blue]
            .into_iter()
            .find(|variant| variant.name() == name)
    }

    /// Vanilla `Variant.getCommonSpawnVariant`.
    fn common<R: Rng>(rng: &mut R) -> Self {
        Self::from_id(rng.random_range(0..4))
    }
}

/// An axolotl, hunting in the water and playing dead when hurt.
///
/// Wiki: <https://minecraft.wiki/w/Axolotl>
pub struct AxolotlEntity {
    pub mob_entity: MobEntity,
    pub ageable_data: AgeableData,
    pub variant: AtomicI32,
    pub playing_dead: AtomicBool,
    pub from_bucket: AtomicBool,
}

fn as_axolotl(mob: &dyn Mob) -> Option<&AxolotlEntity> {
    mob.cast_any().downcast_ref::<AxolotlEntity>()
}

fn is_playing_dead(mob: &dyn Mob) -> bool {
    as_axolotl(mob).is_some_and(AxolotlEntity::is_playing_dead)
}

impl AxolotlEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let mut mob_entity = MobEntity::new(entity);
        Self::set_up_movement(&mut mob_entity);
        mob_entity
            .living_entity
            .entity
            .air_supply
            .store(TOTAL_AIR_SUPPLY, Ordering::Relaxed);
        Arc::new(Self {
            mob_entity,
            ageable_data: AgeableData::default(),
            variant: AtomicI32::new(AxolotlVariant::default().id()),
            playing_dead: AtomicBool::new(false),
            from_bucket: AtomicBool::new(false),
        })
    }

    /// Vanilla `Axolotl` constructor and `createNavigation`.
    fn set_up_movement(mob_entity: &mut MobEntity) {
        *mob_entity
            .move_control
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Box::new(AxolotlMoveControl(
            SmoothSwimmingMoveControl::new(85, 10, 0.1, 0.5, false),
        ));
        *mob_entity
            .look_control
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            Box::new(AxolotlLookControl(SmoothSwimmingLookControl::new(20)));
        let navigator = mob_entity
            .navigator
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *navigator = Navigator::amphibious(false);
        navigator.set_pathfinding_malus(PathType::Water, 0.0);
    }

    #[must_use]
    pub fn get_variant(&self) -> AxolotlVariant {
        AxolotlVariant::from_id(self.variant.load(Ordering::Relaxed))
    }

    pub fn set_variant(&self, variant: AxolotlVariant) {
        self.variant.store(variant.id(), Ordering::Relaxed);
        self.get_entity().set_synced_data(
            pumpkin_data::tracked_data::axolotl::DATA_VARIANT,
            VarInt(variant.id()),
        );
    }

    #[must_use]
    pub fn is_playing_dead(&self) -> bool {
        self.playing_dead.load(Ordering::Relaxed)
    }

    pub fn set_playing_dead(&self, playing_dead: bool) {
        if self.playing_dead.swap(playing_dead, Ordering::Relaxed) != playing_dead {
            self.get_entity().set_synced_data(
                pumpkin_data::tracked_data::axolotl::DATA_PLAYING_DEAD,
                playing_dead,
            );
        }
    }

    /// Vanilla `Axolotl.rehydrate`: a splash of water gives some air back.
    pub fn rehydrate(&self) {
        let entity = self.get_entity();
        let air = entity.air_supply.load(Ordering::Relaxed) + REHYDRATE_AIR_SUPPLY;
        entity.set_air_supply(air.min(TOTAL_AIR_SUPPLY));
    }

    /// Vanilla `Axolotl.handleAirSupply`: dries out on land, 2 damage a second at the end.
    fn handle_air_supply(&self, caller: &dyn EntityBase) {
        let entity = self.get_entity();
        if entity.is_alive() && !entity.is_in_water_or_rain() {
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

    /// The part of vanilla `Axolotl.hurtServer` before the damage: a coin toss on a hit by
    /// something while in the water, likelier the harder the hit, starts playing dead.
    fn maybe_play_dead(&self, amount: f32, has_attacker: bool) {
        let living = &self.mob_entity.living_entity;
        let health = living.health.load();
        let wants_to = {
            let mut rng = self.get_random();
            !self.mob_entity.is_no_ai()
                && rng.random_range(0..3) == 0
                && ((rng.random_range(0..3) as f32) < amount
                    || health / living.get_max_health() < 0.5)
        };
        if wants_to
            && amount < health
            && self.get_entity().is_in_water()
            && has_attacker
            && !self.is_playing_dead()
        {
            self.mob_entity.with_brain(self, |brain| {
                brain.set(types::PLAY_DEAD_TICKS, TOTAL_PLAYDEAD_TIME);
            });
        }
    }

    /// Vanilla `Axolotl.applySupportingEffects`: the player who helped gets regeneration and
    /// loses mining fatigue.
    fn apply_supporting_effects(player: &dyn EntityBase) {
        let Some(living) = player.get_living_entity() else {
            return;
        };
        let current = living.get_effect(&StatusEffect::REGENERATION);
        if current
            .as_ref()
            .is_none_or(|effect| effect.duration != -1 && effect.duration < REGEN_BUFF_MAX_DURATION)
        {
            let previous = current.map_or(0, |effect| effect.duration);
            living.add_effect(Effect {
                effect_type: &StatusEffect::REGENERATION,
                duration: (REGEN_BUFF_BASE_DURATION + previous).min(REGEN_BUFF_MAX_DURATION),
                amplifier: 0,
                ambient: false,
                show_particles: true,
                show_icon: true,
                blend: false,
            });
        }
        living.remove_effect(&StatusEffect::MINING_FATIGUE);
    }
}

/// Vanilla `Axolotl.onStopAttacking`: when the target died to a player nearby, that player gets
/// the axolotl's support.
fn on_stop_attacking(mob: &dyn Mob, target: &Arc<dyn EntityBase>) {
    let Some(living) = target.get_living_entity() else {
        return;
    };
    if !living.is_dead_or_dying() {
        return;
    }
    let Some(killer) = living.get_last_hurt_by_mob() else {
        return;
    };
    if killer.get_player().is_some()
        && mob
            .get_entity()
            .bounding_box
            .load()
            .expand_all(PLAYER_REGEN_DETECTION_RANGE)
            .intersects(&killer.get_entity().bounding_box.load())
    {
        AxolotlEntity::apply_supporting_effects(killer.as_ref());
    }
}

/// Vanilla `Axolotl.AxolotlMoveControl`: not while playing dead.
struct AxolotlMoveControl(SmoothSwimmingMoveControl);

impl Control for AxolotlMoveControl {}

impl MoveControlTrait for AxolotlMoveControl {
    fn tick(&mut self, mob: &dyn Mob) {
        if !is_playing_dead(mob) {
            self.0.tick(mob);
        }
    }

    fn set_wanted_position(&mut self, x: f64, y: f64, z: f64, speed_modifier: f64) {
        self.0.set_wanted_position(x, y, z, speed_modifier);
    }

    fn strafe(&mut self, forward: f32, right: f32) {
        self.0.strafe(forward, right);
    }

    fn has_wanted(&self) -> bool {
        self.0.has_wanted()
    }

    fn get_speed_modifier(&self) -> f64 {
        self.0.get_speed_modifier()
    }

    fn get_wanted_position(&self) -> Vector3<f64> {
        self.0.get_wanted_position()
    }
}

/// Vanilla `Axolotl.AxolotlLookControl`: not while playing dead.
struct AxolotlLookControl(SmoothSwimmingLookControl);

impl Control for AxolotlLookControl {}

impl LookControlTrait for AxolotlLookControl {
    fn tick(&mut self, mob: &dyn Mob) {
        if !is_playing_dead(mob) {
            self.0.tick(mob);
        }
    }

    fn look_at_with_range(
        &mut self,
        x: f64,
        y: f64,
        z: f64,
        max_yaw_change: f32,
        max_pitch_change: f32,
    ) {
        self.0
            .look_at_with_range(x, y, z, max_yaw_change, max_pitch_change);
    }

    fn wanted_position(&self) -> Vector3<f64> {
        self.0.wanted_position()
    }

    fn is_looking_at_target(&self) -> bool {
        self.0.is_looking_at_target()
    }
}

impl AgeableMob for AxolotlEntity {
    fn get_ageable_data(&self) -> &AgeableData {
        &self.ageable_data
    }
}

impl Animal for AxolotlEntity {
    fn is_food(&self, item_stack: &ItemStack) -> bool {
        item_stack.item.has_tag(&tag::Item::MINECRAFT_AXOLOTL_FOOD)
    }

    /// Vanilla `Axolotl.getBreedOffspring`: a parent's colour, rarely the blue one.
    fn get_breed_offspring(&self, partner: &dyn EntityBase) -> Arc<dyn EntityBase> {
        let entity = self.get_entity();
        let baby = Self::new(Entity::new(
            entity.world.load_full(),
            entity.pos.load(),
            &EntityType::AXOLOTL,
        ));
        let mut rng = self.get_random();
        let variant = if rng.random_range(0..RARE_VARIANT_CHANCE) == 0 {
            AxolotlVariant::Blue
        } else if rng.random_bool(0.5) {
            self.get_variant()
        } else {
            partner
                .cast_any()
                .downcast_ref::<Self>()
                .map_or_else(|| self.get_variant(), Self::get_variant)
        };
        baby.set_variant(variant);
        baby.mob_entity.set_persistence_required();
        baby
    }

    /// Vanilla `Axolotl.usePlayerItem`: the bucket of tropical fish leaves a water bucket.
    fn use_player_item(&self, player: &Player, item_stack: &mut ItemStack) {
        if item_stack.item == &Item::TROPICAL_FISH_BUCKET {
            crate::item::items::bucket::fill_bucket_stack(item_stack, player, &Item::WATER_BUCKET);
        } else {
            item_stack.decrement_unless_creative(player.gamemode.load(), 1);
        }
    }
}

impl Bucketable for AxolotlEntity {
    fn is_from_bucket(&self) -> bool {
        self.from_bucket.load(Ordering::Relaxed)
    }

    fn set_from_bucket(&self, from_bucket: bool) {
        self.from_bucket.store(from_bucket, Ordering::Relaxed);
        self.get_entity().set_synced_data(
            pumpkin_data::tracked_data::axolotl::FROM_BUCKET,
            from_bucket,
        );
    }

    fn bucket_item(&self) -> &'static Item {
        &Item::AXOLOTL_BUCKET
    }

    fn pickup_sound(&self) -> Sound {
        Sound::ItemBucketFillAxolotl
    }

    /// Vanilla `Axolotl.saveToBucketTag`: age and hunting cooldown come along.
    fn save_to_bucket_tag(&self, bucket: &mut ItemStack) {
        self.save_default_data_to_bucket_tag(bucket);
        bucket.set_data_component(AxolotlVariantImpl {
            value: self.get_variant().name().into(),
        });
        save_age_to_bucket_tag(self, bucket);
        let cooldown = self.mob_entity.with_brain(self, |brain| {
            brain.time_until_expiry(types::HAS_HUNTING_COOLDOWN.id())
        });
        if let Some(cooldown) = cooldown {
            let mut nbt = bucket
                .get_data_component::<BucketEntityDataImpl>()
                .and_then(|data| data.nbt.clone())
                .unwrap_or_default();
            nbt.put_long("HuntingCooldown", cooldown);
            bucket.set_data_component(BucketEntityDataImpl { nbt: Some(nbt) });
        }
    }

    /// Vanilla `Axolotl.loadFromBucketTag`.
    fn load_from_bucket_tag(&self, nbt: &NbtCompound) {
        self.load_default_data_from_bucket_tag(nbt);
        load_age_from_bucket_tag(self, nbt);
        let cooldown = nbt.get_long("HuntingCooldown");
        self.mob_entity.with_brain(self, |brain| match cooldown {
            Some(ticks) => brain.set_with_expiry(types::HAS_HUNTING_COOLDOWN, true, ticks),
            None => brain.erase(types::HAS_HUNTING_COOLDOWN.id()),
        });
    }
}

impl Mob for AxolotlEntity {
    /// Vanilla `Axolotl.getAmbientSound` and `playAmbientSound`.
    fn get_ambient_sound(&self) -> Option<Sound> {
        if self.is_playing_dead() {
            return None;
        }
        Some(if self.get_entity().is_in_water() {
            Sound::EntityAxolotlIdleWater
        } else {
            Sound::EntityAxolotlIdleAir
        })
    }

    /// Vanilla `Axolotl.checkSpawnObstruction`: only entities obstruct, the water does not.
    fn check_spawn_obstruction(&self, level: &dyn LevelAccessor) -> bool {
        level.is_unobstructed(&self.get_mob_entity().living_entity.entity)
    }

    /// Vanilla `Axolotl.removeWhenFarAway`: only wild, unnamed axolotls despawn.
    fn remove_when_far_away(&self, _distance_sq: f64) -> bool {
        !self.is_from_bucket() && self.get_entity().custom_name.load().is_none()
    }

    /// Vanilla `Axolotl.requiresCustomPersistence`: a bucketed axolotl stays.
    fn requires_custom_persistence(&self) -> bool {
        requires_custom_persistence_base(self) || self.is_from_bucket()
    }

    /// Vanilla `Axolotl.getWalkTargetValue`: no place is better than another.
    fn get_walk_target_value(&self, _pos: &BlockPos) -> f32 {
        0.0
    }

    /// Vanilla `Axolotl.getMaxHeadYRot`.
    fn get_max_head_rotation(&self) -> f32 {
        1.0
    }

    /// Vanilla `Axolotl.isPushedByFluid`.
    fn mob_is_pushed_by_fluids(&self) -> bool {
        false
    }

    /// Vanilla `Axolotl.canBeSeenAsEnemy`: playing dead works on mobs.
    fn mob_can_be_seen_as_enemy(&self) -> bool {
        !self.is_playing_dead()
    }

    fn mob_max_air_supply(&self) -> i32 {
        TOTAL_AIR_SUPPLY
    }

    /// Vanilla `Axolotl.baseTick` replaces what `LivingEntity.baseTick` did to its air, unless
    /// it has no AI.
    fn mob_handles_own_air_supply(&self) -> bool {
        !self.mob_entity.is_no_ai()
    }

    fn mob_tick(&self, caller: &dyn EntityBase) {
        if self.mob_handles_own_air_supply() {
            self.handle_air_supply(caller);
        }
    }

    /// Vanilla `Axolotl.travelInWater`: no gravity, no fluid speed rules.
    fn travel_in_water(&self, caller: &dyn EntityBase, _gravity: f64, _falling: bool) {
        let living = &self.mob_entity.living_entity;
        let entity = &living.entity;
        entity.update_velocity_from_input(
            living.movement_input.load(),
            f64::from(living.get_speed()),
        );
        living.make_move(caller);
        entity.velocity.store(entity.velocity.load() * 0.9);
    }

    /// The vanilla `Axolotl.playAttackSound` that `Mob.doHurtTarget` plays.
    fn on_attack(&self, _target: &dyn EntityBase) {
        self.play_entity_sound(Sound::EntityAxolotlAttack, 1.0, 1.0);
    }

    fn modify_incoming_damage_with_context(
        &self,
        amount: f32,
        _damage_type: DamageType,
        source: Option<&dyn EntityBase>,
        cause: Option<&dyn EntityBase>,
    ) -> f32 {
        self.maybe_play_dead(amount, source.is_some() || cause.is_some());
        amount
    }

    fn make_brain(&self, packed: &PackedMemories) -> Brain {
        make_brain(self, packed)
    }

    /// Vanilla `Axolotl.customServerAiStep`.
    fn custom_server_ai_step(&self, _caller: &dyn EntityBase) {
        self.mob_entity.tick_brain(self);
        let (playing_dead, target) = self.mob_entity.with_brain(self, |brain| {
            update_activity(brain);
            (
                brain
                    .get(types::PLAY_DEAD_TICKS)
                    .is_some_and(|ticks| *ticks > 0),
                brain.get(types::ATTACK_TARGET).cloned(),
            )
        });
        // Vanilla `getTarget` reads the brain.
        self.mob_entity.set_target(target);
        if !self.mob_entity.is_no_ai() {
            self.set_playing_dead(playing_dead);
        }
    }

    /// Vanilla `Axolotl.finalizeSpawn`: a group shares two colours, the third on are babies.
    fn finalize_spawn(
        &self,
        _world: &Arc<World>,
        group_data: Option<SpawnGroupData>,
    ) -> Option<SpawnGroupData> {
        let (variants, group_size) = if let Some(SpawnGroupData::Axolotl {
            variants,
            group_size,
        }) = group_data
        {
            (variants, group_size)
        } else {
            let mut rng = self.get_random();
            (
                [
                    AxolotlVariant::common(&mut rng),
                    AxolotlVariant::common(&mut rng),
                ],
                0,
            )
        };
        let variant = variants[usize::from(self.get_random().random_bool(0.5))];
        self.set_variant(variant);
        if group_size >= 2 {
            self.set_age(-24000);
        }
        self.finalize_ageable_spawn(Some(SpawnGroupData::Ageable {
            group_size,
            should_spawn_baby: false,
            baby_spawn_chance: 0.0,
        }));
        Some(SpawnGroupData::Axolotl {
            variants,
            group_size: group_size + 1,
        })
    }

    fn mob_set_variant_name(&self, name: &str) {
        if let Some(variant) = AxolotlVariant::from_name(name) {
            self.set_variant(variant);
        }
    }

    fn as_ageable(&self) -> Option<&dyn AgeableMob> {
        Some(self)
    }

    fn as_animal(&self) -> Option<&dyn Animal> {
        Some(self)
    }

    fn as_bucketable(&self) -> Option<&dyn Bucketable> {
        Some(self)
    }

    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        nbt.put_int("Variant", self.get_variant().id());
        nbt.put_bool("FromBucket", self.is_from_bucket());
    }

    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        self.set_variant(AxolotlVariant::from_id(nbt.get_int("Variant").unwrap_or(0)));
        self.set_from_bucket(nbt.get_bool("FromBucket").unwrap_or(false));
    }

    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    fn mob_init_data_tracker(&self) {
        let entity = self.get_entity();
        let is_baby = entity.age.load(Ordering::Relaxed) < 0;
        if is_baby {
            entity.set_synced_data(pumpkin_data::tracked_data::axolotl::DATA_BABY_ID, true);
        }
        entity.set_synced_data(
            pumpkin_data::tracked_data::axolotl::DATA_VARIANT,
            VarInt(self.get_variant().id()),
        );
        entity.set_synced_data(
            pumpkin_data::tracked_data::axolotl::DATA_PLAYING_DEAD,
            self.is_playing_dead(),
        );
        entity.set_synced_data(
            pumpkin_data::tracked_data::axolotl::FROM_BUCKET,
            self.is_from_bucket(),
        );
    }

    /// Vanilla `Axolotl.mobInteract`.
    /// Vanilla `Axolotl.mobInteract`: the water bucket picks it up first.
    fn mob_interact_with_hand(
        &self,
        player: &Arc<Player>,
        item_stack: &mut ItemStack,
        _hand: pumpkin_util::Hand,
    ) -> Option<bool> {
        bucket_mob_pickup(player, item_stack, self)
    }

    fn mob_interact(&self, player: &Arc<Player>, item_stack: &mut ItemStack) -> bool {
        self.animal_interact(player, item_stack, Sound::EntityAxolotlIdleAir)
    }
}

const MEMORIES: [MemoryModuleId; 0] = [];

/// Vanilla `AxolotlAi`.
fn make_brain(axolotl: &AxolotlEntity, packed: &PackedMemories) -> Brain {
    BrainProvider {
        memory_types: &MEMORIES,
        sensor_types: &[
            SensorType::NearestLivingEntities,
            SensorType::NearestAdult,
            SensorType::HurtBy,
            SensorType::AxolotlAttackables,
            SensorType::FoodTemptations,
        ],
        activities,
    }
    .make_brain(axolotl, packed)
}

/// Vanilla `AxolotlAi.updateActivity`: playing dead ends only by `ValidatePlayDead`, and the
/// end of a fight stops the hunting for a while.
fn update_activity(brain: &mut BrainState) {
    let old = brain.get_active_non_core_activity();
    if old == Some(Activity::PlayDead) {
        return;
    }
    brain.set_active_activity_to_first_valid(&[
        Activity::PlayDead,
        Activity::Fight,
        Activity::Idle,
    ]);
    if old == Some(Activity::Fight) && brain.get_active_non_core_activity() != Some(Activity::Fight)
    {
        brain.set_with_expiry(types::HAS_HUNTING_COOLDOWN, true, HUNTING_COOLDOWN);
    }
}

fn is_in_water(mob: &dyn Mob) -> bool {
    mob.get_entity().is_in_water()
}

/// Vanilla `AxolotlAi.getSpeedModifierChasing` and `getSpeedModifierFollowingAdult`.
fn chasing_speed(mob: &dyn Mob) -> f32 {
    if is_in_water(mob) { 0.6 } else { 0.15 }
}

/// Vanilla `AxolotlAi.getSpeedModifier`.
fn idle_speed(mob: &dyn Mob) -> f32 {
    if is_in_water(mob) { 0.5 } else { 0.15 }
}

/// Vanilla `AxolotlAi.findNearestValidAttackTarget`.
fn find_attack_target(tick: &BrainTick<'_>) -> Option<Arc<dyn EntityBase>> {
    if is_breeding(tick.state) {
        return None;
    }
    tick.state.get(types::NEAREST_ATTACKABLE).cloned()
}

/// Vanilla `AxolotlAi.canSetWalkTargetFromLookTarget`: stays in or out of the water.
fn look_target_in_same_medium(tick: &BrainTick<'_>) -> bool {
    let entity = tick.mob.get_entity();
    tick.state
        .get(types::LOOK_TARGET)
        .is_some_and(|target: &Arc<dyn PositionTracker>| {
            TryFindWaterGoal::is_water(&entity.world.load(), &target.current_block_position())
                == entity.is_in_water()
        })
}

/// Vanilla `ValidatePlayDead`.
fn validate_play_dead() -> OneShot {
    OneShot::new(
        "ValidatePlayDead",
        vec![
            (types::PLAY_DEAD_TICKS.id(), MemoryStatus::ValuePresent),
            (types::HURT_BY_ENTITY.id(), MemoryStatus::Registered),
        ],
        |tick| {
            let ticks = tick.state.get(types::PLAY_DEAD_TICKS).copied().unwrap_or(0);
            if ticks <= 0 {
                tick.state.erase(types::PLAY_DEAD_TICKS.id());
                tick.state.erase(types::HURT_BY_ENTITY.id());
                tick.state.use_default_activity();
            } else {
                tick.state.set(types::PLAY_DEAD_TICKS, ticks - 1);
            }
            true
        },
    )
}

/// Vanilla `PlayDead`: stops in place and heals, as long as it stays in the water.
#[derive(Default)]
struct PlayDead;

const PLAY_DEAD_CONDITIONS: [(MemoryModuleId, MemoryStatus); 2] = [
    (types::PLAY_DEAD_TICKS.id(), MemoryStatus::ValuePresent),
    (types::HURT_BY_ENTITY.id(), MemoryStatus::ValuePresent),
];

impl Behavior for PlayDead {
    fn entry_conditions(&self) -> &[(MemoryModuleId, MemoryStatus)] {
        &PLAY_DEAD_CONDITIONS
    }

    fn min_duration(&self) -> i32 {
        TOTAL_PLAYDEAD_TIME
    }

    fn max_duration(&self) -> i32 {
        TOTAL_PLAYDEAD_TIME
    }

    fn check_extra_start_conditions(&mut self, tick: &mut BrainTick<'_>) -> bool {
        is_in_water(tick.mob)
    }

    fn can_still_use(&mut self, tick: &mut BrainTick<'_>) -> bool {
        is_in_water(tick.mob) && tick.state.has_memory_value(types::PLAY_DEAD_TICKS.id())
    }

    fn start(&mut self, tick: &mut BrainTick<'_>) {
        tick.state.erase(types::WALK_TARGET.id());
        tick.state.erase(types::LOOK_TARGET.id());
        tick.mob.get_mob_entity().living_entity.add_effect(Effect {
            effect_type: &StatusEffect::REGENERATION,
            duration: TOTAL_PLAYDEAD_TIME,
            amplifier: 0,
            ambient: false,
            show_particles: true,
            show_icon: true,
            blend: false,
        });
    }

    fn debug_name(&self) -> &'static str {
        "PlayDead"
    }
}

fn activities(_mob: &dyn Mob) -> Vec<ActivityData> {
    vec![
        ActivityData::with_priority_start(
            Activity::Core,
            0,
            vec![
                Box::new(Timed::new(LookAtTargetSink::new(45, 90))),
                Box::new(Timed::new(MoveToTargetSink::default())),
                Box::new(validate_play_dead()),
                Box::new(CountDownCooldownTicks::new(
                    types::TEMPTATION_COOLDOWN_TICKS,
                )),
            ],
        ),
        idle_activity(),
        ActivityData::with_gate_memory(
            Activity::Fight,
            0,
            vec![
                Box::new(stop_attacking_if_target_invalid(on_stop_attacking)),
                Box::new(set_walk_target_from_attack_target(chasing_speed)),
                Box::new(melee_attack(20)),
                Box::new(erase_memory_if(types::ATTACK_TARGET.id(), is_breeding)),
            ],
            types::ATTACK_TARGET.id(),
        ),
        ActivityData::new(
            Activity::PlayDead,
            vec![
                (0, Box::new(Timed::new(PlayDead))),
                (
                    1,
                    Box::new(erase_memory_if(types::PLAY_DEAD_TICKS.id(), is_breeding)),
                ),
            ],
            vec![(types::PLAY_DEAD_TICKS.id(), MemoryStatus::ValuePresent)],
            vec![types::PLAY_DEAD_TICKS.id()],
        ),
    ]
}

fn idle_activity() -> ActivityData {
    let wander = GateBehavior::new(
        "AxolotlWander",
        vec![(types::WALK_TARGET.id(), MemoryStatus::ValueAbsent)],
        Vec::new(),
        OrderPolicy::Ordered,
        RunningPolicy::TryAll,
        vec![
            (Box::new(random_stroll::swim(0.5)), 2),
            (
                Box::new(random_stroll::stroll(0.15).only_if(|mob| !is_in_water(mob))),
                2,
            ),
            (
                Box::new(set_walk_target_from_look_target::create_if(
                    look_target_in_same_medium,
                    idle_speed,
                    3,
                )),
                3,
            ),
            (
                Box::new(trigger_if("InWater", Vec::new(), |tick| {
                    is_in_water(tick.mob)
                })),
                5,
            ),
            (
                Box::new(trigger_if("OnGround", Vec::new(), |tick| {
                    tick.mob.get_entity().on_ground.load(Ordering::Relaxed)
                })),
                5,
            ),
        ],
    );
    ActivityData::with_pairs(
        Activity::Idle,
        vec![
            (
                0,
                Box::new(set_entity_look_target_sometimes::create_for(
                    &EntityType::PLAYER,
                    6.0,
                    30,
                    60,
                )),
            ),
            (
                1,
                Box::new(Timed::new(AnimalMakeLove::new().with_speed(0.2))),
            ),
            (
                2,
                Box::new(run_one(vec![
                    (
                        Box::new(Timed::new(FollowTemptation::new(idle_speed, |_| 2.5))),
                        1,
                    ),
                    (Box::new(baby_follow_adult(5, 16, chasing_speed)), 1),
                ])),
            ),
            (3, Box::new(start_attacking(find_attack_target))),
            (
                3,
                Box::new(try_find_liquid(
                    6,
                    0.15,
                    &tag::Fluid::MINECRAFT_AXOLOTL_TRIES_TO_FIND,
                )),
            ),
            (4, Box::new(wander)),
        ],
    )
}
