//! Vanilla `GoatAi`, with `PrepareRamNearestTarget` and `RamTarget`, which only goats use.

use std::sync::Arc;
use std::sync::atomic::Ordering::Relaxed;

use pumpkin_data::attributes::Attributes;
use pumpkin_data::damage::DamageType;
use pumpkin_data::effect::StatusEffect;
use pumpkin_data::entity::{EntityStatus, EntityType};
use pumpkin_data::environment_attribute::Activity;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::tag::{self, Taggable};
use pumpkin_protocol::bedrock::server::actor_event::ActorEventID;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use rand::RngExt;

use super::goat::GoatEntity;
use crate::entity::ai::brain::activity::ActivityData;
use crate::entity::ai::brain::behavior::long_jump::{
    JumpCooldown, LongJumpMidJump, LongJumpSettings, LongJumpToRandomPos,
    default_acceptable_landing_spot,
};
use crate::entity::ai::brain::behavior::timed::Behavior;
use crate::entity::ai::brain::behavior::{
    AnimalMakeLove, AnimalPanic, BehaviorControl, CountDownCooldownTicks, DoNothing,
    FollowTemptation, LookAtTargetSink, MoveToTargetSink, Swim, Timed, animal, random_stroll,
    run_one, set_entity_look_target_sometimes, set_walk_target_from_look_target,
};
use crate::entity::ai::brain::memory::position_tracker::EntityTracker;
use crate::entity::ai::brain::memory::walk_target::WalkTarget;
use crate::entity::ai::brain::memory::{MemoryModuleId, MemoryStatus, types};
use crate::entity::ai::brain::{BrainState, BrainTick};
use crate::entity::ai::target_predicate::TargetPredicate;
use crate::entity::living::LivingEntity;
use crate::entity::mob::Mob;
use crate::entity::{Entity, EntityBase};
use crate::world::World;

/// Vanilla `GoatAi.TIME_BETWEEN_LONG_JUMPS`.
const TIME_BETWEEN_LONG_JUMPS: JumpCooldown = 600..=1200;
/// Vanilla `GoatAi.MAX_LONG_JUMP_HEIGHT` and `MAX_LONG_JUMP_WIDTH`.
const MAX_LONG_JUMP_SIZE: i32 = 5;
/// Vanilla `GoatAi.MAX_JUMP_VELOCITY_MULTIPLIER`.
const MAX_JUMP_VELOCITY_MULTIPLIER: f32 = 3.571_428_8;
/// Vanilla `GoatAi.TIME_BETWEEN_RAMS`.
const TIME_BETWEEN_RAMS: std::ops::RangeInclusive<i32> = 600..=6000;
/// Vanilla `GoatAi.TIME_BETWEEN_RAMS_SCREAMER`.
const TIME_BETWEEN_RAMS_SCREAMER: std::ops::RangeInclusive<i32> = 100..=300;
/// Vanilla `GoatAi.RAM_PREPARE_TIME`.
const RAM_PREPARE_TIME: i64 = 20;
/// Vanilla `GoatAi.RAM_MIN_DISTANCE`.
const RAM_MIN_DISTANCE: i32 = 4;
/// Vanilla `GoatAi.RAM_MAX_DISTANCE`.
const RAM_MAX_DISTANCE: i32 = 7;
/// Vanilla `GoatAi.SPEED_MULTIPLIER_WHEN_PREPARING_TO_RAM`.
const SPEED_WHEN_PREPARING: f32 = 1.25;
/// Vanilla `GoatAi.SPEED_MULTIPLIER_WHEN_RAMMING`.
const SPEED_WHEN_RAMMING: f32 = 3.0;
/// Vanilla `RamTarget.RAM_SPEED_FORCE_FACTOR`.
const RAM_SPEED_FORCE_FACTOR: f32 = 1.65;
/// Vanilla `PrepareRamNearestTarget.TIME_OUT_DURATION`.
const PREPARE_TIME_OUT: i32 = 160;
/// Vanilla `RamTarget.TIME_OUT_DURATION`.
const RAM_TIME_OUT: i32 = 200;

fn as_goat(mob: &dyn Mob) -> Option<&GoatEntity> {
    mob.cast_any().downcast_ref::<GoatEntity>()
}

fn is_screaming(mob: &dyn Mob) -> bool {
    as_goat(mob).is_some_and(GoatEntity::is_screaming)
}

/// Vanilla `GoatAi.initMemories`.
pub fn init_memories(state: &mut BrainState) {
    let mut rng = rand::rng();
    state.set(
        types::LONG_JUMP_COOLDOWN_TICKS,
        rng.random_range(TIME_BETWEEN_LONG_JUMPS),
    );
    state.set(
        types::RAM_COOLDOWN_TICKS,
        rng.random_range(TIME_BETWEEN_RAMS),
    );
}

/// Vanilla `GoatAi.RAM_TARGET_CONDITIONS`.
fn ram_targeting() -> TargetPredicate {
    let mut conditions = TargetPredicate::create_attackable();
    conditions.set_predicate(|target, world| {
        let griefing = world.level_info.load().game_rules.mob_griefing;
        let entity = &target.entity;
        let bounds = entity.bounding_box.load();
        let border = world
            .worldborder
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        entity.entity_type != &EntityType::GOAT
            && (griefing || entity.entity_type != &EntityType::ARMOR_STAND)
            && border.contains(bounds.min.x, bounds.min.z)
            && border.contains(bounds.max.x, bounds.max.z)
    });
    conditions
}

/// Vanilla `level.playSound(null, body, sound, SoundSource.NEUTRAL, 1.0F, 1.0F)`.
fn play_sound(tick: &BrainTick<'_>, sound: Sound) {
    tick.world.play_sound(
        sound,
        SoundCategory::Neutral,
        &tick.mob.get_entity().pos.load(),
    );
}

/// How strong an effect is: 0 without it.
fn effect_level(living: &LivingEntity, effect: &'static StatusEffect) -> f32 {
    living
        .get_effect(effect)
        .map_or(0.0, |active| f32::from(active.amplifier) + 1.0)
}

/// Whether a shield in use faces the goat, as `LivingEntity.damage_past_invulnerability` asks
/// for the hit itself: vanilla `applyItemBlocking` tells the ram to push less.
fn blocks_hit_from(target: &dyn EntityBase, attacker: &Entity) -> bool {
    let Some(living) = target.get_living_entity() else {
        return false;
    };
    if !living.is_blocking() {
        return false;
    }
    let pos = target.get_entity().pos.load();
    let look = Vector3::rotation_vector(0.0, f64::from(target.get_entity().yaw.load()));
    let mut from_attacker = (pos - attacker.pos.load()).normalize();
    from_attacker.y = 0.0;
    from_attacker.dot(&look) < 0.0
}

fn broadcast_ram(entity: &Entity, world: &World, status: EntityStatus) {
    world.broadcast_entity_event(entity, status, ActorEventID::None);
}

const PREPARE_CONDITIONS: [(MemoryModuleId, MemoryStatus); 4] = [
    (types::LOOK_TARGET.id(), MemoryStatus::Registered),
    (types::RAM_COOLDOWN_TICKS.id(), MemoryStatus::ValueAbsent),
    (
        types::NEAREST_VISIBLE_LIVING_ENTITIES.id(),
        MemoryStatus::ValuePresent,
    ),
    (types::RAM_TARGET.id(), MemoryStatus::ValueAbsent),
];

/// Vanilla `PrepareRamNearestTarget.RamCandidate`.
struct RamCandidate {
    start_position: BlockPos,
    target_position: BlockPos,
    target: Arc<dyn EntityBase>,
}

/// Vanilla `PrepareRamNearestTarget`: walks a straight run-up from the nearest target, lowers
/// its head and only then gives the ram a place to run to.
struct PrepareRam {
    targeting: TargetPredicate,
    reached_ram_position: Option<i64>,
    candidate: Option<RamCandidate>,
}

impl PrepareRam {
    fn new() -> Self {
        Self {
            targeting: ram_targeting(),
            reached_ram_position: None,
            candidate: None,
        }
    }

    /// Vanilla `isWalkableBlock`.
    fn is_walkable(mob: &dyn Mob, world: &World, pos: BlockPos) -> bool {
        let mob_entity = mob.get_mob_entity();
        let mut navigator = mob_entity
            .navigator
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !navigator.is_stable_destination(world, &pos) {
            return false;
        }
        let path_type = navigator.get_path_type(&mob_entity.living_entity, pos);
        navigator.get_pathfinding_malus(path_type) == 0.0
    }

    /// Vanilla `calculateRammingStartPosition`.
    fn start_position(mob: &dyn Mob, world: &World, target: &dyn EntityBase) -> Option<BlockPos> {
        let target_pos = target.get_entity().block_pos.load();
        if !Self::is_walkable(mob, world, target_pos) {
            return None;
        }
        let mut possible: Vec<BlockPos> = [(0, -1), (1, 0), (0, 1), (-1, 0)]
            .iter()
            .filter_map(|&(x, z)| {
                let mut furthest = target_pos;
                for _ in 0..RAM_MAX_DISTANCE {
                    let next = furthest.add(x, 0, z);
                    if !Self::is_walkable(mob, world, next) {
                        break;
                    }
                    furthest = next;
                }
                let distance =
                    (furthest.0.x - target_pos.0.x).abs() + (furthest.0.z - target_pos.0.z).abs();
                (distance >= RAM_MIN_DISTANCE).then_some(furthest)
            })
            .collect();
        let own = mob.get_entity().block_pos.load();
        possible.sort_by_key(|pos| {
            let delta = pos.0 - own.0;
            delta.x * delta.x + delta.y * delta.y + delta.z * delta.z
        });
        let mob_entity = mob.get_mob_entity();
        let mut navigator = mob_entity
            .navigator
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        possible.into_iter().find(|pos| {
            navigator
                .create_path(&mob_entity.living_entity, pos.to_centered_f64(), 0)
                .is_some_and(|path| path.can_reach())
        })
    }

    /// Vanilla `chooseRamPosition`.
    fn choose_ram_position(&mut self, mob: &dyn Mob, world: &World, target: Arc<dyn EntityBase>) {
        self.reached_ram_position = None;
        self.candidate =
            Self::start_position(mob, world, target.as_ref()).map(|start_position| RamCandidate {
                start_position,
                target_position: target.get_entity().block_pos.load(),
                target,
            });
    }
}

/// Vanilla `getEdgeOfBlock`.
fn edge_of_block(start: BlockPos, target: BlockPos) -> Vector3<f64> {
    let offset = |to: i32, from: i32| 0.5 * f64::from((to - from).signum());
    Vector3::new(
        f64::from(target.0.x) + 0.5 + offset(target.0.x, start.0.x),
        f64::from(target.0.y),
        f64::from(target.0.z) + 0.5 + offset(target.0.z, start.0.z),
    )
}

impl Behavior for PrepareRam {
    fn entry_conditions(&self) -> &[(MemoryModuleId, MemoryStatus)] {
        &PREPARE_CONDITIONS
    }

    fn min_duration(&self) -> i32 {
        PREPARE_TIME_OUT
    }

    fn max_duration(&self) -> i32 {
        PREPARE_TIME_OUT
    }

    fn start(&mut self, tick: &mut BrainTick<'_>) {
        let context = tick.visibility();
        let target = tick
            .state
            .get(types::NEAREST_VISIBLE_LIVING_ENTITIES)
            .and_then(|visible| {
                visible.find_closest(&context, |entity| {
                    self.targeting
                        .test(tick.world, Some(tick.mob), entity.as_ref())
                })
            });
        if let Some(target) = target {
            self.choose_ram_position(tick.mob, tick.world, target);
        }
    }

    fn can_still_use(&mut self, _tick: &mut BrainTick<'_>) -> bool {
        self.candidate
            .as_ref()
            .is_some_and(|candidate| candidate.target.get_entity().is_alive())
    }

    fn stop(&mut self, tick: &mut BrainTick<'_>) {
        if !tick.state.has_memory_value(types::RAM_TARGET.id()) {
            broadcast_ram(tick.mob.get_entity(), tick.world, EntityStatus::EndRam);
            tick.state.set(
                types::RAM_COOLDOWN_TICKS,
                if is_screaming(tick.mob) {
                    *TIME_BETWEEN_RAMS_SCREAMER.start()
                } else {
                    *TIME_BETWEEN_RAMS.start()
                },
            );
        }
    }

    fn tick(&mut self, tick: &mut BrainTick<'_>) {
        let Some(candidate) = self.candidate.as_ref() else {
            return;
        };
        let (start, target_position) = (candidate.start_position, candidate.target_position);
        let target = candidate.target.clone();
        tick.state.set(
            types::WALK_TARGET,
            WalkTarget::from_block_pos(start, SPEED_WHEN_PREPARING, 0),
        );
        tick.state.set(
            types::LOOK_TARGET,
            Arc::new(EntityTracker::new(target.clone(), true)),
        );
        let entity = tick.mob.get_entity();
        if target.get_entity().block_pos.load() != target_position {
            broadcast_ram(entity, tick.world, EntityStatus::EndRam);
            tick.mob
                .get_mob_entity()
                .navigator
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .stop();
            self.choose_ram_position(tick.mob, tick.world, target);
            return;
        }
        let own = entity.block_pos.load();
        if own != start {
            return;
        }
        broadcast_ram(entity, tick.world, EntityStatus::StartRam);
        let reached = *self.reached_ram_position.get_or_insert(tick.time);
        if tick.time - reached >= RAM_PREPARE_TIME {
            tick.state
                .set(types::RAM_TARGET, edge_of_block(own, target_position));
            let sound = if is_screaming(tick.mob) {
                Sound::EntityGoatScreamingPrepareRam
            } else {
                Sound::EntityGoatPrepareRam
            };
            play_sound(tick, sound);
            self.candidate = None;
        }
    }

    fn debug_name(&self) -> &'static str {
        "PrepareRamNearestTarget"
    }
}

const RAM_CONDITIONS: [(MemoryModuleId, MemoryStatus); 2] = [
    (types::RAM_COOLDOWN_TICKS.id(), MemoryStatus::ValueAbsent),
    (types::RAM_TARGET.id(), MemoryStatus::ValuePresent),
];

/// Vanilla `RamTarget`.
struct RamTarget {
    targeting: TargetPredicate,
    ram_direction: Vector3<f64>,
}

impl RamTarget {
    fn new() -> Self {
        Self {
            targeting: ram_targeting(),
            ram_direction: Vector3::new(0.0, 0.0, 0.0),
        }
    }

    /// Vanilla `finishRam`.
    fn finish(tick: &mut BrainTick<'_>) {
        broadcast_ram(tick.mob.get_entity(), tick.world, EntityStatus::EndRam);
        let cooldown = if is_screaming(tick.mob) {
            TIME_BETWEEN_RAMS_SCREAMER
        } else {
            TIME_BETWEEN_RAMS
        };
        tick.state.set(
            types::RAM_COOLDOWN_TICKS,
            rand::rng().random_range(cooldown),
        );
        tick.state.erase(types::RAM_TARGET.id());
    }

    /// Vanilla `level.getNearbyEntities(LivingEntity, ramTargeting, body, body.getBoundingBox())`.
    fn nearby_target(&self, tick: &BrainTick<'_>) -> Option<Arc<dyn EntityBase>> {
        let entity = tick.mob.get_entity();
        let bounds = entity.bounding_box.load();
        let hits = |other: &dyn EntityBase| {
            other.get_entity().entity_id != entity.entity_id
                && other.get_living_entity().is_some()
                && self.targeting.test(tick.world, Some(tick.mob), other)
        };
        tick.world
            .get_entities_at_box(&bounds)
            .into_iter()
            .find(|other| hits(other.as_ref()))
            .or_else(|| {
                tick.world
                    .get_players_at_box(&bounds)
                    .into_iter()
                    .find(|player| hits(player.as_ref()))
                    .map(|player| player as Arc<dyn EntityBase>)
            })
    }

    /// The hit in vanilla `RamTarget.tick`.
    fn ram(&self, tick: &BrainTick<'_>, target: &dyn EntityBase) {
        let living = &tick.mob.get_mob_entity().living_entity;
        let damage = living.get_attribute_value(&Attributes::ATTACK_DAMAGE) as f32;
        target.damage_with_context(
            target,
            damage,
            DamageType::MOB_ATTACK_NO_AGGRO,
            None,
            Some(tick.mob),
            Some(tick.mob),
        );
        let boost = 0.25
            * (effect_level(living, &StatusEffect::SPEED)
                - effect_level(living, &StatusEffect::SLOWNESS));
        let speed_factor = (living.get_speed() * RAM_SPEED_FORCE_FACTOR).clamp(0.2, 3.0) + boost;
        let blocked = blocks_hit_from(target, tick.mob.get_entity());
        let force = if tick.mob.get_entity().age.load(Relaxed) < 0 {
            1.0
        } else {
            2.5
        };
        let strength = f64::from(if blocked { 0.5 } else { 1.0 } * speed_factor) * force;
        let other = target.get_entity();
        let resistance = target.get_living_entity().map_or(0.0, |other| {
            other.get_attribute_value(&Attributes::KNOCKBACK_RESISTANCE)
        });
        other.apply_knockback(
            strength * (1.0 - resistance),
            self.ram_direction.x,
            self.ram_direction.z,
        );
    }

    /// Vanilla `hasRammedHornBreakingBlock`.
    fn hit_horn_breaking_block(tick: &BrainTick<'_>) -> bool {
        let entity = tick.mob.get_entity();
        let velocity = entity.velocity.load();
        let flat = Vector3::new(velocity.x, 0.0, velocity.z);
        let length = flat.length();
        let facing = if length < 1.0E-4 {
            entity.pos.load()
        } else {
            entity.pos.load() + flat * (1.0 / length)
        };
        let pos = BlockPos::floored(facing.x, facing.y, facing.z);
        let snaps = &tag::Block::MINECRAFT_SNAPS_GOAT_HORN;
        tick.world.get_block(&pos).has_tag(snaps) || tick.world.get_block(&pos.up()).has_tag(snaps)
    }

    fn play_impact(tick: &BrainTick<'_>) {
        let sound = if is_screaming(tick.mob) {
            Sound::EntityGoatScreamingRamImpact
        } else {
            Sound::EntityGoatRamImpact
        };
        play_sound(tick, sound);
    }
}

impl Behavior for RamTarget {
    fn entry_conditions(&self) -> &[(MemoryModuleId, MemoryStatus)] {
        &RAM_CONDITIONS
    }

    fn min_duration(&self) -> i32 {
        RAM_TIME_OUT
    }

    fn max_duration(&self) -> i32 {
        RAM_TIME_OUT
    }

    fn can_still_use(&mut self, tick: &mut BrainTick<'_>) -> bool {
        tick.state.has_memory_value(types::RAM_TARGET.id())
    }

    fn start(&mut self, tick: &mut BrainTick<'_>) {
        let own = tick.mob.get_entity().block_pos.load();
        let Some(&target) = tick.state.get(types::RAM_TARGET) else {
            return;
        };
        let direction = Vector3::new(
            f64::from(own.0.x) - target.x,
            0.0,
            f64::from(own.0.z) - target.z,
        );
        self.ram_direction = direction.normalize();
        tick.state.set(
            types::WALK_TARGET,
            WalkTarget::from_vec(target, SPEED_WHEN_RAMMING, 0),
        );
    }

    fn tick(&mut self, tick: &mut BrainTick<'_>) {
        if let Some(target) = self.nearby_target(tick) {
            self.ram(tick, target.as_ref());
            Self::finish(tick);
            Self::play_impact(tick);
        } else if Self::hit_horn_breaking_block(tick) {
            Self::play_impact(tick);
            if as_goat(tick.mob).is_some_and(GoatEntity::drop_horn) {
                play_sound(tick, Sound::ItemGoatHornBreak);
            }
            Self::finish(tick);
        } else {
            let reached = match (
                tick.state.get(types::WALK_TARGET),
                tick.state.get(types::RAM_TARGET),
            ) {
                (Some(walk), Some(ram)) => {
                    (walk.target().position() - *ram).length_squared() < 0.25 * 0.25
                }
                _ => true,
            };
            if reached {
                Self::finish(tick);
            }
        }
    }

    fn debug_name(&self) -> &'static str {
        "RamTarget"
    }
}

fn long_jump_sound(mob: &dyn Mob) -> Sound {
    if is_screaming(mob) {
        Sound::EntityGoatScreamingLongJump
    } else {
        Sound::EntityGoatLongJump
    }
}

fn idle_activity() -> ActivityData {
    ActivityData::new(
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
            (0, Box::new(Timed::new(AnimalMakeLove::new()))),
            (
                1,
                Box::new(Timed::new(FollowTemptation::new(|_| 1.25, |_| 2.5))),
            ),
            (2, Box::new(animal::baby_follow_adult(5, 16, |_| 1.25))),
            (
                3,
                Box::new(run_one(vec![
                    (Box::new(random_stroll::stroll(1.0)), 2),
                    (
                        Box::new(set_walk_target_from_look_target::create(1.0, 3)),
                        2,
                    ),
                    (Box::new(Timed::new(DoNothing::new(30, 60))), 1),
                ])),
            ),
        ],
        vec![
            (types::RAM_COOLDOWN_TICKS.id(), MemoryStatus::ValuePresent),
            (
                types::LONG_JUMP_COOLDOWN_TICKS.id(),
                MemoryStatus::ValuePresent,
            ),
        ],
        Vec::new(),
    )
}

fn long_jump_activity() -> ActivityData {
    ActivityData::new(
        Activity::LongJump,
        vec![
            (
                0,
                Box::new(Timed::new(LongJumpMidJump::new(
                    TIME_BETWEEN_LONG_JUMPS,
                    Sound::EntityGoatStep,
                ))),
            ),
            (
                1,
                Box::new(Timed::new(LongJumpToRandomPos::new(LongJumpSettings {
                    cooldown: TIME_BETWEEN_LONG_JUMPS,
                    max_height: MAX_LONG_JUMP_SIZE,
                    max_width: MAX_LONG_JUMP_SIZE,
                    max_velocity_multiplier: MAX_JUMP_VELOCITY_MULTIPLIER,
                    jump_sound: long_jump_sound,
                    acceptable_landing_spot: default_acceptable_landing_spot,
                }))),
            ),
        ],
        vec![
            (types::TEMPTING_PLAYER.id(), MemoryStatus::ValueAbsent),
            (types::BREED_TARGET.id(), MemoryStatus::ValueAbsent),
            (types::WALK_TARGET.id(), MemoryStatus::ValueAbsent),
            (
                types::LONG_JUMP_COOLDOWN_TICKS.id(),
                MemoryStatus::ValueAbsent,
            ),
        ],
        Vec::new(),
    )
}

fn ram_activity() -> ActivityData {
    ActivityData::new(
        Activity::Ram,
        vec![
            (0, Box::new(Timed::new(RamTarget::new()))),
            (1, Box::new(Timed::new(PrepareRam::new()))),
        ],
        vec![
            (types::TEMPTING_PLAYER.id(), MemoryStatus::ValueAbsent),
            (types::BREED_TARGET.id(), MemoryStatus::ValueAbsent),
            (types::RAM_COOLDOWN_TICKS.id(), MemoryStatus::ValueAbsent),
        ],
        Vec::new(),
    )
}

/// Vanilla `GoatAi.getActivities`.
pub fn activities(_mob: &dyn Mob) -> Vec<ActivityData> {
    let core: Vec<Box<dyn BehaviorControl>> = vec![
        Box::new(Timed::new(Swim::new(0.8))),
        Box::new(Timed::new(AnimalPanic::new(2.0))),
        Box::new(Timed::new(LookAtTargetSink::new(45, 90))),
        Box::new(Timed::new(MoveToTargetSink::default())),
        Box::new(CountDownCooldownTicks::new(
            types::TEMPTATION_COOLDOWN_TICKS,
        )),
        Box::new(CountDownCooldownTicks::new(types::LONG_JUMP_COOLDOWN_TICKS)),
        Box::new(CountDownCooldownTicks::new(types::RAM_COOLDOWN_TICKS)),
    ];
    vec![
        ActivityData::with_priority_start(Activity::Core, 0, core),
        idle_activity(),
        long_jump_activity(),
        ram_activity(),
    ]
}

/// Vanilla `GoatAi.updateActivity`.
pub fn update_activity(brain: &mut BrainState) {
    brain.set_active_activity_to_first_valid(&[Activity::Ram, Activity::LongJump, Activity::Idle]);
}
