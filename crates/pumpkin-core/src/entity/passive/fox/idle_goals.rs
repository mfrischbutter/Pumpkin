//! Vanilla's goals of a fox with nothing to hunt: shelter, sleep, perching, berries and finds.

use std::sync::Arc;

use pumpkin_data::block_properties::{
    CaveVinesLikeProperties, CaveVinesPlantLikeProperties, NetherWartLikeProperties,
};
use pumpkin_data::data_component_impl::EquipmentSlot;
use pumpkin_data::game_event::GameEvent;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::Sound;
use pumpkin_data::{Block, BlockStateId};
use pumpkin_util::math::position::BlockPos;
use pumpkin_world::world::BlockFlags;
use rand::RngExt;

use super::{FoxEntity, as_fox};
use crate::entity::EntityBase;
use crate::entity::ai::goal::flee_sun::FleeSunGoal;
use crate::entity::ai::goal::move_to_target_pos::{MoveToTargetPos, MoveToTargetPosGoal};
use crate::entity::ai::goal::{Controls, Goal, ParentHandle, to_goal_ticks};
use crate::entity::ai::pathfinder::NavigatorGoal;
use crate::entity::ai::target_predicate::TargetPredicate;
use crate::entity::mob::Mob;
use crate::world::World;
use crate::world::game_event::GameEventContext;

const ALERT_RANGE: f64 = 12.0;
const SHELTER_CHECK_INTERVAL: i32 = 100;
const WAIT_TIME_BEFORE_SLEEP: i32 = 140;
const EAT_BERRIES_SPEED: f64 = 1.2;
const EAT_BERRIES_RANGE: i32 = 12;
const EAT_BERRIES_VERTICAL_RANGE: i32 = 1;
const EAT_BERRIES_WAIT_TICKS: i32 = 40;
const ITEM_SEARCH_RANGE: f64 = 8.0;
const ITEM_WALK_SPEED: f64 = 1.2;

/// Vanilla `FoxBehaviorGoal.hasShelter`.
fn has_shelter(fox: &FoxEntity) -> bool {
    let entity = fox.get_entity();
    let pos = entity.pos.load();
    let top = entity.bounding_box.load().max.y;
    let at = BlockPos::floored(pos.x, top, pos.z);
    let world = entity.world.load();
    !world.can_see_sky(&at) && fox.get_walk_target_value(&at, &**world) >= 0.0
}

/// Vanilla `FoxBehaviorGoal.alertable`: something it would look up for is near.
fn alertable(fox: &FoxEntity) -> bool {
    let entity = fox.get_entity();
    let world = entity.world.load();
    let area = entity
        .bounding_box
        .load()
        .expand(ALERT_RANGE, 6.0, ALERT_RANGE);
    let targeting = TargetPredicate::create_attackable()
        .set_base_max_distance(ALERT_RANGE)
        .ignore_visibility();
    world.get_all_at_box(&area).iter().any(|other| {
        other.get_entity().entity_id != entity.entity_id
            && fox.alertable_target(other.as_ref())
            && targeting.test(&world, Some(fox), other.as_ref())
    })
}

/// Vanilla `Fox.SeekShelterGoal`: out of the sun or a storm, into the shade.
pub struct SeekShelterGoal {
    inner: FleeSunGoal,
    interval: i32,
}

impl SeekShelterGoal {
    #[must_use]
    pub const fn new(speed: f64) -> Self {
        Self {
            inner: FleeSunGoal::new(speed),
            interval: SHELTER_CHECK_INTERVAL,
        }
    }
}

impl Goal for SeekShelterGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        let Some(fox) = as_fox(mob) else {
            return false;
        };
        if fox.is_sleeping() || mob.get_mob_entity().get_target().is_some() {
            return false;
        }
        let entity = fox.get_entity();
        let world = entity.world.load();
        let pos = entity.block_pos.load();
        if world.is_thundering() && world.can_see_sky(&pos) {
            return self.inner.set_wanted_pos(mob);
        }
        if self.interval > 0 {
            self.interval -= 1;
            return false;
        }
        self.interval = SHELTER_CHECK_INTERVAL;
        world.is_bright_outside() && world.can_see_sky(&pos) && self.inner.set_wanted_pos(mob)
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        self.inner.should_continue(mob)
    }

    fn start(&mut self, mob: &dyn Mob) {
        if let Some(fox) = as_fox(mob) {
            fox.clear_states();
        }
        self.inner.start(mob);
    }

    fn controls(&self) -> Controls {
        Controls::MOVE
    }
}

/// Vanilla `Fox.SleepGoal`: by day, in the shade, with nothing near.
pub struct SleepGoal {
    countdown: i32,
}

impl SleepGoal {
    #[must_use]
    pub fn new() -> Self {
        Self {
            countdown: rand::random_range(0..to_goal_ticks(WAIT_TIME_BEFORE_SLEEP)),
        }
    }

    fn can_sleep(&mut self, fox: &FoxEntity) -> bool {
        if self.countdown > 0 {
            self.countdown -= 1;
            return false;
        }
        fox.get_entity().world.load().is_bright_outside()
            && has_shelter(fox)
            && !alertable(fox)
            && !fox.is_in_powder_snow()
    }
}

impl Default for SleepGoal {
    fn default() -> Self {
        Self::new()
    }
}

impl Goal for SleepGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        let Some(fox) = as_fox(mob) else {
            return false;
        };
        // Vanilla `xxa`, `yya` and `zza` are all zero; here the input only fades.
        let input = fox.mob_entity.living_entity.movement_input.load();
        let still = input.x.abs() < 1.0E-3 && input.y.abs() < 1.0E-3 && input.z.abs() < 1.0E-3;
        still && mob.is_navigator_idle() && (self.can_sleep(fox) || fox.is_sleeping())
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        as_fox(mob).is_some_and(|fox| self.can_sleep(fox))
    }

    fn start(&mut self, mob: &dyn Mob) {
        let Some(fox) = as_fox(mob) else {
            return;
        };
        fox.set_sitting(false);
        fox.set_crouching(false);
        fox.set_interested(false);
        fox.mob_entity
            .living_entity
            .jumping
            .store(false, std::sync::atomic::Ordering::Relaxed);
        fox.set_sleeping(true);
        fox.mob_entity.steered().navigator().stop();
    }

    fn stop(&mut self, mob: &dyn Mob) {
        self.countdown = rand::random_range(0..to_goal_ticks(WAIT_TIME_BEFORE_SLEEP));
        if let Some(fox) = as_fox(mob) {
            fox.clear_states();
        }
    }

    fn controls(&self) -> Controls {
        Controls::MOVE | Controls::LOOK | Controls::JUMP
    }
}

/// Vanilla `Fox.PerchAndSearchGoal`: it sits and looks around.
#[derive(Default)]
pub struct PerchAndSearchGoal {
    rel_x: f64,
    rel_z: f64,
    look_time: i32,
    looks_remaining: i32,
}

impl PerchAndSearchGoal {
    fn reset_look(&mut self, mob: &dyn Mob) {
        let angle = std::f64::consts::TAU * mob.get_random().random::<f64>();
        self.rel_x = angle.cos();
        self.rel_z = angle.sin();
        self.look_time = to_goal_ticks(80 + mob.get_random().random_range(0..20));
    }
}

impl Goal for PerchAndSearchGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        let Some(fox) = as_fox(mob) else {
            return false;
        };
        fox.mob_entity
            .living_entity
            .get_last_hurt_by_mob()
            .is_none()
            && mob.get_random().random::<f32>() < 0.02
            && !fox.is_sleeping()
            && mob.get_mob_entity().get_target().is_none()
            && mob.is_navigator_idle()
            && !alertable(fox)
            && !fox.is_pouncing()
            && !fox.is_crouching()
    }

    fn should_continue(&mut self, _mob: &dyn Mob) -> bool {
        self.looks_remaining > 0
    }

    fn start(&mut self, mob: &dyn Mob) {
        self.reset_look(mob);
        self.looks_remaining = 2 + mob.get_random().random_range(0..3);
        if let Some(fox) = as_fox(mob) {
            fox.set_sitting(true);
        }
        mob.get_mob_entity().steered().navigator().stop();
    }

    fn stop(&mut self, mob: &dyn Mob) {
        if let Some(fox) = as_fox(mob) {
            fox.set_sitting(false);
        }
    }

    fn tick(&mut self, mob: &dyn Mob) {
        self.look_time -= 1;
        if self.look_time <= 0 {
            self.looks_remaining -= 1;
            self.reset_look(mob);
        }
        let entity = mob.get_entity();
        let pos = entity.pos.load();
        mob.get_mob_entity()
            .look_control
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .look_at(
                mob,
                pos.x + self.rel_x,
                entity.get_eye_y(),
                pos.z + self.rel_z,
            );
    }

    fn should_run_every_tick(&self) -> bool {
        true
    }

    fn controls(&self) -> Controls {
        Controls::MOVE | Controls::LOOK
    }
}

/// Sweet berries of age 2 and up, or cave vines with glow berries.
fn has_berries(block: &Block, state: BlockStateId) -> bool {
    if block == &Block::SWEET_BERRY_BUSH {
        return NetherWartLikeProperties::from_state_id(state).age >= 2;
    }
    if block == &Block::CAVE_VINES {
        return CaveVinesLikeProperties::from_state_id(state).berries;
    }
    block == &Block::CAVE_VINES_PLANT && CaveVinesPlantLikeProperties::from_state_id(state).berries
}

struct BerryTarget;

impl MoveToTargetPos for BerryTarget {
    fn is_target_pos(&self, world: Arc<World>, pos: BlockPos) -> bool {
        let (block, state) = world.get_block_and_state_id(&pos);
        has_berries(block, state)
    }

    fn get_desired_distance_to_target(&self) -> f64 {
        2.0
    }

    fn path_recalculation_interval(&self) -> i32 {
        100
    }
}

/// Vanilla `Fox.FoxEatBerriesGoal`.
pub struct EatBerriesGoal {
    inner: MoveToTargetPosGoal<BerryTarget>,
    _target: Box<BerryTarget>,
    ticks_waited: i32,
}

impl EatBerriesGoal {
    #[must_use]
    pub fn new() -> Self {
        let target = Box::new(BerryTarget);
        // SAFETY: the target has its own heap allocation owned by this goal, so it stays put
        // and outlives the handle, and it is only ever read through shared references.
        let handle: ParentHandle<BerryTarget> = unsafe { ParentHandle::new(&target) };
        Self {
            inner: MoveToTargetPosGoal::new(
                handle,
                EAT_BERRIES_SPEED,
                EAT_BERRIES_RANGE,
                EAT_BERRIES_VERTICAL_RANGE,
            ),
            _target: target,
            ticks_waited: 0,
        }
    }

    /// Vanilla `onReachedTarget`, with `pickSweetBerries` and `CaveVines.use`.
    fn pick(fox: &FoxEntity, pos: BlockPos) {
        let entity = fox.get_entity();
        let world = entity.world.load_full();
        if !world.level_info.load().game_rules.mob_griefing {
            return;
        }
        let (block, state) = world.get_block_and_state_id(&pos);
        if !has_berries(block, state) {
            return;
        }
        if block == &Block::SWEET_BERRY_BUSH {
            let mut props = NetherWartLikeProperties::from_state_id(state);
            let mut count = 1 + rand::random_range(0..2) + u8::from(props.age == 3);
            if fox.mouth_item().is_empty() {
                fox.mob_entity.set_item_slot(
                    &EquipmentSlot::MAIN_HAND,
                    ItemStack::new(1, &Item::SWEET_BERRIES),
                );
                count -= 1;
            }
            if count > 0 {
                world.drop_stack(&pos, ItemStack::new(count, &Item::SWEET_BERRIES));
            }
            fox.play_entity_sound(Sound::BlockSweetBerryBushPickBerries, 1.0, 1.0);
            props.age = 1;
            world.set_block_state(&pos, props.to_state_id(block), BlockFlags::NOTIFY_LISTENERS);
        } else {
            world.drop_stack(&pos, ItemStack::new(1, &Item::GLOW_BERRIES));
            fox.play_entity_sound(Sound::BlockCaveVinesPickBerries, 1.0, 1.0);
            let picked = if block == &Block::CAVE_VINES {
                let mut props = CaveVinesLikeProperties::from_state_id(state);
                props.berries = false;
                props.to_state_id(block)
            } else {
                let mut props = CaveVinesPlantLikeProperties::from_state_id(state);
                props.berries = false;
                props.to_state_id(block)
            };
            world.set_block_state(&pos, picked, BlockFlags::NOTIFY_LISTENERS);
        }
        let source: &dyn EntityBase = fox;
        world.emit_game_event_with_context(
            GameEvent::BlockChange.name(),
            pos.to_centered_f64(),
            &GameEventContext {
                source_entity: Some(source),
            },
        );
    }
}

impl Default for EatBerriesGoal {
    fn default() -> Self {
        Self::new()
    }
}

impl Goal for EatBerriesGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        as_fox(mob).is_some_and(|fox| !fox.is_sleeping()) && self.inner.can_start(mob)
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        self.inner.should_continue(mob)
    }

    fn start(&mut self, mob: &dyn Mob) {
        self.ticks_waited = 0;
        if let Some(fox) = as_fox(mob) {
            fox.set_sitting(false);
        }
        self.inner.start(mob);
    }

    fn tick(&mut self, mob: &dyn Mob) {
        if self.inner.is_reached_target() {
            if self.ticks_waited >= EAT_BERRIES_WAIT_TICKS {
                if let Some(fox) = as_fox(mob) {
                    Self::pick(fox, self.inner.target_pos);
                }
            } else {
                self.ticks_waited += 1;
            }
        } else if mob.get_random().random::<f32>() < 0.05 {
            mob.play_entity_sound(Sound::EntityFoxSniff, 1.0, 1.0);
        }
        self.inner.tick(mob);
    }

    fn should_run_every_tick(&self) -> bool {
        true
    }

    fn controls(&self) -> Controls {
        Controls::MOVE | Controls::JUMP
    }
}

/// Vanilla `Fox.FoxSearchForItemsGoal`: with an empty mouth, it goes for what lies around.
#[derive(Default)]
pub struct SearchForItemsGoal;

impl SearchForItemsGoal {
    fn nearest_item(fox: &FoxEntity) -> Option<Arc<dyn EntityBase>> {
        let entity = fox.get_entity();
        let world = entity.world.load();
        let area = entity.bounding_box.load().expand(
            ITEM_SEARCH_RANGE,
            ITEM_SEARCH_RANGE,
            ITEM_SEARCH_RANGE,
        );
        world.get_all_at_box(&area).into_iter().find(|other| {
            other
                .get_item_entity()
                .is_some_and(FoxEntity::is_allowed_find)
        })
    }

    fn walk_to_item(mob: &dyn Mob, item: &Arc<dyn EntityBase>) {
        mob.get_mob_entity()
            .steered()
            .navigator()
            .set_progress(NavigatorGoal {
                current_progress: mob.get_entity().pos.load(),
                destination: item.get_entity().pos.load(),
                speed: ITEM_WALK_SPEED,
            });
    }
}

impl Goal for SearchForItemsGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        let Some(fox) = as_fox(mob) else {
            return false;
        };
        fox.mouth_item().is_empty()
            && mob.get_mob_entity().get_target().is_none()
            && fox
                .mob_entity
                .living_entity
                .get_last_hurt_by_mob()
                .is_none()
            && fox.can_move()
            && mob.get_random().random_range(0..to_goal_ticks(10).max(1)) == 0
            && Self::nearest_item(fox).is_some()
    }

    fn start(&mut self, mob: &dyn Mob) {
        if let Some(item) = as_fox(mob).and_then(Self::nearest_item) {
            Self::walk_to_item(mob, &item);
        }
    }

    fn tick(&mut self, mob: &dyn Mob) {
        let Some(fox) = as_fox(mob) else {
            return;
        };
        if fox.mouth_item().is_empty()
            && let Some(item) = Self::nearest_item(fox)
        {
            Self::walk_to_item(mob, &item);
        }
    }

    fn controls(&self) -> Controls {
        Controls::MOVE
    }
}
