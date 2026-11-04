use std::sync::Arc;

use super::BrainTick;
use super::memory::MemoryModuleId;
use super::memory::position_tracker::PositionTracker;

pub mod animal;
pub mod animal_panic;
pub mod charge_attack;
pub mod count_down_cooldown_ticks;
pub mod do_nothing;
pub mod fight;
pub mod gate;
pub mod go_and_give_items_to_target;
pub mod go_to_wanted_item;
pub mod interact_with_door;
pub mod long_jump;
pub mod look_at_target_sink;
pub mod move_to_target_sink;
pub mod one_shot;
pub mod poi;
pub mod random_stroll;
pub mod set_entity_look_target_sometimes;
pub mod set_walk_target_from_look_target;
pub mod shuffling_list;
pub mod spear;
pub mod stay_close_to_target;
pub mod swim;
pub mod target_location;
pub mod timed;
pub mod transport_item_target;
pub mod transport_items;
pub mod try_find_land;
pub mod try_find_liquid;
pub mod utils;
pub mod village;

pub use animal::{AnimalMakeLove, FollowTemptation, RandomLookAround, baby_follow_adult};
pub use animal_panic::AnimalPanic;
pub use count_down_cooldown_ticks::CountDownCooldownTicks;
pub use do_nothing::DoNothing;
pub use fight::{
    erase_memory_if, melee_attack, set_walk_target_from_attack_target, start_attacking,
    start_attacking_with, stop_attacking_if, stop_attacking_if_never_tired,
    stop_attacking_if_target_invalid,
};
pub use gate::{GateBehavior, OrderPolicy, RunningPolicy, run_one, run_one_with_conditions};
pub use go_and_give_items_to_target::GoAndGiveItemsToTarget;
pub use look_at_target_sink::LookAtTargetSink;
pub use move_to_target_sink::MoveToTargetSink;
pub use one_shot::{OneShot, Trigger};
pub use shuffling_list::ShufflingList;
pub use swim::Swim;
pub use timed::{Behavior, DEFAULT_DURATION, Timed};
pub use try_find_liquid::try_find_liquid;

pub type PositionGetter = fn(&mut BrainTick<'_>) -> Option<Arc<dyn PositionTracker>>;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Status {
    Stopped,
    Running,
}

pub trait BehaviorControl: Send + Sync {
    fn status(&self) -> Status;
    fn required_memories(&self) -> &[MemoryModuleId];
    fn try_start(&mut self, tick: &mut BrainTick<'_>) -> bool;
    fn tick_or_stop(&mut self, tick: &mut BrainTick<'_>);
    fn do_stop(&mut self, tick: &mut BrainTick<'_>);
    fn debug_string(&self) -> String;
}

pub(super) struct ActivityBehaviors {
    pub activity: pumpkin_data::environment_attribute::Activity,
    pub behaviors: Vec<Box<dyn BehaviorControl>>,
}

#[must_use]
pub fn required_memories_of(
    conditions: &[(MemoryModuleId, super::memory::MemoryStatus)],
) -> Box<[MemoryModuleId]> {
    conditions.iter().map(|(id, _)| *id).collect()
}
