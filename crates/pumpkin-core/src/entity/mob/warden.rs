use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use pumpkin_data::BlockState;
use pumpkin_data::damage::DamageType;
use pumpkin_data::tag::{self, Taggable};
use pumpkin_data::{
    effect::StatusEffect,
    entity::{EntityPose, EntityStatus, EntityType},
    game_event::GameEvent,
    potion::Effect,
    sound::{Sound, SoundCategory},
    tracked_data,
};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_protocol::bedrock::server::ActorEventID;
use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::{GameMode, math::vector3::Vector3};

use super::warden_ai;
use super::warden_anger::{ANGRY_ANGER, AngerLevel, AngerManagement};
use crate::entity::ai::brain::memory::{MemoryModuleId, PackedMemories, types};
use crate::entity::ai::brain::sensing::SensorType;
use crate::entity::ai::brain::{Brain, BrainProvider, BrainState};
use crate::entity::ai::pathfinder::node::PathType;
use crate::entity::mob::spawn::SpawnGroupData;
use crate::entity::vibration::{VibrationData, VibrationListener, VibrationUser, tick_vibration};
use crate::entity::{
    Entity, EntityBase,
    mob::{Mob, MobEntity},
};
use crate::world::World;
use crate::world::game_event::{GameEventContext, GameEventListener};
use crate::world::level_accessor::LevelAccessor;

const DARKNESS_DISPLAY_LIMIT: i32 = 200;
const DARKNESS_DURATION: i32 = 260;
const DARKNESS_RADIUS: f64 = 20.0;
const DARKNESS_INTERVAL: i32 = 120;
const ANGER_MANAGEMENT_TICK_DELAY: i32 = 20;
/// Vanilla `Warden.DEFAULT_ANGER`.
pub const ANGER_DEFAULT: i32 = 35;
const PROJECTILE_ANGER: i32 = 10;
const ON_HURT_ANGER_BOOST: i32 = 20;
const RECENT_PROJECTILE_TICKS: i64 = 100;
const TOUCH_COOLDOWN_TICKS: i64 = 20;
const VIBRATION_COOLDOWN_TICKS: i64 = 40;
const PROJECTILE_ANGER_DISTANCE: f64 = 30.0;
const HURT_ATTACKER_DISTANCE: f64 = 5.0;
const SONIC_BOOM_AFTER_MELEE: i64 = 40;

const MEMORIES: [MemoryModuleId; 19] = [
    types::NEAREST_VISIBLE_NEMESIS.id(),
    types::RECENT_PROJECTILE.id(),
    types::TOUCH_COOLDOWN.id(),
    types::VIBRATION_COOLDOWN.id(),
    types::NEAREST_ATTACKABLE.id(),
    types::IS_EMERGING.id(),
    types::IS_SNIFFING.id(),
    types::DIG_COOLDOWN.id(),
    types::ROAR_TARGET.id(),
    types::ROAR_SOUND_DELAY.id(),
    types::ROAR_SOUND_COOLDOWN.id(),
    types::SNIFF_COOLDOWN.id(),
    types::DISTURBANCE_LOCATION.id(),
    types::SONIC_BOOM_COOLDOWN.id(),
    types::SONIC_BOOM_SOUND_DELAY.id(),
    types::SONIC_BOOM_SOUND_COOLDOWN.id(),
    types::ATTACK_TARGET.id(),
    types::ATTACK_COOLING_DOWN.id(),
    types::CANT_REACH_WALK_TARGET_SINCE.id(),
];

/// A hit taken, kept for the warden's own tick: its brain may be locked while it is hit.
struct PendingHurt {
    attacker: i32,
    direct: bool,
}

/// The ancient city's blind guard: it hears, sniffs and comes to whoever it is angriest at.
///
/// Wiki: <https://minecraft.wiki/w/Warden>
pub struct WardenEntity {
    pub mob_entity: MobEntity,
    anger: Mutex<AngerManagement>,
    vibration: Mutex<VibrationData>,
    pending_hurts: Mutex<Vec<PendingHurt>>,
    /// A melee hit landed inside the brain's tick, which holds off the sonic boom.
    melee_hit: AtomicBool,
    /// A target was given up inside the brain's tick, which holds off digging.
    dig_cooldown_queued: AtomicBool,
}

impl WardenEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let mob_entity = MobEntity::new(entity);
        Self::set_up_navigation(&mob_entity);
        Arc::new(Self {
            mob_entity,
            anger: Mutex::new(AngerManagement::default()),
            vibration: Mutex::new(VibrationData::default()),
            pending_hurts: Mutex::new(Vec::new()),
            melee_hit: AtomicBool::new(false),
            dig_cooldown_queued: AtomicBool::new(false),
        })
    }

    /// Vanilla's constructor: it floats, and steers around what hurts it.
    fn set_up_navigation(mob_entity: &MobEntity) {
        let steered = mob_entity.steered();
        let mut navigator = steered.navigator();
        navigator.set_can_float(true);
        navigator.set_pathfinding_malus(PathType::UnpassableRail, 0.0);
        navigator.set_pathfinding_malus(PathType::DamageOther, 8.0);
        navigator.set_pathfinding_malus(PathType::PowderSnow, 8.0);
        navigator.set_pathfinding_malus(PathType::Lava, 8.0);
        navigator.set_pathfinding_malus(PathType::DamageFire, 0.0);
        navigator.set_pathfinding_malus(PathType::DangerFire, 0.0);
    }

    fn anger(&self) -> MutexGuard<'_, AngerManagement> {
        self.anger
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Vanilla `finalizeSpawn` for `EntitySpawnReason.TRIGGERED`, as a shrieker summons it.
    pub fn emerge(&self) {
        let entity = &self.mob_entity.living_entity.entity;
        entity.set_pose(EntityPose::Emerging);
        self.mob_entity.with_brain(self, |brain| {
            brain.set_with_expiry(types::IS_EMERGING, (), warden_ai::EMERGE_DURATION);
        });
        entity.world.load().play_sound_fine(
            Sound::EntityWardenAgitated,
            SoundCategory::Hostile,
            &entity.pos.load(),
            5.0,
            1.0,
        );
    }

    pub fn apply_darkness_around(world: &World, position: Vector3<f64>, radius: f64) {
        for player in world.get_nearby_players(position, radius) {
            if !matches!(
                player.gamemode.load(),
                GameMode::Survival | GameMode::Adventure
            ) {
                continue;
            }
            let current = player.living_entity.get_effect(&StatusEffect::DARKNESS);
            if current.is_some_and(|effect| {
                effect.duration < 0 || effect.duration >= DARKNESS_DISPLAY_LIMIT
            }) {
                continue;
            }
            let darkness = Effect {
                effect_type: &StatusEffect::DARKNESS,
                duration: DARKNESS_DURATION,
                amplifier: 0,
                ambient: false,
                show_particles: false,
                show_icon: false,
                blend: true,
            };
            player.send_effect(&darkness);
            player.living_entity.add_effect(darkness);
        }
    }

    fn is_digging_or_emerging(&self) -> bool {
        matches!(
            self.mob_entity.living_entity.entity.pose.load(),
            EntityPose::Digging | EntityPose::Emerging
        )
    }

    /// Vanilla `canTargetEntity`.
    #[must_use]
    pub fn can_target_entity(&self, entity: &dyn EntityBase) -> bool {
        let Some(living) = entity.get_living_entity() else {
            return false;
        };
        let target = entity.get_entity();
        let own = &self.mob_entity.living_entity.entity;
        std::sync::Arc::ptr_eq(&target.world.load_full(), &own.world.load_full())
            && entity
                .get_player()
                .is_none_or(|p| !p.is_creative() && !p.is_spectator())
            && !(self as &dyn EntityBase).is_allied_to(entity)
            && target.entity_type != &EntityType::ARMOR_STAND
            && target.entity_type != &EntityType::WARDEN
            && !target.invulnerable.load(Ordering::Relaxed)
            && !living.is_dead_or_dying()
    }

    fn active_anger(&self) -> i32 {
        let target = self.mob_entity.get_target();
        self.anger().active_anger(target.as_deref())
    }

    /// Vanilla `getAngerLevel`.
    #[must_use]
    pub fn anger_level(&self) -> AngerLevel {
        AngerLevel::by_anger(self.active_anger())
    }

    /// Vanilla `getEntityAngryAt`.
    #[must_use]
    pub fn entity_angry_at(&self) -> Option<Arc<dyn EntityBase>> {
        self.anger_level()
            .is_angry()
            .then(|| {
                self.anger()
                    .active_entity(|entity| self.can_target_entity(entity))
            })
            .flatten()
    }

    pub fn clear_anger(&self, entity: &dyn EntityBase) {
        self.anger().clear_anger(entity);
    }

    /// Asks for vanilla `setDigCooldown` from inside the brain's tick, after it ends.
    pub fn queue_dig_cooldown(&self) {
        self.dig_cooldown_queued.store(true, Ordering::Relaxed);
    }

    /// Vanilla `increaseAngerAt`, for callers that hold the brain.
    pub fn increase_anger_in(
        &self,
        state: &mut BrainState,
        entity: &Arc<dyn EntityBase>,
        amount: i32,
        play_sound: bool,
    ) {
        if self.mob_entity.is_no_ai() || !self.can_target_entity(entity.as_ref()) {
            return;
        }
        warden_ai::set_dig_cooldown(state);
        let maybe_switch_target = state
            .get(types::ATTACK_TARGET)
            .is_none_or(|target| target.get_player().is_none());
        let new_anger = self.anger().increase_anger(entity, amount);
        if entity.get_player().is_some()
            && maybe_switch_target
            && AngerLevel::by_anger(new_anger).is_angry()
        {
            state.erase(types::ATTACK_TARGET.id());
        }
        if play_sound {
            self.play_listening_sound();
        }
    }

    /// Vanilla `increaseAngerAt`.
    fn increase_anger_at(&self, entity: &Arc<dyn EntityBase>, amount: i32, play_sound: bool) {
        self.mob_entity.with_brain(self, |brain| {
            self.increase_anger_in(brain, entity, amount, play_sound);
        });
    }

    fn voice_pitch() -> f32 {
        (rand::random::<f32>() - rand::random::<f32>()) * 0.2 + 1.0
    }

    /// Vanilla `playListeningSound`.
    fn play_listening_sound(&self) {
        if self.mob_entity.living_entity.entity.pose.load() != EntityPose::Roaring {
            self.play_entity_sound(
                self.anger_level().listening_sound(),
                10.0,
                Self::voice_pitch(),
            );
        }
    }

    fn sync_client_anger_level(&self) {
        let anger = self.active_anger();
        self.mob_entity
            .living_entity
            .entity
            .set_synced_data(tracked_data::warden::CLIENT_ANGER_LEVEL, VarInt(anger));
    }

    /// Vanilla `hurtServer` after the hit, for what was hit while the brain may have been locked.
    fn take_pending_hurts(&self) {
        let hurts = std::mem::take(
            &mut *self
                .pending_hurts
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        );
        let world = self.get_entity().world.load();
        for hurt in hurts {
            if self.mob_entity.is_no_ai() || self.is_digging_or_emerging() {
                continue;
            }
            let Some(attacker) = world.get_entity_by_id(hurt.attacker) else {
                continue;
            };
            self.increase_anger_at(&attacker, ANGRY_ANGER + ON_HURT_ANGER_BOOST, false);
            let near = attacker
                .get_entity()
                .pos
                .load()
                .squared_distance_to_vec(&self.get_entity().pos.load())
                < HURT_ATTACKER_DISTANCE * HURT_ATTACKER_DISTANCE;
            if attacker.get_living_entity().is_some() && (hurt.direct || near) {
                self.mob_entity.with_brain(self, |brain| {
                    if !brain.has_memory_value(types::ATTACK_TARGET.id()) {
                        warden_ai::set_attack_target(brain, attacker.clone());
                    }
                });
            }
        }
    }

    /// What behaviors asked for from inside the brain's tick.
    fn take_queued_from_brain(&self) {
        let melee = self.melee_hit.swap(false, Ordering::Relaxed);
        let dig = self.dig_cooldown_queued.swap(false, Ordering::Relaxed);
        if melee || dig {
            self.mob_entity.with_brain(self, |brain| {
                if melee {
                    warden_ai::set_sonic_boom_cooldown(brain, SONIC_BOOM_AFTER_MELEE);
                }
                if dig {
                    warden_ai::set_dig_cooldown(brain);
                }
            });
        }
    }
}

impl VibrationUser for WardenEntity {
    fn vibration_entity(&self) -> &Entity {
        &self.mob_entity.living_entity.entity
    }

    fn vibration_data(&self) -> &Mutex<VibrationData> {
        &self.vibration
    }

    fn can_listen_to(&self, event: GameEvent) -> bool {
        tag::GameEvent::MINECRAFT_WARDEN_CAN_LISTEN
            .0
            .contains(&event.name())
    }

    fn can_receive_vibration(
        &self,
        _world: &World,
        _pos: BlockPos,
        _event: GameEvent,
        context: &GameEventContext<'_>,
    ) -> bool {
        if self.mob_entity.is_no_ai()
            || self.mob_entity.living_entity.is_dead_or_dying()
            || self.is_digging_or_emerging()
            || self.mob_entity.with_brain(self, |brain| {
                brain.has_memory_value(types::VIBRATION_COOLDOWN.id())
            })
        {
            return false;
        }
        context.source_entity.is_none_or(|source| {
            source.get_living_entity().is_none() || self.can_target_entity(source)
        })
    }

    fn on_receive_vibration(
        &self,
        _world: &World,
        pos: BlockPos,
        _event: GameEvent,
        source: Option<Arc<dyn EntityBase>>,
        projectile_owner: Option<Arc<dyn EntityBase>>,
    ) {
        if self.mob_entity.living_entity.is_dead_or_dying() {
            return;
        }
        let entity = self.get_entity();
        entity.world.load().send_entity_status(
            entity,
            EntityStatus::TendrilsShiver,
            ActorEventID::None,
        );
        self.play_entity_sound(Sound::EntityWardenTendrilClicks, 5.0, Self::voice_pitch());
        self.mob_entity.with_brain(self, |brain| {
            brain.set_with_expiry(types::VIBRATION_COOLDOWN, (), VIBRATION_COOLDOWN_TICKS);
            let mut suspicious = pos;
            if let Some(owner) = &projectile_owner {
                let close = owner
                    .get_entity()
                    .pos
                    .load()
                    .squared_distance_to_vec(&entity.pos.load())
                    < PROJECTILE_ANGER_DISTANCE * PROJECTILE_ANGER_DISTANCE;
                if close {
                    if brain.has_memory_value(types::RECENT_PROJECTILE.id()) {
                        if self.can_target_entity(owner.as_ref()) {
                            suspicious = owner.get_entity().block_pos.load();
                        }
                        self.increase_anger_in(brain, owner, ANGER_DEFAULT, true);
                    } else {
                        self.increase_anger_in(brain, owner, PROJECTILE_ANGER, true);
                    }
                }
                brain.set_with_expiry(types::RECENT_PROJECTILE, (), RECENT_PROJECTILE_TICKS);
            } else if let Some(source) = &source {
                self.increase_anger_in(brain, source, ANGER_DEFAULT, true);
            }
            if !self.anger_level().is_angry() {
                let active = self
                    .anger()
                    .active_entity(|entity| self.can_target_entity(entity));
                let is_source = match (&active, &source) {
                    (None, _) => true,
                    (Some(active), Some(source)) => {
                        active.get_entity().entity_id == source.get_entity().entity_id
                    }
                    (Some(_), None) => false,
                };
                if projectile_owner.is_some() || is_source {
                    warden_ai::set_disturbance_location(self, brain, suspicious);
                }
            }
        });
    }
}

impl Mob for WardenEntity {
    fn make_brain(&self, packed: &PackedMemories) -> Brain {
        BrainProvider {
            memory_types: &MEMORIES,
            sensor_types: &[
                SensorType::NearestLivingEntities,
                SensorType::NearestPlayers,
                SensorType::WardenEntity,
            ],
            activities: warden_ai::activities,
        }
        .make_brain(self, packed)
    }

    /// Vanilla `Warden.tick`, the part before `super.tick`.
    fn mob_tick(&self, _caller: &dyn EntityBase) {
        tick_vibration(self);
        if self.mob_entity.persistence_required.load(Ordering::Relaxed)
            || self.requires_custom_persistence()
        {
            self.mob_entity.with_brain(self, |brain| {
                warden_ai::set_dig_cooldown(brain);
            });
        }
    }

    /// Vanilla `Warden.customServerAiStep`.
    fn custom_server_ai_step(&self, _caller: &dyn EntityBase) {
        self.take_pending_hurts();
        self.mob_entity.tick_brain(self);
        self.take_queued_from_brain();

        let entity = self.get_entity();
        let tick_count = entity.tick_count.load(Ordering::Relaxed);
        let world = entity.world.load_full();
        if (tick_count + entity.entity_id) % DARKNESS_INTERVAL == 0 {
            Self::apply_darkness_around(&world, entity.pos.load(), DARKNESS_RADIUS);
        }
        if tick_count % ANGER_MANAGEMENT_TICK_DELAY == 0 {
            self.anger()
                .tick(&world, |suspect| self.can_target_entity(suspect));
            self.sync_client_anger_level();
        }

        let target = self.mob_entity.with_brain(self, |brain| {
            warden_ai::update_activity(brain);
            brain.get(types::ATTACK_TARGET).cloned()
        });
        // Vanilla `getTarget` reads the brain.
        self.mob_entity.set_target(target);
    }

    /// Vanilla `Warden.finalizeSpawn`.
    fn finalize_spawn(
        &self,
        _world: &Arc<World>,
        group_data: Option<SpawnGroupData>,
    ) -> Option<SpawnGroupData> {
        self.mob_entity.with_brain(self, |brain| {
            brain.set_with_expiry(types::DIG_COOLDOWN, (), warden_ai::DIGGING_COOLDOWN);
        });
        self.finalize_spawn_super(group_data)
    }

    fn mob_init_data_tracker(&self) {
        self.mob_entity
            .living_entity
            .entity
            .set_synced_data(tracked_data::warden::CLIENT_ANGER_LEVEL, VarInt(0));
    }

    fn for_each_game_event_listener(&self, visit: &mut dyn FnMut(&dyn GameEventListener)) {
        visit(&VibrationListener(self));
    }

    /// Vanilla `Warden.canAttack`.
    fn can_attack(&self, target: &dyn EntityBase) -> bool {
        self.can_target_entity(target)
    }

    /// Vanilla `Warden.doHurtTarget`; the boom's cooldown waits for the brain to be free.
    fn do_hurt_target(&self, target: &dyn EntityBase) -> bool {
        let entity = self.get_entity();
        entity.world.load().send_entity_status(
            entity,
            EntityStatus::StartAttacking,
            ActorEventID::StartAttacking,
        );
        self.play_entity_sound(Sound::EntityWardenAttackImpact, 10.0, Self::voice_pitch());
        self.melee_hit.store(true, Ordering::Relaxed);
        let damaged = self.mob_entity.try_attack(entity, target);
        if damaged {
            self.on_attack(target);
        }
        damaged
    }

    /// Vanilla `Warden.isInvulnerableTo`: nothing reaches it in the ground.
    fn mob_is_invulnerable_to(
        &self,
        damage_type: &DamageType,
        _source: Option<&dyn EntityBase>,
        cause: Option<&dyn EntityBase>,
    ) -> bool {
        (self.is_digging_or_emerging()
            && !damage_type.has_tag(&tag::DamageType::MINECRAFT_BYPASSES_INVULNERABILITY))
            || self.get_entity().is_invulnerable_to(damage_type, cause)
    }

    fn after_hurt(
        &self,
        _damage_type: DamageType,
        source: Option<&dyn EntityBase>,
        cause: Option<&dyn EntityBase>,
    ) {
        let Some(attacker) = cause.or(source) else {
            return;
        };
        let id = attacker.get_entity().entity_id;
        self.pending_hurts
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(PendingHurt {
                attacker: id,
                direct: source.is_some_and(|direct| direct.get_entity().entity_id == id),
            });
    }

    fn mob_is_pushable(&self) -> bool {
        !self.is_digging_or_emerging() && self.mob_entity.living_entity.is_pushable()
    }

    /// Vanilla `Warden.doPush`: what it bumps into it gets angry at.
    fn mob_do_push(&self, entity: &Arc<dyn EntityBase>) {
        if self.mob_entity.is_no_ai() {
            return;
        }
        self.mob_entity.with_brain(self, |brain| {
            if !brain.has_memory_value(types::TOUCH_COOLDOWN.id()) {
                brain.set_with_expiry(types::TOUCH_COOLDOWN, (), TOUCH_COOLDOWN_TICKS);
                self.increase_anger_in(brain, entity, ANGER_DEFAULT, true);
                warden_ai::set_disturbance_location(
                    self,
                    brain,
                    entity.get_entity().block_pos.load(),
                );
            }
        });
    }

    /// Vanilla `Warden.getAmbientSound`.
    fn get_ambient_sound(&self) -> Option<Sound> {
        let pose = self.mob_entity.living_entity.entity.pose.load();
        (pose != EntityPose::Roaring && !self.is_digging_or_emerging())
            .then(|| self.anger_level().ambient_sound())
    }

    /// Vanilla `Warden.getWalkTargetValue`: every spot is alike, light does not matter.
    fn get_walk_target_value(&self, _pos: &BlockPos, _level: &dyn LevelAccessor) -> f32 {
        0.0
    }

    /// Vanilla `Warden.removeWhenFarAway`.
    fn remove_when_far_away(&self, _distance_sq: f64) -> bool {
        false
    }

    fn mob_play_step_sound(&self, _pos: &BlockPos, _state: &BlockState) {
        self.play_entity_sound(Sound::EntityWardenStep, 10.0, 1.0);
    }

    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    fn check_spawn_obstruction(&self, level: &dyn LevelAccessor) -> bool {
        let entity = &self.mob_entity.living_entity.entity;
        let bounding_box = entity.bounding_box.load();
        !level.contains_any_liquid(bounding_box)
            && level.is_unobstructed(entity)
            && level.no_collision(&bounding_box)
    }

    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        nbt.put_compound("anger", self.anger().write_nbt());
        nbt.put_compound(
            "listener",
            self.vibration
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .write_nbt(),
        );
    }

    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        if let Some(anger) = nbt.get_compound("anger") {
            *self.anger() = AngerManagement::read_nbt(anger);
        }
        if let Some(listener) = nbt.get_compound("listener") {
            *self
                .vibration
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) =
                VibrationData::read_nbt(listener);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entity::ai::test_support::world;
    use crate::entity::passive::allay::AllayEntity;

    #[tokio::test]
    async fn in_the_ground_it_is_short_and_nothing_hurts_it()
    -> Result<(), Box<dyn std::error::Error>> {
        let folder = tempfile::tempdir()?;
        let world = world(&folder);
        let warden = WardenEntity::new(Entity::new(
            world,
            Vector3::new(0.5, 64.0, 0.5),
            &EntityType::WARDEN,
        ));
        let entity = warden.get_entity();
        assert!(!warden.mob_is_invulnerable_to(&DamageType::GENERIC, None, None));
        assert!((entity.height() - 2.9).abs() < 1.0e-4);

        warden.emerge();
        assert!(entity.pose.load() == EntityPose::Emerging);
        assert!(warden.mob_entity.with_brain(&*warden, |brain| {
            brain.has_memory_value(types::IS_EMERGING.id())
        }));
        assert!(warden.mob_is_invulnerable_to(&DamageType::GENERIC, None, None));
        assert!(!warden.mob_is_pushable());
        assert!((entity.height() - 1.0).abs() < 1.0e-4);

        entity.set_pose(EntityPose::Standing);
        assert!(!warden.mob_is_invulnerable_to(&DamageType::GENERIC, None, None));
        assert!((entity.height() - 2.9).abs() < 1.0e-4);
        Ok(())
    }

    #[tokio::test]
    async fn it_is_angry_at_a_suspect_from_80() -> Result<(), Box<dyn std::error::Error>> {
        let folder = tempfile::tempdir()?;
        let world = world(&folder);
        let warden = WardenEntity::new(Entity::new(
            world.clone(),
            Vector3::new(0.5, 64.0, 0.5),
            &EntityType::WARDEN,
        ));
        let suspect: Arc<dyn EntityBase> = AllayEntity::new(Entity::new(
            world,
            Vector3::new(4.5, 64.0, 0.5),
            &EntityType::ALLAY,
        ));
        assert!(warden.can_target_entity(suspect.as_ref()));
        assert!(warden.entity_angry_at().is_none());

        warden.increase_anger_at(&suspect, ANGER_DEFAULT, false);
        warden.increase_anger_at(&suspect, ANGER_DEFAULT, false);
        assert_eq!(warden.anger_level(), AngerLevel::Agitated);
        assert!(warden.entity_angry_at().is_none());

        warden.increase_anger_at(&suspect, ANGER_DEFAULT, false);
        assert_eq!(warden.anger_level(), AngerLevel::Angry);
        assert!(warden.entity_angry_at().is_some());

        warden.clear_anger(suspect.as_ref());
        assert_eq!(warden.anger_level(), AngerLevel::Calm);
        Ok(())
    }

    #[tokio::test]
    async fn it_does_not_target_its_own_kind() -> Result<(), Box<dyn std::error::Error>> {
        let folder = tempfile::tempdir()?;
        let world = world(&folder);
        let warden = WardenEntity::new(Entity::new(
            world.clone(),
            Vector3::new(0.5, 64.0, 0.5),
            &EntityType::WARDEN,
        ));
        let other = WardenEntity::new(Entity::new(
            world,
            Vector3::new(4.5, 64.0, 0.5),
            &EntityType::WARDEN,
        ));
        assert!(!warden.can_target_entity(other.as_ref()));
        Ok(())
    }
}
