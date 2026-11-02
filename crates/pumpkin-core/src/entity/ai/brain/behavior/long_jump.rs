use std::sync::atomic::Ordering::Relaxed;

use pumpkin_data::attributes::Attributes;
use pumpkin_data::entity::EntityPose;
use pumpkin_data::sound::Sound;
use pumpkin_data::tag::{self, Taggable};
use pumpkin_util::math::boundingbox::BoundingBox;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use rand::RngExt;
use rand::seq::SliceRandom;

use super::super::BrainTick;
use super::super::memory::position_tracker::{BlockPosTracker, PositionTracker};
use super::super::memory::{MemoryModuleId, MemoryStatus, types};
use super::timed::Behavior;
use crate::entity::mob::Mob;
use crate::world::level_accessor::LevelAccessor;

/// Vanilla `LongJumpToRandomPos.FIND_JUMP_TRIES`.
const FIND_JUMP_TRIES: i32 = 20;
/// Vanilla `LongJumpToRandomPos.PREPARE_JUMP_DURATION`.
const PREPARE_JUMP_DURATION: i64 = 40;
/// Vanilla `LongJumpToRandomPos.MIN_PATHFIND_DISTANCE_TO_VALID_JUMP`.
const MIN_PATHFIND_DISTANCE_TO_VALID_JUMP: f32 = 8.0;
/// Vanilla `LongJumpToRandomPos.ALLOWED_ANGLES`.
const ALLOWED_ANGLES: [i32; 4] = [65, 70, 75, 80];

/// Vanilla `UniformInt` of ticks between two long jumps.
pub type JumpCooldown = std::ops::RangeInclusive<i32>;

fn start_cooldown(tick: &mut BrainTick<'_>, cooldown: &JumpCooldown) {
    let ticks = tick.mob.get_random().random_range(cooldown.clone());
    tick.state.set(types::LONG_JUMP_COOLDOWN_TICKS, ticks);
}

const MID_JUMP_CONDITIONS: [(MemoryModuleId, MemoryStatus); 2] = [
    (types::LOOK_TARGET.id(), MemoryStatus::Registered),
    (types::LONG_JUMP_MID_JUMP.id(), MemoryStatus::ValuePresent),
];

/// Vanilla `LongJumpMidJump`: the flight, up to the landing.
pub struct LongJumpMidJump {
    cooldown: JumpCooldown,
    landing_sound: Sound,
}

impl LongJumpMidJump {
    #[must_use]
    pub const fn new(cooldown: JumpCooldown, landing_sound: Sound) -> Self {
        Self {
            cooldown,
            landing_sound,
        }
    }
}

impl Behavior for LongJumpMidJump {
    fn entry_conditions(&self) -> &[(MemoryModuleId, MemoryStatus)] {
        &MID_JUMP_CONDITIONS
    }

    fn min_duration(&self) -> i32 {
        100
    }

    fn max_duration(&self) -> i32 {
        100
    }

    fn can_still_use(&mut self, tick: &mut BrainTick<'_>) -> bool {
        !tick.mob.get_entity().on_ground.load(Relaxed)
    }

    fn start(&mut self, tick: &mut BrainTick<'_>) {
        let mob_entity = tick.mob.get_mob_entity();
        mob_entity
            .living_entity
            .discard_friction
            .store(true, Relaxed);
        mob_entity
            .living_entity
            .entity
            .set_pose(EntityPose::LongJumping);
    }

    fn stop(&mut self, tick: &mut BrainTick<'_>) {
        let living = &tick.mob.get_mob_entity().living_entity;
        let entity = &living.entity;
        if entity.on_ground.load(Relaxed) {
            let velocity = entity.velocity.load();
            entity
                .velocity
                .store(Vector3::new(velocity.x * 0.1, velocity.y, velocity.z * 0.1));
            tick.mob.play_entity_sound(self.landing_sound, 2.0, 1.0);
        }
        living.discard_friction.store(false, Relaxed);
        entity.set_pose(EntityPose::Standing);
        tick.state.erase(types::LONG_JUMP_MID_JUMP.id());
        start_cooldown(tick, &self.cooldown);
    }

    fn debug_name(&self) -> &'static str {
        "LongJumpMidJump"
    }
}

const TO_RANDOM_POS_CONDITIONS: [(MemoryModuleId, MemoryStatus); 3] = [
    (types::LOOK_TARGET.id(), MemoryStatus::Registered),
    (
        types::LONG_JUMP_COOLDOWN_TICKS.id(),
        MemoryStatus::ValueAbsent,
    ),
    (types::LONG_JUMP_MID_JUMP.id(), MemoryStatus::ValueAbsent),
];

/// Vanilla `LongJumpToRandomPos.PossibleJump`.
#[derive(Clone, Copy)]
struct PossibleJump {
    target: BlockPos,
    weight: i32,
}

/// What a species needs to say about its long jumps.
pub struct LongJumpSettings {
    pub cooldown: JumpCooldown,
    pub max_height: i32,
    pub max_width: i32,
    pub max_velocity_multiplier: f32,
    pub jump_sound: Sound,
    pub acceptable_landing_spot: fn(&dyn Mob, &BlockPos) -> bool,
}

/// Vanilla `LongJumpToRandomPos`, and `LongJumpToPreferredBlock` with `preferred`: stands for
/// two seconds aiming at a spot it cannot walk to, then jumps.
pub struct LongJumpToRandomPos {
    settings: LongJumpSettings,
    preferred: Option<(&'static tag::Tag, f32)>,
    candidates: Vec<PossibleJump>,
    not_preferred: Vec<PossibleJump>,
    wanting_preferred: bool,
    initial_position: Option<Vector3<f64>>,
    chosen_jump: Option<Vector3<f64>>,
    find_jump_tries: i32,
    prepare_jump_start: i64,
}

impl LongJumpToRandomPos {
    #[must_use]
    pub const fn new(settings: LongJumpSettings) -> Self {
        Self {
            settings,
            preferred: None,
            candidates: Vec::new(),
            not_preferred: Vec::new(),
            wanting_preferred: false,
            initial_position: None,
            chosen_jump: None,
            find_jump_tries: 0,
            prepare_jump_start: 0,
        }
    }

    /// Vanilla `LongJumpToPreferredBlock`.
    #[must_use]
    pub const fn preferring(mut self, blocks: &'static tag::Tag, chance: f32) -> Self {
        self.preferred = Some((blocks, chance));
        self
    }

    /// Vanilla `WeightedRandom.getRandomItem`, taking the item off the list.
    fn take_weighted(&mut self, tick: &BrainTick<'_>) -> Option<PossibleJump> {
        let total: i32 = self.candidates.iter().map(|jump| jump.weight).sum();
        if total <= 0 {
            return self.candidates.pop();
        }
        let mut roll = tick.mob.get_random().random_range(0..total);
        let index = self.candidates.iter().position(|jump| {
            roll -= jump.weight;
            roll < 0
        })?;
        Some(self.candidates.swap_remove(index))
    }

    /// Vanilla `getJumpCandidate` of both classes.
    fn next_candidate(&mut self, tick: &BrainTick<'_>) -> Option<PossibleJump> {
        let Some((blocks, _)) = self.preferred.filter(|_| self.wanting_preferred) else {
            return self.take_weighted(tick);
        };
        while !self.candidates.is_empty() {
            if let Some(jump) = self.take_weighted(tick) {
                if tick.world.get_block(&jump.target.down()).has_tag(blocks) {
                    return Some(jump);
                }
                self.not_preferred.push(jump);
            }
        }
        (!self.not_preferred.is_empty()).then(|| self.not_preferred.remove(0))
    }

    /// Vanilla `pickCandidate`: a jump counts only when walking there takes too long.
    fn pick_candidate(&mut self, tick: &mut BrainTick<'_>) {
        let own = tick.mob.get_entity().block_pos.load();
        while let Some(jump) = self.next_candidate(tick) {
            let target = jump.target;
            if (own.0.x == target.0.x && own.0.z == target.0.z)
                || !(self.settings.acceptable_landing_spot)(tick.mob, &target)
            {
                continue;
            }
            let Some(velocity) = self.optimal_jump_vector(tick, target.to_centered_f64()) else {
                continue;
            };
            tick.state.set(
                types::LOOK_TARGET,
                std::sync::Arc::new(BlockPosTracker::new(target))
                    as std::sync::Arc<dyn PositionTracker>,
            );
            if !can_walk_to(tick.mob, target) {
                self.chosen_jump = Some(velocity);
                self.prepare_jump_start = tick.time;
                return;
            }
        }
    }

    /// Vanilla `calculateOptimalJumpVector`.
    fn optimal_jump_vector(
        &self,
        tick: &BrainTick<'_>,
        target: Vector3<f64>,
    ) -> Option<Vector3<f64>> {
        let mut angles = ALLOWED_ANGLES;
        angles.shuffle(&mut tick.mob.get_random());
        let living = &tick.mob.get_mob_entity().living_entity;
        let max_velocity = living.get_attribute_value(&Attributes::JUMP_STRENGTH)
            * f64::from(self.settings.max_velocity_multiplier);
        angles
            .into_iter()
            .find_map(|angle| jump_vector_for_angle(tick.mob, target, max_velocity, angle))
    }
}

/// `navigation.createPath(target, 0, 8)` reaching the target.
fn can_walk_to(mob: &dyn Mob, target: BlockPos) -> bool {
    let mob_entity = mob.get_mob_entity();
    let path = mob_entity
        .navigator
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .create_path(&mob_entity.living_entity, target.0.to_f64(), 0);
    path.is_some_and(|path| {
        path.can_reach()
            && path
                .get_end_node()
                .is_some_and(|node| node.walked_dist < MIN_PATHFIND_DISTANCE_TO_VALID_JUMP)
    })
}

/// Vanilla `LongJumpUtil.calculateJumpVectorForAngle`, with its collision check.
fn jump_vector_for_angle(
    mob: &dyn Mob,
    target: Vector3<f64>,
    max_velocity: f64,
    angle: i32,
) -> Option<Vector3<f64>> {
    let entity = mob.get_entity();
    let position = entity.pos.load();
    let plane = Vector3::new(target.x - position.x, 0.0, target.z - position.z).normalize() * 0.5;
    let direction = target - plane - position;
    let radians = f64::from(angle) * std::f64::consts::PI / 180.0;
    let xz_angle = direction.z.atan2(direction.x);
    let r2 = direction.x * direction.x + direction.z * direction.z;
    let r = r2.sqrt();
    let gravity = mob.get_gravity();
    let (sin, cos) = radians.sin_cos();
    let v0_squared = r2 * gravity / (r * (2.0 * radians).sin() - 2.0 * direction.y * cos * cos);
    if v0_squared < 0.0 {
        return None;
    }
    let v0 = v0_squared.sqrt();
    if v0 > max_velocity {
        return None;
    }
    let (v0_r, v0_y) = (v0 * cos, v0 * sin);
    let (sin_xz, cos_xz) = xz_angle.sin_cos();
    let samples = (r / v0_r).ceil() as i32 * 2;
    let mut previous: Option<Vector3<f64>> = None;
    let mut ri = 0.0;
    for _ in 0..samples - 1 {
        ri += r / f64::from(samples);
        let yi = sin / cos * ri - ri * ri * gravity / (2.0 * v0_squared * cos * cos);
        let sample = Vector3::new(
            position.x + ri * cos_xz,
            position.y + yi,
            position.z + ri * sin_xz,
        );
        if let Some(previous) = previous
            && !is_clear_transition(mob, previous, sample)
        {
            return None;
        }
        previous = Some(sample);
    }
    Some(Vector3::new(v0_r * cos_xz, v0_y, v0_r * sin_xz) * 0.95)
}

/// Vanilla `LongJumpUtil.isClearTransition`.
fn is_clear_transition(mob: &dyn Mob, from: Vector3<f64>, to: Vector3<f64>) -> bool {
    let entity = mob.get_entity();
    let dimensions = entity.get_dimensions(EntityPose::LongJumping);
    let world = entity.world.load();
    let delta = to - from;
    let min_dimension = f64::from(dimensions.width.min(dimensions.height));
    let checks = (delta.length() / min_dimension).ceil() as i32;
    let step = delta.normalize() * (min_dimension * 0.9);
    let mut point = from;
    for i in 0..checks {
        point = if i == checks - 1 { to } else { point + step };
        let bounds = BoundingBox::new_from_pos(point.x, point.y, point.z, &dimensions);
        if !world.no_collision(&bounds) {
            return false;
        }
    }
    true
}

impl Behavior for LongJumpToRandomPos {
    fn entry_conditions(&self) -> &[(MemoryModuleId, MemoryStatus)] {
        &TO_RANDOM_POS_CONDITIONS
    }

    fn min_duration(&self) -> i32 {
        200
    }

    fn max_duration(&self) -> i32 {
        200
    }

    fn check_extra_start_conditions(&mut self, tick: &mut BrainTick<'_>) -> bool {
        let entity = tick.mob.get_entity();
        let can_start = entity.on_ground.load(Relaxed)
            && !entity.is_in_water()
            && !entity.touching_lava.load(Relaxed)
            && tick.world.get_block(&entity.block_pos.load()) != &pumpkin_data::Block::HONEY_BLOCK;
        if !can_start {
            start_cooldown(tick, &self.settings.cooldown);
        }
        can_start
    }

    fn can_still_use(&mut self, tick: &mut BrainTick<'_>) -> bool {
        let entity = tick.mob.get_entity();
        let valid = self.initial_position == Some(entity.pos.load())
            && self.find_jump_tries > 0
            && !entity.is_in_water()
            && (self.chosen_jump.is_some() || !self.candidates.is_empty());
        if !valid && !tick.state.has_memory_value(types::LONG_JUMP_MID_JUMP.id()) {
            start_cooldown(tick, &self.settings.cooldown);
            tick.state.erase(types::LOOK_TARGET.id());
        }
        valid
    }

    fn start(&mut self, tick: &mut BrainTick<'_>) {
        self.chosen_jump = None;
        self.find_jump_tries = FIND_JUMP_TRIES;
        let entity = tick.mob.get_entity();
        self.initial_position = Some(entity.pos.load());
        let own = entity.block_pos.load();
        let (width, height) = (self.settings.max_width, self.settings.max_height);
        self.candidates.clear();
        for x in -width..=width {
            for y in -height..=height {
                for z in -width..=width {
                    if (x, y, z) != (0, 0, 0) {
                        self.candidates.push(PossibleJump {
                            target: BlockPos::new(own.0.x + x, own.0.y + y, own.0.z + z),
                            // Vanilla `Mth.ceil(mobPos.distSqr(pos))`.
                            weight: x * x + y * y + z * z,
                        });
                    }
                }
            }
        }
        self.not_preferred.clear();
        self.wanting_preferred = self
            .preferred
            .is_some_and(|(_, chance)| tick.mob.get_random().random::<f32>() < chance);
    }

    fn tick(&mut self, tick: &mut BrainTick<'_>) {
        let Some(jump) = self.chosen_jump else {
            self.find_jump_tries -= 1;
            self.pick_candidate(tick);
            return;
        };
        if tick.time - self.prepare_jump_start < PREPARE_JUMP_DURATION {
            return;
        }
        let living = &tick.mob.get_mob_entity().living_entity;
        living.discard_friction.store(true, Relaxed);
        let length = jump.length();
        let boosted = length + living.get_jump_boost_power();
        living.entity.set_velocity(jump * (boosted / length));
        tick.state.set(types::LONG_JUMP_MID_JUMP, true);
        tick.mob
            .play_entity_sound(self.settings.jump_sound, 1.0, 1.0);
    }

    fn debug_name(&self) -> &'static str {
        "LongJumpToRandomPos"
    }
}
