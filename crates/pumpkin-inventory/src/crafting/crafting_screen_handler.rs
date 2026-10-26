//! Crafting screen handler implementation.
//!
//! This module provides screen handlers for crafting mechanics:
//! - [`CraftingScreenHandler`] - Trait for crafting screen handlers
//! - [`CraftingTableScreenHandler`] - The 3x3 crafting table UI
//! - [`ResultSlot`] - The special result slot that shows crafted items
//!
//! # Recipe Matching
//!
//! Crafting recipes are matched against the items in the crafting grid.
//! The system supports:
//! - Shaped recipes (specific patterns)
//! - Shapeless recipes (any arrangement)
//! - Transmute recipes (upgrading items)
//! - Special recipes (like decorated pots)

use std::any::Any;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex, Weak};

use super::recipe_provider::{GenericRecipe, RecipeProvider};
use super::recipes::{RecipeFinderScreenHandler, RecipeInputInventory};
use crate::crafting::crafting_inventory::CraftingInventory;
use crate::player::player_inventory::PlayerInventory;
use crate::screen_handler::{
    InventoryPlayer, ScreenHandler, ScreenHandlerBehaviour, ScreenHandlerListener,
};
use crate::slot::{NormalSlot, Slot};

use crate::inventory::Inventory;
use pumpkin_data::data_component::DataComponent;
use pumpkin_data::data_component_impl::{
    DataComponentImpl, PotDecorationsImpl, ProvidesPotteryPatternImpl,
};
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::recipes::{CraftingRecipeTypes, RECIPES_CRAFTING, RecipeResultStruct};
use pumpkin_data::screen::WindowType;
use pumpkin_data::statistic::StatisticCategory;
use pumpkin_data::tag;
use pumpkin_data::tag::Taggable;
use pumpkin_protocol::codec::recipe::{
    DynamicRecipe, OwnedCraftingRecipe, OwnedRecipeIngredient, OwnedRecipeResult,
};

/// The result slot in a crafting screen.
pub struct ResultSlot {
    /// The crafting inventory (grid) that provides recipe input.
    pub inventory: Arc<dyn RecipeInputInventory>,
    /// Protocol ID for this slot (assigned by screen handler).
    pub id: AtomicU8,
    /// The cached result item stack.
    pub result: Arc<Mutex<ItemStack>>,
    /// Provider for dynamic recipes.
    pub recipe_provider: Option<Arc<dyn RecipeProvider>>,
    /// The player using the grid, known once a sync handler is attached.
    player: Mutex<Option<Weak<dyn InventoryPlayer>>>,
}

pub struct RecipeResult {
    pub item_id: String,
    pub count: u8,
    pub pot_decorations: Option<[u16; 4]>,
    /// Component patch of the result, set by plugin recipes.
    pub components: Vec<(DataComponent, Option<Box<dyn DataComponentImpl>>)>,
    /// The id of the matched plugin or datapack recipe; built-in vanilla recipes have none.
    pub recipe_id: Option<String>,
}

impl RecipeResult {
    fn from_vanilla(result: &RecipeResultStruct) -> Self {
        Self {
            item_id: result.id.to_string(),
            count: result.count,
            pot_decorations: None,
            components: Vec::new(),
            recipe_id: None,
        }
    }

    fn from_dynamic(recipe_id: Option<&str>, result: &OwnedRecipeResult) -> Self {
        Self {
            item_id: result.item_id.clone(),
            count: result.count,
            pot_decorations: None,
            components: result.components.clone(),
            recipe_id: recipe_id.map(str::to_string),
        }
    }

    #[must_use]
    pub fn to_item_stack(&self) -> ItemStack {
        let key = self
            .item_id
            .strip_prefix("minecraft:")
            .unwrap_or(&self.item_id);
        let item = pumpkin_data::item::Item::from_registry_key(key)
            .unwrap_or(&pumpkin_data::item::Item::AIR);
        let mut stack = ItemStack::new_with_component(self.count, item, self.components.clone());
        if let Some(sherds) = self.pot_decorations {
            let sherd_names = sherds
                .into_iter()
                .map(|id| {
                    let reg_key =
                        pumpkin_data::item::Item::from_id(id).map_or("brick", |it| it.registry_key);
                    if reg_key.starts_with("minecraft:") {
                        std::borrow::Cow::Borrowed(reg_key)
                    } else {
                        std::borrow::Cow::Owned(format!("minecraft:{reg_key}"))
                    }
                })
                .collect();
            stack.set_data_component(PotDecorationsImpl {
                sherds: sherd_names,
            });
        }
        stack
    }
}

/// Checks if a recipe pattern is symmetrical horizontally.
fn is_symmetrical_horizontally(pattern: &[&str]) -> bool {
    let width = pattern.first().map_or(0, |s| s.len());
    for row in pattern {
        if row.len() != width {
            return false;
        }
        for j in 0..width / 2 {
            if row.chars().nth(j) != row.chars().nth(width - j - 1) {
                return false;
            }
        }
    }
    true
}

/// Checks if a crafting recipe matches the current inventory state.
#[expect(clippy::too_many_lines)]
fn recipe_matches(
    recipe: GenericRecipe<'_>,
    input_height: usize,
    input_width: usize,
    top_x: usize,
    top_y: usize,
    count: usize,
    inventory: &dyn RecipeInputInventory,
) -> Option<RecipeResult> {
    match recipe {
        GenericRecipe::Vanilla(CraftingRecipeTypes::CraftingShaped {
            key,
            pattern,
            result,
            ..
        }) => {
            #[allow(clippy::redundant_closure_for_method_calls)]
            if pattern.len() != input_height
                || pattern.first().map_or(0, |f| f.len()) != input_width
            {
                return None;
            }

            if count
                != pattern
                    .iter()
                    .map(|l| l.chars().filter(|c| *c != ' ').count())
                    .sum::<usize>()
            {
                return None;
            }

            let x_offset = top_x;
            let y_offset = top_y;

            let mut matched = true;
            'outer: for (y, row_str) in pattern.iter().enumerate() {
                for (x, current_key) in row_str.chars().enumerate() {
                    let slot = inventory
                        .get_stack((y + y_offset) * inventory.get_width() + (x + x_offset));
                    if current_key == ' ' {
                        if !slot.is_empty() {
                            matched = false;
                            break 'outer;
                        }
                        continue;
                    }

                    let Some(ingredient) = key
                        .iter()
                        .find_map(|(k, v)| (*k == current_key).then_some(v))
                    else {
                        matched = false;
                        break 'outer;
                    };

                    if !ingredient.match_item(slot.item) {
                        matched = false;
                        break 'outer;
                    }
                }
            }

            if !matched && !is_symmetrical_horizontally(pattern) {
                matched = true;
                'outer: for y in 0..pattern.len() {
                    for x in 0..pattern[y].len() {
                        let Some(current_key) = pattern[y].chars().nth(x) else {
                            matched = false;
                            break 'outer;
                        };
                        let slot = inventory.get_stack(
                            (y + y_offset) * inventory.get_height()
                                + (x_offset + input_width - 1 - x),
                        );
                        if current_key == ' ' {
                            if !slot.is_empty() {
                                matched = false;
                                break 'outer;
                            }
                            continue;
                        }
                        let Some(ingredient) = key
                            .iter()
                            .find_map(|(k, v)| (*k == current_key).then_some(v))
                        else {
                            matched = false;
                            break 'outer;
                        };
                        if !ingredient.match_item(slot.item) {
                            matched = false;
                            break 'outer;
                        }
                    }
                }
            }

            matched.then(|| RecipeResult::from_vanilla(result))
        }
        GenericRecipe::Vanilla(CraftingRecipeTypes::CraftingShapeless {
            ingredients,
            result,
            ..
        }) => {
            if count != ingredients.len() {
                return None;
            }
            let mut ingredient_used = vec![false; ingredients.len()];
            'next_slot: for i in 0..inventory.size() {
                let slot = inventory.get_stack(i);
                if slot.is_empty() {
                    continue 'next_slot;
                }
                for i in 0..ingredients.len() {
                    if !ingredient_used[i] && ingredients[i].match_item(slot.item) {
                        ingredient_used[i] = true;
                        continue 'next_slot;
                    }
                }
                return None;
            }
            Some(RecipeResult::from_vanilla(result))
        }
        GenericRecipe::Vanilla(CraftingRecipeTypes::CraftingTransmute {
            input,
            material,
            result,
            ..
        }) => {
            if count != 2 {
                return None;
            }
            'item_stack: for i in 0..inventory.size() {
                let slot = inventory.get_stack(i);
                if slot.is_empty() {
                    continue 'item_stack;
                }
                if !material.match_item(slot.item) && !input.match_item(slot.item) {
                    return None;
                }
            }
            Some(RecipeResult::from_vanilla(result))
        }
        GenericRecipe::Vanilla(CraftingRecipeTypes::CraftingDecoratedPot { .. }) => {
            if count != 4 || inventory.get_width() != 3 || inventory.get_height() != 3 {
                return None;
            }
            let mut sherds = [0u16; 4];
            for (idx, position) in (1..=7).step_by(2).enumerate() {
                let slot = inventory.get_stack(position);
                if slot.is_empty()
                    || (!slot
                        .item
                        .has_tag(&tag::Item::MINECRAFT_DECORATED_POT_INGREDIENTS)
                        && slot
                            .get_data_component::<ProvidesPotteryPatternImpl>()
                            .is_none())
                {
                    return None;
                }
                sherds[idx] = slot.item.id;
            }
            Some(RecipeResult {
                item_id: "minecraft:decorated_pot".to_string(),
                count: 1,
                pot_decorations: Some(sherds),
                components: Vec::new(),
                recipe_id: None,
            })
        }
        GenericRecipe::Dynamic(OwnedCraftingRecipe::Shaped {
            recipe_id,
            pattern,
            key,
            result,
            ..
        }) => {
            #[allow(clippy::redundant_closure_for_method_calls)]
            if pattern.len() != input_height
                || pattern.first().map_or(0, |f| f.len()) != input_width
            {
                return None;
            }
            if count
                != pattern
                    .iter()
                    .map(|l| l.chars().filter(|c| *c != ' ').count())
                    .sum::<usize>()
            {
                return None;
            }
            let x_offset = top_x;
            let y_offset = top_y;
            let mut matched = true;
            'outer: for (y, row_str) in pattern.iter().enumerate() {
                for (x, current_key) in row_str.chars().enumerate() {
                    let slot = inventory
                        .get_stack((y + y_offset) * inventory.get_width() + (x + x_offset));
                    if current_key == ' ' {
                        if !slot.is_empty() {
                            matched = false;
                            break 'outer;
                        }
                        continue;
                    }
                    let Some(ingredient) =
                        key.iter().find(|(k, _)| *k == current_key).map(|(_, v)| v)
                    else {
                        matched = false;
                        break 'outer;
                    };
                    if !ingredient.match_stack(&slot) {
                        matched = false;
                        break 'outer;
                    }
                }
            }
            matched.then(|| RecipeResult::from_dynamic(recipe_id.as_deref(), result))
        }
        GenericRecipe::Dynamic(OwnedCraftingRecipe::Shapeless {
            recipe_id,
            ingredients,
            result,
            ..
        }) => {
            if count != ingredients.len() {
                return None;
            }
            let stacks: Vec<ItemStack> = (0..inventory.size())
                .map(|i| inventory.get_stack(i))
                .filter(|stack| !stack.is_empty())
                .collect();
            // A greedy pick fails once a stack ingredient and a plain one accept the same item
            // (a custom item and its base item), so assign stacks to ingredients as a matching.
            let mut owners = vec![None; ingredients.len()];
            let all_assigned = (0..stacks.len()).all(|slot| {
                let mut visited = vec![false; ingredients.len()];
                assign_ingredient(slot, &stacks, ingredients, &mut visited, &mut owners)
            });
            all_assigned.then(|| RecipeResult::from_dynamic(recipe_id.as_deref(), result))
        }
        _ => None,
    }
}

/// Finds an ingredient for the stack at `slot`, moving earlier stacks to other ingredients when
/// needed (augmenting path). `owners` holds the stack index each ingredient is assigned to.
fn assign_ingredient(
    slot: usize,
    stacks: &[ItemStack],
    ingredients: &[OwnedRecipeIngredient],
    visited: &mut [bool],
    owners: &mut [Option<usize>],
) -> bool {
    for (i, ingredient) in ingredients.iter().enumerate() {
        if visited[i] || !ingredient.match_stack(&stacks[slot]) {
            continue;
        }
        visited[i] = true;
        let current = owners[i];
        if current
            .is_none_or(|owner| assign_ingredient(owner, stacks, ingredients, visited, owners))
        {
            owners[i] = Some(slot);
            return true;
        }
    }
    false
}

#[must_use]
pub fn match_crafting_recipe(
    inventory: &dyn RecipeInputInventory,
    provider: Option<&dyn RecipeProvider>,
) -> Option<RecipeResult> {
    let mut count: usize = 0;
    let inventory_width = inventory.get_width();
    let mut top_x = 9;
    let mut top_y = 9;
    let mut bottom_x = 0;
    let mut bottom_y = 0;
    for i in 0..inventory.size() {
        let x = i % inventory_width;
        let y = i / inventory_width;
        let slot = inventory.get_stack(i);
        if !slot.is_empty() {
            top_x = top_x.min(x);
            top_y = top_y.min(y);
            bottom_x = bottom_x.max(x);
            bottom_y = bottom_y.max(y);
            count += 1;
        }
    }
    if count == 0 {
        return None;
    }
    let input_width = bottom_x + 1 - top_x;
    let input_height = bottom_y + 1 - top_y;

    let dynamic = provider.map(RecipeProvider::get_dynamic_recipes);
    let crafting = || {
        dynamic.iter().flatten().filter_map(|recipe| {
            if let DynamicRecipe::Crafting(crafting) = recipe {
                Some(crafting)
            } else {
                None
            }
        })
    };
    let try_match = |recipe| {
        recipe_matches(
            recipe,
            input_height,
            input_width,
            top_x,
            top_y,
            count,
            inventory,
        )
    };

    // Recipes that look at stack components come first, so a stack a plugin recipe claims
    // does not craft as its plain item through a vanilla recipe.
    crafting()
        .filter(|recipe| recipe.has_stack_ingredients())
        .find_map(|recipe| try_match(GenericRecipe::Dynamic(recipe)))
        .or_else(|| {
            RECIPES_CRAFTING
                .iter()
                .find_map(|recipe| try_match(GenericRecipe::Vanilla(recipe)))
        })
        .or_else(|| {
            crafting()
                .filter(|recipe| !recipe.has_stack_ingredients())
                .find_map(|recipe| try_match(GenericRecipe::Dynamic(recipe)))
        })
}

impl ResultSlot {
    pub fn new(
        inventory: Arc<dyn RecipeInputInventory>,
        provider: Option<Arc<dyn RecipeProvider>>,
    ) -> Self {
        Self {
            inventory,
            id: AtomicU8::new(0),
            result: Arc::new(Mutex::new(ItemStack::EMPTY.clone())),
            recipe_provider: provider,
            player: Mutex::new(None),
        }
    }

    fn match_recipe(&self) -> Option<RecipeResult> {
        match_crafting_recipe(&*self.inventory, self.recipe_provider.as_deref())
    }

    // CraftingMenu.slotChangedCraftingGrid
    fn refill_output(&self) -> ItemStack {
        let matched = self.match_recipe();
        let mut result = matched
            .as_ref()
            .map_or_else(|| ItemStack::EMPTY.clone(), RecipeResult::to_item_stack);
        let player = self
            .player
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .and_then(Weak::upgrade);
        if let Some(player) = player {
            let recipe_id = matched.as_ref().and_then(|m| m.recipe_id.as_deref());
            player.fire_prepare_item_craft_event(&*self.inventory, recipe_id, &mut result);
        }
        *self
            .result
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = result.clone();
        result
    }
}

impl Slot for ResultSlot {
    fn get_inventory(&self) -> Arc<dyn Inventory> {
        self.inventory.clone()
    }
    fn get_index(&self) -> usize {
        999
    }
    fn set_id(&self, id: usize) {
        self.id.store(id as u8, Ordering::Relaxed);
    }
    fn on_quick_move_crafted(&self, _stack: ItemStack, _stack_prev: ItemStack) {
        self.refill_output();
    }
    fn on_take_item(&self, player: &dyn InventoryPlayer, stack: &ItemStack) {
        player.increment_stat(
            StatisticCategory::Crafted,
            stack.item.id as i32,
            stack.item_count as i32,
        );
        for i in 0..self.inventory.size() {
            self.inventory.remove_stack_specific(i, 1);
        }
        self.mark_dirty();
    }
    fn can_insert(&self, _stack: &ItemStack) -> bool {
        false
    }
    fn get_stack(&self) -> ItemStack {
        self.result
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
    fn get_cloned_stack(&self) -> ItemStack {
        self.result
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
    fn has_stack(&self) -> bool {
        !self
            .result
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_empty()
    }
    fn set_stack(&self, _stack: ItemStack) {
        self.refill_output();
    }
    fn set_stack_prev(&self, _stack: ItemStack, _previous_stack: ItemStack) {
        self.refill_output();
    }
    fn mark_dirty(&self) {
        self.inventory.mark_dirty();
    }
    fn get_max_item_count(&self) -> u8 {
        let mut count = u8::MAX;
        for i in 0..self.inventory.size() {
            let slot = self.inventory.get_stack(i);
            if !slot.is_empty() {
                count = count.min(slot.item_count);
            }
        }
        count
    }
    fn take_stack(&self, _amount: u8) -> ItemStack {
        if self.has_stack() {
            self.result
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
        } else {
            ItemStack::EMPTY.clone()
        }
    }
}

impl ScreenHandlerListener for ResultSlot {
    fn on_slot_update(&self, screen_handler: &ScreenHandlerBehaviour, slot: u8, _stack: ItemStack) {
        if (0..=(self.inventory.get_width() * self.inventory.get_height()))
            .contains(&(slot as usize))
        {
            if let Some(player) = screen_handler
                .sync_handler
                .as_ref()
                .and_then(|sync_handler| sync_handler.player())
            {
                self.player
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .replace(Arc::downgrade(&player));
            }
            let result = self.refill_output();
            let next_revision = screen_handler.next_revision();
            if let Some(sync_handler) = screen_handler.sync_handler.as_ref() {
                sync_handler.update_slot(screen_handler, 0, &result, next_revision);
            }
        }
    }
}

pub trait CraftingScreenHandler<I: RecipeInputInventory>:
    RecipeFinderScreenHandler + ScreenHandler
{
    fn add_recipe_slots(
        &mut self,
        crafing_inventory: Arc<dyn RecipeInputInventory>,
        provider: Option<Arc<dyn RecipeProvider>>,
    ) {
        let result_slot = Arc::new(ResultSlot::new(crafing_inventory.clone(), provider));
        self.add_slot(result_slot.clone());
        let width = crafing_inventory.get_width();
        let height = crafing_inventory.get_height();
        for i in 0..width {
            for j in 0..height {
                let input_slot = NormalSlot::new(crafing_inventory.clone(), j + i * width);
                self.add_slot(Arc::new(input_slot));
            }
        }
        self.add_listener(result_slot);
    }
}

pub struct CraftingTableScreenHandler {
    behaviour: ScreenHandlerBehaviour,
    crafting_inventory: Arc<dyn RecipeInputInventory>,
}

impl CraftingTableScreenHandler {
    pub fn new(
        sync_id: u8,
        player_inventory: &Arc<PlayerInventory>,
        provider: Option<Arc<dyn RecipeProvider>>,
    ) -> Self {
        let crafting_inventory: Arc<dyn RecipeInputInventory> =
            Arc::new(CraftingInventory::new(3, 3));
        let mut crafting_table_handler = Self {
            behaviour: ScreenHandlerBehaviour::new(sync_id, Some(WindowType::Crafting)),
            crafting_inventory: crafting_inventory.clone(),
        };
        crafting_table_handler.add_recipe_slots(crafting_inventory, provider);
        let player_inventory: Arc<dyn Inventory> = player_inventory.clone();
        crafting_table_handler.add_player_slots(&player_inventory);
        crafting_table_handler
    }
}

impl RecipeFinderScreenHandler for CraftingTableScreenHandler {}

impl ScreenHandler for CraftingTableScreenHandler {
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
    fn get_behaviour(&self) -> &ScreenHandlerBehaviour {
        &self.behaviour
    }
    fn get_behaviour_mut(&mut self) -> &mut ScreenHandlerBehaviour {
        &mut self.behaviour
    }
    fn on_closed(&mut self, player: &dyn InventoryPlayer) {
        self.default_on_closed(player);
        self.drop_inventory(player, self.crafting_inventory.clone());
    }
    fn quick_move(&mut self, player: &dyn InventoryPlayer, slot_index: i32) -> ItemStack {
        let slot = self.get_behaviour().slots[slot_index as usize].clone();
        if slot.has_stack() {
            let mut slot_stack = slot.get_stack();
            let stack_prev = slot_stack.clone();
            if slot_index == 0 {
                if !self.insert_item(&mut slot_stack, 10, 46, true) {
                    return ItemStack::EMPTY.clone();
                }
            } else if (1..=9).contains(&slot_index) {
                if !self.insert_item(&mut slot_stack, 10, 46, false) {
                    return ItemStack::EMPTY.clone();
                }
            } else if (10..46).contains(&slot_index) {
                if !self.insert_item(&mut slot_stack, 1, 10, false) {
                    if slot_index < 37 {
                        if !self.insert_item(&mut slot_stack, 37, 46, false) {
                            return ItemStack::EMPTY.clone();
                        }
                    } else if !self.insert_item(&mut slot_stack, 10, 37, false) {
                        return ItemStack::EMPTY.clone();
                    }
                }
            } else if !self.insert_item(&mut slot_stack, 10, 46, false) {
                return ItemStack::EMPTY.clone();
            }
            let stack = slot_stack.clone();
            drop(slot_stack);
            if stack.is_empty() {
                slot.set_stack_prev(ItemStack::EMPTY.clone(), stack_prev.clone());
            } else {
                slot.mark_dirty();
            }
            if stack.item_count == stack_prev.item_count {
                return ItemStack::EMPTY.clone();
            }

            let mut taken_stack = stack_prev.clone();
            taken_stack.set_count(stack_prev.item_count - stack.item_count);
            slot.on_take_item(player, &taken_stack);

            if slot_index == 0 {
                slot.on_quick_move_crafted(stack.clone(), stack_prev.clone());
                if !stack.is_empty() {
                    player.drop_item(stack, false);
                }
            }
            return stack_prev;
        }
        ItemStack::EMPTY.clone()
    }
}

impl CraftingScreenHandler<CraftingInventory> for CraftingTableScreenHandler {}
