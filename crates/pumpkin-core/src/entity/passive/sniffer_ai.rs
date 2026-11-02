use std::sync::Arc;

use pumpkin_data::entity::EntityType;
use pumpkin_data::environment_attribute::Activity;
use rand::RngExt;

use super::sniffer::{SnifferEntity, SnifferState};
use crate::entity::ai::brain::activity::ActivityData;
use crate::entity::ai::brain::behavior::timed::Behavior;
use crate::entity::ai::brain::behavior::village::set_entity_look_target;
use crate::entity::ai::brain::behavior::{
    AnimalMakeLove, AnimalPanic, CountDownCooldownTicks, DoNothing, FollowTemptation,
    LookAtTargetSink, MoveToTargetSink, Swim, Timed, random_stroll, run_one,
    set_walk_target_from_look_target,
};
use crate::entity::ai::brain::memory::position_tracker::BlockPosTracker;
use crate::entity::ai::brain::memory::walk_target::WalkTarget;
use crate::entity::ai::brain::memory::{MemoryModuleId, MemoryStatus, types};
use crate::entity::ai::brain::{BrainState, BrainTick};
use crate::entity::mob::Mob;
use crate::entity::{EntityBase, ageable::AgeableMob};

/// Vanilla `SnifferAi.SNIFFING_COOLDOWN_TICKS`.
const SNIFFING_COOLDOWN_TICKS: i64 = 9600;
/// Longer than any wait here: vanilla's `stop` reads `timedOut`, which `Timed` keeps to itself,
/// so these behaviours keep their own deadline and let `can_still_use` end them.
const OWN_DEADLINE: i32 = 1_000_000;

fn as_sniffer(mob: &dyn Mob) -> Option<&SnifferEntity> {
    mob.cast_any().downcast_ref::<SnifferEntity>()
}

/// Vanilla `Sniffer.isTempted`.
pub(super) fn is_tempted(state: &BrainState) -> bool {
    state.has_memory_value(types::TEMPTING_PLAYER.id())
}

/// Vanilla `Sniffer.canSniff`, with the brain at hand.
fn can_sniff(tick: &BrainTick<'_>) -> bool {
    let mob_entity = tick.mob.get_mob_entity();
    let entity = tick.mob.get_entity();
    !is_tempted(tick.state)
        && !tick.state.has_memory_value(types::IS_PANICKING.id())
        && !entity.is_in_water()
        && !mob_entity.is_in_love()
        && entity.on_ground.load(std::sync::atomic::Ordering::Relaxed)
        && !entity.has_vehicle()
        && !entity.is_leashed()
}

/// Vanilla `SnifferAi.resetSniffing`.
fn reset_sniffing(tick: &mut BrainTick<'_>) {
    tick.state.erase(types::SNIFFER_DIGGING.id());
    tick.state.erase(types::SNIFFER_SNIFFING_TARGET.id());
    transition(tick, SnifferState::Idling);
}

fn transition(tick: &BrainTick<'_>, state: SnifferState) {
    if let Some(sniffer) = as_sniffer(tick.mob) {
        sniffer.transition_to(state);
    }
}

/// The anonymous subclasses in `SnifferAi`: this behaviour, but `start` resets the sniffing.
struct ResetsSniffing<B: Behavior>(B);

impl<B: Behavior> Behavior for ResetsSniffing<B> {
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

    fn start(&mut self, tick: &mut BrainTick<'_>) {
        reset_sniffing(tick);
        self.0.start(tick);
    }

    fn tick(&mut self, tick: &mut BrainTick<'_>) {
        self.0.tick(tick);
    }

    fn stop(&mut self, tick: &mut BrainTick<'_>) {
        self.0.stop(tick);
    }

    fn can_still_use(&mut self, tick: &mut BrainTick<'_>) -> bool {
        self.0.can_still_use(tick)
    }

    fn debug_name(&self) -> &'static str {
        self.0.debug_name()
    }
}

const DIGGING_CONDITIONS: [(MemoryModuleId, MemoryStatus); 4] = [
    (types::IS_PANICKING.id(), MemoryStatus::ValueAbsent),
    (types::WALK_TARGET.id(), MemoryStatus::ValueAbsent),
    (types::SNIFFER_DIGGING.id(), MemoryStatus::ValuePresent),
    (types::SNIFF_COOLDOWN.id(), MemoryStatus::ValueAbsent),
];

/// Vanilla `SnifferAi.Digging`.
#[derive(Default)]
struct Digging {
    ends_at: i64,
}

impl Behavior for Digging {
    fn entry_conditions(&self) -> &[(MemoryModuleId, MemoryStatus)] {
        &DIGGING_CONDITIONS
    }

    fn min_duration(&self) -> i32 {
        OWN_DEADLINE
    }

    fn max_duration(&self) -> i32 {
        OWN_DEADLINE
    }

    fn check_extra_start_conditions(&mut self, tick: &mut BrainTick<'_>) -> bool {
        can_sniff(tick)
    }

    fn can_still_use(&mut self, tick: &mut BrainTick<'_>) -> bool {
        tick.time <= self.ends_at
            && tick.state.has_memory_value(types::SNIFFER_DIGGING.id())
            && as_sniffer(tick.mob).is_some_and(|sniffer| sniffer.can_dig(tick))
            && !tick.mob.get_mob_entity().is_in_love()
    }

    fn start(&mut self, tick: &mut BrainTick<'_>) {
        self.ends_at = tick.time + tick.mob.get_random().random_range(160..=180);
        transition(tick, SnifferState::Digging);
    }

    fn stop(&mut self, tick: &mut BrainTick<'_>) {
        if tick.time > self.ends_at {
            tick.state
                .set_with_expiry(types::SNIFF_COOLDOWN, (), SNIFFING_COOLDOWN_TICKS);
        } else {
            reset_sniffing(tick);
        }
    }

    fn debug_name(&self) -> &'static str {
        "SnifferDigging"
    }
}

const HAPPY_CONDITIONS: [(MemoryModuleId, MemoryStatus); 1] =
    [(types::SNIFFER_HAPPY.id(), MemoryStatus::ValuePresent)];

/// Vanilla `SnifferAi.FeelingHappy`.
struct FeelingHappy;

impl Behavior for FeelingHappy {
    fn entry_conditions(&self) -> &[(MemoryModuleId, MemoryStatus)] {
        &HAPPY_CONDITIONS
    }

    fn min_duration(&self) -> i32 {
        40
    }

    fn max_duration(&self) -> i32 {
        100
    }

    fn can_still_use(&mut self, _tick: &mut BrainTick<'_>) -> bool {
        true
    }

    fn start(&mut self, tick: &mut BrainTick<'_>) {
        transition(tick, SnifferState::FeelingHappy);
    }

    fn stop(&mut self, tick: &mut BrainTick<'_>) {
        transition(tick, SnifferState::Idling);
        tick.state.erase(types::SNIFFER_HAPPY.id());
    }

    fn debug_name(&self) -> &'static str {
        "SnifferFeelingHappy"
    }
}

const FINISHED_DIGGING_CONDITIONS: [(MemoryModuleId, MemoryStatus); 4] = [
    (types::IS_PANICKING.id(), MemoryStatus::ValueAbsent),
    (types::WALK_TARGET.id(), MemoryStatus::ValueAbsent),
    (types::SNIFFER_DIGGING.id(), MemoryStatus::ValuePresent),
    (types::SNIFF_COOLDOWN.id(), MemoryStatus::ValuePresent),
];

/// Vanilla `SnifferAi.FinishedDigging`, 40 ticks of getting up.
#[derive(Default)]
struct FinishedDigging {
    ends_at: i64,
}

impl Behavior for FinishedDigging {
    fn entry_conditions(&self) -> &[(MemoryModuleId, MemoryStatus)] {
        &FINISHED_DIGGING_CONDITIONS
    }

    fn min_duration(&self) -> i32 {
        OWN_DEADLINE
    }

    fn max_duration(&self) -> i32 {
        OWN_DEADLINE
    }

    fn can_still_use(&mut self, tick: &mut BrainTick<'_>) -> bool {
        tick.time <= self.ends_at && tick.state.has_memory_value(types::SNIFFER_DIGGING.id())
    }

    fn start(&mut self, tick: &mut BrainTick<'_>) {
        self.ends_at = tick.time + 40;
        transition(tick, SnifferState::Rising);
    }

    fn stop(&mut self, tick: &mut BrainTick<'_>) {
        transition(tick, SnifferState::Idling);
        if tick.time > self.ends_at
            && let Some(sniffer) = as_sniffer(tick.mob)
        {
            sniffer.store_explored_position(tick);
        }
        tick.state.erase(types::SNIFFER_DIGGING.id());
        tick.state.set(types::SNIFFER_HAPPY, true);
    }

    fn debug_name(&self) -> &'static str {
        "SnifferFinishedDigging"
    }
}

const SCENTING_CONDITIONS: [(MemoryModuleId, MemoryStatus); 5] = [
    (types::IS_PANICKING.id(), MemoryStatus::ValueAbsent),
    (types::SNIFFER_DIGGING.id(), MemoryStatus::ValueAbsent),
    (
        types::SNIFFER_SNIFFING_TARGET.id(),
        MemoryStatus::ValueAbsent,
    ),
    (types::SNIFFER_HAPPY.id(), MemoryStatus::ValueAbsent),
    (types::BREED_TARGET.id(), MemoryStatus::ValueAbsent),
];

/// Vanilla `SnifferAi.Scenting`.
struct Scenting;

impl Behavior for Scenting {
    fn entry_conditions(&self) -> &[(MemoryModuleId, MemoryStatus)] {
        &SCENTING_CONDITIONS
    }

    fn min_duration(&self) -> i32 {
        40
    }

    fn max_duration(&self) -> i32 {
        80
    }

    fn check_extra_start_conditions(&mut self, tick: &mut BrainTick<'_>) -> bool {
        !is_tempted(tick.state)
    }

    fn can_still_use(&mut self, _tick: &mut BrainTick<'_>) -> bool {
        true
    }

    fn start(&mut self, tick: &mut BrainTick<'_>) {
        transition(tick, SnifferState::Scenting);
    }

    fn stop(&mut self, tick: &mut BrainTick<'_>) {
        transition(tick, SnifferState::Idling);
    }

    fn debug_name(&self) -> &'static str {
        "SnifferScenting"
    }
}

const SEARCHING_CONDITIONS: [(MemoryModuleId, MemoryStatus); 3] = [
    (types::WALK_TARGET.id(), MemoryStatus::ValuePresent),
    (types::IS_PANICKING.id(), MemoryStatus::ValueAbsent),
    (
        types::SNIFFER_SNIFFING_TARGET.id(),
        MemoryStatus::ValuePresent,
    ),
];

/// Vanilla `SnifferAi.Searching`: walks to the spot it smelled, for 30 seconds at most.
struct Searching;

impl Behavior for Searching {
    fn entry_conditions(&self) -> &[(MemoryModuleId, MemoryStatus)] {
        &SEARCHING_CONDITIONS
    }

    fn min_duration(&self) -> i32 {
        600
    }

    fn max_duration(&self) -> i32 {
        600
    }

    fn check_extra_start_conditions(&mut self, tick: &mut BrainTick<'_>) -> bool {
        can_sniff(tick)
    }

    fn can_still_use(&mut self, tick: &mut BrainTick<'_>) -> bool {
        if !can_sniff(tick) {
            transition(tick, SnifferState::Idling);
            return false;
        }
        let walk_target = tick
            .state
            .get(types::WALK_TARGET)
            .map(|walk| walk.target().current_block_position());
        let sniffing_target = tick.state.get(types::SNIFFER_SNIFFING_TARGET).copied();
        walk_target.is_some() && walk_target == sniffing_target
    }

    fn start(&mut self, tick: &mut BrainTick<'_>) {
        transition(tick, SnifferState::Searching);
    }

    fn stop(&mut self, tick: &mut BrainTick<'_>) {
        if can_sniff(tick) && as_sniffer(tick.mob).is_some_and(|sniffer| sniffer.can_dig(tick)) {
            tick.state.set(types::SNIFFER_DIGGING, true);
        }
        tick.state.erase(types::WALK_TARGET.id());
        tick.state.erase(types::SNIFFER_SNIFFING_TARGET.id());
    }

    fn debug_name(&self) -> &'static str {
        "SnifferSearching"
    }
}

const SNIFFING_CONDITIONS: [(MemoryModuleId, MemoryStatus); 3] = [
    (types::WALK_TARGET.id(), MemoryStatus::ValueAbsent),
    (
        types::SNIFFER_SNIFFING_TARGET.id(),
        MemoryStatus::ValueAbsent,
    ),
    (types::SNIFF_COOLDOWN.id(), MemoryStatus::ValueAbsent),
];

/// Vanilla `SnifferAi.Sniffing`: a sniff that runs its full time picks a spot to dig at.
#[derive(Default)]
struct Sniffing {
    ends_at: i64,
}

impl Behavior for Sniffing {
    fn entry_conditions(&self) -> &[(MemoryModuleId, MemoryStatus)] {
        &SNIFFING_CONDITIONS
    }

    fn min_duration(&self) -> i32 {
        OWN_DEADLINE
    }

    fn max_duration(&self) -> i32 {
        OWN_DEADLINE
    }

    fn check_extra_start_conditions(&mut self, tick: &mut BrainTick<'_>) -> bool {
        !tick.mob.get_mob_entity().living_entity.is_baby() && can_sniff(tick)
    }

    fn can_still_use(&mut self, tick: &mut BrainTick<'_>) -> bool {
        tick.time <= self.ends_at && can_sniff(tick)
    }

    fn start(&mut self, tick: &mut BrainTick<'_>) {
        self.ends_at = tick.time + tick.mob.get_random().random_range(40..=80);
        transition(tick, SnifferState::Sniffing);
    }

    fn stop(&mut self, tick: &mut BrainTick<'_>) {
        transition(tick, SnifferState::Idling);
        if tick.time <= self.ends_at {
            return;
        }
        let spot = as_sniffer(tick.mob).and_then(|sniffer| sniffer.calculate_dig_position(tick));
        if let Some(pos) = spot {
            tick.state.set(types::SNIFFER_SNIFFING_TARGET, pos);
            tick.state.set(
                types::WALK_TARGET,
                WalkTarget::new(Arc::new(BlockPosTracker::new(pos)), 1.25, 0),
            );
        }
    }

    fn debug_name(&self) -> &'static str {
        "SnifferSniffing"
    }
}

fn is_player(entity: &Arc<dyn EntityBase>) -> bool {
    entity.get_entity().entity_type == &EntityType::PLAYER
}

fn temptation_distance(mob: &dyn Mob) -> f64 {
    if mob.as_ageable().is_some_and(AgeableMob::is_baby) {
        2.5
    } else {
        3.5
    }
}

fn core_activity() -> ActivityData {
    ActivityData::with_priority_start(
        Activity::Core,
        0,
        vec![
            Box::new(Timed::new(Swim::new(0.8))),
            Box::new(Timed::new(ResetsSniffing(AnimalPanic::new(2.0)))),
            Box::new(Timed::new(MoveToTargetSink::new(500, 700))),
            Box::new(CountDownCooldownTicks::new(
                types::TEMPTATION_COOLDOWN_TICKS,
            )),
        ],
    )
}

fn idle_activity() -> ActivityData {
    ActivityData::with_pairs_and_conditions(
        Activity::Idle,
        vec![
            (
                0,
                Box::new(Timed::new(ResetsSniffing(AnimalMakeLove::new()))),
            ),
            (
                1,
                Box::new(Timed::new(ResetsSniffing(FollowTemptation::new(
                    |_| 1.25,
                    temptation_distance,
                )))),
            ),
            (2, Box::new(Timed::new(LookAtTargetSink::new(45, 90)))),
            (3, Box::new(Timed::new(FeelingHappy))),
            (
                4,
                Box::new(run_one(vec![
                    (
                        Box::new(set_walk_target_from_look_target::create(1.0, 3)),
                        2,
                    ),
                    (Box::new(Timed::new(Scenting)), 1),
                    (Box::new(Timed::new(Sniffing::default())), 1),
                    (Box::new(set_entity_look_target(is_player, 6.0)), 1),
                    (Box::new(random_stroll::stroll(1.0)), 1),
                    (Box::new(DoNothing::new(5, 20)), 2),
                ])),
            ),
        ],
        vec![(types::SNIFFER_DIGGING.id(), MemoryStatus::ValueAbsent)],
    )
}

/// Vanilla `SnifferAi.getActivities`.
pub(super) fn activities(_mob: &dyn Mob) -> Vec<ActivityData> {
    vec![
        core_activity(),
        idle_activity(),
        ActivityData::with_pairs_and_conditions(
            Activity::Sniff,
            vec![(0, Box::new(Timed::new(Searching)))],
            vec![
                (types::IS_PANICKING.id(), MemoryStatus::ValueAbsent),
                (
                    types::SNIFFER_SNIFFING_TARGET.id(),
                    MemoryStatus::ValuePresent,
                ),
                (types::WALK_TARGET.id(), MemoryStatus::ValuePresent),
            ],
        ),
        ActivityData::with_pairs_and_conditions(
            Activity::Dig,
            vec![
                (0, Box::new(Timed::new(Digging::default()))),
                (0, Box::new(Timed::new(FinishedDigging::default()))),
            ],
            vec![
                (types::IS_PANICKING.id(), MemoryStatus::ValueAbsent),
                (types::WALK_TARGET.id(), MemoryStatus::ValueAbsent),
                (types::SNIFFER_DIGGING.id(), MemoryStatus::ValuePresent),
            ],
        ),
    ]
}

/// Vanilla `SnifferAi.updateActivity`.
pub(super) fn update_activity(state: &mut BrainState) {
    state.set_active_activity_to_first_valid(&[Activity::Dig, Activity::Sniff, Activity::Idle]);
}
