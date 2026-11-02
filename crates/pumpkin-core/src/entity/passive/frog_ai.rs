use std::sync::Arc;

use pumpkin_data::Block;
use pumpkin_data::BlockDirection;
use pumpkin_data::entity::{EntityPose, EntityType};
use pumpkin_data::environment_attribute::Activity;
use pumpkin_data::fluid::Fluid;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::tag::{self, Taggable};
use pumpkin_util::math::position::BlockPos;
use pumpkin_world::world::BlockFlags;
use std::sync::atomic::Ordering::Relaxed;

use super::frog::{FrogEntity, can_eat};
use crate::block::blocks::frogspawn::FrogspawnBlock;
use crate::entity::EntityBase;
use crate::entity::ai::brain::activity::ActivityData;
use crate::entity::ai::brain::behavior::long_jump::{
    JumpCooldown, LongJumpMidJump, LongJumpSettings, LongJumpToRandomPos,
};
use crate::entity::ai::brain::behavior::one_shot::trigger_if;
use crate::entity::ai::brain::behavior::timed::Behavior;
use crate::entity::ai::brain::behavior::try_find_land::{try_find_land, try_find_land_near_liquid};
use crate::entity::ai::brain::behavior::utils::{self, is_breeding};
use crate::entity::ai::brain::behavior::{
    AnimalMakeLove, AnimalPanic, BehaviorControl, CountDownCooldownTicks, FollowTemptation,
    GateBehavior, LookAtTargetSink, MoveToTargetSink, OneShot, OrderPolicy, RunningPolicy, Timed,
    random_stroll, run_one, run_one_with_conditions, set_entity_look_target_sometimes,
    set_walk_target_from_look_target, start_attacking, stop_attacking_if_target_invalid,
};
use crate::entity::ai::brain::memory::walk_target::WalkTarget;
use crate::entity::ai::brain::memory::{MemoryModuleId, MemoryStatus, types};
use crate::entity::ai::brain::{BrainState, BrainTick};
use crate::entity::ai::pathfinder::node::PathType;
use crate::entity::mob::Mob;

/// Vanilla `FrogAi.TIME_BETWEEN_LONG_JUMPS`.
pub const TIME_BETWEEN_LONG_JUMPS: JumpCooldown = 100..=140;
/// Vanilla `FrogAi.MAX_LONG_JUMP_HEIGHT`.
const MAX_LONG_JUMP_HEIGHT: i32 = 2;
/// Vanilla `FrogAi.MAX_LONG_JUMP_WIDTH`.
const MAX_LONG_JUMP_WIDTH: i32 = 4;
/// Vanilla `FrogAi.MAX_JUMP_VELOCITY_MULTIPLIER`.
const MAX_JUMP_VELOCITY_MULTIPLIER: f32 = 3.571_428_8;
/// Vanilla `ShootTongue.EATING_DISTANCE`.
const EATING_DISTANCE: f32 = 1.75;
/// Vanilla `ShootTongue.EATING_MOVEMENT_FACTOR`.
const EATING_MOVEMENT_FACTOR: f64 = 0.75;
/// Vanilla `ShootTongue.CATCH_ANIMATION_DURATION`.
const CATCH_ANIMATION_DURATION: i32 = 6;
/// Vanilla `ShootTongue.TONGUE_ANIMATION_DURATION`.
const TONGUE_ANIMATION_DURATION: i32 = 10;
/// Vanilla `ShootTongue.UNREACHABLE_TONGUE_TARGETS_COOLDOWN_DURATION`.
const UNREACHABLE_TARGETS_COOLDOWN: i64 = 100;
/// Vanilla `ShootTongue.MAX_UNREACHBLE_TONGUE_TARGETS_IN_MEMORY`.
const MAX_UNREACHABLE_TARGETS: usize = 5;
/// Vanilla `Croak.CROAK_TICKS`.
const CROAK_TICKS: i32 = 60;

fn as_frog(mob: &dyn Mob) -> Option<&FrogEntity> {
    mob.cast_any().downcast_ref::<FrogEntity>()
}

const CROAK_CONDITIONS: [(MemoryModuleId, MemoryStatus); 1] =
    [(types::WALK_TARGET.id(), MemoryStatus::ValueAbsent)];

/// Vanilla `Croak`.
#[derive(Default)]
struct Croak {
    counter: i32,
}

impl Behavior for Croak {
    fn entry_conditions(&self) -> &[(MemoryModuleId, MemoryStatus)] {
        &CROAK_CONDITIONS
    }

    fn min_duration(&self) -> i32 {
        100
    }

    fn max_duration(&self) -> i32 {
        100
    }

    fn check_extra_start_conditions(&mut self, tick: &mut BrainTick<'_>) -> bool {
        tick.mob.get_entity().pose.load() == EntityPose::Standing
    }

    fn can_still_use(&mut self, _tick: &mut BrainTick<'_>) -> bool {
        self.counter < CROAK_TICKS
    }

    fn start(&mut self, tick: &mut BrainTick<'_>) {
        let entity = tick.mob.get_entity();
        if !entity.is_in_water() && !entity.touching_lava.load(Relaxed) {
            entity.set_pose(EntityPose::Croaking);
            self.counter = 0;
        }
    }

    fn stop(&mut self, tick: &mut BrainTick<'_>) {
        tick.mob.get_entity().set_pose(EntityPose::Standing);
    }

    fn tick(&mut self, _tick: &mut BrainTick<'_>) {
        self.counter += 1;
    }

    fn debug_name(&self) -> &'static str {
        "Croak"
    }
}

const TONGUE_CONDITIONS: [(MemoryModuleId, MemoryStatus); 4] = [
    (types::WALK_TARGET.id(), MemoryStatus::ValueAbsent),
    (types::LOOK_TARGET.id(), MemoryStatus::Registered),
    (types::ATTACK_TARGET.id(), MemoryStatus::ValuePresent),
    (types::IS_PANICKING.id(), MemoryStatus::ValueAbsent),
];

/// Vanilla `ShootTongue.State`.
#[derive(Clone, Copy, PartialEq, Eq)]
enum TongueState {
    MoveToTarget,
    CatchAnimation,
    EatAnimation,
    Done,
}

/// Vanilla `ShootTongue`: walks up to the prey, pulls it in and eats it.
struct ShootTongue {
    eat_animation_timer: i32,
    calculate_path_counter: i32,
    state: TongueState,
}

impl Default for ShootTongue {
    fn default() -> Self {
        Self {
            eat_animation_timer: 0,
            calculate_path_counter: 0,
            state: TongueState::Done,
        }
    }
}

/// Vanilla `ShootTongue.canPathfindToTarget`.
fn can_pathfind_to_target(mob: &dyn Mob, target: &dyn EntityBase) -> bool {
    let mob_entity = mob.get_mob_entity();
    mob_entity
        .navigator
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .create_path(&mob_entity.living_entity, target.get_entity().pos.load(), 0)
        .is_some_and(|path| path.get_dist_to_target() < EATING_DISTANCE)
}

/// Vanilla `ShootTongue.addUnreachableTargetToMemory`.
fn add_unreachable_target(state: &mut BrainState, target: &dyn EntityBase) {
    let uuid = target.get_entity().entity_uuid;
    let mut targets = state
        .get(types::UNREACHABLE_TONGUE_TARGETS)
        .cloned()
        .unwrap_or_default();
    if !targets.contains(&uuid) {
        if targets.len() == MAX_UNREACHABLE_TARGETS {
            targets.remove(0);
        }
        targets.push(uuid);
    }
    state.set_with_expiry(
        types::UNREACHABLE_TONGUE_TARGETS,
        targets,
        UNREACHABLE_TARGETS_COOLDOWN,
    );
}

impl ShootTongue {
    /// Vanilla `eatEntity`.
    fn eat(tick: &BrainTick<'_>, target: &Arc<dyn EntityBase>) {
        tick.mob.play_entity_sound(Sound::EntityFrogEat, 2.0, 1.0);
        if target.get_entity().is_alive() {
            tick.mob.do_hurt_target(target.as_ref());
            if !target.get_entity().is_alive() {
                target.get_entity().remove();
            }
        }
    }
}

impl Behavior for ShootTongue {
    fn entry_conditions(&self) -> &[(MemoryModuleId, MemoryStatus)] {
        &TONGUE_CONDITIONS
    }

    fn min_duration(&self) -> i32 {
        100
    }

    fn max_duration(&self) -> i32 {
        100
    }

    fn check_extra_start_conditions(&mut self, tick: &mut BrainTick<'_>) -> bool {
        let Some(target) = tick.state.get(types::ATTACK_TARGET).cloned() else {
            return false;
        };
        let reachable = can_pathfind_to_target(tick.mob, target.as_ref());
        if !reachable {
            tick.state.erase(types::ATTACK_TARGET.id());
            add_unreachable_target(tick.state, target.as_ref());
        }
        reachable
            && tick.mob.get_entity().pose.load() != EntityPose::Croaking
            && can_eat(target.as_ref())
    }

    fn can_still_use(&mut self, tick: &mut BrainTick<'_>) -> bool {
        tick.state.has_memory_value(types::ATTACK_TARGET.id())
            && self.state != TongueState::Done
            && !tick.state.has_memory_value(types::IS_PANICKING.id())
    }

    fn start(&mut self, tick: &mut BrainTick<'_>) {
        let Some(target) = tick.state.get(types::ATTACK_TARGET).cloned() else {
            return;
        };
        if let Some(frog) = as_frog(tick.mob) {
            frog.set_tongue_target(Some(target.get_entity().entity_id));
        }
        tick.state.set(
            types::WALK_TARGET,
            WalkTarget::from_vec(target.get_entity().pos.load(), 2.0, 0),
        );
        utils::look_at_entity(tick.state, target);
        self.calculate_path_counter = 10;
        self.state = TongueState::MoveToTarget;
    }

    fn stop(&mut self, tick: &mut BrainTick<'_>) {
        tick.state.erase(types::ATTACK_TARGET.id());
        if let Some(frog) = as_frog(tick.mob) {
            frog.set_tongue_target(None);
        }
        tick.mob.get_entity().set_pose(EntityPose::Standing);
    }

    fn tick(&mut self, tick: &mut BrainTick<'_>) {
        let Some(target) = tick.state.get(types::ATTACK_TARGET).cloned() else {
            return;
        };
        let entity = tick.mob.get_entity();
        let target_entity = target.get_entity();
        if let Some(frog) = as_frog(tick.mob) {
            frog.set_tongue_target(Some(target_entity.entity_id));
        }
        match self.state {
            TongueState::MoveToTarget => {
                let own = entity.pos.load();
                let prey = target_entity.pos.load();
                if prey.squared_distance_to_vec(&own).sqrt() < f64::from(EATING_DISTANCE) {
                    tick.mob
                        .play_entity_sound(Sound::EntityFrogTongue, 2.0, 1.0);
                    entity.set_pose(EntityPose::UsingTongue);
                    target_entity.set_velocity((own - prey).normalize() * EATING_MOVEMENT_FACTOR);
                    self.eat_animation_timer = 0;
                    self.state = TongueState::CatchAnimation;
                } else if self.calculate_path_counter <= 0 {
                    tick.state
                        .set(types::WALK_TARGET, WalkTarget::from_vec(prey, 2.0, 0));
                    self.calculate_path_counter = 10;
                } else {
                    self.calculate_path_counter -= 1;
                }
            }
            TongueState::CatchAnimation => {
                let timer = self.eat_animation_timer;
                self.eat_animation_timer += 1;
                if timer >= CATCH_ANIMATION_DURATION {
                    self.state = TongueState::EatAnimation;
                    Self::eat(tick, &target);
                }
            }
            TongueState::EatAnimation => {
                if self.eat_animation_timer >= TONGUE_ANIMATION_DURATION {
                    self.state = TongueState::Done;
                } else {
                    self.eat_animation_timer += 1;
                }
            }
            TongueState::Done => {}
        }
    }

    fn debug_name(&self) -> &'static str {
        "ShootTongue"
    }
}

/// Vanilla `TryLaySpawnOnFluidNearLand` with frogspawn: on the shore, onto the water beside it.
fn try_lay_spawn() -> OneShot {
    OneShot::new(
        "TryLaySpawnOnFluidNearLand",
        vec![
            (types::ATTACK_TARGET.id(), MemoryStatus::ValueAbsent),
            (types::WALK_TARGET.id(), MemoryStatus::ValuePresent),
            (types::IS_PREGNANT.id(), MemoryStatus::ValuePresent),
        ],
        |tick| {
            let entity = tick.mob.get_entity();
            if entity.is_in_water() || !entity.on_ground.load(Relaxed) {
                return false;
            }
            let below = entity.block_pos.load().down();
            let world = tick.world;
            for side in [
                BlockDirection::North,
                BlockDirection::South,
                BlockDirection::West,
                BlockDirection::East,
            ] {
                let beside = below.offset(side.to_offset());
                let supports = world
                    .get_exact_fluid(&beside)
                    .has_tag(&tag::Fluid::MINECRAFT_SUPPORTS_FROGSPAWN)
                    || world
                        .get_block(&beside)
                        .has_tag(&tag::Block::MINECRAFT_SUPPORTS_FROGSPAWN);
                let spawn_pos = beside.up();
                if !world
                    .get_block_state(&beside)
                    .is_side_solid(BlockDirection::Up)
                    && supports
                    && world.get_block_state(&spawn_pos).is_air()
                {
                    world.set_block_state(
                        &spawn_pos,
                        Block::FROGSPAWN.default_state.id,
                        BlockFlags::NOTIFY_ALL,
                    );
                    FrogspawnBlock::schedule_hatch_check(world, spawn_pos);
                    world.emit_game_event(
                        pumpkin_data::game_event::GameEvent::BlockPlace.name(),
                        spawn_pos.to_f64(),
                    );
                    world.play_sound(
                        Sound::EntityFrogLaySpawn,
                        SoundCategory::Blocks,
                        &entity.pos.load(),
                    );
                    tick.state.erase(types::IS_PREGNANT.id());
                    return true;
                }
            }
            true
        },
    )
}

/// Vanilla `FrogAi.isAcceptableLandingSpot` with `defaultAcceptableLandingSpot`.
fn is_acceptable_landing_spot(mob: &dyn Mob, target: &BlockPos) -> bool {
    let world = mob.get_entity().world.load();
    let below = target.down();
    if [*target, below, target.up()]
        .iter()
        .any(|pos| *world.get_fluid(pos) != Fluid::EMPTY)
    {
        return false;
    }
    let state = world.get_block_state(target);
    let prefer = &tag::Block::MINECRAFT_FROG_PREFER_JUMP_TO;
    if world.get_block(target).has_tag(prefer) || world.get_block(&below).has_tag(prefer) {
        return true;
    }
    let mob_entity = mob.get_mob_entity();
    let living = &mob_entity.living_entity;
    let mut navigator = mob_entity
        .navigator
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let path_type = navigator.get_path_type(living, *target);
    if path_type == PathType::Trapdoor
        || (state.is_air() && navigator.get_path_type(living, below) == PathType::Trapdoor)
    {
        return true;
    }
    world.get_block_state(&below).is_solid_render()
        && navigator.get_pathfinding_malus(path_type) == 0.0
}

/// Vanilla `FrogAi.canAttack`, with the memory `StartAttacking` reads.
fn find_attack_target(tick: &BrainTick<'_>) -> Option<Arc<dyn EntityBase>> {
    if is_breeding(tick.state) {
        return None;
    }
    tick.state.get(types::NEAREST_ATTACKABLE).cloned()
}

fn on_ground(tick: &BrainTick<'_>) -> bool {
    tick.mob.get_entity().on_ground.load(Relaxed)
}

fn look_at_players() -> OneShot {
    set_entity_look_target_sometimes::create_for(&EntityType::PLAYER, 6.0, 30, 60)
}

fn follow_temptation() -> Timed<FollowTemptation> {
    Timed::new(FollowTemptation::new(|_| 1.25, |_| 2.5))
}

fn idle_activity() -> ActivityData {
    ActivityData::new(
        Activity::Idle,
        vec![
            (0, Box::new(look_at_players())),
            (0, Box::new(Timed::new(AnimalMakeLove::new()))),
            (1, Box::new(follow_temptation())),
            (2, Box::new(start_attacking(find_attack_target))),
            (3, Box::new(try_find_land(6, 1.0))),
            (
                4,
                Box::new(run_one_with_conditions(
                    vec![(types::WALK_TARGET.id(), MemoryStatus::ValueAbsent)],
                    vec![
                        (Box::new(random_stroll::stroll(1.0)), 1),
                        (
                            Box::new(set_walk_target_from_look_target::create(1.0, 3)),
                            1,
                        ),
                        (Box::new(Timed::new(Croak::default())), 3),
                        (Box::new(trigger_if("OnGround", Vec::new(), on_ground)), 2),
                    ],
                )),
            ),
        ],
        vec![
            (types::LONG_JUMP_MID_JUMP.id(), MemoryStatus::ValueAbsent),
            (types::IS_IN_WATER.id(), MemoryStatus::ValueAbsent),
        ],
        Vec::new(),
    )
}

fn swim_activity() -> ActivityData {
    ActivityData::new(
        Activity::Swim,
        vec![
            (0, Box::new(look_at_players())),
            (1, Box::new(follow_temptation())),
            (2, Box::new(start_attacking(find_attack_target))),
            (3, Box::new(try_find_land(8, 1.5))),
            (
                5,
                Box::new(GateBehavior::new(
                    "FrogSwim",
                    vec![(types::WALK_TARGET.id(), MemoryStatus::ValueAbsent)],
                    Vec::new(),
                    OrderPolicy::Ordered,
                    RunningPolicy::TryAll,
                    vec![
                        (Box::new(random_stroll::swim(0.75)), 1),
                        (Box::new(random_stroll::stroll(1.0)), 1),
                        (
                            Box::new(set_walk_target_from_look_target::create(1.0, 3)),
                            1,
                        ),
                        (
                            Box::new(trigger_if("InWater", Vec::new(), |tick| {
                                tick.mob.get_entity().is_in_water()
                            })),
                            5,
                        ),
                    ],
                )),
            ),
        ],
        vec![
            (types::LONG_JUMP_MID_JUMP.id(), MemoryStatus::ValueAbsent),
            (types::IS_IN_WATER.id(), MemoryStatus::ValuePresent),
        ],
        Vec::new(),
    )
}

fn lay_spawn_activity() -> ActivityData {
    ActivityData::new(
        Activity::LaySpawn,
        vec![
            (0, Box::new(look_at_players())),
            (1, Box::new(start_attacking(find_attack_target))),
            (
                2,
                Box::new(try_find_land_near_liquid(
                    8,
                    1.0,
                    &tag::Fluid::MINECRAFT_FROG_TRIES_TO_FIND_LAND_NEAR,
                )),
            ),
            (3, Box::new(try_lay_spawn())),
            (
                4,
                Box::new(run_one(vec![
                    (Box::new(random_stroll::stroll(1.0)), 2),
                    (
                        Box::new(set_walk_target_from_look_target::create(1.0, 3)),
                        1,
                    ),
                    (Box::new(Timed::new(Croak::default())), 2),
                    (Box::new(trigger_if("OnGround", Vec::new(), on_ground)), 1),
                ])),
            ),
        ],
        vec![
            (types::LONG_JUMP_MID_JUMP.id(), MemoryStatus::ValueAbsent),
            (types::IS_PREGNANT.id(), MemoryStatus::ValuePresent),
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
                    Sound::EntityFrogStep,
                ))),
            ),
            (
                1,
                Box::new(Timed::new(
                    LongJumpToRandomPos::new(LongJumpSettings {
                        cooldown: TIME_BETWEEN_LONG_JUMPS,
                        max_height: MAX_LONG_JUMP_HEIGHT,
                        max_width: MAX_LONG_JUMP_WIDTH,
                        max_velocity_multiplier: MAX_JUMP_VELOCITY_MULTIPLIER,
                        jump_sound: Sound::EntityFrogLongJump,
                        acceptable_landing_spot: is_acceptable_landing_spot,
                    })
                    .preferring(&tag::Block::MINECRAFT_FROG_PREFER_JUMP_TO, 0.5),
                )),
            ),
        ],
        vec![
            (types::TEMPTING_PLAYER.id(), MemoryStatus::ValueAbsent),
            (types::BREED_TARGET.id(), MemoryStatus::ValueAbsent),
            (
                types::LONG_JUMP_COOLDOWN_TICKS.id(),
                MemoryStatus::ValueAbsent,
            ),
            (types::IS_IN_WATER.id(), MemoryStatus::ValueAbsent),
        ],
        Vec::new(),
    )
}

/// Vanilla `FrogAi.getActivities`.
pub fn activities(_mob: &dyn Mob) -> Vec<ActivityData> {
    let core: Vec<Box<dyn BehaviorControl>> = vec![
        Box::new(Timed::new(AnimalPanic::new(2.0))),
        Box::new(Timed::new(LookAtTargetSink::new(45, 90))),
        Box::new(Timed::new(MoveToTargetSink::default())),
        Box::new(CountDownCooldownTicks::new(
            types::TEMPTATION_COOLDOWN_TICKS,
        )),
        Box::new(CountDownCooldownTicks::new(types::LONG_JUMP_COOLDOWN_TICKS)),
    ];
    vec![
        ActivityData::with_priority_start(Activity::Core, 0, core),
        idle_activity(),
        swim_activity(),
        lay_spawn_activity(),
        ActivityData::with_gate_memory(
            Activity::Tongue,
            0,
            vec![
                Box::new(stop_attacking_if_target_invalid(|_, _| {})),
                Box::new(Timed::new(ShootTongue::default())),
            ],
            types::ATTACK_TARGET.id(),
        ),
        long_jump_activity(),
    ]
}

/// Vanilla `FrogAi.updateActivity`.
pub fn update_activity(brain: &mut BrainState) {
    brain.set_active_activity_to_first_valid(&[
        Activity::Tongue,
        Activity::LaySpawn,
        Activity::LongJump,
        Activity::Swim,
        Activity::Idle,
    ]);
}

/// Vanilla `TadpoleAi.getActivities`.
pub fn tadpole_activities(_mob: &dyn Mob) -> Vec<ActivityData> {
    let core: Vec<Box<dyn BehaviorControl>> = vec![
        Box::new(Timed::new(AnimalPanic::new(2.0))),
        Box::new(Timed::new(LookAtTargetSink::new(45, 90))),
        Box::new(Timed::new(MoveToTargetSink::default())),
        Box::new(CountDownCooldownTicks::new(
            types::TEMPTATION_COOLDOWN_TICKS,
        )),
    ];
    let wander = GateBehavior::new(
        "TadpoleSwim",
        vec![(types::WALK_TARGET.id(), MemoryStatus::ValueAbsent)],
        Vec::new(),
        OrderPolicy::Ordered,
        RunningPolicy::TryAll,
        vec![
            (Box::new(random_stroll::swim(0.5)), 2),
            (
                Box::new(set_walk_target_from_look_target::create(0.5, 3)),
                3,
            ),
            (
                Box::new(trigger_if("InWater", Vec::new(), |tick| {
                    tick.mob.get_entity().is_in_water()
                })),
                5,
            ),
        ],
    );
    vec![
        ActivityData::with_priority_start(Activity::Core, 0, core),
        ActivityData::new(
            Activity::Idle,
            vec![
                (0, Box::new(look_at_players())),
                (1, Box::new(follow_temptation())),
                (2, Box::new(wander)),
            ],
            Vec::new(),
            Vec::new(),
        ),
    ]
}
