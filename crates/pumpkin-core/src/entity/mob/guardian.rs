use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::{Arc, Weak};

use pumpkin_data::effect::StatusEffect;
use pumpkin_data::entity::{EntityStatus, EntityType};
use pumpkin_data::fluid::Fluid;
use pumpkin_data::potion::Effect;
use pumpkin_data::tag::{self, Taggable};
use pumpkin_data::{damage::DamageType, sound::Sound};
use pumpkin_protocol::bedrock::server::actor_event::ActorEventID;
use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_protocol::java::client::play::{CGameEvent, GameEvent};
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::{Difficulty, GameMode};
use rand::RngExt;

use crate::entity::ai::behavior::hunting::{self, Hunt};
use crate::entity::ai::target_match::TargetMatch;
use crate::entity::custom_sound::CustomSound;
use crate::entity::living::LivingEntity;
use crate::entity::{
    Entity, EntityBase,
    ai::{
        control::guardian_move_control::GuardianMoveControl,
        goal::{
            Controls, Goal, look_around::RandomLookAroundGoal, look_at_entity::LookAtEntityGoal,
            move_towards_restriction::MoveTowardsRestrictionGoal, wander_around::WanderAroundGoal,
        },
        pathfinder::{Navigator, node::PathType},
    },
    mob::{Mob, MobEntity},
};
use crate::plugin::api::events::entity::entity_target::TargetReason;
use crate::world::World;

const GUARDIAN_PREY: TargetMatch = TargetMatch::AnyOf(&[
    &EntityType::PLAYER,
    &EntityType::SQUID,
    &EntityType::GLOW_SQUID,
    &EntityType::AXOLOTL,
]);

/// Vanilla `GuardianAttackSelector`: nothing within 3 blocks.
fn outside_melee_range(hunter: &dyn Mob, target: &LivingEntity, _world: &World) -> bool {
    let hunter_pos = hunter.get_mob_entity().living_entity.entity.pos.load();
    target
        .entity
        .pos
        .load()
        .squared_distance_to_vec(&hunter_pos)
        > 9.0
}

pub const GUARDIAN_HUNTS: &[Hunt] =
    &[Hunt::new(1, GUARDIAN_PREY, true).matching(outside_melee_range)];

const WATER_ACCELERATION: f64 = 0.1;
const WATER_FRICTION: f64 = 0.9;
const SINK_SPEED: f64 = 0.005;
const LAND_HOP_HORIZONTAL: f32 = 0.4;
const LAND_HOP_VERTICAL: f64 = 0.5;
/// `Guardian.getMaxHeadXRot`, also the elder's.
pub const MAX_HEAD_X_ROT: f32 = 180.0;

/// Vanilla `Guardian`'s water navigation (water malus 0) and move control, shared with the
/// elder guardian. Returns the synced `DATA_ID_MOVING` flag the move control keeps.
pub fn initialize_guardian_movement(mob: &mut MobEntity) -> Arc<AtomicBool> {
    let moving = Arc::new(AtomicBool::new(false));
    let navigator = mob
        .navigator
        .get_mut()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    *navigator = Navigator::water_bound(false);
    navigator.set_pathfinding_malus(PathType::Water, 0.0);
    *mob.move_control
        .get_mut()
        .unwrap_or_else(std::sync::PoisonError::into_inner) =
        Box::new(GuardianMoveControl::new(moving.clone()));
    moving
}

/// Vanilla `Guardian.travelInWater`: no gravity, and a slow sink while idle without a target.
pub fn guardian_travel_in_water(mob: &dyn Mob, caller: &dyn EntityBase, moving: bool) {
    let mob_entity = mob.get_mob_entity();
    let living = &mob_entity.living_entity;
    let entity = &living.entity;
    entity.update_velocity_from_input(living.movement_input.load(), WATER_ACCELERATION);
    living.make_move(caller);
    let mut velocity = entity.velocity.load() * WATER_FRICTION;
    if !moving && mob_entity.get_target().is_none() {
        velocity.y -= SINK_SPEED;
    }
    entity.velocity.store(velocity);
}

/// The server part of vanilla `Guardian.aiStep`: full air in water, a random hop on land.
pub fn guardian_ai_step(mob: &dyn Mob) {
    let living = &mob.get_mob_entity().living_entity;
    let entity = &living.entity;
    if !living.is_alive() {
        return;
    }
    if entity.is_in_water() {
        entity.set_air_supply(300);
    } else if entity.on_ground.load(Ordering::Relaxed) {
        let mut rng = mob.get_random();
        let x = (rng.random::<f32>() * 2.0 - 1.0) * LAND_HOP_HORIZONTAL;
        let z = (rng.random::<f32>() * 2.0 - 1.0) * LAND_HOP_HORIZONTAL;
        entity.velocity.store(entity.velocity.load().add_raw(
            f64::from(x),
            LAND_HOP_VERTICAL,
            f64::from(z),
        ));
        entity.yaw.store(rng.random::<f32>() * 360.0);
        entity.on_ground.store(false, Ordering::Relaxed);
        entity.velocity_dirty.store(true, Ordering::Relaxed);
    }
}

/// Vanilla `Guardian.getWalkTargetValue`: water first, else dark spots like any monster.
pub fn guardian_walk_target_value(mob: &dyn Mob, pos: &BlockPos) -> f32 {
    let world = mob.get_entity().world.load();
    let light_cost = world.get_light_level_dependent_magic_value(pos) - 0.5;
    if world
        .get_fluid_and_fluid_state(pos)
        .0
        .matches_type(&Fluid::WATER)
    {
        10.0 + light_cost
    } else {
        -light_cost
    }
}

/// Vanilla `Guardian.getAttackDuration`, 60 for the elder.
const ATTACK_DURATION: i32 = 80;
const ELDER_ATTACK_DURATION: i32 = 60;
/// Vanilla `ElderGuardian`: mining fatigue III for 5 minutes on players within 50 blocks
/// every minute, given to those whose fatigue is weaker or ends within a minute.
const EFFECT_INTERVAL: i32 = 1200;
const EFFECT_RADIUS: f64 = 50.0;
const EFFECT_DURATION: i32 = 6000;
const EFFECT_AMPLIFIER: u8 = 2;
const EFFECT_DISPLAY_LIMIT: i32 = 1200;

/// Vanilla `Guardian` and its subclass `ElderGuardian`.
pub struct GuardianEntity {
    pub mob_entity: MobEntity,
    moving: Arc<AtomicBool>,
    is_elder: bool,
    /// Vanilla `DATA_ID_ATTACK_TARGET`: the entity id of the laser's target, 0 for none.
    active_attack_target: AtomicI32,
    /// Set to make the stroll goal run on its next check (`randomStrollGoal.trigger`).
    stroll_trigger: Arc<AtomicBool>,
}

impl GuardianEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let is_elder = entity.entity_type.id == EntityType::ELDER_GUARDIAN.id;
        let mut mob_entity = MobEntity::new(entity);
        let moving = initialize_guardian_movement(&mut mob_entity);
        if is_elder {
            // Vanilla `ElderGuardian` constructor.
            mob_entity.set_persistence_required();
        }
        let stroll_trigger = Arc::new(AtomicBool::new(false));
        let guardian = Self {
            mob_entity,
            moving,
            is_elder,
            active_attack_target: AtomicI32::new(0),
            stroll_trigger: stroll_trigger.clone(),
        };
        let mob_arc = Arc::new(guardian);
        let mob_weak: Weak<dyn Mob> = {
            let mob_arc: Arc<dyn Mob> = mob_arc.clone();
            Arc::downgrade(&mob_arc)
        };

        {
            // Vanilla `Guardian.registerGoals`; the elder strolls every 400 ticks.
            let goal_selector = &mob_arc.mob_entity.goals_selector;
            goal_selector.add_goal(4, Box::new(GuardianAttackGoal { attack_time: 0 }));
            goal_selector.add_goal(
                5,
                Box::new(LookingGoal(MoveTowardsRestrictionGoal::new(1.0))),
            );
            goal_selector.add_goal(
                7,
                Box::new(GuardianStrollGoal {
                    inner: WanderAroundGoal::with_interval(1.0, if is_elder { 400 } else { 80 }),
                    trigger: stroll_trigger,
                }),
            );
            goal_selector.add_goal(
                8,
                LookAtEntityGoal::with_default(mob_weak.clone(), &EntityType::PLAYER, 8.0),
            );
            goal_selector.add_goal(
                8,
                Box::new(LookAtEntityGoal::new(
                    mob_weak,
                    &EntityType::GUARDIAN,
                    12.0,
                    0.01,
                    false,
                )),
            );
            goal_selector.add_goal(9, Box::new(RandomLookAroundGoal::default()));

            let target_selector = &mob_arc.mob_entity.target_selector;
            hunting::apply(target_selector, &mob_arc.mob_entity, GUARDIAN_HUNTS);
        };

        mob_arc
    }

    const fn attack_duration(&self) -> i32 {
        if self.is_elder {
            ELDER_ATTACK_DURATION
        } else {
            ATTACK_DURATION
        }
    }

    fn set_active_attack_target(&self, entity_id: i32) {
        self.active_attack_target
            .store(entity_id, Ordering::Relaxed);
        self.get_entity().set_synced_data(
            pumpkin_data::tracked_data::guardian::DATA_ID_ATTACK_TARGET,
            VarInt(entity_id),
        );
    }

    fn has_active_attack_target(&self) -> bool {
        self.active_attack_target.load(Ordering::Relaxed) != 0
    }

    /// Vanilla `ElderGuardian.customServerAiStep`: the curse every minute, and a home of
    /// radius 16 where it first ticks.
    fn elder_server_ai_step(&self) {
        let mob_entity = &self.mob_entity;
        let entity = &mob_entity.living_entity.entity;
        if (entity.tick_count.load(Ordering::Relaxed) + entity.entity_id) % EFFECT_INTERVAL == 0 {
            let pos = entity.pos.load();
            let world = entity.world.load();
            let packet = CGameEvent::new(
                GameEvent::PlayElderGuardianMobAppearance,
                if entity.is_silent() { 0.0 } else { 1.0 },
            );
            for player in world.get_nearby_players(pos, EFFECT_RADIUS) {
                let survival = matches!(
                    player.gamemode.load(),
                    GameMode::Survival | GameMode::Adventure
                );
                let weaker = player
                    .living_entity
                    .get_effect(&StatusEffect::MINING_FATIGUE)
                    .is_none_or(|effect| {
                        effect.amplifier < EFFECT_AMPLIFIER
                            || (effect.duration != -1 && effect.duration < EFFECT_DISPLAY_LIMIT)
                    });
                if !survival || !weaker {
                    continue;
                }
                player.living_entity.add_effect(Effect {
                    effect_type: &StatusEffect::MINING_FATIGUE,
                    duration: EFFECT_DURATION,
                    amplifier: EFFECT_AMPLIFIER,
                    ambient: false,
                    show_particles: true,
                    show_icon: true,
                    blend: false,
                });
                player.try_send_client_packet(&packet);
            }
        }
        if !mob_entity.has_position_target() {
            mob_entity.set_home_to(entity.block_pos.load(), 16);
        }
    }
}

fn as_guardian(mob: &dyn Mob) -> Option<&GuardianEntity> {
    mob.cast_any().downcast_ref::<GuardianEntity>()
}

impl CustomSound for GuardianEntity {
    fn hurt_sound(&self) -> Option<Sound> {
        Some(match (self.is_elder, self.get_entity().is_in_water()) {
            (false, true) => Sound::EntityGuardianHurt,
            (false, false) => Sound::EntityGuardianHurtLand,
            (true, true) => Sound::EntityElderGuardianHurt,
            (true, false) => Sound::EntityElderGuardianHurtLand,
        })
    }

    fn death_sound(&self) -> Option<Sound> {
        Some(match (self.is_elder, self.get_entity().is_in_water()) {
            (false, true) => Sound::EntityGuardianDeath,
            (false, false) => Sound::EntityGuardianDeathLand,
            (true, true) => Sound::EntityElderGuardianDeath,
            (true, false) => Sound::EntityElderGuardianDeathLand,
        })
    }
}

impl Mob for GuardianEntity {
    /// Vanilla `Guardian.getAmbientSound` and `ElderGuardian.getAmbientSound`.
    fn get_ambient_sound(&self) -> Option<Sound> {
        Some(match (self.is_elder, self.get_entity().is_in_water()) {
            (false, true) => Sound::EntityGuardianAmbient,
            (false, false) => Sound::EntityGuardianAmbientLand,
            (true, true) => Sound::EntityElderGuardianAmbient,
            (true, false) => Sound::EntityElderGuardianAmbientLand,
        })
    }

    /// Vanilla `Guardian.getAmbientSoundInterval`.
    fn get_ambient_sound_interval(&self) -> i32 {
        160
    }

    fn as_custom_sound(&self) -> Option<&dyn CustomSound> {
        Some(self)
    }

    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    /// Vanilla `Guardian.checkSpawnObstruction`: only entities obstruct, the water does not.
    fn check_spawn_obstruction(&self, world: &World) -> bool {
        world.is_unobstructed(&self.mob_entity.living_entity.entity)
    }

    fn get_max_look_pitch_change(&self) -> f32 {
        MAX_HEAD_X_ROT
    }

    /// Vanilla `Guardian.aiStep`: it faces where its head looks while it aims the laser.
    fn before_mob_ai_step(&self) {
        guardian_ai_step(self);
        let entity = self.get_entity();
        if entity.is_alive() && self.has_active_attack_target() {
            entity.yaw.store(entity.head_yaw.load());
        }
    }

    fn custom_server_ai_step(&self, _caller: &dyn EntityBase) {
        if self.is_elder && self.get_entity().is_alive() {
            self.elder_server_ai_step();
        }
    }

    fn travel_in_water(&self, caller: &dyn EntityBase, _gravity: f64, _falling: bool) {
        guardian_travel_in_water(self, caller, self.moving.load(Ordering::Relaxed));
    }

    fn get_walk_target_value(&self, pos: &BlockPos) -> f32 {
        guardian_walk_target_value(self, pos)
    }

    /// Vanilla `Guardian.hurtServer`: its spikes hurt a living attacker for 2 while it does
    /// not move, and being hurt sends it strolling.
    fn on_damage(&self, damage_type: DamageType, source: Option<&dyn EntityBase>) {
        if !self.moving.load(Ordering::Relaxed)
            && damage_type != DamageType::THORNS
            && !damage_type.has_tag(&tag::DamageType::MINECRAFT_AVOIDS_GUARDIAN_THORNS)
            && let Some(src) = source
            && src.get_living_entity().is_some()
        {
            let guardian: &dyn EntityBase = self;
            let _ = src.damage_with_context(
                src,
                2.0,
                DamageType::THORNS,
                None,
                Some(guardian),
                Some(guardian),
            );
        }
        self.stroll_trigger.store(true, Ordering::Relaxed);
    }
}

/// Vanilla `Guardian.GuardianAttackGoal`: aims the laser at its target for 4 seconds (3 for
/// the elder) while it sees it, then hits with magic and its bite.
struct GuardianAttackGoal {
    attack_time: i32,
}

impl GuardianAttackGoal {
    fn look_at_target(mob: &dyn Mob, target: &Arc<dyn EntityBase>) {
        mob.get_mob_entity()
            .look_control
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .look_at_entity_with_range(target, 90.0, 90.0);
    }

    fn forget_target(mob: &dyn Mob) {
        mob.set_mob_target(None, TargetReason::ForgotTarget);
    }

    /// The end of the aim: 1 magic damage, 2 more on hard and 2 more from an elder, then the bite.
    fn hit(guardian: &GuardianEntity, mob: &dyn Mob, target: &Arc<dyn EntityBase>) {
        let world = mob.get_entity().world.load();
        let mut magic_damage = 1.0;
        if world.level_info.load().difficulty == Difficulty::Hard {
            magic_damage += 2.0;
        }
        if guardian.is_elder {
            magic_damage += 2.0;
        }
        let attacker: &dyn EntityBase = mob;
        let _ = target.damage_with_context(
            target.as_ref(),
            magic_damage,
            DamageType::INDIRECT_MAGIC,
            None,
            Some(attacker),
            Some(attacker),
        );
        let _ = mob.get_mob_entity().try_attack(attacker, target.as_ref());
    }
}

impl Goal for GuardianAttackGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        mob.get_mob_entity()
            .get_target()
            .is_some_and(|target| target.get_entity().is_alive())
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        let Some(guardian) = as_guardian(mob) else {
            return false;
        };
        let pos = mob.get_entity().pos.load();
        self.can_start(mob)
            && (guardian.is_elder
                || mob.get_mob_entity().get_target().is_some_and(|target| {
                    target.get_entity().pos.load().squared_distance_to_vec(&pos) > 9.0
                }))
    }

    fn start(&mut self, mob: &dyn Mob) {
        self.attack_time = -10;
        mob.get_mob_entity().steered().navigator().stop();
        if let Some(target) = mob.get_mob_entity().get_target() {
            Self::look_at_target(mob, &target);
        }
        mob.get_entity()
            .velocity_dirty
            .store(true, Ordering::Relaxed);
    }

    fn stop(&mut self, mob: &dyn Mob) {
        if let Some(guardian) = as_guardian(mob) {
            guardian.set_active_attack_target(0);
            guardian.stroll_trigger.store(true, Ordering::Relaxed);
        }
        Self::forget_target(mob);
    }

    fn tick(&mut self, mob: &dyn Mob) {
        let Some(guardian) = as_guardian(mob) else {
            return;
        };
        let Some(target) = mob.get_mob_entity().get_target() else {
            return;
        };
        mob.get_mob_entity().steered().navigator().stop();
        Self::look_at_target(mob, &target);
        if !mob.has_line_of_sight(target.get_entity()) {
            Self::forget_target(mob);
            return;
        }
        self.attack_time += 1;
        let entity = mob.get_entity();
        if self.attack_time == 0 {
            guardian.set_active_attack_target(target.get_entity().entity_id);
            if !entity.is_silent() {
                entity.world.load().send_entity_status(
                    entity,
                    EntityStatus::GuardianAttackSound,
                    ActorEventID::None,
                );
            }
        } else if self.attack_time >= guardian.attack_duration() {
            Self::hit(guardian, mob, &target);
            Self::forget_target(mob);
        }
    }

    fn should_run_every_tick(&self) -> bool {
        true
    }

    fn controls(&self) -> Controls {
        Controls::MOVE | Controls::LOOK
    }
}

/// Vanilla `RandomStrollGoal` of the guardian: with the look flag too, and run at once when
/// triggered.
struct GuardianStrollGoal {
    inner: WanderAroundGoal,
    trigger: Arc<AtomicBool>,
}

impl Goal for GuardianStrollGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        if self.trigger.swap(false, Ordering::Relaxed) {
            self.inner.trigger();
        }
        self.inner.can_start(mob)
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        self.inner.should_continue(mob)
    }

    fn start(&mut self, mob: &dyn Mob) {
        self.inner.start(mob);
    }

    fn stop(&mut self, mob: &dyn Mob) {
        self.inner.stop(mob);
    }

    fn controls(&self) -> Controls {
        Controls::MOVE | Controls::LOOK
    }
}

/// A goal with the look flag added, like the guardian's `MoveTowardsRestrictionGoal`.
struct LookingGoal<G: Goal>(G);

impl<G: Goal> Goal for LookingGoal<G> {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        self.0.can_start(mob)
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        self.0.should_continue(mob)
    }

    fn start(&mut self, mob: &dyn Mob) {
        self.0.start(mob);
    }

    fn stop(&mut self, mob: &dyn Mob) {
        self.0.stop(mob);
    }

    fn tick(&mut self, mob: &dyn Mob) {
        self.0.tick(mob);
    }

    fn controls(&self) -> Controls {
        self.0.controls() | Controls::LOOK
    }
}
