//! What villagers do at work: the farmer's fields and composter, showing goods to players and
//! thanking heroes.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use pumpkin_data::block_properties::ComposterLikeProperties;
use pumpkin_data::data_component_impl::EquipmentSlot;
use pumpkin_data::effect::StatusEffect;
use pumpkin_data::entity::EntityType;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::tag;
use pumpkin_data::tag::Taggable;
use pumpkin_data::world::WorldEvent;
use pumpkin_data::{Block, BlockStateId};
use pumpkin_util::math::position::BlockPos;
use pumpkin_world::world::BlockFlags;
use rand::RngExt;
use rand::seq::IndexedRandom;

use super::VillagerEntity;
use super::data::VillagerProfession;
use crate::block::blocks::composter::ComposterBlock;
use crate::block::blocks::plant::crop::crop_age;
use crate::entity::EntityBase;
use crate::entity::ai::brain::BrainTick;
use crate::entity::ai::brain::behavior::poi::closer_to_center_than;
use crate::entity::ai::brain::behavior::{Behavior, utils};
use crate::entity::ai::brain::memory::position_tracker::{BlockPosTracker, EntityTracker};
use crate::entity::ai::brain::memory::walk_target::WalkTarget;
use crate::entity::ai::brain::memory::{MemoryModuleId, MemoryStatus, types};
use crate::entity::mob::Mob;
use crate::entity::player::Player;
use crate::world::World;
use crate::world::loot::{LootContextParameters, generate_loot_from_handle};

fn as_villager(mob: &dyn Mob) -> Option<&VillagerEntity> {
    mob.cast_any().downcast_ref::<VillagerEntity>()
}

fn is_ripe(block: &Block, state: BlockStateId) -> bool {
    crop_age(block, state).is_some_and(|(age, max)| age >= max)
}

fn is_growing(block: &Block, state: BlockStateId) -> bool {
    crop_age(block, state).is_some_and(|(age, max)| age < max)
}

fn walk_and_look_at_block(tick: &mut BrainTick<'_>, pos: BlockPos, close_enough_dist: i32) {
    let tracker = Arc::new(BlockPosTracker::new(pos));
    tick.state.set(types::LOOK_TARGET, tracker.clone());
    tick.state.set(
        types::WALK_TARGET,
        WalkTarget::new(tracker, 0.5, close_enough_dist),
    );
}

fn neighbours(center: BlockPos) -> impl Iterator<Item = BlockPos> {
    BlockPos::iterate(
        BlockPos::new(center.0.x - 1, center.0.y - 1, center.0.z - 1),
        BlockPos::new(center.0.x + 1, center.0.y + 1, center.0.z + 1),
    )
}

/// Vanilla `HarvestFarmland`.
#[derive(Default)]
pub(super) struct HarvestFarmland {
    above_farmland_pos: Option<BlockPos>,
    next_ok_start_time: i64,
    time_worked_so_far: i32,
    valid_farmland: Vec<BlockPos>,
}

const HARVEST_CONDITIONS: [(MemoryModuleId, MemoryStatus); 3] = [
    (types::LOOK_TARGET.id(), MemoryStatus::ValueAbsent),
    (types::WALK_TARGET.id(), MemoryStatus::ValueAbsent),
    (types::SECONDARY_JOB_SITE.id(), MemoryStatus::ValuePresent),
];

/// Vanilla `HarvestFarmland.HARVEST_DURATION`.
const HARVEST_DURATION: i32 = 200;

impl HarvestFarmland {
    fn pick_farmland(&self, mob: &dyn Mob) -> Option<BlockPos> {
        self.valid_farmland.choose(&mut mob.get_random()).copied()
    }

    fn valid_pos(world: &World, pos: &BlockPos, body: &VillagerEntity) -> bool {
        let (block, state) = world.get_block_and_state(pos);
        is_ripe(block, state.id)
            || (state.is_air()
                && world.get_block(&pos.down()) == &Block::FARMLAND
                && body.has_farm_seeds())
    }

    /// Plants the first seeds of the inventory at `pos`.
    fn plant(world: &Arc<World>, pos: BlockPos, body: &VillagerEntity) {
        let seed = {
            let mut inventory = body
                .inventory
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            inventory.iter_mut().find_map(|stack| {
                if stack.is_empty()
                    || !stack
                        .item
                        .has_tag(&tag::Item::MINECRAFT_VILLAGER_PLANTABLE_SEEDS)
                {
                    return None;
                }
                let block = Block::from_item_id(stack.item.id)?;
                stack.decrement(1);
                Some(block)
            })
        };
        if let Some(block) = seed {
            world.set_block_state(&pos, block.default_state.id, BlockFlags::NOTIFY_ALL);
            world.emit_game_event(
                pumpkin_data::game_event::GameEvent::BlockPlace.name(),
                pos.to_centered_f64(),
            );
            world.play_sound(Sound::ItemCropPlant, SoundCategory::Blocks, &pos.to_f64());
        }
    }
}

impl Behavior for HarvestFarmland {
    fn entry_conditions(&self) -> &[(MemoryModuleId, MemoryStatus)] {
        &HARVEST_CONDITIONS
    }

    fn check_extra_start_conditions(&mut self, tick: &mut BrainTick<'_>) -> bool {
        let Some(body) = as_villager(tick.mob) else {
            return false;
        };
        if !tick.world.level_info.load().game_rules.mob_griefing
            || body.profession() != VillagerProfession::Farmer
        {
            return false;
        }
        self.valid_farmland = neighbours(body.get_entity().block_pos.load())
            .filter(|pos| Self::valid_pos(tick.world, pos, body))
            .collect();
        self.above_farmland_pos = self.pick_farmland(tick.mob);
        self.above_farmland_pos.is_some()
    }

    fn start(&mut self, tick: &mut BrainTick<'_>) {
        if tick.time > self.next_ok_start_time
            && let Some(pos) = self.above_farmland_pos
        {
            walk_and_look_at_block(tick, pos, 1);
        }
    }

    fn stop(&mut self, tick: &mut BrainTick<'_>) {
        tick.state.erase(types::LOOK_TARGET.id());
        tick.state.erase(types::WALK_TARGET.id());
        self.time_worked_so_far = 0;
        self.next_ok_start_time = tick.time + 40;
    }

    fn tick(&mut self, tick: &mut BrainTick<'_>) {
        let Some(body) = as_villager(tick.mob) else {
            return;
        };
        let position = body.get_entity().pos.load();
        if self
            .above_farmland_pos
            .is_some_and(|pos| !closer_to_center_than(pos, position, 1.0))
        {
            return;
        }
        if let Some(pos) = self.above_farmland_pos
            && tick.time > self.next_ok_start_time
        {
            let (block, state) = tick.world.get_block_and_state(&pos);
            if is_ripe(block, state.id) {
                tick.world.break_block(&pos, None, BlockFlags::NOTIFY_ALL);
            }
            if state.is_air()
                && tick.world.get_block(&pos.down()) == &Block::FARMLAND
                && body.has_farm_seeds()
            {
                Self::plant(tick.world, pos, body);
            }
            if is_growing(block, state.id) {
                self.valid_farmland.retain(|candidate| *candidate != pos);
                self.above_farmland_pos = self.pick_farmland(tick.mob);
                if let Some(next) = self.above_farmland_pos {
                    self.next_ok_start_time = tick.time + 20;
                    walk_and_look_at_block(tick, next, 1);
                }
            }
        }
        self.time_worked_so_far += 1;
    }

    fn can_still_use(&mut self, _tick: &mut BrainTick<'_>) -> bool {
        self.time_worked_so_far < HARVEST_DURATION
    }

    fn debug_name(&self) -> &'static str {
        "HarvestFarmland"
    }
}

/// Vanilla `WorkAtComposter.useWorkstation`.
pub(super) fn use_composter(world: &Arc<World>, pos: BlockPos, body: &VillagerEntity) {
    let (block, state) = world.get_block_and_state(&pos);
    if block != &Block::COMPOSTER {
        return;
    }
    body.make_bread();
    compost_items(world, pos, body, state.id);
}

/// Vanilla `WorkAtComposter.compostItems`: a full composter gives its bone meal, then above ten
/// of a kind, up to twenty seeds go in one by one until the composter is ready.
fn compost_items(world: &Arc<World>, pos: BlockPos, body: &VillagerEntity, state: BlockStateId) {
    let source: Option<&dyn EntityBase> = Some(body);
    let started = if ComposterLikeProperties::from_state_id(state).level == 8 {
        ComposterBlock::extract_produce(source, state, world, &pos)
    } else {
        state
    };
    let mut current = started;
    let mut total_items_to_use = 20;
    let mut seen = [0i32; 2];
    let mut inventory = body
        .inventory
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    'stacks: for stack in inventory.iter_mut().rev() {
        if total_items_to_use <= 0 {
            break;
        }
        let index = if stack.item == &Item::WHEAT_SEEDS {
            0
        } else if stack.item == &Item::BEETROOT_SEEDS {
            1
        } else {
            continue;
        };
        let stack_size = i32::from(stack.item_count);
        seen[index] += stack_size;
        let items_to_use = (seen[index] - 10).min(total_items_to_use).min(stack_size);
        if items_to_use <= 0 {
            continue;
        }
        total_items_to_use -= items_to_use;
        for _ in 0..items_to_use {
            current = ComposterBlock::insert_item(source, current, world, stack, &pos);
            if ComposterLikeProperties::from_state_id(current).level == 7 {
                break 'stacks;
            }
        }
    }
    drop(inventory);
    // Vanilla compares with the state before the bone meal came out.
    world.sync_world_event(WorldEvent::ComposterFill, pos, i32::from(current != state));
}

/// Vanilla `UseBonemeal`.
#[derive(Default)]
pub(super) struct UseBonemeal {
    next_work_cycle_time: i64,
    last_bonemealing_session: i64,
    time_worked_so_far: i32,
    crop_pos: Option<BlockPos>,
}

const BONEMEAL_CONDITIONS: [(MemoryModuleId, MemoryStatus); 2] = [
    (types::LOOK_TARGET.id(), MemoryStatus::ValueAbsent),
    (types::WALK_TARGET.id(), MemoryStatus::ValueAbsent),
];

/// Vanilla `UseBonemeal.BONEMEALING_DURATION`.
const BONEMEALING_DURATION: i32 = 80;

impl UseBonemeal {
    fn pick_next_target(world: &World, mob: &dyn Mob) -> Option<BlockPos> {
        let growing: Vec<BlockPos> = neighbours(mob.get_entity().block_pos.load())
            .filter(|pos| {
                let (block, state) = world.get_block_and_state(pos);
                is_growing(block, state.id)
            })
            .collect();
        growing.choose(&mut mob.get_random()).copied()
    }
}

impl Behavior for UseBonemeal {
    fn entry_conditions(&self) -> &[(MemoryModuleId, MemoryStatus)] {
        &BONEMEAL_CONDITIONS
    }

    fn check_extra_start_conditions(&mut self, tick: &mut BrainTick<'_>) -> bool {
        let tick_count = i64::from(tick.mob.get_entity().tick_count.load(Ordering::Relaxed));
        if tick_count % 10 != 0
            || (self.last_bonemealing_session != 0
                && self.last_bonemealing_session + 160 > tick_count)
        {
            return false;
        }
        let has_bone_meal =
            as_villager(tick.mob).is_some_and(|body| body.count_item(&Item::BONE_MEAL) > 0);
        if !has_bone_meal {
            return false;
        }
        self.crop_pos = Self::pick_next_target(tick.world, tick.mob);
        self.crop_pos.is_some()
    }

    fn can_still_use(&mut self, _tick: &mut BrainTick<'_>) -> bool {
        self.time_worked_so_far < BONEMEALING_DURATION && self.crop_pos.is_some()
    }

    fn start(&mut self, tick: &mut BrainTick<'_>) {
        if let Some(pos) = self.crop_pos {
            walk_and_look_at_block(tick, pos, 1);
        }
        tick.mob.get_mob_entity().set_item_slot(
            &EquipmentSlot::MAIN_HAND,
            ItemStack::new(1, &Item::BONE_MEAL),
        );
        self.next_work_cycle_time = tick.time;
        self.time_worked_so_far = 0;
    }

    fn stop(&mut self, tick: &mut BrainTick<'_>) {
        tick.mob
            .get_mob_entity()
            .set_item_slot(&EquipmentSlot::MAIN_HAND, ItemStack::EMPTY.clone());
        self.last_bonemealing_session =
            i64::from(tick.mob.get_entity().tick_count.load(Ordering::Relaxed));
    }

    fn tick(&mut self, tick: &mut BrainTick<'_>) {
        let Some(target) = self.crop_pos else {
            return;
        };
        let Some(body) = as_villager(tick.mob) else {
            return;
        };
        if tick.time < self.next_work_cycle_time
            || !closer_to_center_than(target, body.get_entity().pos.load(), 1.0)
        {
            return;
        }
        if body.count_item(&Item::BONE_MEAL) > 0 {
            let block = tick.world.get_block(&target);
            let state = tick.world.get_block_state_id(&target);
            if tick
                .world
                .block_registry
                .bone_meal(block, tick.world, &target, state)
            {
                body.remove_items(&Item::BONE_MEAL, 1);
                tick.world
                    .sync_world_event(WorldEvent::ParticlesAndSoundPlantGrowth, target, 15);
                self.crop_pos = Self::pick_next_target(tick.world, tick.mob);
                if let Some(next) = self.crop_pos {
                    walk_and_look_at_block(tick, next, 1);
                }
                self.next_work_cycle_time = tick.time + 40;
            }
        }
        self.time_worked_so_far += 1;
    }

    fn debug_name(&self) -> &'static str {
        "UseBonemeal"
    }
}

/// Vanilla `ShowTradesToPlayer.MAX_LOOK_TIME`.
const MAX_LOOK_TIME: i32 = 900;
/// Vanilla `ShowTradesToPlayer.STARTING_LOOK_TIME`.
const STARTING_LOOK_TIME: i32 = 40;

/// Vanilla `ShowTradesToPlayer`.
pub(super) struct ShowTradesToPlayer {
    min_duration: i32,
    max_duration: i32,
    player_item: Option<&'static Item>,
    display_items: Vec<ItemStack>,
    cycle_counter: i32,
    display_index: usize,
    look_time: i32,
}

impl ShowTradesToPlayer {
    pub(super) const fn new(min_duration: i32, max_duration: i32) -> Self {
        Self {
            min_duration,
            max_duration,
            player_item: None,
            display_items: Vec::new(),
            cycle_counter: 0,
            display_index: 0,
            look_time: 0,
        }
    }

    fn target(tick: &BrainTick<'_>) -> Option<Arc<dyn EntityBase>> {
        let target = tick.state.get(types::INTERACTION_TARGET)?;
        let entity = target.get_entity();
        let body = tick.mob.get_entity();
        (entity.entity_type == &EntityType::PLAYER
            && body.is_alive()
            && entity.is_alive()
            && body.age.load(Ordering::Relaxed) >= 0
            && body.pos.load().squared_distance_to_vec(&entity.pos.load()) <= 17.0)
            .then(|| target.clone())
    }

    fn look_at(tick: &mut BrainTick<'_>, target: Arc<dyn EntityBase>) {
        tick.state.set(
            types::LOOK_TARGET,
            Arc::new(EntityTracker::new(target, true)),
        );
    }

    fn display(tick: &BrainTick<'_>, stack: ItemStack) {
        let mob = tick.mob.get_mob_entity();
        mob.set_item_slot(&EquipmentSlot::MAIN_HAND, stack);
        mob.set_drop_chance(&EquipmentSlot::MAIN_HAND, 0.0);
    }

    fn clear_held_item(tick: &BrainTick<'_>) {
        let mob = tick.mob.get_mob_entity();
        mob.set_item_slot(&EquipmentSlot::MAIN_HAND, ItemStack::EMPTY.clone());
        mob.set_drop_chance(&EquipmentSlot::MAIN_HAND, 0.085);
    }
}

const SHOW_TRADES_CONDITIONS: [(MemoryModuleId, MemoryStatus); 1] =
    [(types::INTERACTION_TARGET.id(), MemoryStatus::ValuePresent)];

impl Behavior for ShowTradesToPlayer {
    fn entry_conditions(&self) -> &[(MemoryModuleId, MemoryStatus)] {
        &SHOW_TRADES_CONDITIONS
    }

    fn min_duration(&self) -> i32 {
        self.min_duration
    }

    fn max_duration(&self) -> i32 {
        self.max_duration
    }

    fn check_extra_start_conditions(&mut self, tick: &mut BrainTick<'_>) -> bool {
        Self::target(tick).is_some()
    }

    fn can_still_use(&mut self, tick: &mut BrainTick<'_>) -> bool {
        Self::target(tick).is_some() && self.look_time > 0
    }

    fn start(&mut self, tick: &mut BrainTick<'_>) {
        if let Some(target) = Self::target(tick) {
            Self::look_at(tick, target);
        }
        self.cycle_counter = 0;
        self.display_index = 0;
        self.look_time = STARTING_LOOK_TIME;
    }

    fn tick(&mut self, tick: &mut BrainTick<'_>) {
        let Some(target) = Self::target(tick) else {
            return;
        };
        Self::look_at(tick, target.clone());
        let held = target
            .cast_any()
            .downcast_ref::<Player>()
            .map(|player| player.inventory().held_item())
            .filter(|stack| !stack.is_empty())
            .map(|stack| stack.item);
        if self.player_item != held {
            self.player_item = held;
            self.display_items.clear();
            if let (Some(item), Some(body)) = (held, as_villager(tick.mob)) {
                self.display_items = body.offers_paid_with(item);
                if let Some(first) = self.display_items.first() {
                    self.look_time = MAX_LOOK_TIME;
                    Self::display(tick, first.clone());
                }
            }
        }
        if self.display_items.is_empty() {
            Self::clear_held_item(tick);
            self.look_time = self.look_time.min(STARTING_LOOK_TIME);
        } else if self.display_items.len() >= 2 {
            self.cycle_counter += 1;
            if self.cycle_counter >= STARTING_LOOK_TIME {
                self.display_index = (self.display_index + 1) % self.display_items.len();
                self.cycle_counter = 0;
                Self::display(tick, self.display_items[self.display_index].clone());
            }
        }
        self.look_time -= 1;
    }

    fn stop(&mut self, tick: &mut BrainTick<'_>) {
        tick.state.erase(types::INTERACTION_TARGET.id());
        Self::clear_held_item(tick);
        self.player_item = None;
    }

    fn debug_name(&self) -> &'static str {
        "ShowTradesToPlayer"
    }
}

/// Vanilla `GiveGiftToHero`.
pub(super) struct GiveGiftToHero {
    time_until_next_gift: i32,
    gift_given_during_this_run: bool,
    time_since_start: i64,
}

impl Default for GiveGiftToHero {
    fn default() -> Self {
        Self {
            time_until_next_gift: 600,
            gift_given_during_this_run: false,
            time_since_start: 0,
        }
    }
}

const GIFT_CONDITIONS: [(MemoryModuleId, MemoryStatus); 4] = [
    (types::WALK_TARGET.id(), MemoryStatus::Registered),
    (types::LOOK_TARGET.id(), MemoryStatus::Registered),
    (types::INTERACTION_TARGET.id(), MemoryStatus::Registered),
    (
        types::NEAREST_VISIBLE_PLAYER.id(),
        MemoryStatus::ValuePresent,
    ),
];

/// The loot table of vanilla `GiveGiftToHero.GIFTS`.
fn gift_table(body: &VillagerEntity) -> &'static str {
    if body.get_entity().age.load(Ordering::Relaxed) < 0 {
        return "gameplay/hero_of_the_village/baby_gift";
    }
    match body.profession() {
        VillagerProfession::Armorer => "gameplay/hero_of_the_village/armorer_gift",
        VillagerProfession::Butcher => "gameplay/hero_of_the_village/butcher_gift",
        VillagerProfession::Cartographer => "gameplay/hero_of_the_village/cartographer_gift",
        VillagerProfession::Cleric => "gameplay/hero_of_the_village/cleric_gift",
        VillagerProfession::Farmer => "gameplay/hero_of_the_village/farmer_gift",
        VillagerProfession::Fisherman => "gameplay/hero_of_the_village/fisherman_gift",
        VillagerProfession::Fletcher => "gameplay/hero_of_the_village/fletcher_gift",
        VillagerProfession::Leatherworker => "gameplay/hero_of_the_village/leatherworker_gift",
        VillagerProfession::Librarian => "gameplay/hero_of_the_village/librarian_gift",
        VillagerProfession::Mason => "gameplay/hero_of_the_village/mason_gift",
        VillagerProfession::Shepherd => "gameplay/hero_of_the_village/shepherd_gift",
        VillagerProfession::Toolsmith => "gameplay/hero_of_the_village/toolsmith_gift",
        VillagerProfession::Weaponsmith => "gameplay/hero_of_the_village/weaponsmith_gift",
        VillagerProfession::None | VillagerProfession::Nitwit => {
            "gameplay/hero_of_the_village/unemployed_gift"
        }
    }
}

impl GiveGiftToHero {
    fn hero(tick: &BrainTick<'_>) -> Option<Arc<Player>> {
        tick.state
            .get(types::NEAREST_VISIBLE_PLAYER)
            .filter(|player| {
                player
                    .living_entity
                    .has_effect(&StatusEffect::HERO_OF_THE_VILLAGE)
            })
            .cloned()
    }

    fn throw_gift(tick: &BrainTick<'_>, body: &VillagerEntity, player: &Player) {
        let Some(table) = tick.world.get_loot_table(gift_table(body)) else {
            return;
        };
        let this: &dyn EntityBase = body;
        let params = LootContextParameters {
            world: Some(tick.world),
            position: Some(body.get_entity().pos.load()),
            this_entity: Some(this),
            ..Default::default()
        };
        for stack in generate_loot_from_handle(&table, rand::random(), &params) {
            utils::throw_item(body.get_entity(), stack, player.get_entity().pos.load());
        }
    }
}

impl Behavior for GiveGiftToHero {
    fn entry_conditions(&self) -> &[(MemoryModuleId, MemoryStatus)] {
        &GIFT_CONDITIONS
    }

    fn min_duration(&self) -> i32 {
        100
    }

    fn max_duration(&self) -> i32 {
        100
    }

    fn check_extra_start_conditions(&mut self, tick: &mut BrainTick<'_>) -> bool {
        if Self::hero(tick).is_none() {
            return false;
        }
        if self.time_until_next_gift > 0 {
            self.time_until_next_gift -= 1;
            return false;
        }
        true
    }

    fn can_still_use(&mut self, tick: &mut BrainTick<'_>) -> bool {
        Self::hero(tick).is_some() && !self.gift_given_during_this_run
    }

    fn start(&mut self, tick: &mut BrainTick<'_>) {
        self.gift_given_during_this_run = false;
        self.time_since_start = tick.time;
        if let Some(player) = Self::hero(tick) {
            let player: Arc<dyn EntityBase> = player;
            tick.state.set(types::INTERACTION_TARGET, player.clone());
            utils::look_at_entity(tick.state, player);
        }
    }

    fn tick(&mut self, tick: &mut BrainTick<'_>) {
        let Some(player) = Self::hero(tick) else {
            return;
        };
        let target: Arc<dyn EntityBase> = player.clone();
        utils::look_at_entity(tick.state, target.clone());
        let distance = player
            .get_entity()
            .block_pos
            .load()
            .0
            .to_f64()
            .squared_distance_to_vec(&tick.mob.get_entity().block_pos.load().0.to_f64());
        if distance < 25.0 {
            if tick.time - self.time_since_start > 20 {
                if let Some(body) = as_villager(tick.mob) {
                    Self::throw_gift(tick, body, &player);
                }
                self.gift_given_during_this_run = true;
            }
        } else {
            utils::set_walk_and_look_target_memories_to_entity(tick.state, target, 0.5, 5);
        }
    }

    fn stop(&mut self, tick: &mut BrainTick<'_>) {
        self.time_until_next_gift = 600 + tick.mob.get_random().random_range(0..=6000);
        tick.state.erase(types::INTERACTION_TARGET.id());
        tick.state.erase(types::WALK_TARGET.id());
        tick.state.erase(types::LOOK_TARGET.id());
    }

    fn debug_name(&self) -> &'static str {
        "GiveGiftToHero"
    }
}
