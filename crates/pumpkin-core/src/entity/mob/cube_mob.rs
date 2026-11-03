//! Vanilla `AbstractCubeMob`: what slimes, magma cubes and sulfur cubes share.

use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::{Arc, PoisonError};

use crossbeam::atomic::AtomicCell;
use pumpkin_data::attributes::Attributes;
use pumpkin_data::effect::StatusEffect;
use pumpkin_data::sound::Sound;
use pumpkin_data::tracked_data;
use pumpkin_util::math::boundingbox::EntityDimensions;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_util::math::wrap_degrees;
use rand::RngExt;

use crate::entity::ai::control::{Control, MoveControlTrait, move_control::MoveControl};
use crate::entity::ai::goal::goal_selector::GoalSelector;
use crate::entity::ai::goal::{Controls, Goal, to_goal_ticks};
use crate::entity::mob::{Mob, MobEntity};
use crate::entity::{Entity, EntityBase, RemovalReason};

pub const MIN_SIZE: i32 = 1;
pub const MAX_SIZE: i32 = 127;

/// Speed modifier `CubeMobFloatGoal` swims with.
const FLOAT_SPEED_MODIFIER: f64 = 1.2;
/// Speed modifier `CubeMobKeepOnJumpingGoal` hops with.
const JUMPING_SPEED_MODIFIER: f64 = 1.0;
/// Ticks `CubeMobAttackGoal` chases before it gives up for a moment.
const GROW_TIRED_TICKS: i32 = 300;
/// Degrees per tick `CubeMobAttackGoal` and the sulfur cube's goals turn towards their target.
pub const ATTACK_TURN_SPEED: f32 = 10.0;
/// Vanilla `decreaseSquish`: the part of the squish kept each tick.
pub const DEFAULT_SQUISH_DECAY: f32 = 0.6;

/// The fields of vanilla `AbstractCubeMob` and its `CubeMobMoveControl`.
pub struct CubeMobState {
    size: AtomicI32,
    pub(crate) jump_delay: AtomicI32,
    target_yaw: AtomicCell<f32>,
    is_aggressive: AtomicBool,
    speed_modifier: AtomicCell<Option<f64>>,
    was_on_ground: AtomicBool,
    pub squish: AtomicCell<f32>,
    pub target_squish: AtomicCell<f32>,
    pub o_squish: AtomicCell<f32>,
}

impl Default for CubeMobState {
    fn default() -> Self {
        Self {
            size: AtomicI32::new(MIN_SIZE),
            jump_delay: AtomicI32::new(0),
            target_yaw: AtomicCell::new(0.0),
            is_aggressive: AtomicBool::new(false),
            speed_modifier: AtomicCell::new(None),
            was_on_ground: AtomicBool::new(false),
            squish: AtomicCell::new(0.0),
            target_squish: AtomicCell::new(0.0),
            o_squish: AtomicCell::new(0.0),
        }
    }
}

impl CubeMobState {
    #[must_use]
    pub fn get_size(&self) -> i32 {
        self.size.load(Ordering::Relaxed)
    }

    #[must_use]
    pub fn is_tiny(&self) -> bool {
        self.get_size() <= MIN_SIZE
    }

    #[must_use]
    pub fn was_on_ground(&self) -> bool {
        self.was_on_ground.load(Ordering::Relaxed)
    }

    pub fn set_was_on_ground(&self, on_ground: bool) {
        self.was_on_ground.store(on_ground, Ordering::Relaxed);
    }

    /// Vanilla `CubeMobMoveControl.setDirection`: the yaw to turn to and the jump rhythm.
    pub fn set_direction(&self, yaw: f32, is_aggressive: bool) {
        self.target_yaw.store(yaw);
        self.is_aggressive.store(is_aggressive, Ordering::Relaxed);
    }

    /// Vanilla `CubeMobMoveControl.setWantedMovement`: hop on next tick.
    pub fn set_wanted_movement(&self, speed_modifier: f64) {
        self.speed_modifier.store(Some(speed_modifier));
    }

    /// Vanilla `AbstractCubeMob.tick` before `super.tick()`.
    pub fn tick_squish(&self) {
        let squish = self.squish.load();
        self.o_squish.store(squish);
        self.squish
            .store(squish + (self.target_squish.load() - squish) * 0.5);
    }
}

/// What a cube mob species overrides of vanilla `AbstractCubeMob`.
pub trait CubeMob: Mob {
    fn cube_state(&self) -> &CubeMobState;

    /// Vanilla `getJumpSound`.
    fn jump_sound(&self) -> Sound;

    /// Vanilla `getSquishSound`.
    fn squish_sound(&self) -> Sound;

    /// Vanilla `canDealDamage`: whether touching hurts.
    fn can_deal_damage(&self) -> bool;

    /// Vanilla `getJumpDelay`.
    fn jump_delay(&self) -> i32 {
        self.get_random().random_range(10..30)
    }

    /// Vanilla `decreaseSquish`: the part of the squish kept each tick.
    fn squish_decay(&self) -> f32 {
        DEFAULT_SQUISH_DECAY
    }

    /// Vanilla `setCubeMobHealth`.
    fn cube_max_health(&self, actual_size: i32) -> f64 {
        f64::from(actual_size * actual_size)
    }

    /// Vanilla `SulfurCubeMobMoveControl.tick`: `false` leaves the move control idle.
    fn hops(&self) -> bool {
        true
    }

    fn is_tiny(&self) -> bool {
        self.cube_state().is_tiny()
    }

    /// Vanilla `getSoundVolume`.
    fn cube_sound_volume(&self) -> f32 {
        0.4 * self.cube_state().get_size() as f32
    }

    /// Vanilla `getSoundPitch`, for the jump sound.
    fn cube_sound_pitch(&self) -> f32 {
        let pitch_adjuster = if self.is_tiny() { 1.4 } else { 0.8 };
        let mut rng = self.get_random();
        ((rng.random::<f32>() - rng.random::<f32>()) * 0.2 + 1.0) * pitch_adjuster
    }

    /// Vanilla `AbstractCubeMob.setSize`: resizes the cube and sets the attributes that follow
    /// its size. Returns the size after clamping.
    fn set_cube_size(&self, size: i32, update_health: bool) -> i32 {
        let actual_size = size.clamp(MIN_SIZE, MAX_SIZE);
        let living = &self.get_mob_entity().living_entity;
        let entity = &living.entity;
        self.cube_state().size.store(actual_size, Ordering::Relaxed);
        entity.set_synced_data(tracked_data::abstract_cube_mob::ID_SIZE, actual_size);

        // Vanilla `AbstractCubeMob.refreshDimensions` keeps the position a resize would fudge.
        let pos = entity.pos.load();
        let entity_type = entity.entity_type;
        let dimensions = EntityDimensions::new(
            entity_type.dimension[0],
            entity_type.dimension[1],
            entity_type.eye_height,
        );
        entity.set_default_dimensions(dimensions.scale(actual_size as f32));
        entity.set_pos(pos);

        living.set_attribute_base(&Attributes::MAX_HEALTH, self.cube_max_health(actual_size));
        living.set_attribute_base(
            &Attributes::MOVEMENT_SPEED,
            f64::from(0.2f32 + 0.1 * actual_size as f32),
        );
        if update_health {
            living.set_health(living.get_max_health());
        }

        if let Some(meta) = self.mob_metadata() {
            entity.send_bedrock_actor_data(&meta.to_bedrock_actor_data());
        }
        actual_size
    }
}

/// Vanilla `AbstractCubeMob` constructor and `CubeMobFloatGoal`: its move control and floating.
pub fn install_move_control(mob: &MobEntity, state: Arc<CubeMobState>) {
    *mob.move_control
        .lock()
        .unwrap_or_else(PoisonError::into_inner) = Box::new(CubeMobMoveControl::new(state));
    mob.navigator
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .set_can_float(true);
}

/// The goals of vanilla `AbstractCubeMob.registerGoals`.
pub fn add_common_goals(goals: &GoalSelector) {
    goals.add_goal(1, Box::new(CubeMobFloatGoal));
    goals.add_goal(4, Box::new(CubeMobRandomDirectionGoal::default()));
    goals.add_goal(5, Box::new(CubeMobKeepOnJumpingGoal));
}

/// Vanilla `AbstractCubeMob.tick` after `super.tick()`: the landing squish.
pub fn land_squish(cube: &dyn CubeMob) {
    let state = cube.cube_state();
    let on_ground = cube.get_entity().on_ground.load(Ordering::Relaxed);
    let was_on_ground = state.was_on_ground.swap(on_ground, Ordering::Relaxed);
    if on_ground && !was_on_ground {
        // The client draws the landing particles; vanilla's server adds none.
        let mut rng = cube.get_random();
        let pitch = ((rng.random::<f32>() - rng.random::<f32>()) * 0.2 + 1.0) / 0.8;
        cube.play_entity_sound(cube.squish_sound(), cube.cube_sound_volume(), pitch);
        state.target_squish.store(-0.5);
    } else if !on_ground && was_on_ground {
        state.target_squish.store(1.0);
    }
    state
        .target_squish
        .store(state.target_squish.load() * cube.squish_decay());
}

/// Vanilla `jumpFromGround`: straight up with the jump power, `boost` on top.
pub fn jump_straight_up(mob: &dyn Mob, boost: f32) {
    let entity = mob.get_entity();
    let mut velocity = entity.velocity.load();
    velocity.y = mob.get_jump_power() + f64::from(boost);
    entity.velocity.store(velocity);
    entity.velocity_dirty.store(true, Ordering::Relaxed);
}

/// Vanilla `Mob.lookAt`: turns towards `target` by at most `max_turn` degrees.
pub fn look_at(entity: &Entity, target: &dyn EntityBase, max_turn: f32) {
    let target_entity = target.get_entity();
    let pos = entity.pos.load();
    let target_pos = target_entity.pos.load();
    let dx = target_pos.x - pos.x;
    let dz = target_pos.z - pos.z;
    let dy = if target.get_living_entity().is_some() {
        target_entity.get_eye_y() - entity.get_eye_y()
    } else {
        let bounding_box = target_entity.bounding_box.load();
        f64::midpoint(bounding_box.min.y, bounding_box.max.y) - entity.get_eye_y()
    };
    let horizontal = dx.hypot(dz);
    let yaw = dz.atan2(dx).to_degrees() as f32 - 90.0;
    let pitch = -(dy.atan2(horizontal).to_degrees() as f32);
    entity.set_pitch(rotlerp(entity.pitch.load(), pitch, max_turn));
    entity.yaw.store(rotlerp(entity.yaw.load(), yaw, max_turn));
}

/// Vanilla `Mob.rotlerp`: `current` turned towards `target` by at most `max_turn`, unwrapped.
fn rotlerp(current: f32, target: f32, max_turn: f32) -> f32 {
    current + wrap_degrees(target - current).clamp(-max_turn, max_turn)
}

/// The test of vanilla `AbstractCubeMob.remove`. Chunk unloads keep the dying cube whole, so its
/// children never land in an unloading chunk.
pub fn should_split(cube: &dyn CubeMob) -> bool {
    let destroyed = cube
        .get_entity()
        .removal_reason
        .load()
        .as_ref()
        .is_some_and(RemovalReason::should_destroy);
    cube.cube_state().get_size() > MIN_SIZE
        && destroyed
        && cube.get_mob_entity().living_entity.is_dead_or_dying()
}

/// The loop of vanilla `AbstractCubeMob.remove`: `count` children of half the parent's size.
///
/// `create` builds a child and `set_up` is vanilla `setUpSplitCube`'s part before `snapTo`.
pub fn spawn_split_cubes<C: CubeMob + 'static>(
    parent: &C,
    count: i32,
    create: impl Fn(Entity) -> Arc<C>,
    set_up: impl Fn(&C, i32),
) {
    let entity = parent.get_entity();
    let world = entity.world.load_full();
    let pos = entity.pos.load();
    let offset = entity.entity_dimension.load().width / 2.0;
    let half_size = parent.cube_state().get_size() / 2;
    let mut rng = parent.get_random();
    let team = parent.get_team_name();
    for i in 0..count {
        let xd = ((i % 2) as f32 - 0.5) * offset;
        let zd = ((i / 2) as f32 - 0.5) * offset;
        let child = create(Entity::new(world.clone(), pos, entity.entity_type));
        copy_split_state(parent, &*child, team.as_deref());
        set_up(&*child, half_size);
        let child_entity = child.get_entity();
        child_entity.set_pos(Vector3::new(
            pos.x + f64::from(xd),
            pos.y + 0.5,
            pos.z + f64::from(zd),
        ));
        child_entity.yaw.store(rng.random::<f32>() * 360.0);
        child_entity.set_pitch(0.0);
        world.spawn_entity(child);
    }
}

/// Vanilla `ConversionType.convertCommon`, for the state Pumpkin keeps.
fn copy_split_state(parent: &dyn Mob, child: &dyn Mob, team: Option<&str>) {
    let from = parent.get_entity();
    let to = child.get_entity();
    let custom_name = from.custom_name.load_full();
    if let Some(name) = &*custom_name {
        to.set_custom_name(name.clone());
    }
    to.set_custom_name_visible(from.custom_name_visible.load(Ordering::Relaxed));
    let parent_mob = parent.get_mob_entity();
    let child_mob = child.get_mob_entity();
    child_mob.set_left_handed(parent_mob.is_left_handed());
    child_mob.set_no_ai(parent_mob.is_no_ai());
    if parent_mob.persistence_required.load(Ordering::Relaxed) {
        child_mob.set_persistence_required();
    }
    to.set_invulnerable(from.invulnerable.load(Ordering::Relaxed));
    to.set_has_no_gravity(from.has_no_gravity());
    to.portal_cooldown.store(
        from.portal_cooldown.load(Ordering::Relaxed),
        Ordering::Relaxed,
    );
    to.set_silent(from.is_silent());
    let tags = from
        .scoreboard_tags
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    *to.scoreboard_tags
        .lock()
        .unwrap_or_else(PoisonError::into_inner) = tags;
    if let Some(team) = team {
        to.join_team(team);
    }
}

/// Vanilla `CubeMobMoveControl`: turns to the wanted yaw and hops with a delay between jumps.
pub struct CubeMobMoveControl {
    state: Arc<CubeMobState>,
}

impl CubeMobMoveControl {
    #[must_use]
    pub const fn new(state: Arc<CubeMobState>) -> Self {
        Self { state }
    }
}

impl Control for CubeMobMoveControl {}

impl MoveControlTrait for CubeMobMoveControl {
    fn set_wanted_position(&mut self, _x: f64, _y: f64, _z: f64, speed_modifier: f64) {
        self.state.set_wanted_movement(speed_modifier);
    }

    fn strafe(&mut self, _forward: f32, _right: f32) {
        self.state.speed_modifier.store(None);
    }

    fn has_wanted(&self) -> bool {
        self.state.speed_modifier.load().is_some()
    }

    fn tick(&mut self, mob: &dyn Mob) {
        let Some(cube) = mob.as_cube_mob() else {
            return;
        };
        if !cube.hops() {
            return;
        }
        let mob_entity = mob.get_mob_entity();
        let living_entity = &mob_entity.living_entity;
        let entity = &living_entity.entity;
        let state = &self.state;

        let current_yaw = entity.yaw.load();
        let new_yaw =
            MoveControl::rotlerp(current_yaw, state.target_yaw.load(), MoveControl::MAX_TURN);
        entity.yaw.store(new_yaw);
        entity.head_yaw.store(new_yaw);
        entity.body_yaw.store(new_yaw);

        let Some(speed_modifier) = state.speed_modifier.take() else {
            let input = living_entity.movement_input.load();
            living_entity
                .movement_input
                .store(Vector3::new(input.x, input.y, 0.0));
            return;
        };
        mob_entity.set_speed(
            (speed_modifier * living_entity.get_attribute_value(&Attributes::MOVEMENT_SPEED))
                as f32,
        );
        if !entity.on_ground.load(Ordering::Relaxed) {
            return;
        }
        if state.jump_delay.fetch_sub(1, Ordering::Relaxed) <= 0 {
            let mut next_delay = cube.jump_delay();
            if state.is_aggressive.load(Ordering::Relaxed) {
                next_delay /= 3;
            }
            state.jump_delay.store(next_delay, Ordering::Relaxed);
            mob_entity
                .jump_control
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .jump();
            // Vanilla `doPlayJumpSound`.
            if state.get_size() > 0 {
                cube.play_entity_sound(
                    cube.jump_sound(),
                    cube.cube_sound_volume(),
                    cube.cube_sound_pitch(),
                );
            }
        } else {
            mob_entity.set_speed(0.0);
            let input = living_entity.movement_input.load();
            living_entity
                .movement_input
                .store(Vector3::new(0.0, input.y, 0.0));
        }
    }
}

fn is_in_liquid(entity: &Entity) -> bool {
    entity.touching_water.load(Ordering::Relaxed) || entity.touching_lava.load(Ordering::Relaxed)
}

/// Vanilla `CubeMobFloatGoal`: jumps and swims while in water or lava.
pub struct CubeMobFloatGoal;

impl Goal for CubeMobFloatGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        is_in_liquid(mob.get_entity())
    }

    fn tick(&mut self, mob: &dyn Mob) {
        if mob.get_random().random::<f32>() < 0.8 {
            mob.get_mob_entity()
                .jump_control
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .jump();
        }
        if let Some(cube) = mob.as_cube_mob() {
            cube.cube_state().set_wanted_movement(FLOAT_SPEED_MODIFIER);
        }
    }

    fn should_run_every_tick(&self) -> bool {
        true
    }

    fn controls(&self) -> Controls {
        Controls::JUMP | Controls::MOVE
    }
}

/// Vanilla `CubeMobAttackGoal`: turns towards the target and hops at it faster.
#[derive(Default)]
pub struct CubeMobAttackGoal {
    grow_tired_timer: i32,
}

impl CubeMobAttackGoal {
    fn can_attack_target(mob: &dyn Mob) -> bool {
        mob.get_mob_entity()
            .get_target()
            .is_some_and(|target| mob.can_attack(target.as_ref()))
    }
}

impl Goal for CubeMobAttackGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        Self::can_attack_target(mob)
    }

    fn start(&mut self, _mob: &dyn Mob) {
        self.grow_tired_timer = to_goal_ticks(GROW_TIRED_TICKS);
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        if !Self::can_attack_target(mob) {
            return false;
        }
        self.grow_tired_timer -= 1;
        self.grow_tired_timer > 0
    }

    fn tick(&mut self, mob: &dyn Mob) {
        let Some(cube) = mob.as_cube_mob() else {
            return;
        };
        let entity = mob.get_entity();
        if let Some(target) = mob.get_mob_entity().get_target() {
            look_at(entity, target.as_ref(), ATTACK_TURN_SPEED);
        }
        cube.cube_state()
            .set_direction(entity.yaw.load(), cube.can_deal_damage());
    }

    fn should_run_every_tick(&self) -> bool {
        true
    }

    fn controls(&self) -> Controls {
        Controls::LOOK
    }
}

/// Vanilla `CubeMobRandomDirectionGoal`: faces a new random way every few seconds.
#[derive(Default)]
pub struct CubeMobRandomDirectionGoal {
    chosen_degrees: f32,
    next_randomize_time: i32,
}

impl Goal for CubeMobRandomDirectionGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        let mob_entity = mob.get_mob_entity();
        let living = &mob_entity.living_entity;
        mob_entity.get_target().is_none()
            && (living.entity.on_ground.load(Ordering::Relaxed)
                || is_in_liquid(&living.entity)
                || living.has_effect(&StatusEffect::LEVITATION))
    }

    fn tick(&mut self, mob: &dyn Mob) {
        self.next_randomize_time -= 1;
        if self.next_randomize_time <= 0 {
            let mut rng = mob.get_random();
            self.next_randomize_time = self.get_tick_count(40 + rng.random_range(0..60));
            self.chosen_degrees = rng.random_range(0..360) as f32;
        }
        if let Some(cube) = mob.as_cube_mob() {
            cube.cube_state().set_direction(self.chosen_degrees, false);
        }
    }

    fn controls(&self) -> Controls {
        Controls::LOOK
    }
}

/// Vanilla `CubeMobKeepOnJumpingGoal`: hops on whenever it is not riding.
pub struct CubeMobKeepOnJumpingGoal;

impl Goal for CubeMobKeepOnJumpingGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        !mob.get_entity().has_vehicle()
    }

    fn tick(&mut self, mob: &dyn Mob) {
        if let Some(cube) = mob.as_cube_mob() {
            cube.cube_state().set_wanted_movement(JUMPING_SPEED_MODIFIER);
        }
    }

    fn controls(&self) -> Controls {
        Controls::JUMP | Controls::MOVE
    }
}
