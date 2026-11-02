use crate::world::level_accessor::LevelAccessor;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicI32, Ordering},
};

use crossbeam::atomic::AtomicCell;
use pumpkin_data::BlockState;
use pumpkin_data::attributes::Attributes;
use pumpkin_data::damage::DamageType;
use pumpkin_data::data_component_impl::EquipmentSlot;
use pumpkin_data::game_event::GameEvent;
use pumpkin_data::particle::Particle;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::tag::{self, Taggable};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_protocol::java::client::play::CEntityStatus;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_world::clip::{ClipBlock, ClipContext, ClipFluid};

use super::creaking_ai;
use crate::block::entities::creaking_heart::with_heart;
use crate::entity::ai::brain::memory::{MemoryModuleId, PackedMemories, types};
use crate::entity::ai::brain::sensing::SensorType;
use crate::entity::ai::brain::{Brain, BrainProvider};
use crate::entity::ai::pathfinder::node::PathType;
use crate::entity::{
    Entity, EntityBase,
    mob::{Mob, MobEntity},
    player::Player,
};

pub const ATTACK_ANIMATION_DURATION: i32 = 15;
pub const MAX_HEALTH: f32 = 1.0;
pub const ATTACK_DAMAGE: f32 = 3.0;
pub const FOLLOW_RANGE: f32 = 32.0;
pub const ACTIVATION_RANGE_SQ: f64 = 144.0;
pub const ATTACK_INTERVAL: i64 = 40;
pub const MOVEMENT_SPEED_WHEN_FIGHTING: f32 = 0.4;
pub const SPEED_MULTIPLIER_WHEN_IDLING: f32 = 0.3;
pub const INVULNERABILITY_ANIMATION_DURATION: i32 = 8;
pub const TWITCH_DEATH_DURATION: i32 = 45;
pub const MAX_PLAYER_STUCK_COUNTER: i32 = 4;
/// Vanilla's cone in `isLookingAtMe(player, 0.5, false, ...)`.
const LOOK_CONE: f64 = 0.5;

const MEMORIES: [MemoryModuleId; 0] = [];

pub struct CreakingEntity {
    pub mob_entity: MobEntity,
    can_move: AtomicBool,
    is_active: AtomicBool,
    is_tearing_down: AtomicBool,
    home_pos: AtomicCell<Option<BlockPos>>,
    attack_animation_remaining_ticks: AtomicI32,
    invulnerability_animation_remaining_ticks: AtomicI32,
    player_stuck_counter: AtomicI32,
    death_time: AtomicI32,
}

impl CreakingEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let mut mob_entity = MobEntity::new(entity);
        mob_entity
            .navigator
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .set_can_float(true);
        let creaking = Self {
            mob_entity,
            can_move: AtomicBool::new(true),
            is_active: AtomicBool::new(false),
            is_tearing_down: AtomicBool::new(false),
            home_pos: AtomicCell::new(None),
            attack_animation_remaining_ticks: AtomicI32::new(0),
            invulnerability_animation_remaining_ticks: AtomicI32::new(0),
            player_stuck_counter: AtomicI32::new(0),
            death_time: AtomicI32::new(0),
        };

        // Initialize attributes
        {
            let mut attributes = creaking
                .mob_entity
                .living_entity
                .attributes
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);

            if let Some(health) = attributes.get_mut(&Attributes::MAX_HEALTH.id) {
                health.base_value = f64::from(MAX_HEALTH);
                health.dirty.store(true, Ordering::Relaxed);
            }
            if let Some(speed) = attributes.get_mut(&Attributes::MOVEMENT_SPEED.id) {
                speed.base_value = f64::from(MOVEMENT_SPEED_WHEN_FIGHTING);
                speed.dirty.store(true, Ordering::Relaxed);
            }
            if let Some(damage) = attributes.get_mut(&Attributes::ATTACK_DAMAGE.id) {
                damage.base_value = f64::from(ATTACK_DAMAGE);
                damage.dirty.store(true, Ordering::Relaxed);
            }
            if let Some(follow) = attributes.get_mut(&Attributes::FOLLOW_RANGE.id) {
                follow.base_value = f64::from(FOLLOW_RANGE);
                follow.dirty.store(true, Ordering::Relaxed);
            }
            if let Some(step) = attributes.get_mut(&Attributes::STEP_HEIGHT.id) {
                step.base_value = 1.0625;
                step.dirty.store(true, Ordering::Relaxed);
            }
        }
        creaking.mob_entity.living_entity.health.store(MAX_HEALTH);

        Arc::new(creaking)
    }

    /// Vanilla `setTransient`: bound to a heart, it walks through what would hurt others.
    pub fn set_transient(&self, pos: BlockPos) {
        self.set_home_pos(Some(pos));
        let mut navigator = self
            .mob_entity
            .navigator
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        navigator.set_pathfinding_malus(PathType::DamageOther, 8.0);
        navigator.set_pathfinding_malus(PathType::PowderSnow, 8.0);
        navigator.set_pathfinding_malus(PathType::Lava, 8.0);
        navigator.set_pathfinding_malus(PathType::DamageFire, 0.0);
        navigator.set_pathfinding_malus(PathType::DangerFire, 0.0);
    }

    pub fn is_heart_bound(&self) -> bool {
        self.get_home_pos().is_some()
    }

    pub fn can_move(&self) -> bool {
        self.can_move.load(Ordering::Relaxed)
    }

    pub fn set_can_move(&self, can_move: bool) {
        self.can_move.store(can_move, Ordering::Relaxed);
        self.get_entity()
            .set_synced_data(pumpkin_data::tracked_data::creaking::CAN_MOVE, can_move);
    }

    pub fn is_active(&self) -> bool {
        self.is_active.load(Ordering::Relaxed)
    }

    pub fn set_is_active(&self, active: bool) {
        self.is_active.store(active, Ordering::Relaxed);
        self.get_entity()
            .set_synced_data(pumpkin_data::tracked_data::creaking::IS_ACTIVE, active);
    }

    pub fn is_tearing_down(&self) -> bool {
        self.is_tearing_down.load(Ordering::Relaxed)
    }

    pub fn set_tearing_down(&self) {
        self.is_tearing_down.store(true, Ordering::Relaxed);
        self.get_entity()
            .set_synced_data(pumpkin_data::tracked_data::creaking::IS_TEARING_DOWN, true);
    }

    pub fn get_home_pos(&self) -> Option<BlockPos> {
        self.home_pos.load()
    }

    pub fn set_home_pos(&self, pos: Option<BlockPos>) {
        self.home_pos.store(pos);
        if let Some(p) = pos {
            self.mob_entity.position_target.store(p);
            self.mob_entity
                .position_target_range
                .store(32, Ordering::Relaxed);
        } else {
            self.mob_entity
                .position_target_range
                .store(-1, Ordering::Relaxed);
        }
        self.get_entity()
            .set_synced_data(pumpkin_data::tracked_data::creaking::HOME_POS, pos);
    }

    /// Vanilla `Mob.stopInPlace`.
    fn stop_in_place(&self) {
        self.get_entity().velocity.store(Vector3::default());
        self.mob_entity.steered().navigator().stop();
    }

    /// Vanilla `activate`.
    pub fn activate(&self, player: &Arc<Player>) {
        self.mob_entity.with_brain(self, |brain| {
            brain.set(types::ATTACK_TARGET, player.clone() as Arc<dyn EntityBase>);
        });
        self.mob_entity.set_target(Some(player.clone()));
        self.emit_action();
        self.play_sound(Sound::EntityCreakingActivate);
        self.set_is_active(true);
    }

    /// Vanilla `deactivate`.
    pub fn deactivate(&self) {
        self.mob_entity
            .with_brain(self, |brain| brain.erase(types::ATTACK_TARGET.id()));
        self.mob_entity.set_target(None);
        self.emit_action();
        self.play_sound(Sound::EntityCreakingDeactivate);
        self.set_is_active(false);
    }

    fn emit_action(&self) {
        let entity = self.get_entity();
        entity
            .world
            .load()
            .emit_game_event(GameEvent::EntityAction.name(), entity.pos.load());
    }

    /// Vanilla `playerIsStuckInYou`.
    pub fn player_is_stuck_in_you(&self) -> bool {
        let entity = self.get_entity();
        let own_box = entity.bounding_box.load();
        let players = entity
            .world
            .load()
            .get_nearby_players(entity.pos.load(), f64::from(FOLLOW_RANGE));
        for player in players {
            let p = player.get_entity().get_eye_pos();
            if p.x >= own_box.min.x
                && p.x <= own_box.max.x
                && p.y >= own_box.min.y
                && p.y <= own_box.max.y
                && p.z >= own_box.min.z
                && p.z <= own_box.max.z
            {
                let count = self.player_stuck_counter.fetch_add(1, Ordering::Relaxed) + 1;
                return count > MAX_PLAYER_STUCK_COUNTER;
            }
        }
        self.player_stuck_counter.store(0, Ordering::Relaxed);
        false
    }

    /// Vanilla `LivingEntity.isLookingAtMe(player, 0.5, false, true, heights)`.
    pub fn is_looking_at_me(&self, player: &Player) -> bool {
        let entity = self.get_entity();
        let pos = entity.pos.load();
        let eye_y = entity.get_eye_y();
        let heights = [eye_y, pos.y + 0.5, f64::midpoint(eye_y, pos.y)];

        let viewer = player.get_entity();
        let viewer_eye = viewer.get_eye_pos();
        let pitch = f64::from(viewer.pitch.load()).to_radians();
        let yaw = f64::from(viewer.yaw.load()).to_radians();
        let look = Vector3::new(
            -yaw.sin() * pitch.cos(),
            -pitch.sin(),
            yaw.cos() * pitch.cos(),
        );
        let world = entity.world.load();
        heights.iter().any(|&height| {
            let target = Vector3::new(pos.x, height, pos.z);
            let direction = (target - viewer_eye).normalize();
            look.dot(&direction) > 1.0 - LOOK_CONE
                && world
                    .clip(&ClipContext::new(
                        viewer_eye,
                        target,
                        ClipBlock::Visual,
                        ClipFluid::None,
                    ))
                    .is_miss
        })
    }

    /// Vanilla `LivingEntity.PLAYER_NOT_WEARING_DISGUISE_ITEM` negated.
    fn is_player_disguised(player: &Player) -> bool {
        let equipment = player.living_entity.entity_equipment.try_lock();
        if let Ok(equipment) = equipment
            && let Some(head_stack) = equipment.equipment.get(&EquipmentSlot::HEAD)
            && !head_stack.is_empty()
        {
            return head_stack
                .item
                .has_tag(&tag::Item::MINECRAFT_GAZE_DISGUISE_EQUIPMENT);
        }
        false
    }

    /// Vanilla `checkCanMove`.
    pub fn check_can_move(&self) -> bool {
        let entity = self.get_entity();
        let world = entity.world.load();
        let pos = entity.pos.load();
        let active = self.is_active();

        let players = world.get_nearby_players(pos, f64::from(FOLLOW_RANGE));
        if players.is_empty() {
            if active {
                self.deactivate();
            }
            return true;
        }

        let mut has_potential_target = false;
        for player in players {
            if !self.can_attack(player.as_ref())
                || player.gamemode.load() == pumpkin_util::GameMode::Creative
                || player.gamemode.load() == pumpkin_util::GameMode::Spectator
            {
                continue;
            }
            has_potential_target = true;
            if (!active || !Self::is_player_disguised(&player)) && self.is_looking_at_me(&player) {
                if active {
                    return false;
                }
                let target_pos = player.get_entity().pos.load();
                if pos.squared_distance_to_vec(&target_pos) < ACTIVATION_RANGE_SQ {
                    self.activate(&player);
                    return false;
                }
            }
        }

        if !has_potential_target && active {
            self.deactivate();
        }

        true
    }

    /// Vanilla `tearDown`: it crumbles away.
    pub fn tear_down(&self) {
        let entity = self.get_entity();
        let world = entity.world.load();
        let bounds = entity.bounding_box.load();
        let center = Vector3::new(
            f64::midpoint(bounds.min.x, bounds.max.x),
            f64::midpoint(bounds.min.y, bounds.max.y),
            f64::midpoint(bounds.min.z, bounds.max.z),
        );
        world.spawn_particle(
            center,
            Vector3::new(0.3, 0.6, 0.3),
            0.0,
            100,
            Particle::BlockCrumble,
        );
        self.play_sound(Sound::EntityCreakingDeath);
        entity.remove();
    }

    fn play_sound(&self, sound: Sound) {
        let entity = self.get_entity();
        entity.world.load().play_sound_fine(
            sound,
            SoundCategory::Hostile,
            &entity.pos.load(),
            1.0,
            1.0,
        );
    }

    fn tick_animations(&self) {
        for counter in [
            &self.invulnerability_animation_remaining_ticks,
            &self.attack_animation_remaining_ticks,
        ] {
            if counter.load(Ordering::Relaxed) > 0 {
                counter.fetch_sub(1, Ordering::Relaxed);
            }
        }
    }

    /// The freezing part of vanilla `Creaking.aiStep`.
    fn update_can_move(&self) {
        let can_move = self.can_move();
        let now_can_move = self.check_can_move();
        if now_can_move != can_move {
            self.emit_action();
            if now_can_move {
                self.play_sound(Sound::EntityCreakingUnfreeze);
            } else {
                self.stop_in_place();
                self.play_sound(Sound::EntityCreakingFreeze);
            }
            self.set_can_move(now_can_move);
        }
    }

    /// Whether the heart it is bound to still counts it as its protector.
    fn has_heart_protection(&self, home_pos: &BlockPos) -> bool {
        let entity = self.get_entity();
        with_heart(&entity.world.load(), home_pos, |heart| {
            heart.is_protector(entity.entity_uuid)
        })
        .unwrap_or(false)
    }
}

impl Mob for CreakingEntity {
    fn make_brain(&self, packed: &PackedMemories) -> Brain {
        BrainProvider {
            memory_types: &MEMORIES,
            sensor_types: &[
                SensorType::NearestLivingEntities,
                SensorType::NearestPlayers,
            ],
            activities: creaking_ai::activities,
        }
        .make_brain(self, packed)
    }

    /// Vanilla `Creaking.customServerAiStep`.
    fn custom_server_ai_step(&self, _caller: &dyn EntityBase) {
        self.mob_entity.tick_brain(self);
        let target = self.mob_entity.with_brain(self, |brain| {
            creaking_ai::update_activity(brain);
            brain.get(types::ATTACK_TARGET).cloned()
        });
        // Vanilla `getTarget` reads the brain.
        self.mob_entity.set_target(target);
    }

    /// Frozen under a gaze it neither thinks nor moves: vanilla stops its controls, its navigation,
    /// its melee and its swimming one by one.
    fn is_mob_immobile(&self) -> bool {
        !self.can_move()
    }

    /// Vanilla `Creaking.isPushable` and its `push`.
    fn mob_is_pushable(&self) -> bool {
        self.can_move() && self.get_mob_entity().living_entity.is_pushable()
    }

    /// Vanilla `Creaking.doHurtTarget`.
    fn do_hurt_target(&self, target: &dyn EntityBase) -> bool {
        if target.get_living_entity().is_none() {
            return false;
        }
        self.attack_animation_remaining_ticks
            .store(ATTACK_ANIMATION_DURATION, Ordering::Relaxed);
        let entity = self.get_entity();
        entity.world.load().broadcast_to_chunk(
            entity.chunk_pos.load(),
            &CEntityStatus::new(entity.entity_id, 4),
        );
        self.play_sound(Sound::EntityCreakingAttack);
        self.mob_entity.try_attack(entity, target)
    }

    /// Vanilla `Creaking.getWalkTargetValue`: every spot is alike, light does not matter.
    fn get_walk_target_value(&self, _pos: &BlockPos, _level: &dyn LevelAccessor) -> f32 {
        0.0
    }

    /// Vanilla `Creaking.getAmbientSound`: quiet once active.
    fn get_ambient_sound(&self) -> Option<Sound> {
        (!self.is_active()).then_some(Sound::EntityCreakingAmbient)
    }

    fn mob_play_step_sound(&self, _pos: &BlockPos, _state: &BlockState) {
        self.play_entity_sound(Sound::EntityCreakingStep, 0.15, 1.0);
    }

    /// Vanilla `Creaking.xpReward`.
    fn get_base_experience_reward(&self) -> u32 {
        0
    }

    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        if let Some(pos) = self.get_home_pos() {
            let mut sub = NbtCompound::new();
            sub.put_int("x", pos.0.x);
            sub.put_int("y", pos.0.y);
            sub.put_int("z", pos.0.z);
            nbt.put_compound("home_pos", sub);
        }
    }

    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        if let Some(sub) = nbt.get_compound("home_pos")
            && let (Some(x), Some(y), Some(z)) =
                (sub.get_int("x"), sub.get_int("y"), sub.get_int("z"))
        {
            self.set_transient(BlockPos::new(x, y, z));
        }
    }

    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    /// Vanilla `Creaking.tick`, `aiStep` and `tickDeath`.
    fn mob_tick(&self, caller: &dyn EntityBase) {
        let entity = self.get_entity();
        if self.is_heart_bound() && self.is_tearing_down() {
            let death_time = self.death_time.fetch_add(1, Ordering::Relaxed) + 1;
            if death_time > TWITCH_DEATH_DURATION && !entity.is_removed() {
                self.tear_down();
            }
            return;
        }
        if !entity.is_alive() {
            return;
        }
        self.tick_animations();
        self.update_can_move();
        if !self.can_move() {
            // Vanilla `Creaking.knockback` does nothing while frozen.
            let velocity = entity.velocity.load();
            entity.velocity.store(Vector3::new(0.0, velocity.y, 0.0));
        }
        if let Some(home_pos) = self.get_home_pos()
            && !self.has_heart_protection(&home_pos)
        {
            caller.damage(caller, f32::MAX, DamageType::GENERIC_KILL);
        }
    }

    /// Vanilla `Creaking.hurtServer`: bound to a heart it takes no damage and calls the heart.
    fn pre_damage_with_context(
        &self,
        damage_type: DamageType,
        source: Option<&dyn EntityBase>,
        cause: Option<&dyn EntityBase>,
    ) -> bool {
        let Some(home_pos) = self.get_home_pos() else {
            return true;
        };
        if damage_type.has_tag(&tag::DamageType::MINECRAFT_BYPASSES_INVULNERABILITY) {
            return true;
        }
        let entity = self.get_entity();
        if !entity.is_alive()
            || self
                .invulnerability_animation_remaining_ticks
                .load(Ordering::Relaxed)
                > 0
        {
            return false;
        }
        let by_player = cause.or(source).is_some_and(|by| by.get_player().is_some());
        let direct_counts = source.is_some_and(|direct| {
            direct.get_living_entity().is_some() || direct.get_projectile_owner().is_some()
        });
        if !direct_counts && !by_player {
            return false;
        }
        self.invulnerability_animation_remaining_ticks
            .store(INVULNERABILITY_ANIMATION_DURATION, Ordering::Relaxed);
        let world = entity.world.load_full();
        world.broadcast_to_chunk(
            entity.chunk_pos.load(),
            &CEntityStatus::new(entity.entity_id, 66),
        );
        self.emit_action();
        let hurt_heart = with_heart(&world, &home_pos, |heart| {
            let protector = heart.is_protector(entity.entity_uuid);
            if protector && by_player {
                heart.creaking_hurt(&world);
            }
            protector
        });
        if hurt_heart == Some(true) {
            self.play_sound(Sound::EntityCreakingSway);
        }
        false
    }
}
