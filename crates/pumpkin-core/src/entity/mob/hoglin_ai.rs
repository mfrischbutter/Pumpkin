use std::sync::{Arc, Mutex, PoisonError};

use pumpkin_data::attributes::Attributes;
use pumpkin_data::damage::DamageType;
use pumpkin_data::entity::EntityStatus;
use pumpkin_data::environment_attribute::Activity;
use pumpkin_data::sound::Sound;
use pumpkin_protocol::bedrock::server::actor_event::ActorEventID;
use pumpkin_util::math::vector3::Vector3;
use rand::RngExt;

use super::Mob;
use crate::entity::ai::brain::activity::ActivityData;
use crate::entity::ai::brain::behavior::animal::{AnimalMakeLove, baby_follow_adult};
use crate::entity::ai::brain::behavior::fight::{
    erase_memory_if, melee_attack, set_walk_target_from_attack_target, start_attacking,
    stop_attacking_if_target_invalid,
};
use crate::entity::ai::brain::behavior::utils::is_breeding;
use crate::entity::ai::brain::behavior::village::{
    set_walk_target_away_from_entity, set_walk_target_away_from_pos,
};
use crate::entity::ai::brain::behavior::{
    BehaviorControl, DoNothing, LookAtTargetSink, MoveToTargetSink, OneShot, Timed, random_stroll,
    run_one, set_entity_look_target_sometimes, set_walk_target_from_look_target,
};
use crate::entity::ai::brain::memory::{MemoryModuleId, MemoryStatus, types};
use crate::entity::ai::brain::{BrainState, BrainTick};
use crate::entity::{Entity, EntityBase};

/// Vanilla `HoglinAi.ATTACK_INTERVAL`.
pub const ATTACK_INTERVAL: i64 = 40;
/// Vanilla `HoglinAi.BABY_ATTACK_INTERVAL`.
pub const BABY_ATTACK_INTERVAL: i64 = 15;
/// Vanilla `HoglinAi.ATTACK_DURATION`.
pub const ATTACK_DURATION: i64 = 200;
/// Vanilla `HoglinAi.REPELLENT_PACIFY_TIME`.
const REPELLENT_PACIFY_TIME: i64 = 200;

/// Vanilla `HoglinAi.setAttackTarget` and `Zoglin.setAttackTarget`.
pub fn set_attack_target(brain: &mut BrainState, target: Arc<dyn EntityBase>) {
    brain.erase(types::CANT_REACH_WALK_TARGET_SINCE.id());
    brain.erase(types::BREED_TARGET.id());
    brain.set_with_expiry(types::ATTACK_TARGET, target, ATTACK_DURATION);
}

/// Vanilla `HoglinAi.piglinsOutnumberHoglins`, for an adult.
#[must_use]
pub fn piglins_outnumber_hoglins(brain: &BrainState) -> bool {
    let piglins = brain
        .get(types::VISIBLE_ADULT_PIGLIN_COUNT)
        .copied()
        .unwrap_or(0);
    let hoglins = brain
        .get(types::VISIBLE_ADULT_HOGLIN_COUNT)
        .copied()
        .unwrap_or(0)
        + 1;
    piglins > hoglins
}

pub fn is_baby_mob(mob: &dyn Mob) -> bool {
    mob.get_mob_entity().living_entity.is_baby()
}

pub fn is_adult_mob(mob: &dyn Mob) -> bool {
    !is_baby_mob(mob)
}

/// Vanilla `HoglinAi.findNearestValidAttackTarget`.
fn find_attack_target(tick: &BrainTick<'_>) -> Option<Arc<dyn EntityBase>> {
    if tick.state.has_memory_value(types::PACIFIED.id()) || is_breeding(tick.state) {
        return None;
    }
    tick.state
        .get(types::NEAREST_VISIBLE_ATTACKABLE_PLAYER)
        .map(|player| player.clone() as Arc<dyn EntityBase>)
}

/// Vanilla `BecomePassiveIfMemoryPresent` for the repellent.
fn become_passive_near_repellent() -> OneShot {
    OneShot::new(
        "BecomePassiveIfMemoryPresent",
        vec![
            (types::ATTACK_TARGET.id(), MemoryStatus::Registered),
            (types::PACIFIED.id(), MemoryStatus::ValueAbsent),
            (types::NEAREST_REPELLENT.id(), MemoryStatus::ValuePresent),
        ],
        |tick| {
            tick.state
                .set_with_expiry(types::PACIFIED, true, REPELLENT_PACIFY_TIME);
            tick.state.erase(types::ATTACK_TARGET.id());
            true
        },
    )
}

/// `EraseMemoryIf(HoglinAi::wantsToStopFleeing, AVOID_TARGET)`.
fn stop_fleeing_when_safe() -> OneShot {
    OneShot::new(
        "EraseMemoryIf",
        vec![(types::AVOID_TARGET.id(), MemoryStatus::ValuePresent)],
        |tick| {
            if is_adult_mob(tick.mob) && !piglins_outnumber_hoglins(tick.state) {
                tick.state.erase(types::AVOID_TARGET.id());
                return true;
            }
            false
        },
    )
}

/// Vanilla `HoglinAi.createIdleMovementBehaviors`, shared with the zoglin.
#[must_use]
pub fn idle_movement() -> crate::entity::ai::brain::behavior::GateBehavior {
    run_one(vec![
        (Box::new(random_stroll::stroll(0.4)), 2),
        (
            Box::new(set_walk_target_from_look_target::create(0.4, 3)),
            2,
        ),
        (Box::new(DoNothing::new(30, 60)), 1),
    ])
}

pub const MEMORIES: [MemoryModuleId; 3] = [
    types::PACIFIED.id(),
    types::ATTACK_COOLING_DOWN.id(),
    types::BREED_TARGET.id(),
];

pub fn activities(_mob: &dyn Mob) -> Vec<ActivityData> {
    let idle: Vec<Box<dyn BehaviorControl>> = vec![
        Box::new(become_passive_near_repellent()),
        Box::new(Timed::new(AnimalMakeLove::new().with_speed(0.6))),
        Box::new(set_walk_target_away_from_pos(
            types::NEAREST_REPELLENT,
            1.0,
            8,
        )),
        Box::new(start_attacking(find_attack_target)),
        Box::new(
            set_walk_target_away_from_entity(types::NEAREST_VISIBLE_ADULT_PIGLIN, 0.4, 8)
                .only_if(is_adult_mob),
        ),
        Box::new(set_entity_look_target_sometimes::create(8.0, 30, 60)),
        Box::new(baby_follow_adult(5, 16, |_| 0.6)),
        Box::new(idle_movement()),
    ];
    let fight: Vec<Box<dyn BehaviorControl>> = vec![
        Box::new(become_passive_near_repellent()),
        Box::new(Timed::new(AnimalMakeLove::new().with_speed(0.6))),
        Box::new(set_walk_target_from_attack_target(|_| 1.0)),
        Box::new(melee_attack(ATTACK_INTERVAL).only_if(is_adult_mob)),
        Box::new(melee_attack(BABY_ATTACK_INTERVAL).only_if(is_baby_mob)),
        Box::new(stop_attacking_if_target_invalid(|_, _| {})),
        Box::new(erase_memory_if(types::ATTACK_TARGET.id(), is_breeding)),
    ];
    let retreat: Vec<Box<dyn BehaviorControl>> = vec![
        Box::new(set_walk_target_away_from_entity(
            types::AVOID_TARGET,
            1.3,
            15,
        )),
        Box::new(idle_movement()),
        Box::new(set_entity_look_target_sometimes::create(8.0, 30, 60)),
        Box::new(stop_fleeing_when_safe()),
    ];
    vec![
        ActivityData::with_priority_start(
            Activity::Core,
            0,
            vec![
                Box::new(Timed::new(LookAtTargetSink::new(45, 90))),
                Box::new(Timed::new(MoveToTargetSink::default())),
            ],
        ),
        ActivityData::with_priority_start(Activity::Idle, 10, idle),
        ActivityData::with_gate_memory(Activity::Fight, 10, fight, types::ATTACK_TARGET.id()),
        ActivityData::with_gate_memory(Activity::Avoid, 10, retreat, types::AVOID_TARGET.id()),
    ]
}

/// Keeps `source` for the next brain step: hits and damage can happen inside the mob's own
/// brain tick, where the brain cannot be locked again.
pub fn remember_entity(
    slot: &Mutex<Option<Arc<dyn EntityBase>>>,
    mob: &Entity,
    source: &dyn EntityBase,
) {
    *slot.lock().unwrap_or_else(PoisonError::into_inner) = mob
        .world
        .load()
        .get_entity_by_id(source.get_entity().entity_id);
}

/// Takes what `remember_entity` kept.
pub fn take_remembered(slot: &Mutex<Option<Arc<dyn EntityBase>>>) -> Option<Arc<dyn EntityBase>> {
    slot.lock().unwrap_or_else(PoisonError::into_inner).take()
}

/// The shared start of vanilla `Hoglin.doHurtTarget` and `Zoglin.doHurtTarget`.
pub fn attack_and_throw(body: &dyn Mob, target: &dyn EntityBase, sound: Sound) -> bool {
    if target.get_living_entity().is_none() {
        return false;
    }
    let entity = body.get_entity();
    entity.world.load().send_entity_status(
        entity,
        EntityStatus::StartAttacking,
        ActorEventID::StartAttacking,
    );
    body.play_entity_sound(sound, 1.0, 1.0);
    hurt_and_throw_target(body, target)
}

/// Vanilla `HoglinBase.hurtAndThrowTarget`, shared with zoglins.
pub fn hurt_and_throw_target(body: &dyn Mob, target: &dyn EntityBase) -> bool {
    let mob = body.get_mob_entity();
    let attack_damage = mob
        .living_entity
        .get_attribute_value(&Attributes::ATTACK_DAMAGE) as f32;
    let baby = mob.living_entity.is_baby();
    let damage = if !baby && attack_damage as i32 > 0 {
        attack_damage / 2.0 + body.get_random().random_range(0..attack_damage as i32) as f32
    } else {
        attack_damage
    };
    let hurt =
        mob.try_attack_with_damage(body.get_entity(), target, damage, DamageType::MOB_ATTACK);
    if hurt && !baby {
        throw_target(body, target);
    }
    hurt
}

/// Vanilla `HoglinBase.throwTarget`, shared with zoglins.
pub fn throw_target(body: &dyn Mob, target: &dyn EntityBase) {
    let knockback = body
        .get_mob_entity()
        .living_entity
        .get_attribute_value(&Attributes::ATTACK_KNOCKBACK);
    let resistance = target.get_living_entity().map_or(0.0, |living| {
        living.get_attribute_value(&Attributes::KNOCKBACK_RESISTANCE)
    });
    let power = knockback - resistance;
    if power <= 0.0 {
        return;
    }
    let attacker = body.get_entity().pos.load();
    let entity = target.get_entity();
    let position = entity.pos.load();
    let mut rng = body.get_random();
    let angle = f64::from(rng.random_range(-10..=10)).to_radians();
    let scale = power * f64::from(rng.random::<f32>() * 0.5 + 0.2);
    let direction = Vector3::new(position.x - attacker.x, 0.0, position.z - attacker.z).normalize();
    let (sin, cos) = angle.sin_cos();
    let vertical = power * f64::from(rng.random::<f32>()) * 0.5;
    entity.add_velocity(Vector3::new(
        (direction.x * cos + direction.z * sin) * scale,
        vertical,
        (direction.z * cos - direction.x * sin) * scale,
    ));
}
