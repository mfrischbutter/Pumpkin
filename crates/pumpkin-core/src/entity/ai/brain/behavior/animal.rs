//! The brain behaviours vanilla's brain animals share: following a tempting player, making
//! love, a baby following an adult and looking around.

use std::sync::Arc;

use pumpkin_util::math::vector3::Vector3;
use rand::RngExt;

use super::super::BrainTick;
use super::super::memory::{
    MemoryModuleId, MemoryStatus,
    position_tracker::{BlockPosTracker, EntityTracker, PositionTracker},
    types,
    walk_target::WalkTarget,
};
use super::OneShot;
use super::timed::Behavior;
use crate::entity::EntityBase;
use crate::entity::ai::goal::breed::BreedGoal;
use crate::entity::mob::Mob;

/// Vanilla `FollowTemptation.TEMPTATION_COOLDOWN`.
const TEMPTATION_COOLDOWN: i32 = 100;

const FOLLOW_TEMPTATION_CONDITIONS: [(MemoryModuleId, MemoryStatus); 6] = [
    (types::LOOK_TARGET.id(), MemoryStatus::Registered),
    (types::WALK_TARGET.id(), MemoryStatus::Registered),
    (
        types::TEMPTATION_COOLDOWN_TICKS.id(),
        MemoryStatus::ValueAbsent,
    ),
    (types::TEMPTING_PLAYER.id(), MemoryStatus::ValuePresent),
    (types::BREED_TARGET.id(), MemoryStatus::ValueAbsent),
    (types::IS_PANICKING.id(), MemoryStatus::ValueAbsent),
];

/// Vanilla `FollowTemptation`: walks to the player holding its food, stopping `close_enough`
/// away, then waits 100 ticks before it can be tempted again.
pub struct FollowTemptation {
    speed: fn(&dyn Mob) -> f32,
    close_enough: fn(&dyn Mob) -> f64,
}

impl FollowTemptation {
    #[must_use]
    pub const fn new(speed: fn(&dyn Mob) -> f32, close_enough: fn(&dyn Mob) -> f64) -> Self {
        Self {
            speed,
            close_enough,
        }
    }
}

impl Behavior for FollowTemptation {
    fn entry_conditions(&self) -> &[(MemoryModuleId, MemoryStatus)] {
        &FOLLOW_TEMPTATION_CONDITIONS
    }

    fn max_duration(&self) -> i32 {
        i32::MAX
    }

    fn can_still_use(&mut self, tick: &mut BrainTick<'_>) -> bool {
        tick.state.has_memory_value(types::TEMPTING_PLAYER.id())
            && !tick.state.has_memory_value(types::BREED_TARGET.id())
            && !tick.state.has_memory_value(types::IS_PANICKING.id())
    }

    fn start(&mut self, tick: &mut BrainTick<'_>) {
        tick.state.set(types::IS_TEMPTED, true);
    }

    fn tick(&mut self, tick: &mut BrainTick<'_>) {
        let Some(player) = tick.state.get(types::TEMPTING_PLAYER).cloned() else {
            return;
        };
        let player: Arc<dyn EntityBase> = player;
        tick.state.set(
            types::LOOK_TARGET,
            Arc::new(EntityTracker::new(player.clone(), true)) as Arc<dyn PositionTracker>,
        );
        let close_enough = (self.close_enough)(tick.mob);
        let distance = tick
            .mob
            .get_entity()
            .pos
            .load()
            .squared_distance_to_vec(&player.get_entity().pos.load());
        if distance < close_enough * close_enough {
            tick.state.erase(types::WALK_TARGET.id());
        } else {
            tick.state.set(
                types::WALK_TARGET,
                WalkTarget::new(
                    Arc::new(EntityTracker::new(player, false)),
                    (self.speed)(tick.mob),
                    2,
                ),
            );
        }
    }

    fn stop(&mut self, tick: &mut BrainTick<'_>) {
        tick.state.erase(types::IS_TEMPTED.id());
        tick.state
            .set(types::TEMPTATION_COOLDOWN_TICKS, TEMPTATION_COOLDOWN);
        tick.state.erase(types::WALK_TARGET.id());
        tick.state.erase(types::LOOK_TARGET.id());
    }

    fn debug_name(&self) -> &'static str {
        "FollowTemptation"
    }
}

const MAKE_LOVE_CONDITIONS: [(MemoryModuleId, MemoryStatus); 5] = [
    (
        types::NEAREST_VISIBLE_LIVING_ENTITIES.id(),
        MemoryStatus::ValuePresent,
    ),
    (types::BREED_TARGET.id(), MemoryStatus::ValueAbsent),
    (types::WALK_TARGET.id(), MemoryStatus::Registered),
    (types::LOOK_TARGET.id(), MemoryStatus::Registered),
    (types::IS_PANICKING.id(), MemoryStatus::ValueAbsent),
];

/// Vanilla `AnimalMakeLove`: an animal in love walks to a visible partner in love and, after
/// 60 to 110 ticks close together, they have a baby.
pub struct AnimalMakeLove {
    speed: f32,
    spawn_child_at: i64,
}

impl AnimalMakeLove {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            speed: 1.0,
            spawn_child_at: 0,
        }
    }

    /// Vanilla's `speedModifier` argument, 1.0 by default.
    #[must_use]
    pub const fn with_speed(mut self, speed: f32) -> Self {
        self.speed = speed;
        self
    }

    fn find_partner(tick: &BrainTick<'_>) -> Option<Arc<dyn EntityBase>> {
        let visible = tick.state.get(types::NEAREST_VISIBLE_LIVING_ENTITIES)?;
        let mob = tick.mob;
        let kind = mob.get_entity().entity_type;
        let ctx = tick.visibility();
        visible.find_closest(&ctx, |candidate| {
            candidate.get_entity().entity_type == kind
                && candidate.is_in_love()
                && candidate.is_breeding_ready()
                && !candidate.is_panicking()
                && mob
                    .as_animal()
                    .is_some_and(|animal| animal.can_mate(candidate.as_ref()))
        })
    }

    fn walk_to_each_other(
        &self,
        mob: &dyn Mob,
        partner: &Arc<dyn EntityBase>,
        tick: &mut BrainTick<'_>,
    ) {
        let speed = self.speed;
        tick.state.set(
            types::LOOK_TARGET,
            Arc::new(EntityTracker::new(partner.clone(), true)) as Arc<dyn PositionTracker>,
        );
        tick.state.set(
            types::WALK_TARGET,
            WalkTarget::new(
                Arc::new(EntityTracker::new(partner.clone(), false)),
                speed,
                2,
            ),
        );
        if let Some(partner_mob) = partner.get_mob() {
            let me = tick.world.get_entity_by_id(mob.get_entity().entity_id);
            partner_mob
                .get_mob_entity()
                .with_brain(partner_mob, |brain| {
                    if let Some(me) = me {
                        brain.set(
                            types::LOOK_TARGET,
                            Arc::new(EntityTracker::new(me.clone(), true))
                                as Arc<dyn PositionTracker>,
                        );
                        brain.set(
                            types::WALK_TARGET,
                            WalkTarget::new(Arc::new(EntityTracker::new(me, false)), speed, 2),
                        );
                    }
                });
        }
    }
}

impl Default for AnimalMakeLove {
    fn default() -> Self {
        Self::new()
    }
}

impl Behavior for AnimalMakeLove {
    fn entry_conditions(&self) -> &[(MemoryModuleId, MemoryStatus)] {
        &MAKE_LOVE_CONDITIONS
    }

    fn min_duration(&self) -> i32 {
        110
    }

    fn max_duration(&self) -> i32 {
        110
    }

    fn check_extra_start_conditions(&mut self, tick: &mut BrainTick<'_>) -> bool {
        tick.mob.get_mob_entity().is_in_love() && Self::find_partner(tick).is_some()
    }

    fn start(&mut self, tick: &mut BrainTick<'_>) {
        let Some(partner) = Self::find_partner(tick) else {
            return;
        };
        tick.state.set(types::BREED_TARGET, partner.clone());
        if let Some(partner_mob) = partner.get_mob()
            && let Some(me) = tick.world.get_entity_by_id(tick.mob.get_entity().entity_id)
        {
            partner_mob
                .get_mob_entity()
                .with_brain(partner_mob, |brain| brain.set(types::BREED_TARGET, me));
        }
        self.walk_to_each_other(tick.mob, &partner, tick);
        self.spawn_child_at = tick.time + 60 + i64::from(tick.mob.get_random().random_range(0..50));
    }

    fn can_still_use(&mut self, tick: &mut BrainTick<'_>) -> bool {
        let Some(partner) = tick.state.get(types::BREED_TARGET).cloned() else {
            return false;
        };
        partner.get_entity().entity_type == tick.mob.get_entity().entity_type
            && partner.get_entity().is_alive()
            && tick
                .mob
                .as_animal()
                .is_some_and(|animal| animal.can_mate(partner.as_ref()))
            && tick.time <= self.spawn_child_at
            && !tick.mob.is_panicking()
            && !partner.is_panicking()
    }

    fn tick(&mut self, tick: &mut BrainTick<'_>) {
        let Some(partner) = tick.state.get(types::BREED_TARGET).cloned() else {
            return;
        };
        self.walk_to_each_other(tick.mob, &partner, tick);
        let distance = tick
            .mob
            .get_entity()
            .pos
            .load()
            .squared_distance_to_vec(&partner.get_entity().pos.load());
        if distance < 9.0 && tick.time >= self.spawn_child_at {
            BreedGoal::breed(tick.mob, partner.as_ref());
            tick.state.erase(types::BREED_TARGET.id());
            if let Some(partner_mob) = partner.get_mob() {
                partner_mob
                    .get_mob_entity()
                    .with_brain(partner_mob, |brain| brain.erase(types::BREED_TARGET.id()));
            }
        }
    }

    fn stop(&mut self, tick: &mut BrainTick<'_>) {
        tick.state.erase(types::BREED_TARGET.id());
        tick.state.erase(types::WALK_TARGET.id());
        tick.state.erase(types::LOOK_TARGET.id());
        self.spawn_child_at = 0;
    }

    fn debug_name(&self) -> &'static str {
        "AnimalMakeLove"
    }
}

/// Vanilla `BabyFollowAdult`: a baby walks to the nearest visible adult when it is between
/// `min` and `max` blocks away.
#[must_use]
pub fn baby_follow_adult(min: i32, max: i32, speed: fn(&dyn Mob) -> f32) -> OneShot {
    OneShot::new(
        "BabyFollowAdult",
        vec![
            (
                types::NEAREST_VISIBLE_ADULT.id(),
                MemoryStatus::ValuePresent,
            ),
            (types::LOOK_TARGET.id(), MemoryStatus::Registered),
            (types::WALK_TARGET.id(), MemoryStatus::ValueAbsent),
        ],
        move |tick| {
            if tick
                .mob
                .get_entity()
                .age
                .load(std::sync::atomic::Ordering::Relaxed)
                >= 0
            {
                return false;
            }
            let Some(adult) = tick.state.get(types::NEAREST_VISIBLE_ADULT).cloned() else {
                return false;
            };
            let distance = tick
                .mob
                .get_entity()
                .pos
                .load()
                .squared_distance_to_vec(&adult.get_entity().pos.load());
            let (near, far) = (f64::from(min), f64::from(max + 1));
            if distance >= far * far || distance < near * near {
                return false;
            }
            tick.state.set(
                types::LOOK_TARGET,
                Arc::new(EntityTracker::new(adult.clone(), true)) as Arc<dyn PositionTracker>,
            );
            tick.state.set(
                types::WALK_TARGET,
                WalkTarget::new(
                    Arc::new(EntityTracker::new(adult, false)),
                    speed(tick.mob),
                    min - 1,
                ),
            );
            true
        },
    )
}

const LOOK_AROUND_CONDITIONS: [(MemoryModuleId, MemoryStatus); 2] = [
    (types::LOOK_TARGET.id(), MemoryStatus::ValueAbsent),
    (types::GAZE_COOLDOWN_TICKS.id(), MemoryStatus::ValueAbsent),
];

/// Vanilla `RandomLookAround` (the brain one): looks at a random spot up to `max_yaw` to
/// either side and waits `interval` ticks.
pub struct RandomLookAround {
    interval: (i32, i32),
    max_yaw: f32,
    min_pitch: f32,
    pitch_range: f32,
}

impl RandomLookAround {
    #[must_use]
    pub const fn new(interval: (i32, i32), max_yaw: f32, min_pitch: f32, max_pitch: f32) -> Self {
        Self {
            interval,
            max_yaw,
            min_pitch,
            pitch_range: max_pitch - min_pitch,
        }
    }
}

impl Behavior for RandomLookAround {
    fn entry_conditions(&self) -> &[(MemoryModuleId, MemoryStatus)] {
        &LOOK_AROUND_CONDITIONS
    }

    fn start(&mut self, tick: &mut BrainTick<'_>) {
        let entity = tick.mob.get_entity();
        let (pitch, yaw, wait) = {
            let mut rng = tick.mob.get_random();
            let pitch = rng
                .random::<f32>()
                .mul_add(self.pitch_range, self.min_pitch)
                .clamp(-90.0, 90.0);
            let yaw = pumpkin_util::math::wrap_degrees(
                (2.0 * rng.random::<f32>()).mul_add(self.max_yaw, entity.yaw.load()) - self.max_yaw,
            );
            (
                pitch,
                yaw,
                rng.random_range(self.interval.0..=self.interval.1),
            )
        };
        let direction = Vector3::rotation_vector(f64::from(pitch), f64::from(yaw));
        let target = entity.get_eye_pos() + direction;
        tick.state.set(
            types::LOOK_TARGET,
            Arc::new(BlockPosTracker::new(target.to_block_pos())) as Arc<dyn PositionTracker>,
        );
        tick.state.set(types::GAZE_COOLDOWN_TICKS, wait);
    }

    fn debug_name(&self) -> &'static str {
        "RandomLookAround"
    }
}
