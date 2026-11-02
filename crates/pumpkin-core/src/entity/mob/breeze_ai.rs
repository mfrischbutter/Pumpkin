use std::sync::Arc;

use pumpkin_data::entity::{EntityPose, EntityType};
use pumpkin_data::environment_attribute::Activity;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::tag;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_world::clip::{ClipBlock, ClipContext, ClipFluid};
use rand::RngExt;
use rand::seq::SliceRandom;

use super::breeze::BreezeEntity;
use crate::entity::EntityBase;
use crate::entity::ai::brain::activity::ActivityData;
use crate::entity::ai::brain::behavior::long_jump::jump_vector_for_angle;
use crate::entity::ai::brain::behavior::timed::Behavior;
use crate::entity::ai::brain::behavior::{
    DoNothing, LookAtTargetSink, MoveToTargetSink, Swim, Timed, random_stroll, run_one,
    start_attacking, stop_attacking_if,
};
use crate::entity::ai::brain::memory::walk_target::WalkTarget;
use crate::entity::ai::brain::memory::{MemoryModuleId, MemoryStatus, types};
use crate::entity::ai::brain::sensing::is_entity_attackable;
use crate::entity::ai::brain::{BrainState, BrainTick};
use crate::entity::ai::util::default_random_pos;
use crate::entity::mob::Mob;
use crate::entity::projectile::ThrownItemEntity;
use crate::entity::projectile::wind_charge::{WIND_CHARGE_GRAVITY, WindChargeEntity};
use crate::world::natural_spawner::is_block_dangerous;

/// Vanilla `BreezeAi.SPEED_MULTIPLIER_WHEN_SLIDING`.
const SLIDE_SPEED: f32 = 0.6;
/// Vanilla `BreezeAi.TICKS_TO_REMEMBER_SEEN_TARGET`.
const TICKS_TO_REMEMBER_SEEN_TARGET: i32 = 100;
/// Vanilla `BreezeAi.JUMP_CIRCLE_INNER_RADIUS`.
const JUMP_CIRCLE_INNER_RADIUS: f64 = 4.0;
/// Vanilla `BreezeAi.JUMP_CIRCLE_MIDDLE_RADIUS`.
const JUMP_CIRCLE_MIDDLE_RADIUS: f64 = 8.0;
/// Vanilla `Shoot.ATTACK_RANGE_MAX_SQRT`.
const ATTACK_RANGE_SQR: f64 = 256.0;
/// Vanilla `Shoot.PROJECTILE_MOVEMENT_SCALE`.
const PROJECTILE_POWER: f64 = 0.7;
/// Vanilla `Shoot.SHOOT_INITIAL_DELAY_TICKS`.
const SHOOT_INITIAL_DELAY: i64 = 15;
/// Vanilla `Shoot.SHOOT_RECOVER_DELAY_TICKS`.
const SHOOT_RECOVER_DELAY: i64 = 4;
/// Vanilla `Shoot.SHOOT_COOLDOWN_TICKS`.
const SHOOT_COOLDOWN: i64 = 10;
/// Vanilla `LongJump.REQUIRED_AIR_BLOCKS_ABOVE`.
const REQUIRED_AIR_BLOCKS_ABOVE: i32 = 4;
/// Vanilla `LongJump.JUMP_COOLDOWN_TICKS`.
const JUMP_COOLDOWN: i64 = 10;
/// Vanilla `LongJump.JUMP_COOLDOWN_WHEN_HURT_TICKS`.
const JUMP_COOLDOWN_WHEN_HURT: i64 = 2;
/// Vanilla `LongJump.INHALING_DURATION_TICKS`.
const INHALING_DURATION: i64 = 10;
/// Vanilla `LongJump.MAX_JUMP_VELOCITY_MULTIPLIER`.
const MAX_JUMP_VELOCITY_MULTIPLIER: f64 = 0.058_333_334;
/// Vanilla `LongJump.ALLOWED_ANGLES`.
const ALLOWED_ANGLES: [i32; 5] = [40, 55, 60, 75, 80];
/// How long the breeze holds its shot after other moves, vanilla's `setMemoryWithExpiry` calls.
const SHOOT_AFTER_SLIDE: i64 = 60;
const SHOOT_AFTER_JUMP: i64 = 100;

fn as_breeze(mob: &dyn Mob) -> Option<&BreezeEntity> {
    mob.cast_any().downcast_ref::<BreezeEntity>()
}

fn pose(tick: &BrainTick<'_>) -> EntityPose {
    tick.mob.get_entity().pose.load()
}

/// Vanilla `lookAt(EntityAnchorArgument.Anchor.EYES, target)`.
fn look_at(tick: &BrainTick<'_>, target: Vector3<f64>) {
    let entity = tick.mob.get_entity();
    entity.look_at(target);
    let yaw = entity.yaw.load();
    entity.head_yaw.store(yaw);
    entity.body_yaw.store(yaw);
}

/// Vanilla `BreezeUtil.hasLineOfSight`.
fn has_line_of_sight(tick: &BrainTick<'_>, target: Vector3<f64>) -> bool {
    let from = tick.mob.get_entity().pos.load();
    let range = tick
        .mob
        .get_mob_entity()
        .living_entity
        .get_attribute_value(&pumpkin_data::attributes::Attributes::FOLLOW_RANGE)
        .max(50.0);
    if target.squared_distance_to_vec(&from) > range * range {
        return false;
    }
    let hit = tick.world.clip(&ClipContext::new(
        from,
        target,
        ClipBlock::Collider,
        ClipFluid::None,
    ));
    BlockPos::floored_v(hit.location) == BlockPos::floored_v(target) || hit.is_miss
}

/// A value of a standard normal distribution, vanilla `RandomSource.nextGaussian`.
fn gaussian(mob: &dyn Mob) -> f64 {
    let mut random = mob.get_random();
    let (a, b): (f64, f64) = (random.random(), random.random());
    (-2.0 * (1.0 - a).ln()).sqrt() * (std::f64::consts::TAU * b).cos()
}

/// Vanilla `BreezeUtil.randomPointBehindTarget`.
fn random_point_behind_target(mob: &dyn Mob, enemy: &dyn EntityBase) -> Vector3<f64> {
    let entity = enemy.get_entity();
    let view = f64::from(entity.head_yaw.load()) + 180.0 + gaussian(mob) * 45.0;
    let radius = 4.0 + f64::from(mob.get_random().random::<f32>()) * 4.0;
    let radians = view.to_radians();
    entity.pos.load() + Vector3::new(-radians.sin(), 0.0, radians.cos()) * radius
}

/// The nearest shape on `to`'s side of `from`, one block above it.
fn snap_along(tick: &BrainTick<'_>, from: Vector3<f64>, to: Vector3<f64>) -> Option<BlockPos> {
    let hit = tick.world.clip(&ClipContext::new(
        from,
        to,
        ClipBlock::Collider,
        ClipFluid::None,
    ));
    (!hit.is_miss).then(|| BlockPos::floored_v(hit.location).up())
}

/// Vanilla `LongJump.snapToSurface`.
fn snap_to_surface(tick: &BrainTick<'_>, target: Vector3<f64>) -> Option<BlockPos> {
    snap_along(tick, target, target - Vector3::new(0.0, 10.0, 0.0))
        .or_else(|| snap_along(tick, target, target + Vector3::new(0.0, 10.0, 0.0)))
}

/// Vanilla `LongJump.canJumpFromCurrentPosition`.
fn can_jump_from_here(tick: &BrainTick<'_>) -> bool {
    let here = tick.mob.get_entity().block_pos.load();
    if tick.world.get_block(&here) == &pumpkin_data::Block::HONEY_BLOCK {
        return false;
    }
    (1..=REQUIRED_AIR_BLOCKS_ABOVE).all(|up| {
        let above = BlockPos::new(here.0.x, here.0.y + up, here.0.z);
        tick.world.get_block_state(&above).is_air()
            || pumpkin_data::tag::Taggable::has_tag(
                tick.world.get_fluid(&above),
                &tag::Fluid::MINECRAFT_WATER,
            )
    })
}

/// Vanilla `LongJump.canRun`.
fn can_long_jump(tick: &mut BrainTick<'_>) -> bool {
    let entity = tick.mob.get_entity();
    let on_ground = entity.on_ground.load(std::sync::atomic::Ordering::Relaxed);
    if !on_ground && !entity.is_in_water() {
        return false;
    }
    let eye = entity.get_eye_y() - entity.pos.load().y;
    if entity.water_height.load() > eye
        || entity
            .touching_lava
            .load(std::sync::atomic::Ordering::SeqCst)
    {
        return false;
    }
    if tick.state.has_memory_value(types::BREEZE_JUMP_TARGET.id()) {
        return true;
    }
    let Some(target) = tick.state.get(types::ATTACK_TARGET).cloned() else {
        return false;
    };
    let pos = entity.pos.load();
    let distance = target.get_entity().pos.load().squared_distance_to_vec(&pos);
    let follow = tick
        .mob
        .get_mob_entity()
        .living_entity
        .get_attribute_value(&pumpkin_data::attributes::Attributes::FOLLOW_RANGE);
    if distance >= follow * follow {
        tick.state.erase(types::ATTACK_TARGET.id());
        return false;
    }
    if distance.sqrt() - JUMP_CIRCLE_INNER_RADIUS <= 0.0 || !can_jump_from_here(tick) {
        return false;
    }
    let behind = random_point_behind_target(tick.mob, target.as_ref());
    let Some(spot) = snap_to_surface(tick, behind) else {
        return false;
    };
    if is_block_dangerous(entity.entity_type, tick.world.get_block_state(&spot.down())) {
        return false;
    }
    let center = spot.to_centered_f64();
    let above = Vector3::new(
        center.x,
        center.y + f64::from(REQUIRED_AIR_BLOCKS_ABOVE),
        center.z,
    );
    if !has_line_of_sight(tick, center) && !has_line_of_sight(tick, above) {
        return false;
    }
    tick.state.set(types::BREEZE_JUMP_TARGET, spot);
    true
}

/// Vanilla `LongJump.calculateOptimalJumpVector`.
fn optimal_jump_vector(tick: &BrainTick<'_>, spot: BlockPos) -> Option<Vector3<f64>> {
    let center = spot.to_centered_f64();
    let target = Vector3::new(center.x, f64::from(spot.0.y), center.z);
    let mut angles = ALLOWED_ANGLES;
    angles.shuffle(&mut tick.mob.get_random());
    let follow = tick
        .mob
        .get_mob_entity()
        .living_entity
        .get_attribute_value(&pumpkin_data::attributes::Attributes::FOLLOW_RANGE);
    let max_velocity = MAX_JUMP_VELOCITY_MULTIPLIER * follow;
    angles
        .iter()
        .find_map(|&angle| jump_vector_for_angle(tick.mob, target, max_velocity, angle, false))
}

const LONG_JUMP_CONDITIONS: [(MemoryModuleId, MemoryStatus); 7] = [
    (types::ATTACK_TARGET.id(), MemoryStatus::ValuePresent),
    (types::BREEZE_JUMP_COOLDOWN.id(), MemoryStatus::ValueAbsent),
    (types::BREEZE_JUMP_INHALING.id(), MemoryStatus::Registered),
    (types::BREEZE_JUMP_TARGET.id(), MemoryStatus::Registered),
    (types::BREEZE_SHOOT.id(), MemoryStatus::ValueAbsent),
    (types::WALK_TARGET.id(), MemoryStatus::ValueAbsent),
    (types::BREEZE_LEAVING_WATER.id(), MemoryStatus::Registered),
];

/// Vanilla `LongJump`, the breeze's own.
struct BreezeLongJump;

impl Behavior for BreezeLongJump {
    fn entry_conditions(&self) -> &[(MemoryModuleId, MemoryStatus)] {
        &LONG_JUMP_CONDITIONS
    }

    fn min_duration(&self) -> i32 {
        200
    }

    fn max_duration(&self) -> i32 {
        200
    }

    fn check_extra_start_conditions(&mut self, tick: &mut BrainTick<'_>) -> bool {
        can_long_jump(tick)
    }

    fn can_still_use(&mut self, tick: &mut BrainTick<'_>) -> bool {
        pose(tick) != EntityPose::Standing
            && !tick
                .state
                .has_memory_value(types::BREEZE_JUMP_COOLDOWN.id())
    }

    fn start(&mut self, tick: &mut BrainTick<'_>) {
        if !tick
            .state
            .has_memory_value(types::BREEZE_JUMP_INHALING.id())
        {
            tick.state
                .set_with_expiry(types::BREEZE_JUMP_INHALING, (), INHALING_DURATION);
        }
        let entity = tick.mob.get_entity();
        entity.set_pose(EntityPose::Inhaling);
        tick.world.play_sound(
            Sound::EntityBreezeCharge,
            SoundCategory::Hostile,
            &entity.pos.load(),
        );
        if let Some(spot) = tick.state.get(types::BREEZE_JUMP_TARGET).copied() {
            look_at(tick, spot.to_centered_f64());
        }
    }

    fn tick(&mut self, tick: &mut BrainTick<'_>) {
        let entity = tick.mob.get_entity();
        let in_water = entity.is_in_water();
        if !in_water
            && tick
                .state
                .has_memory_value(types::BREEZE_LEAVING_WATER.id())
        {
            tick.state.erase(types::BREEZE_LEAVING_WATER.id());
        }
        let state = pose(tick);
        if state == EntityPose::Inhaling
            && !tick
                .state
                .has_memory_value(types::BREEZE_JUMP_INHALING.id())
        {
            take_off(tick, in_water);
            return;
        }
        let on_ground = entity.on_ground.load(std::sync::atomic::Ordering::Relaxed);
        let landed_in_water = in_water
            && !tick
                .state
                .has_memory_value(types::BREEZE_LEAVING_WATER.id());
        if state == EntityPose::LongJumping && (on_ground || landed_in_water) {
            tick.mob
                .play_entity_sound(Sound::EntityBreezeLand, 1.0, 1.0);
            entity.set_pose(EntityPose::Standing);
            tick.mob
                .get_mob_entity()
                .living_entity
                .discard_friction
                .store(false, std::sync::atomic::Ordering::Relaxed);
            let cooldown = if tick.state.has_memory_value(types::HURT_BY.id()) {
                JUMP_COOLDOWN_WHEN_HURT
            } else {
                JUMP_COOLDOWN
            };
            tick.state
                .set_with_expiry(types::BREEZE_JUMP_COOLDOWN, (), cooldown);
            tick.state
                .set_with_expiry(types::BREEZE_SHOOT, (), SHOOT_AFTER_JUMP);
        }
    }

    fn stop(&mut self, tick: &mut BrainTick<'_>) {
        let entity = tick.mob.get_entity();
        if matches!(pose(tick), EntityPose::LongJumping | EntityPose::Inhaling) {
            entity.set_pose(EntityPose::Standing);
        }
        tick.state.erase(types::BREEZE_JUMP_TARGET.id());
        tick.state.erase(types::BREEZE_JUMP_INHALING.id());
        tick.state.erase(types::BREEZE_LEAVING_WATER.id());
    }

    fn debug_name(&self) -> &'static str {
        "BreezeLongJump"
    }
}

/// The take-off in vanilla `LongJump.tick`.
fn take_off(tick: &mut BrainTick<'_>, in_water: bool) {
    let entity = tick.mob.get_entity();
    let velocity = tick
        .state
        .get(types::BREEZE_JUMP_TARGET)
        .copied()
        .and_then(|spot| optimal_jump_vector(tick, spot));
    let Some(velocity) = velocity else {
        entity.set_pose(EntityPose::Standing);
        return;
    };
    if in_water {
        tick.state.set(types::BREEZE_LEAVING_WATER, ());
    }
    tick.mob
        .play_entity_sound(Sound::EntityBreezeJump, 1.0, 1.0);
    entity.set_pose(EntityPose::LongJumping);
    entity.yaw.store(entity.body_yaw.load());
    tick.mob
        .get_mob_entity()
        .living_entity
        .discard_friction
        .store(true, std::sync::atomic::Ordering::Relaxed);
    entity.velocity.store(velocity);
    entity
        .velocity_dirty
        .store(true, std::sync::atomic::Ordering::Relaxed);
}

const SHOOT_CONDITIONS: [(MemoryModuleId, MemoryStatus); 7] = [
    (types::ATTACK_TARGET.id(), MemoryStatus::ValuePresent),
    (types::BREEZE_SHOOT_COOLDOWN.id(), MemoryStatus::ValueAbsent),
    (types::BREEZE_SHOOT_CHARGING.id(), MemoryStatus::ValueAbsent),
    (
        types::BREEZE_SHOOT_RECOVERING.id(),
        MemoryStatus::ValueAbsent,
    ),
    (types::BREEZE_SHOOT.id(), MemoryStatus::ValuePresent),
    (types::WALK_TARGET.id(), MemoryStatus::ValueAbsent),
    (types::BREEZE_JUMP_TARGET.id(), MemoryStatus::ValueAbsent),
];

/// Vanilla `Shoot`.
struct Shoot;

impl Behavior for Shoot {
    fn entry_conditions(&self) -> &[(MemoryModuleId, MemoryStatus)] {
        &SHOOT_CONDITIONS
    }

    fn min_duration(&self) -> i32 {
        (SHOOT_INITIAL_DELAY + 1 + SHOOT_RECOVER_DELAY) as i32
    }

    fn max_duration(&self) -> i32 {
        self.min_duration()
    }

    fn check_extra_start_conditions(&mut self, tick: &mut BrainTick<'_>) -> bool {
        if pose(tick) != EntityPose::Standing {
            return false;
        }
        let pos = tick.mob.get_entity().pos.load();
        let Some(target) = tick.state.get(types::ATTACK_TARGET) else {
            return false;
        };
        let within =
            target.get_entity().pos.load().squared_distance_to_vec(&pos) < ATTACK_RANGE_SQR;
        if !within {
            tick.state.erase(types::BREEZE_SHOOT.id());
        }
        within
    }

    fn can_still_use(&mut self, tick: &mut BrainTick<'_>) -> bool {
        tick.state.has_memory_value(types::ATTACK_TARGET.id())
            && tick.state.has_memory_value(types::BREEZE_SHOOT.id())
    }

    fn start(&mut self, tick: &mut BrainTick<'_>) {
        tick.mob.get_entity().set_pose(EntityPose::Shooting);
        tick.state
            .set_with_expiry(types::BREEZE_SHOOT_CHARGING, (), SHOOT_INITIAL_DELAY);
        tick.mob
            .play_entity_sound(Sound::EntityBreezeInhale, 1.0, 1.0);
    }

    fn tick(&mut self, tick: &mut BrainTick<'_>) {
        let Some(target) = tick.state.get(types::ATTACK_TARGET).cloned() else {
            return;
        };
        look_at(tick, target.get_entity().pos.load());
        if tick
            .state
            .has_memory_value(types::BREEZE_SHOOT_CHARGING.id())
            || tick
                .state
                .has_memory_value(types::BREEZE_SHOOT_RECOVERING.id())
        {
            return;
        }
        tick.state
            .set_with_expiry(types::BREEZE_SHOOT_RECOVERING, (), SHOOT_RECOVER_DELAY);
        fire_wind_charge(tick, target.as_ref());
        tick.mob
            .play_entity_sound(Sound::EntityBreezeShoot, 1.5, 1.0);
    }

    fn stop(&mut self, tick: &mut BrainTick<'_>) {
        if pose(tick) == EntityPose::Shooting {
            tick.mob.get_entity().set_pose(EntityPose::Standing);
        }
        tick.state
            .set_with_expiry(types::BREEZE_SHOOT_COOLDOWN, (), SHOOT_COOLDOWN);
        tick.state.erase(types::BREEZE_SHOOT.id());
    }

    fn debug_name(&self) -> &'static str {
        "BreezeShoot"
    }
}

/// The projectile of vanilla `Shoot.tick`, aimed at the target's middle.
fn fire_wind_charge(tick: &BrainTick<'_>, target: &dyn EntityBase) {
    let entity = tick.mob.get_entity();
    let target_entity = target.get_entity();
    let pos = entity.pos.load();
    let firing_y = as_breeze(tick.mob).map_or(pos.y, BreezeEntity::firing_y);
    let target_pos = target_entity.pos.load();
    let share = if target_entity.has_vehicle() {
        0.8
    } else {
        0.3
    };
    let target_y = target_pos.y + f64::from(target_entity.height()) * share;
    let start = Vector3::new(pos.x, firing_y, pos.z);
    let body =
        crate::entity::Entity::new(tick.world.clone(), start, &EntityType::BREEZE_WIND_CHARGE);
    let thrown = ThrownItemEntity::new(body, entity, WIND_CHARGE_GRAVITY);
    thrown.entity.pos.store(start);
    let difficulty = tick.world.level_info.load().difficulty as i32;
    thrown.set_velocity(
        target_pos.x - pos.x,
        target_y - firing_y,
        target_pos.z - pos.z,
        PROJECTILE_POWER,
        f64::from(5 - difficulty * 4),
    );
    tick.world
        .spawn_entity(Arc::new(WindChargeEntity::new_breeze(thrown)));
}

const SHOOT_WHEN_STUCK_CONDITIONS: [(MemoryModuleId, MemoryStatus); 5] = [
    (types::ATTACK_TARGET.id(), MemoryStatus::ValuePresent),
    (types::BREEZE_JUMP_INHALING.id(), MemoryStatus::ValueAbsent),
    (types::BREEZE_JUMP_TARGET.id(), MemoryStatus::ValueAbsent),
    (types::WALK_TARGET.id(), MemoryStatus::ValueAbsent),
    (types::BREEZE_SHOOT.id(), MemoryStatus::ValueAbsent),
];

/// Vanilla `ShootWhenStuck`.
struct ShootWhenStuck;

impl Behavior for ShootWhenStuck {
    fn entry_conditions(&self) -> &[(MemoryModuleId, MemoryStatus)] {
        &SHOOT_WHEN_STUCK_CONDITIONS
    }

    fn check_extra_start_conditions(&mut self, tick: &mut BrainTick<'_>) -> bool {
        let entity = tick.mob.get_entity();
        entity.has_vehicle()
            || entity.is_in_water()
            || tick
                .mob
                .get_mob_entity()
                .living_entity
                .has_effect(&pumpkin_data::effect::StatusEffect::LEVITATION)
    }

    fn start(&mut self, tick: &mut BrainTick<'_>) {
        tick.state
            .set_with_expiry(types::BREEZE_SHOOT, (), SHOOT_AFTER_SLIDE);
    }

    fn debug_name(&self) -> &'static str {
        "BreezeShootWhenStuck"
    }
}

const SLIDE_CONDITIONS: [(MemoryModuleId, MemoryStatus); 4] = [
    (types::ATTACK_TARGET.id(), MemoryStatus::ValuePresent),
    (types::WALK_TARGET.id(), MemoryStatus::ValueAbsent),
    (types::BREEZE_JUMP_COOLDOWN.id(), MemoryStatus::ValueAbsent),
    (types::BREEZE_SHOOT.id(), MemoryStatus::ValueAbsent),
];

/// Vanilla `Slide`: slides away when too close, else behind the enemy or around it.
struct Slide;

impl Behavior for Slide {
    fn entry_conditions(&self) -> &[(MemoryModuleId, MemoryStatus)] {
        &SLIDE_CONDITIONS
    }

    fn check_extra_start_conditions(&mut self, tick: &mut BrainTick<'_>) -> bool {
        let entity = tick.mob.get_entity();
        entity.on_ground.load(std::sync::atomic::Ordering::Relaxed)
            && !entity.is_in_water()
            && pose(tick) == EntityPose::Standing
    }

    fn start(&mut self, tick: &mut BrainTick<'_>) {
        let Some(enemy) = tick.state.get(types::ATTACK_TARGET).cloned() else {
            return;
        };
        let position = slide_away(tick, enemy.as_ref()).unwrap_or_else(|| {
            if tick.mob.get_random().random::<bool>() {
                random_point_behind_target(tick.mob, enemy.as_ref())
            } else {
                random_point_in_middle_circle(tick.mob, enemy.as_ref())
            }
        });
        tick.state.set(
            types::WALK_TARGET,
            WalkTarget::from_vec(position, SLIDE_SPEED, 0),
        );
    }

    fn debug_name(&self) -> &'static str {
        "BreezeSlide"
    }
}

/// The inner ring of vanilla `Slide.start`.
fn slide_away(tick: &BrainTick<'_>, enemy: &dyn EntityBase) -> Option<Vector3<f64>> {
    let entity = tick.mob.get_entity();
    let pos = entity.pos.load();
    let enemy_pos = enemy.get_entity().pos.load();
    if !within_inner_circle(pos, enemy_pos) {
        return None;
    }
    let away = default_random_pos::get_pos_away(tick.mob, 5, 5, enemy_pos)?;
    (has_line_of_sight(tick, away)
        && enemy_pos.squared_distance_to_vec(&away) > enemy_pos.squared_distance_to_vec(&pos))
    .then_some(away)
}

/// Vanilla `Breeze.withinInnerCircleRange`.
fn within_inner_circle(pos: Vector3<f64>, target: Vector3<f64>) -> bool {
    let center = BlockPos::floored_v(pos).to_centered_f64();
    let horizontal = (target.x - center.x).hypot(target.z - center.z);
    horizontal < JUMP_CIRCLE_INNER_RADIUS && (target.y - center.y).abs() < 10.0
}

/// Vanilla `Slide.randomPointInMiddleCircle`.
fn random_point_in_middle_circle(mob: &dyn Mob, enemy: &dyn EntityBase) -> Vector3<f64> {
    let pos = mob.get_entity().pos.load();
    let direction = enemy.get_entity().pos.load() - pos;
    let share = f64::from(mob.get_random().random::<f32>());
    let distance = direction.length()
        - (JUMP_CIRCLE_MIDDLE_RADIUS
            + (JUMP_CIRCLE_INNER_RADIUS - JUMP_CIRCLE_MIDDLE_RADIUS) * share);
    pos + direction.normalize() * distance
}

/// Vanilla `BreezeAi.SlideToTargetSink`.
struct SlideToTargetSink(MoveToTargetSink);

impl Behavior for SlideToTargetSink {
    fn entry_conditions(&self) -> &[(MemoryModuleId, MemoryStatus)] {
        self.0.entry_conditions()
    }

    fn min_duration(&self) -> i32 {
        self.0.min_duration()
    }

    fn max_duration(&self) -> i32 {
        self.0.max_duration()
    }

    fn check_extra_start_conditions(&mut self, tick: &mut BrainTick<'_>) -> bool {
        self.0.check_extra_start_conditions(tick)
    }

    fn can_still_use(&mut self, tick: &mut BrainTick<'_>) -> bool {
        self.0.can_still_use(tick)
    }

    fn start(&mut self, tick: &mut BrainTick<'_>) {
        self.0.start(tick);
        tick.mob
            .play_entity_sound(Sound::EntityBreezeSlide, 1.0, 1.0);
        tick.mob.get_entity().set_pose(EntityPose::Sliding);
    }

    fn tick(&mut self, tick: &mut BrainTick<'_>) {
        self.0.tick(tick);
    }

    fn stop(&mut self, tick: &mut BrainTick<'_>) {
        self.0.stop(tick);
        tick.mob.get_entity().set_pose(EntityPose::Standing);
        if tick.state.has_memory_value(types::ATTACK_TARGET.id()) {
            tick.state
                .set_with_expiry(types::BREEZE_SHOOT, (), SHOOT_AFTER_SLIDE);
        }
    }

    fn debug_name(&self) -> &'static str {
        "BreezeSlideToTargetSink"
    }
}

fn nearest_attackable(tick: &BrainTick<'_>) -> Option<Arc<dyn EntityBase>> {
    tick.state.get(types::NEAREST_ATTACKABLE).cloned()
}

/// Whoever hurt it, as a `HURT_BY_ENTITY` memory gives.
fn hurt_by(tick: &BrainTick<'_>) -> Option<Arc<dyn EntityBase>> {
    tick.state
        .get(types::HURT_BY_ENTITY)
        .cloned()
        .filter(|entity| entity.get_living_entity().is_some())
}

/// Vanilla `Sensor.wasEntityAttackableLastNTicks` negated, as the fight's stop condition.
fn lost_sight_for_a_while() -> impl FnMut(&BrainTick<'_>, &dyn EntityBase) -> bool + Send + Sync {
    let mut positives_left = 0;
    move |tick, target| {
        if is_entity_attackable(&tick.visibility(), target) {
            positives_left = TICKS_TO_REMEMBER_SEEN_TARGET;
            false
        } else if positives_left > 0 {
            positives_left -= 1;
            false
        } else {
            true
        }
    }
}

/// Vanilla `BreezeAi.getActivities`.
pub(super) fn activities(_mob: &dyn Mob) -> Vec<ActivityData> {
    vec![
        ActivityData::with_priority_start(
            Activity::Core,
            0,
            vec![
                Box::new(Timed::new(Swim::new(0.8))),
                Box::new(Timed::new(LookAtTargetSink::new(45, 90))),
            ],
        ),
        ActivityData::with_pairs(
            Activity::Idle,
            vec![
                (0, Box::new(start_attacking(nearest_attackable))),
                (1, Box::new(start_attacking(hurt_by))),
                (
                    2,
                    Box::new(Timed::new(SlideToTargetSink(MoveToTargetSink::new(20, 40)))),
                ),
                (
                    3,
                    Box::new(run_one(vec![
                        (Box::new(DoNothing::new(20, 100)), 1),
                        (Box::new(random_stroll::stroll(SLIDE_SPEED)), 2),
                    ])),
                ),
            ],
        ),
        ActivityData::with_pairs_and_conditions(
            Activity::Fight,
            vec![
                (
                    0,
                    Box::new(stop_attacking_if(lost_sight_for_a_while(), |_, _| {})),
                ),
                (1, Box::new(Timed::new(Shoot))),
                (2, Box::new(Timed::new(BreezeLongJump))),
                (3, Box::new(Timed::new(ShootWhenStuck))),
                (4, Box::new(Timed::new(Slide))),
            ],
            vec![
                (types::ATTACK_TARGET.id(), MemoryStatus::ValuePresent),
                (types::WALK_TARGET.id(), MemoryStatus::ValueAbsent),
            ],
        ),
    ]
}

/// Vanilla `BreezeAi.updateActivity`.
pub(super) fn update_activity(state: &mut BrainState) {
    state.set_active_activity_to_first_valid(&[Activity::Fight, Activity::Idle]);
}
