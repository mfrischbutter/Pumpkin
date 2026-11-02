use std::sync::Arc;

use pumpkin_data::environment_attribute::Activity;
use pumpkin_data::sound::Sound;
use pumpkin_data::tag::{self, Taggable};
use rand::RngExt;

use crate::entity::EntityBase;
use crate::entity::ai::brain::activity::ActivityData;
use crate::entity::ai::brain::behavior::charge_attack::ChargeAttack;
use crate::entity::ai::brain::behavior::utils::is_breeding;
use crate::entity::ai::brain::behavior::{
    AnimalMakeLove, AnimalPanic, BehaviorControl, CountDownCooldownTicks, FollowTemptation,
    GateBehavior, LookAtTargetSink, MoveToTargetSink, OrderPolicy, RunningPolicy, Timed,
    random_stroll, set_walk_target_from_look_target, start_attacking_with,
};
use crate::entity::ai::brain::memory::{MemoryModuleId, MemoryStatus, types};
use crate::entity::ai::brain::sensing::is_entity_attackable_ignoring_line_of_sight;
use crate::entity::ai::brain::{BrainState, BrainTick};
use crate::entity::mob::Mob;

/// Vanilla `NautilusAi.TIME_BETWEEN_NON_PLAYER_ATTACKS`.
pub const TIME_BETWEEN_NON_PLAYER_ATTACKS: std::ops::RangeInclusive<i32> = 2400..=3600;
/// Vanilla `NautilusAi.TIME_BETWEEN_ATTACKS`.
const TIME_BETWEEN_ATTACKS: i32 = 80;
/// Vanilla `NautilusAi.ATTACK_KNOCKBACK_FORCE`.
const KNOCKBACK_FORCE: f32 = 2.0;
/// Vanilla `NautilusAi.MAX_CHARGE_DISTANCE`.
const MAX_CHARGE_DISTANCE: f64 = 12.0;
/// Vanilla `NautilusAi.MAX_TARGET_DETECTION_DISTANCE`.
const MAX_DETECTION_DISTANCE: f64 = 11.0;
/// Vanilla `ANGER_DURATION`.
pub const ANGER_DURATION: i64 = 400;

pub const MEMORIES: [MemoryModuleId; 2] =
    [types::ANGRY_AT.id(), types::ATTACK_TARGET_COOLDOWN.id()];

fn close_enough(mob: &dyn Mob) -> f64 {
    if mob.get_mob_entity().living_entity.is_baby() {
        2.5
    } else {
        3.5
    }
}

fn is_tame(mob: &dyn Mob) -> bool {
    mob.as_tamable()
        .is_some_and(crate::entity::passive::tamable::TamableAnimal::is_tame)
}

/// Vanilla `NautilusAi.isHostileTarget`.
fn is_hostile_target(entity: &dyn EntityBase) -> bool {
    let entity = entity.get_entity();
    entity.is_in_water()
        && entity
            .entity_type
            .has_tag(&tag::EntityType::MINECRAFT_NAUTILUS_HOSTILES)
}

/// The grudge of vanilla `findNearestValidAttackTarget`.
fn angry_at_target(tick: &BrainTick<'_>) -> Option<Arc<dyn EntityBase>> {
    let id = *tick.state.get(types::ANGRY_AT)?;
    let target = tick.world.get_entity_by_uuid(id)?;
    (target.get_entity().is_in_water()
        && is_entity_attackable_ignoring_line_of_sight(&tick.visibility(), target.as_ref()))
    .then_some(target)
}

/// Vanilla `NautilusAi.findNearestValidAttackTarget`: whoever hurt it, or at long intervals
/// perhaps a hostile mob in the water.
fn find_attack_target(tick: &mut BrainTick<'_>) -> Option<Arc<dyn EntityBase>> {
    let entity = tick.mob.get_entity();
    if is_breeding(tick.state)
        || !entity.is_in_water()
        || tick.mob.get_mob_entity().living_entity.is_baby()
        || is_tame(tick.mob)
    {
        return None;
    }
    if let Some(target) = angry_at_target(tick) {
        return Some(target);
    }
    if tick
        .state
        .has_memory_value(types::ATTACK_TARGET_COOLDOWN.id())
    {
        return None;
    }
    let (cooldown, skip) = {
        let mut random = tick.mob.get_random();
        (
            random.random_range(TIME_BETWEEN_NON_PLAYER_ATTACKS),
            random.random::<f32>() < 0.5,
        )
    };
    tick.state.set(types::ATTACK_TARGET_COOLDOWN, cooldown);
    if skip {
        return None;
    }
    let visible = tick.state.get(types::NEAREST_VISIBLE_LIVING_ENTITIES)?;
    visible.find_closest(&tick.visibility(), |candidate| {
        is_hostile_target(candidate.as_ref())
    })
}

fn wander() -> Box<dyn BehaviorControl> {
    Box::new(GateBehavior::new(
        "NautilusWander",
        vec![(types::WALK_TARGET.id(), MemoryStatus::ValueAbsent)],
        Vec::new(),
        OrderPolicy::Ordered,
        RunningPolicy::TryAll,
        vec![
            (Box::new(random_stroll::swim(1.0)), 2),
            (
                Box::new(set_walk_target_from_look_target::create(1.0, 3)),
                3,
            ),
        ],
    ))
}

fn core_behaviors() -> Vec<Box<dyn BehaviorControl>> {
    vec![
        Box::new(Timed::new(LookAtTargetSink::new(45, 90))),
        Box::new(Timed::new(MoveToTargetSink::default())),
        Box::new(CountDownCooldownTicks::new(
            types::TEMPTATION_COOLDOWN_TICKS,
        )),
        Box::new(CountDownCooldownTicks::new(types::CHARGE_COOLDOWN_TICKS)),
        Box::new(CountDownCooldownTicks::new(types::ATTACK_TARGET_COOLDOWN)),
    ]
}

fn fight_activity(speed: f32, sound: Sound) -> ActivityData {
    ActivityData::with_pairs_and_conditions(
        Activity::Fight,
        vec![(
            0,
            Box::new(Timed::new(ChargeAttack::new(
                TIME_BETWEEN_ATTACKS,
                speed,
                KNOCKBACK_FORCE,
                MAX_CHARGE_DISTANCE,
                MAX_DETECTION_DISTANCE,
                sound,
            ))),
        )],
        vec![
            (types::ATTACK_TARGET.id(), MemoryStatus::ValuePresent),
            (types::TEMPTING_PLAYER.id(), MemoryStatus::ValueAbsent),
            (types::BREED_TARGET.id(), MemoryStatus::ValueAbsent),
            (types::CHARGE_COOLDOWN_TICKS.id(), MemoryStatus::ValueAbsent),
        ],
    )
}

/// Vanilla `NautilusAi.getActivities`.
pub fn activities(_mob: &dyn Mob) -> Vec<ActivityData> {
    let mut core = vec![Box::new(Timed::new(AnimalPanic::new(1.6))) as Box<dyn BehaviorControl>];
    core.extend(core_behaviors());
    vec![
        ActivityData::with_priority_start(Activity::Core, 0, core),
        ActivityData::with_pairs(
            Activity::Idle,
            vec![
                (
                    1,
                    Box::new(Timed::new(AnimalMakeLove::new().with_speed(0.4))),
                ),
                (
                    2,
                    Box::new(Timed::new(FollowTemptation::new(|_| 1.3, close_enough))),
                ),
                (3, Box::new(start_attacking_with(find_attack_target))),
                (4, wander()),
            ],
        ),
        fight_activity(0.6, Sound::EntityNautilusDash),
    ]
}

/// Vanilla `ZombieNautilusAi.getActivities`.
pub fn zombie_activities(_mob: &dyn Mob) -> Vec<ActivityData> {
    vec![
        ActivityData::with_priority_start(Activity::Core, 0, core_behaviors()),
        ActivityData::with_pairs(
            Activity::Idle,
            vec![
                (
                    1,
                    Box::new(Timed::new(FollowTemptation::new(|_| 0.9, close_enough))),
                ),
                (2, Box::new(start_attacking_with(find_attack_target))),
                (3, wander()),
            ],
        ),
        fight_activity(0.5, Sound::EntityZombieNautilusDash),
    ]
}

/// Vanilla `NautilusAi.updateActivity`.
pub fn update_activity(state: &mut BrainState) {
    state.set_active_activity_to_first_valid(&[Activity::Fight, Activity::Idle]);
}
