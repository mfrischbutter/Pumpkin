//! Vanilla `WardenAi` and the behaviors in `ai.behavior.warden`.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use pumpkin_data::attributes::Attributes;
use pumpkin_data::damage::DamageType;
use pumpkin_data::entity::{EntityPose, EntityStatus};
use pumpkin_data::environment_attribute::Activity;
use pumpkin_data::particle::Particle;
use pumpkin_data::sound::Sound;
use pumpkin_util::math::vector3::Vector3;

use super::warden::{ANGER_DEFAULT, WardenEntity};
use crate::entity::EntityBase;
use crate::entity::ai::brain::BrainState;
use crate::entity::ai::brain::BrainTick;
use crate::entity::ai::brain::activity::ActivityData;
use crate::entity::ai::brain::behavior::target_location::go_to_target_location;
use crate::entity::ai::brain::behavior::timed::Behavior;
use crate::entity::ai::brain::behavior::{
    DoNothing, LookAtTargetSink, MoveToTargetSink, OneShot, Swim, Timed, melee_attack,
    random_stroll, run_one_with_conditions, set_walk_target_from_attack_target,
    stop_attacking_if_never_tired,
};
use crate::entity::ai::brain::memory::position_tracker::{
    BlockPosTracker, EntityTracker, PositionTracker,
};
use crate::entity::ai::brain::memory::{MemoryModuleId, MemoryStatus, types};
use crate::entity::mob::Mob;

const SPEED_WHEN_IDLING: f32 = 0.5;
const SPEED_WHEN_INVESTIGATING: f32 = 0.7;
const SPEED_WHEN_FIGHTING: f32 = 1.2;
const MELEE_ATTACK_COOLDOWN: i64 = 18;
const DIGGING_DURATION: i32 = 100;
/// Vanilla `WardenAi.EMERGE_DURATION`.
pub const EMERGE_DURATION: i64 = 134;
const ROAR_DURATION: i32 = 84;
const SNIFFING_DURATION: i32 = 84;
pub const DIGGING_COOLDOWN: i64 = 1200;
const DISTURBANCE_EXPIRY: i64 = 100;
const ROAR_SOUND_DELAY: i64 = 25;
const ROAR_ANGER_INCREASE: i32 = 20;
const SONIC_BOOM_DISTANCE_XZ: f64 = 15.0;
const SONIC_BOOM_DISTANCE_Y: f64 = 20.0;
const SONIC_BOOM_DURATION: i32 = 60;
const SONIC_BOOM_SOUND_DELAY: i64 = 34;
const SONIC_BOOM_COOLDOWN: i64 = 40;
const SONIC_BOOM_DAMAGE: f32 = 10.0;
const SONIC_BOOM_KNOCKBACK_VERTICAL: f64 = 0.5;
const SONIC_BOOM_KNOCKBACK_HORIZONTAL: f64 = 2.5;
/// Vanilla's `EntityAttachment.WARDEN_CHEST`.
const CHEST_HEIGHT: f64 = 1.6;
const SNIFF_ANGER_DISTANCE_XZ: f64 = 6.0;
const SNIFF_ANGER_DISTANCE_Y: f64 = 20.0;

fn as_warden(mob: &dyn Mob) -> Option<&WardenEntity> {
    mob.cast_any().downcast_ref::<WardenEntity>()
}

fn set_pose(tick: &BrainTick<'_>, pose: EntityPose) {
    tick.mob.get_entity().set_pose(pose);
}

/// Back to standing, if the pose is still the behavior's.
fn stand_up_from(tick: &BrainTick<'_>, pose: EntityPose) {
    if tick.mob.get_entity().pose.load() == pose {
        set_pose(tick, EntityPose::Standing);
    }
}

fn look_at_entity(state: &mut BrainState, target: Arc<dyn EntityBase>) {
    state.set(
        types::LOOK_TARGET,
        Arc::new(EntityTracker::new(target, true)) as Arc<dyn PositionTracker>,
    );
}

/// Vanilla `Entity.closerThan(other, xz, y)`.
fn closer_than(a: &dyn EntityBase, b: &dyn EntityBase, xz: f64, y: f64) -> bool {
    let (a, b) = (a.get_entity().pos.load(), b.get_entity().pos.load());
    let (dx, dy, dz) = (a.x - b.x, a.y - b.y, a.z - b.z);
    dx * dx + dz * dz < xz * xz && dy * dy < y * y
}

/// Vanilla `WardenAi.setDigCooldown`.
pub fn set_dig_cooldown(state: &mut BrainState) {
    if state.has_memory_value(types::DIG_COOLDOWN.id()) {
        state.set_with_expiry(types::DIG_COOLDOWN, (), DIGGING_COOLDOWN);
    }
}

/// Vanilla `WardenAi.setDisturbanceLocation`.
pub fn set_disturbance_location(
    warden: &WardenEntity,
    state: &mut BrainState,
    position: pumpkin_util::math::position::BlockPos,
) {
    if warden.entity_angry_at().is_none() && !state.has_memory_value(types::ATTACK_TARGET.id()) {
        set_dig_cooldown(state);
        state.set_with_expiry(types::SNIFF_COOLDOWN, (), DISTURBANCE_EXPIRY);
        state.set_with_expiry(
            types::LOOK_TARGET,
            Arc::new(BlockPosTracker::new(position)) as Arc<dyn PositionTracker>,
            DISTURBANCE_EXPIRY,
        );
        state.set_with_expiry(types::DISTURBANCE_LOCATION, position, DISTURBANCE_EXPIRY);
        state.erase(types::WALK_TARGET.id());
    }
}

/// Vanilla `SonicBoom.setCooldown`.
pub fn set_sonic_boom_cooldown(state: &mut BrainState, cooldown: i64) {
    state.set_with_expiry(types::SONIC_BOOM_COOLDOWN, (), cooldown);
}

/// Vanilla `Warden.setAttackTarget`.
pub fn set_attack_target(state: &mut BrainState, target: Arc<dyn EntityBase>) {
    state.erase(types::ROAR_TARGET.id());
    state.set(types::ATTACK_TARGET, target);
    state.erase(types::CANT_REACH_WALK_TARGET_SINCE.id());
    set_sonic_boom_cooldown(state, 200);
}

/// Vanilla `Emerging`.
struct Emerging;

impl Behavior for Emerging {
    fn entry_conditions(&self) -> &[(MemoryModuleId, MemoryStatus)] {
        const CONDITIONS: [(MemoryModuleId, MemoryStatus); 3] = [
            (types::IS_EMERGING.id(), MemoryStatus::ValuePresent),
            (types::WALK_TARGET.id(), MemoryStatus::ValueAbsent),
            (types::LOOK_TARGET.id(), MemoryStatus::Registered),
        ];
        &CONDITIONS
    }

    fn min_duration(&self) -> i32 {
        EMERGE_DURATION as i32
    }

    fn max_duration(&self) -> i32 {
        EMERGE_DURATION as i32
    }

    fn start(&mut self, tick: &mut BrainTick<'_>) {
        set_pose(tick, EntityPose::Emerging);
        tick.mob
            .play_entity_sound(Sound::EntityWardenEmerge, 5.0, 1.0);
    }

    fn can_still_use(&mut self, _tick: &mut BrainTick<'_>) -> bool {
        true
    }

    fn stop(&mut self, tick: &mut BrainTick<'_>) {
        stand_up_from(tick, EntityPose::Emerging);
    }

    fn debug_name(&self) -> &'static str {
        "Emerging"
    }
}

/// Vanilla `Digging`: it goes back into the ground, and that is the end of it.
struct Digging;

impl Behavior for Digging {
    fn entry_conditions(&self) -> &[(MemoryModuleId, MemoryStatus)] {
        const CONDITIONS: [(MemoryModuleId, MemoryStatus); 2] = [
            (types::ATTACK_TARGET.id(), MemoryStatus::ValueAbsent),
            (types::WALK_TARGET.id(), MemoryStatus::ValueAbsent),
        ];
        &CONDITIONS
    }

    fn min_duration(&self) -> i32 {
        DIGGING_DURATION
    }

    fn max_duration(&self) -> i32 {
        DIGGING_DURATION
    }

    fn check_extra_start_conditions(&mut self, tick: &mut BrainTick<'_>) -> bool {
        let entity = tick.mob.get_entity();
        entity.on_ground.load(Ordering::Relaxed)
            || entity.is_in_water()
            || entity.touching_lava.load(Ordering::SeqCst)
    }

    fn start(&mut self, tick: &mut BrainTick<'_>) {
        let entity = tick.mob.get_entity();
        if entity.on_ground.load(Ordering::Relaxed) {
            set_pose(tick, EntityPose::Digging);
            tick.mob.play_entity_sound(Sound::EntityWardenDig, 5.0, 1.0);
        } else {
            tick.mob
                .play_entity_sound(Sound::EntityWardenAgitated, 5.0, 1.0);
            entity.remove();
        }
    }

    fn can_still_use(&mut self, tick: &mut BrainTick<'_>) -> bool {
        !tick.mob.get_entity().is_removed()
    }

    fn stop(&mut self, tick: &mut BrainTick<'_>) {
        let entity = tick.mob.get_entity();
        if !entity.is_removed() {
            entity.remove();
        }
    }

    fn debug_name(&self) -> &'static str {
        "Digging"
    }
}

/// Vanilla `ForceUnmount`.
fn force_unmount() -> OneShot {
    OneShot::new("ForceUnmount", Vec::new(), |tick| {
        let Some(warden) = as_warden(tick.mob) else {
            return false;
        };
        let entity = warden.get_entity();
        let Some(vehicle) = entity.get_vehicle() else {
            return false;
        };
        vehicle.get_entity().remove_passenger(entity.entity_id);
        true
    })
}

/// Vanilla `Roar`.
struct Roar;

impl Behavior for Roar {
    fn entry_conditions(&self) -> &[(MemoryModuleId, MemoryStatus)] {
        const CONDITIONS: [(MemoryModuleId, MemoryStatus); 4] = [
            (types::ROAR_TARGET.id(), MemoryStatus::ValuePresent),
            (types::ATTACK_TARGET.id(), MemoryStatus::ValueAbsent),
            (types::ROAR_SOUND_COOLDOWN.id(), MemoryStatus::Registered),
            (types::ROAR_SOUND_DELAY.id(), MemoryStatus::Registered),
        ];
        &CONDITIONS
    }

    fn min_duration(&self) -> i32 {
        ROAR_DURATION
    }

    fn max_duration(&self) -> i32 {
        ROAR_DURATION
    }

    fn start(&mut self, tick: &mut BrainTick<'_>) {
        tick.state
            .set_with_expiry(types::ROAR_SOUND_DELAY, (), ROAR_SOUND_DELAY);
        tick.state.erase(types::WALK_TARGET.id());
        let Some(target) = tick.state.get(types::ROAR_TARGET).cloned() else {
            return;
        };
        look_at_entity(tick.state, target.clone());
        set_pose(tick, EntityPose::Roaring);
        if let Some(warden) = as_warden(tick.mob) {
            warden.increase_anger_in(tick.state, &target, ROAR_ANGER_INCREASE, false);
        }
    }

    fn can_still_use(&mut self, _tick: &mut BrainTick<'_>) -> bool {
        true
    }

    fn tick(&mut self, tick: &mut BrainTick<'_>) {
        if !tick.state.has_memory_value(types::ROAR_SOUND_DELAY.id())
            && !tick.state.has_memory_value(types::ROAR_SOUND_COOLDOWN.id())
        {
            tick.state.set_with_expiry(
                types::ROAR_SOUND_COOLDOWN,
                (),
                i64::from(ROAR_DURATION) - ROAR_SOUND_DELAY,
            );
            tick.mob
                .play_entity_sound(Sound::EntityWardenRoar, 3.0, 1.0);
        }
    }

    fn stop(&mut self, tick: &mut BrainTick<'_>) {
        stand_up_from(tick, EntityPose::Roaring);
        if let Some(target) = tick.state.get(types::ROAR_TARGET).cloned() {
            set_attack_target(tick.state, target);
        }
        tick.state.erase(types::ROAR_TARGET.id());
    }

    fn debug_name(&self) -> &'static str {
        "Roar"
    }
}

/// Vanilla `SetRoarTarget.create(Warden::getEntityAngryAt)`.
fn set_roar_target() -> OneShot {
    OneShot::new(
        "SetRoarTarget",
        vec![
            (types::ROAR_TARGET.id(), MemoryStatus::ValueAbsent),
            (types::ATTACK_TARGET.id(), MemoryStatus::ValueAbsent),
            (
                types::CANT_REACH_WALK_TARGET_SINCE.id(),
                MemoryStatus::Registered,
            ),
        ],
        |tick| {
            let Some(warden) = as_warden(tick.mob) else {
                return false;
            };
            let Some(target) = warden
                .entity_angry_at()
                .filter(|target| warden.can_target_entity(target.as_ref()))
            else {
                return false;
            };
            tick.state.set(types::ROAR_TARGET, target);
            tick.state.erase(types::CANT_REACH_WALK_TARGET_SINCE.id());
            true
        },
    )
}

/// Vanilla `SetWardenLookTarget`.
fn set_warden_look_target() -> OneShot {
    OneShot::new(
        "SetWardenLookTarget",
        vec![
            (types::LOOK_TARGET.id(), MemoryStatus::Registered),
            (types::DISTURBANCE_LOCATION.id(), MemoryStatus::Registered),
            (types::ROAR_TARGET.id(), MemoryStatus::Registered),
            (types::ATTACK_TARGET.id(), MemoryStatus::ValueAbsent),
        ],
        |tick| {
            let target = tick
                .state
                .get(types::ROAR_TARGET)
                .map(|target| target.get_entity().block_pos.load())
                .or_else(|| tick.state.get(types::DISTURBANCE_LOCATION).copied());
            let Some(target) = target else {
                return false;
            };
            tick.state.set(
                types::LOOK_TARGET,
                Arc::new(BlockPosTracker::new(target)) as Arc<dyn PositionTracker>,
            );
            true
        },
    )
}

/// Vanilla `TryToSniff`.
fn try_to_sniff() -> OneShot {
    OneShot::new(
        "TryToSniff",
        vec![
            (types::IS_SNIFFING.id(), MemoryStatus::Registered),
            (types::WALK_TARGET.id(), MemoryStatus::Registered),
            (types::SNIFF_COOLDOWN.id(), MemoryStatus::ValueAbsent),
            (types::NEAREST_ATTACKABLE.id(), MemoryStatus::ValuePresent),
            (types::DISTURBANCE_LOCATION.id(), MemoryStatus::ValueAbsent),
        ],
        |tick| {
            tick.state.set(types::IS_SNIFFING, ());
            tick.state
                .set_with_expiry(types::SNIFF_COOLDOWN, (), rand::random_range(100..=200));
            tick.state.erase(types::WALK_TARGET.id());
            set_pose(tick, EntityPose::Sniffing);
            true
        },
    )
}

/// Vanilla `Sniffing`.
struct Sniffing;

impl Behavior for Sniffing {
    fn entry_conditions(&self) -> &[(MemoryModuleId, MemoryStatus)] {
        const CONDITIONS: [(MemoryModuleId, MemoryStatus); 7] = [
            (types::IS_SNIFFING.id(), MemoryStatus::ValuePresent),
            (types::ATTACK_TARGET.id(), MemoryStatus::ValueAbsent),
            (types::WALK_TARGET.id(), MemoryStatus::ValueAbsent),
            (types::LOOK_TARGET.id(), MemoryStatus::Registered),
            (types::NEAREST_ATTACKABLE.id(), MemoryStatus::Registered),
            (types::DISTURBANCE_LOCATION.id(), MemoryStatus::Registered),
            (types::SNIFF_COOLDOWN.id(), MemoryStatus::Registered),
        ];
        &CONDITIONS
    }

    fn min_duration(&self) -> i32 {
        SNIFFING_DURATION
    }

    fn max_duration(&self) -> i32 {
        SNIFFING_DURATION
    }

    fn start(&mut self, tick: &mut BrainTick<'_>) {
        tick.mob
            .play_entity_sound(Sound::EntityWardenSniff, 5.0, 1.0);
    }

    fn can_still_use(&mut self, _tick: &mut BrainTick<'_>) -> bool {
        true
    }

    fn stop(&mut self, tick: &mut BrainTick<'_>) {
        stand_up_from(tick, EntityPose::Sniffing);
        tick.state.erase(types::IS_SNIFFING.id());
        let Some(warden) = as_warden(tick.mob) else {
            return;
        };
        let Some(entity) = tick
            .state
            .get(types::NEAREST_ATTACKABLE)
            .cloned()
            .filter(|entity| warden.can_target_entity(entity.as_ref()))
        else {
            return;
        };
        if closer_than(
            warden,
            entity.as_ref(),
            SNIFF_ANGER_DISTANCE_XZ,
            SNIFF_ANGER_DISTANCE_Y,
        ) {
            warden.increase_anger_in(tick.state, &entity, ANGER_DEFAULT, true);
        }
        if !tick
            .state
            .has_memory_value(types::DISTURBANCE_LOCATION.id())
        {
            set_disturbance_location(warden, tick.state, entity.get_entity().block_pos.load());
        }
    }

    fn debug_name(&self) -> &'static str {
        "Sniffing"
    }
}

/// Vanilla `SonicBoom`.
struct SonicBoom;

impl Behavior for SonicBoom {
    fn entry_conditions(&self) -> &[(MemoryModuleId, MemoryStatus)] {
        const CONDITIONS: [(MemoryModuleId, MemoryStatus); 4] = [
            (types::ATTACK_TARGET.id(), MemoryStatus::ValuePresent),
            (types::SONIC_BOOM_COOLDOWN.id(), MemoryStatus::ValueAbsent),
            (
                types::SONIC_BOOM_SOUND_COOLDOWN.id(),
                MemoryStatus::Registered,
            ),
            (types::SONIC_BOOM_SOUND_DELAY.id(), MemoryStatus::Registered),
        ];
        &CONDITIONS
    }

    fn min_duration(&self) -> i32 {
        SONIC_BOOM_DURATION
    }

    fn max_duration(&self) -> i32 {
        SONIC_BOOM_DURATION
    }

    fn check_extra_start_conditions(&mut self, tick: &mut BrainTick<'_>) -> bool {
        let Some(warden) = as_warden(tick.mob) else {
            return false;
        };
        tick.state.get(types::ATTACK_TARGET).is_some_and(|target| {
            closer_than(
                warden,
                target.as_ref(),
                SONIC_BOOM_DISTANCE_XZ,
                SONIC_BOOM_DISTANCE_Y,
            )
        })
    }

    fn can_still_use(&mut self, _tick: &mut BrainTick<'_>) -> bool {
        true
    }

    fn start(&mut self, tick: &mut BrainTick<'_>) {
        tick.state.set_with_expiry(
            types::ATTACK_COOLING_DOWN,
            true,
            i64::from(SONIC_BOOM_DURATION),
        );
        tick.state
            .set_with_expiry(types::SONIC_BOOM_SOUND_DELAY, (), SONIC_BOOM_SOUND_DELAY);
        let entity = tick.mob.get_entity();
        entity.world.load().send_entity_status(
            entity,
            EntityStatus::SonicCharge,
            pumpkin_protocol::bedrock::server::ActorEventID::None,
        );
        tick.mob
            .play_entity_sound(Sound::EntityWardenSonicCharge, 3.0, 1.0);
    }

    fn tick(&mut self, tick: &mut BrainTick<'_>) {
        if let Some(target) = tick.state.get(types::ATTACK_TARGET).cloned() {
            look_at_entity(tick.state, target);
        }
        if tick
            .state
            .has_memory_value(types::SONIC_BOOM_SOUND_DELAY.id())
            || tick
                .state
                .has_memory_value(types::SONIC_BOOM_SOUND_COOLDOWN.id())
        {
            return;
        }
        tick.state.set_with_expiry(
            types::SONIC_BOOM_SOUND_COOLDOWN,
            (),
            i64::from(SONIC_BOOM_DURATION) - SONIC_BOOM_SOUND_DELAY,
        );
        let Some(warden) = as_warden(tick.mob) else {
            return;
        };
        if let Some(target) = tick
            .state
            .get(types::ATTACK_TARGET)
            .cloned()
            .filter(|target| {
                warden.can_target_entity(target.as_ref())
                    && closer_than(
                        warden,
                        target.as_ref(),
                        SONIC_BOOM_DISTANCE_XZ,
                        SONIC_BOOM_DISTANCE_Y,
                    )
            })
        {
            fire_sonic_boom(warden, target.as_ref());
        }
    }

    fn stop(&mut self, tick: &mut BrainTick<'_>) {
        set_sonic_boom_cooldown(tick.state, SONIC_BOOM_COOLDOWN);
    }

    fn debug_name(&self) -> &'static str {
        "SonicBoom"
    }
}

/// The boom itself: particles along the line to the target's eyes, which nothing can block.
fn fire_sonic_boom(warden: &WardenEntity, target: &dyn EntityBase) {
    let entity = warden.get_entity();
    let world = entity.world.load();
    let source = entity.pos.load() + Vector3::new(0.0, CHEST_HEIGHT, 0.0);
    let target_entity = target.get_entity();
    let target_pos = target_entity.pos.load();
    let delta = Vector3::new(target_pos.x, target_entity.get_eye_y(), target_pos.z) - source;
    let direction = delta.normalize();
    let steps = delta.length().floor() as i32 + 7;
    for step in 1..steps {
        let step = f64::from(step);
        world.spawn_particle(
            source + direction.multiply(step, step, step),
            Vector3::new(0.0, 0.0, 0.0),
            0.0,
            1,
            Particle::SonicBoom,
        );
    }
    warden.play_entity_sound(Sound::EntityWardenSonicBoom, 3.0, 1.0);
    if target.damage_with_context(
        target,
        SONIC_BOOM_DAMAGE,
        DamageType::SONIC_BOOM,
        None,
        Some(warden),
        Some(warden),
    ) {
        let resistance = 1.0
            - target.get_living_entity().map_or(0.0, |living| {
                living.get_attribute_value(&Attributes::KNOCKBACK_RESISTANCE)
            });
        let horizontal = SONIC_BOOM_KNOCKBACK_HORIZONTAL * resistance;
        let vertical = SONIC_BOOM_KNOCKBACK_VERTICAL * resistance;
        target_entity.velocity.store(
            target_entity.velocity.load()
                + Vector3::new(
                    direction.x * horizontal,
                    direction.y * vertical,
                    direction.z * horizontal,
                ),
        );
        if target.get_player().is_some() {
            target_entity.send_velocity();
        }
    }
}

/// Vanilla's `DIG_COOLDOWN_SETTER`.
fn dig_cooldown_setter() -> OneShot {
    OneShot::new(
        "DigCooldownSetter",
        vec![(types::DIG_COOLDOWN.id(), MemoryStatus::Registered)],
        |tick| {
            set_dig_cooldown(tick.state);
            true
        },
    )
}

/// Vanilla `SetEntityLookTarget.create(entity -> isTarget(body, entity), followRange)`.
fn look_at_attack_target() -> OneShot {
    OneShot::new(
        "SetEntityLookTarget",
        vec![
            (types::LOOK_TARGET.id(), MemoryStatus::ValueAbsent),
            (
                types::NEAREST_VISIBLE_LIVING_ENTITIES.id(),
                MemoryStatus::ValuePresent,
            ),
        ],
        |tick| {
            let Some(target) = tick.state.get(types::ATTACK_TARGET).cloned() else {
                return false;
            };
            let range = tick
                .mob
                .get_mob_entity()
                .living_entity
                .get_attribute_value(&Attributes::FOLLOW_RANGE);
            let in_range = target
                .get_entity()
                .pos
                .load()
                .squared_distance_to_vec(&tick.mob.get_entity().pos.load())
                <= range * range;
            let visible = in_range
                && tick
                    .state
                    .get(types::NEAREST_VISIBLE_LIVING_ENTITIES)
                    .is_some_and(|visible| visible.contains(target.as_ref(), &tick.visibility()));
            if visible {
                look_at_entity(tick.state, target);
            }
            visible
        },
    )
}

/// Vanilla `WardenAi.onTargetInvalid`.
fn on_target_invalid(mob: &dyn Mob, target: &Arc<dyn EntityBase>) {
    let Some(warden) = as_warden(mob) else {
        return;
    };
    if !warden.can_target_entity(target.as_ref()) {
        warden.clear_anger(target.as_ref());
    }
    warden.queue_dig_cooldown();
}

/// Vanilla `WardenAi.getActivities`.
pub(super) fn activities(_mob: &dyn Mob) -> Vec<ActivityData> {
    vec![
        ActivityData::with_priority_start(
            Activity::Core,
            0,
            vec![
                Box::new(Timed::new(Swim::new(0.8))),
                Box::new(set_warden_look_target()),
                Box::new(Timed::new(LookAtTargetSink::new(45, 90))),
                Box::new(Timed::new(MoveToTargetSink::default())),
            ],
        ),
        ActivityData::with_gate_memory(
            Activity::Emerge,
            5,
            vec![Box::new(Timed::new(Emerging))],
            types::IS_EMERGING.id(),
        ),
        ActivityData::with_pairs_and_conditions(
            Activity::Dig,
            vec![
                (0, Box::new(force_unmount())),
                (1, Box::new(Timed::new(Digging))),
            ],
            vec![
                (types::ROAR_TARGET.id(), MemoryStatus::ValueAbsent),
                (types::DIG_COOLDOWN.id(), MemoryStatus::ValueAbsent),
            ],
        ),
        ActivityData::with_priority_start(
            Activity::Idle,
            10,
            vec![
                Box::new(set_roar_target()),
                Box::new(try_to_sniff()),
                Box::new(run_one_with_conditions(
                    vec![(types::IS_SNIFFING.id(), MemoryStatus::ValueAbsent)],
                    vec![
                        (Box::new(random_stroll::stroll(SPEED_WHEN_IDLING)), 2),
                        (Box::new(DoNothing::new(30, 60)), 1),
                    ],
                )),
            ],
        ),
        ActivityData::with_gate_memory(
            Activity::Investigate,
            5,
            vec![
                Box::new(set_roar_target()),
                Box::new(go_to_target_location(
                    types::DISTURBANCE_LOCATION,
                    2,
                    SPEED_WHEN_INVESTIGATING,
                )),
            ],
            types::DISTURBANCE_LOCATION.id(),
        ),
        ActivityData::with_gate_memory(
            Activity::Sniff,
            5,
            vec![Box::new(set_roar_target()), Box::new(Timed::new(Sniffing))],
            types::IS_SNIFFING.id(),
        ),
        ActivityData::with_gate_memory(
            Activity::Roar,
            10,
            vec![Box::new(Timed::new(Roar))],
            types::ROAR_TARGET.id(),
        ),
        ActivityData::with_gate_memory(
            Activity::Fight,
            10,
            vec![
                Box::new(dig_cooldown_setter()),
                Box::new(stop_attacking_if_never_tired(
                    |tick, target| {
                        as_warden(tick.mob).is_none_or(|warden| {
                            !warden.anger_level().is_angry() || !warden.can_target_entity(target)
                        })
                    },
                    on_target_invalid,
                )),
                Box::new(look_at_attack_target()),
                Box::new(set_walk_target_from_attack_target(|_| SPEED_WHEN_FIGHTING)),
                Box::new(Timed::new(SonicBoom)),
                Box::new(melee_attack(MELEE_ATTACK_COOLDOWN)),
            ],
            types::ATTACK_TARGET.id(),
        ),
    ]
}

/// Vanilla `WardenAi.updateActivity`.
pub(super) fn update_activity(state: &mut BrainState) {
    state.set_active_activity_to_first_valid(&[
        Activity::Emerge,
        Activity::Dig,
        Activity::Roar,
        Activity::Fight,
        Activity::Investigate,
        Activity::Sniff,
        Activity::Idle,
    ]);
}
