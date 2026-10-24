use std::sync::Arc;

use pumpkin_data::{
    item_stack::ItemStack,
    recipes::{CookingRecipe, CookingRecipeKind},
};
use pumpkin_protocol::codec::recipe::OwnedCookingRecipe;

use crate::block::entities::{BlockEntity, PropertyDelegate};
use crate::server::RecipeManager;
pub use pumpkin_inventory::ExperienceContainer;
use pumpkin_inventory::{Clearable, Inventory};

/// Trait for extracting smelting experience from cooking block entities.
pub trait CookingBlockEntityBase:
    Sync + Send + Inventory + PropertyDelegate + BlockEntity + Clearable
{
    fn get_cooking_time_spent(&self) -> u16;
    fn get_cooking_total_time(&self) -> u16;
    fn get_lit_time_remaining(&self) -> u16;
    fn get_lit_total_time(&self) -> u16;

    /// Track that a recipe was used (for XP calculation on extraction)
    /// Uses the result item ID as the recipe identifier
    fn add_recipe_used(&self, recipe: &CookingMatch);
    /// Extract and reset accumulated experience, returning the total as an integer
    /// Calculates XP from tracked recipes and clears the `recipes_used` map
    fn extract_experience_from_recipes(&self) -> i32;

    fn get_input_item(&self) -> ItemStack;
    fn get_fuel_item(&self) -> ItemStack;
    fn get_output_item(&self) -> ItemStack;

    fn set_cooking_time_spent(&self, spent_time: u16);
    fn set_cooking_total_time(&self, total_time: u16);
    fn set_lit_time_remaining(&self, remaining_time: u16);
    fn set_lit_total_time(&self, total_time: u16);

    fn is_burning(&self) -> bool;
    fn can_accept_recipe_output(&self, result: Option<&ItemStack>, max_count: u8) -> bool;
    fn craft_recipe(&self, recipe: Option<&CookingMatch>, result: &ItemStack) -> bool;
    /// Whether the input slot is filled and the output slot is not full.
    fn output_has_room(&self) -> bool;

    /// Remembers the server's recipe manager for the lookups that have no world at hand
    /// (inventory changes and experience).
    fn attach_recipe_manager(&self, recipe_manager: &Arc<RecipeManager>);
    /// Finds the recipe this block entity cooks `input` with.
    fn find_recipe(&self, input: &ItemStack) -> Option<CookingMatch>;
}

/// A cooking recipe that matched an input: a generated vanilla one, or one from a plugin or a
/// datapack.
#[derive(Clone)]
pub struct CookingMatch {
    /// The recipe id, as `RecipesUsed` stores it.
    pub recipe_id: String,
    pub cooking_time: i32,
    pub experience: f32,
    /// The stack the recipe puts into the output slot.
    pub result: ItemStack,
}

impl CookingMatch {
    #[must_use]
    pub fn from_vanilla(recipe: &CookingRecipe) -> Option<Self> {
        Some(Self {
            recipe_id: recipe.recipe_id.to_string(),
            cooking_time: recipe.cookingtime,
            experience: recipe.experience,
            result: cooking_recipe_result(recipe)?,
        })
    }

    #[must_use]
    pub fn from_dynamic(recipe: &OwnedCookingRecipe) -> Self {
        Self {
            recipe_id: recipe.recipe_id.clone(),
            cooking_time: recipe.cooking_time,
            experience: recipe.experience,
            result: recipe.result.to_item_stack(),
        }
    }

    /// Finds the recipe `kind` cooks `input` with. Without a recipe manager only the generated
    /// vanilla recipes are searched.
    #[must_use]
    pub fn find(
        recipe_manager: Option<&RecipeManager>,
        input: &ItemStack,
        kind: &CookingRecipeKind,
    ) -> Option<Self> {
        recipe_manager.map_or_else(
            || Self::find_vanilla(input, kind),
            |recipe_manager| recipe_manager.find_cooking_recipe(input, kind),
        )
    }

    /// Finds the generated vanilla recipe `kind` cooks `input` with.
    #[must_use]
    pub fn find_vanilla(input: &ItemStack, kind: &CookingRecipeKind) -> Option<Self> {
        if input.is_empty() {
            return None;
        }
        pumpkin_data::recipes::get_cooking_recipe_with_ingredient(input.item, kind.clone())
            .and_then(Self::from_vanilla)
    }
}

/// Builds the stack a cooking recipe puts into the output slot.
#[must_use]
pub fn cooking_recipe_result(recipe: &CookingRecipe) -> Option<ItemStack> {
    let item = pumpkin_data::item::Item::from_registry_key(
        recipe
            .result
            .id
            .strip_prefix("minecraft:")
            .unwrap_or(recipe.result.id),
    )?;
    Some(ItemStack::new(recipe.result.count, item))
}

#[macro_export]
macro_rules! impl_cooking_block_entity_base {
    ($struct_name:ty) => {
        impl CookingBlockEntityBase for $struct_name {
            fn get_cooking_time_spent(&self) -> u16 {
                self.cooking_time_spent.load(Ordering::Relaxed)
            }

            fn get_cooking_total_time(&self) -> u16 {
                self.cooking_total_time.load(Ordering::Relaxed)
            }

            fn get_lit_time_remaining(&self) -> u16 {
                self.lit_time_remaining.load(Ordering::Relaxed)
            }

            fn get_lit_total_time(&self) -> u16 {
                self.lit_total_time.load(Ordering::Relaxed)
            }

            fn get_input_item(&self) -> ItemStack {
                let items = self
                    .items
                    .read()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                items[0].clone()
            }

            fn get_fuel_item(&self) -> ItemStack {
                let items = self
                    .items
                    .read()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                items[1].clone()
            }

            fn get_output_item(&self) -> ItemStack {
                let items = self
                    .items
                    .read()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                items[2].clone()
            }

            fn set_cooking_time_spent(&self, spent_time: u16) {
                self.cooking_time_spent.store(spent_time, Ordering::Relaxed);
            }

            fn set_cooking_total_time(&self, total_time: u16) {
                self.cooking_total_time.store(total_time, Ordering::Relaxed);
            }

            fn set_lit_time_remaining(&self, remaining_time: u16) {
                self.lit_time_remaining
                    .store(remaining_time, Ordering::Relaxed);
            }

            fn set_lit_total_time(&self, total_time: u16) {
                self.lit_total_time.store(total_time, Ordering::Relaxed);
            }

            fn is_burning(&self) -> bool {
                self.get_lit_time_remaining() > 0
            }

            fn add_recipe_used(
                &self,
                recipe: &$crate::block::entities::furnace_like_block_entity::CookingMatch,
            ) {
                // Track recipe usage by recipe ID for XP calculation
                let recipe_id = recipe.recipe_id.clone();
                let mut recipes = self
                    .recipes_used
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                *recipes.entry(recipe_id).or_insert(0) += 1;
            }

            fn extract_experience_from_recipes(&self) -> i32 {
                // Calculate total XP from tracked recipes and clear the map (vanilla behavior)
                let recipe_manager = self.recipe_manager.get().and_then(std::sync::Weak::upgrade);
                let mut recipes = self
                    .recipes_used
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let mut total_xp: f32 = 0.0;
                for (recipe_id, count) in recipes.iter() {
                    // Plugin and datapack recipes are only known to the recipe manager
                    let xp = recipe_manager.as_ref().map_or_else(
                        || pumpkin_data::recipes::get_recipe_experience(recipe_id),
                        |recipe_manager| recipe_manager.cooking_experience(recipe_id),
                    );
                    if let Some(xp) = xp {
                        total_xp += xp * (*count as f32);
                    }
                }
                recipes.clear();
                total_xp.floor() as i32
            }

            fn can_accept_recipe_output(
                &self,
                result: Option<&pumpkin_data::item_stack::ItemStack>,
                max_count: u8,
            ) -> bool {
                let Some(result) = result else { return false };
                let Ok(items) = self.items.try_read() else {
                    return false;
                };

                let is_top_items_empty = items[0].is_empty();
                let side_item_stack = &items[2];

                if side_item_stack.is_empty() {
                    return !is_top_items_empty;
                }

                // Vanilla `canBurn` only stacks onto the same item with the same components.
                // A plugin result can hold more than one item, so it has to fit as a whole.
                !is_top_items_empty
                    && side_item_stack.are_items_and_components_equal(result)
                    && side_item_stack.item_count + result.item_count
                        <= max_count.min(side_item_stack.get_max_stack_size())
            }
            fn output_has_room(&self) -> bool {
                let Ok(items) = self.items.try_read() else {
                    return false;
                };
                !items[0].is_empty()
                    && items[2].item_count
                        < self
                            .get_max_count_per_stack()
                            .min(items[2].get_max_stack_size())
            }
            fn craft_recipe(
                &self,
                recipe: Option<&$crate::block::entities::furnace_like_block_entity::CookingMatch>,
                result: &ItemStack,
            ) -> bool {
                let Some(recipe) = recipe else { return false };
                // A plugin can empty the result to smelt the source into nothing.
                if !result.is_empty()
                    && !self.can_accept_recipe_output(Some(result), self.get_max_count_per_stack())
                {
                    return false;
                }
                let Ok(mut items) = self.items.try_write() else {
                    return false;
                };

                if items[2].is_empty() {
                    items[2] = result.clone();
                } else if items[2].are_items_and_components_equal(result) {
                    items[2].increment(result.item_count);
                }

                // Track recipe usage for XP calculation (vanilla RecipesUsed format)
                self.add_recipe_used(recipe);

                if items[0].item.id == pumpkin_data::item::Item::WET_SPONGE.id
                    && !items[1].is_empty()
                    && items[1].item.id == pumpkin_data::item::Item::BUCKET.id
                {
                    items[1] = ItemStack::new(1, &pumpkin_data::item::Item::WATER_BUCKET);
                }

                items[0].decrement(1);
                true
            }

            fn attach_recipe_manager(
                &self,
                recipe_manager: &std::sync::Arc<$crate::server::RecipeManager>,
            ) {
                if self.recipe_manager.get().is_none() {
                    let _ = self
                        .recipe_manager
                        .set(std::sync::Arc::downgrade(recipe_manager));
                }
            }

            fn find_recipe(
                &self,
                input: &ItemStack,
            ) -> Option<$crate::block::entities::furnace_like_block_entity::CookingMatch> {
                let recipe_manager = self.recipe_manager.get().and_then(std::sync::Weak::upgrade);
                $crate::block::entities::furnace_like_block_entity::CookingMatch::find(
                    recipe_manager.as_deref(),
                    input,
                    &Self::RECIPE_KIND,
                )
            }
        }
    };
}

#[macro_export]
macro_rules! impl_property_delegate_for_cooking {
    ($struct_name:ty) => {
        impl $crate::block::entities::PropertyDelegate for $struct_name {
            fn get_property(&self, index: i32) -> i32 {
                match index {
                    0 => self.get_lit_time_remaining() as i32,
                    1 => self.get_lit_total_time() as i32,
                    2 => self.get_cooking_time_spent() as i32,
                    3 => self.get_cooking_total_time() as i32,
                    _ => 0,
                }
            }

            fn set_property(&self, index: i32, value: i32) {
                let value = value as u16;
                match index {
                    0 => self.set_lit_time_remaining(value),
                    1 => self.set_lit_total_time(value),
                    2 => self.set_cooking_time_spent(value),
                    3 => self.set_cooking_total_time(value),
                    _ => {}
                }
            }

            fn get_properties_size(&self) -> i32 {
                4
            }
        }
    };
}

#[macro_export]
macro_rules! impl_clearable_for_cooking {
    ($struct_name:ty) => {
        impl pumpkin_inventory::Clearable for $struct_name {
            fn clear(&self) {
                let mut items = self
                    .items
                    .write()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                items.fill_with(|| ItemStack::EMPTY.clone());
                self.mark_dirty();
            }
        }
    };
}

#[macro_export]
macro_rules! impl_experience_container_for_cooking {
    ($struct_name:ty) => {
        impl $crate::block::entities::furnace_like_block_entity::ExperienceContainer
            for $struct_name
        {
            fn extract_experience(&self) -> i32 {
                // Delegate to the CookingBlockEntityBase method
                self.extract_experience_from_recipes()
            }
        }
    };
}

#[macro_export]
macro_rules! impl_inventory_for_cooking {
    ($struct_name:ty) => {
        impl pumpkin_inventory::Inventory for $struct_name {
            fn size(&self) -> usize {
                Self::INVENTORY_SIZE
            }

            fn is_empty(&self) -> bool {
                let items = self
                    .items
                    .read()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                items.iter().all(|s| s.is_empty())
            }

            fn get_stack(&self, slot: usize) -> ItemStack {
                let items = self
                    .items
                    .read()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                items[slot].clone()
            }

            fn remove_stack(&self, slot: usize) -> ItemStack {
                let mut items = self
                    .items
                    .write()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let removed = std::mem::replace(&mut items[slot], ItemStack::EMPTY.clone());
                self.mark_dirty();
                removed
            }

            fn remove_stack_specific(&self, slot: usize, amount: u8) -> ItemStack {
                let mut items = self
                    .items
                    .write()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let res = if !items[slot].is_empty() && amount > 0 {
                    items[slot].split(amount)
                } else {
                    ItemStack::EMPTY.clone()
                };
                self.mark_dirty();
                res
            }

            fn set_stack(&self, slot: usize, stack: ItemStack) {
                let mut items = self
                    .items
                    .write()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let is_same_item = !stack.is_empty()
                    && ItemStack::are_items_and_components_equal(&items[slot], &stack);

                items[slot] = stack.clone();

                if slot == 0 && !is_same_item {
                    let cooking_time = self
                        .find_recipe(&stack)
                        .map_or(0, |recipe| recipe.cooking_time);
                    self.set_cooking_total_time(cooking_time as u16);
                    self.set_cooking_time_spent(0);
                }

                // Always consider the inventory changed when setting a stack
                self.mark_dirty();
            }

            fn mark_dirty(&self) {
                self.dirty.store(true, Ordering::Relaxed);
                self.comparator_dirty.store(true, Ordering::Relaxed);
            }

            fn as_any(&self) -> &dyn std::any::Any {
                self
            }
        }
    };
}

#[macro_export]
macro_rules! impl_block_entity_for_cooking {
    ($struct_name:ty) => {
        impl $crate::block::entities::BlockEntity for $struct_name {
            #[expect(clippy::too_many_lines)]
            fn tick(
                &self,
                world: &Arc<$crate::world::World>,
            ) {
                let is_burning = self.is_burning();
                let mut is_dirty = false;
                if self.is_burning() {
                    self.lit_time_remaining.fetch_sub(1, Ordering::Relaxed);
                }

                let (top_item, bottom_item) = if let Ok(items_guard) = self.items.try_read() {
                    (items_guard[0].clone(), items_guard[1].clone())
                } else {
                    return;
                };

                let is_top_items_empty = top_item.is_empty();

                if let Some(server) = world.server.upgrade() {
                    self.attach_recipe_manager(&server.recipe_manager);
                }
                let furnace_recipe = self.find_recipe(&top_item);
                let recipe_result = furnace_recipe.as_ref().map(|recipe| recipe.result.clone());

                // The output slot may hold what a plugin gave for an earlier smelt instead of the
                // vanilla result; then the smelt event decides, and `craft_recipe` checks the
                // result it returns.
                let can_accept_output = self.can_accept_recipe_output(
                    recipe_result.as_ref(),
                    self.get_max_count_per_stack(),
                ) || (recipe_result.is_some()
                    && self.output_has_room()
                    && world.server.upgrade().is_some_and(|server| {
                        server
                            .plugin_manager
                            .has_handlers::<$crate::plugin::api::events::inventory::furnace_smelt::FurnaceSmeltEvent>()
                    }));

                let bottom_items_is_empty = bottom_item.is_empty();
                if self.is_burning() || !bottom_items_is_empty && !is_top_items_empty {
                    if !self.is_burning() && can_accept_output {
                        let is_fast = matches!(
                            Self::RECIPE_KIND,
                            CookingRecipeKind::Blasting | CookingRecipeKind::Smoking
                        );
                        let fuel_component = bottom_item
                            .get_data_component::<pumpkin_data::data_component_impl::CookingFuelImpl>();
                        let adjusted_fuel_ticks = fuel_component
                            .map_or(0, |fuel| fuel.get_burn_ticks(is_fast));

                        let mut burn_ticks = adjusted_fuel_ticks;
                        let mut burn_cancelled = false;
                        if let Some(server) = world.server.upgrade() {
                            let mut burn_event = $crate::plugin::api::events::inventory::furnace_burn::FurnaceBurnEvent::new(
                                self.position,
                                bottom_item.item.registry_key.to_string(),
                                adjusted_fuel_ticks as u32,
                                bottom_item,
                            );
                            server.plugin_manager.fire_blocking(&server, &mut burn_event);
                            if burn_event.cancelled {
                                burn_cancelled = true;
                            } else {
                                burn_ticks = burn_event.burn_time as u16;
                            }
                        }
                        if !burn_cancelled {
                            self.set_lit_time_remaining(burn_ticks);
                            self.set_lit_total_time(burn_ticks);

                            if self.is_burning() {
                                is_dirty = true;
                                if let Ok(mut items_guard) = self.items.try_write() {
                                    if !items_guard[1].is_empty() {
                                        let remainder = items_guard[1]
                                            .get_data_component::<
                                                pumpkin_data::data_component_impl::UseRemainderImpl,
                                            >()
                                            .and_then(|component| component.remainder.as_deref())
                                            .and_then(pumpkin_data::item::Item::from_registry_key);
                                        items_guard[1].decrement(1);
                                        if let Some(remainder_item) = remainder
                                        {
                                            let remainder_stack = ItemStack::new(1, remainder_item);
                                            if items_guard[1].is_empty() {
                                                items_guard[1] = remainder_stack;
                                            } else {
                                                let pos = self.position.to_centered_f64();
                                                let entity = $crate::entity::Entity::new(
                                                    world.clone(),
                                                    pos,
                                                    &pumpkin_data::entity::EntityType::ITEM,
                                                );
                                                world.spawn_entity(std::sync::Arc::new(
                                                    $crate::entity::item::ItemEntity::new(
                                                        entity,
                                                        remainder_stack,
                                                    ),
                                                ));
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }

                    if self.is_burning() && can_accept_output {
                        let mut start_cancelled = false;
                        if self.get_cooking_time_spent() == 0 {
                            // An input set before the recipe manager was known got no cook time
                            if self.get_cooking_total_time() == 0
                                && let Some(recipe) = &furnace_recipe
                            {
                                self.set_cooking_total_time(recipe.cooking_time as u16);
                            }
                            if let Some(server) = world.server.upgrade() {
                                let mut start_event = $crate::plugin::api::events::inventory::furnace_start_smelt::FurnaceStartSmeltEvent::new(
                                    self.position,
                                    top_item.item.registry_key.to_string(),
                                    self.get_cooking_total_time() as u32,
                                    top_item.clone(),
                                );
                                server.plugin_manager.fire_blocking(&server, &mut start_event);
                                if start_event.cancelled {
                                    start_cancelled = true;
                                } else {
                                    self.set_cooking_total_time(start_event.cooking_time as u16);
                                }
                            }
                        }
                        if !start_cancelled {
                            self.cooking_time_spent.fetch_add(1, Ordering::Relaxed);

                            if self.get_cooking_time_spent() == self.get_cooking_total_time() {
                                self.set_cooking_time_spent(0);
                                if let Some(cooking_recipe) = furnace_recipe
                                    && let Some(mut result) = recipe_result
                                {
                                    let cooking_total_time = cooking_recipe.cooking_time;
                                    self.set_cooking_total_time(cooking_total_time as u16);

                                    let mut smelt_cancelled = false;
                                    if let Some(server) = world.server.upgrade() {
                                        let mut smelt_event = $crate::plugin::api::events::inventory::furnace_smelt::FurnaceSmeltEvent::new(
                                            self.position,
                                            top_item.item.registry_key.to_string(),
                                            format!(
                                                "minecraft:{}",
                                                cooking_recipe.result.item.registry_key
                                            ),
                                            top_item,
                                            result,
                                        );
                                        server.plugin_manager.fire_blocking(&server, &mut smelt_event);
                                        if smelt_event.cancelled {
                                            smelt_cancelled = true;
                                        }
                                        result = smelt_event.result;
                                    }
                                    if !smelt_cancelled {
                                        self.craft_recipe(Some(&cooking_recipe), &result);
                                        is_dirty = true;
                                    }
                                }
                            }
                        }
                    } else {
                        self.set_cooking_time_spent(0);
                    }
                } else if !self.is_burning() && self.get_cooking_time_spent() > 0 {
                    let _ = self.cooking_time_spent.try_update(
                        Ordering::Acquire,
                        Ordering::Acquire,
                        |v| {
                            Some(
                                v.saturating_sub(2)
                                    .min(self.cooking_total_time.load(Ordering::Acquire)),
                            )
                        },
                    );
                }

                if is_burning != self.is_burning() {
                    is_dirty = true;
                    let (furnace_block, furnace_block_state) =
                        world.get_block_and_state(&self.position);
                    let mut props =
                        pumpkin_data::block_properties::FurnaceLikeProperties::from_state_id(furnace_block_state.id);

                    props.lit = self.is_burning();
                    world.set_block_state(
                        &self.position,
                        props.to_state_id(furnace_block),
                        $crate::world::BlockFlags::NOTIFY_ALL,
                    );
                }

                if is_dirty {
                    self.mark_dirty();
                }
            }

            fn resource_location(&self) -> &'static str {
                Self::ID
            }

            fn get_position(&self) -> BlockPos {
                self.position
            }

            fn from_nbt(nbt: &pumpkin_nbt::compound::NbtCompound, position: BlockPos) -> Self
            where
                Self: Sized,
            {
                let cooking_total_time = AtomicU16::new(
                    nbt.get_short("cooking_total_time")
                        .map_or(0, |cooking_total_time| cooking_total_time as u16),
                );
                let cooking_time_spent = AtomicU16::new(
                    nbt.get_short("cooking_time_spent")
                        .map_or(0, |cooking_time_spent| cooking_time_spent as u16),
                );
                let lit_total_time = AtomicU16::new(
                    nbt.get_short("lit_total_time")
                        .map_or(0, |lit_total_time| lit_total_time as u16),
                );
                let lit_time_remaining = AtomicU16::new(
                    nbt.get_short("lit_time_remaining")
                        .map_or(0, |lit_time_remaining| lit_time_remaining as u16),
                );
                // Load RecipesUsed from NBT (vanilla format: map of recipe ID -> craft count)
                let mut recipes_used_map = HashMap::new();
                if let Some(recipes_compound) = nbt.get_compound("RecipesUsed") {
                    for (recipe_id, tag) in &recipes_compound.child_tags {
                        if let pumpkin_nbt::tag::NbtTag::Int(count) = tag {
                            recipes_used_map.insert(recipe_id.to_string(), *count as u32);
                        }
                    }
                }

                let mut furnace = Self {
                    position,
                    dirty: AtomicBool::new(false),
                    comparator_dirty: AtomicBool::new(false),
                    items: std::sync::RwLock::new(from_fn(|_| ItemStack::EMPTY.clone())),
                    cooking_total_time,
                    cooking_time_spent,
                    lit_total_time,
                    lit_time_remaining,
                    recipes_used: std::sync::Mutex::new(recipes_used_map),
                    recipe_manager: std::sync::OnceLock::new(),
                };
                pumpkin_inventory::sync_read_items_from_nbt(nbt, furnace.items.get_mut().unwrap_or_else(std::sync::PoisonError::into_inner));

                furnace
            }

            fn write_nbt(&self, nbt: &mut pumpkin_nbt::compound::NbtCompound) {
                nbt.put_short("cooking_total_time", self.get_cooking_total_time() as i16);
                nbt.put_short("cooking_time_spent", self.get_cooking_time_spent() as i16);
                nbt.put_short("lit_total_time", self.get_lit_total_time() as i16);
                nbt.put_short("lit_time_remaining", self.get_lit_time_remaining() as i16);

                // Save RecipesUsed in vanilla format (map of recipe ID -> craft count)
                {
                    let recipes = self
                        .recipes_used
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    if !recipes.is_empty() {
                        let mut recipes_compound = pumpkin_nbt::compound::NbtCompound::new();
                        for (recipe_id, count) in recipes.iter() {
                            recipes_compound.put(
                                recipe_id.as_str(),
                                pumpkin_nbt::tag::NbtTag::Int(*count as i32),
                            );
                        }
                        nbt.put(
                            "RecipesUsed",
                            pumpkin_nbt::tag::NbtTag::Compound(recipes_compound),
                        );
                    }
                }

                self.write_inventory_nbt(nbt, true);
            }

            fn get_inventory(
                self: Arc<Self>,
            ) -> Option<Arc<dyn pumpkin_inventory::Inventory>> {
                Some(self)
            }

            fn is_comparator_dirty(&self) -> bool {
                self.comparator_dirty.load(Ordering::Relaxed)
            }

            fn clear_comparator_dirty(&self) {
                self.comparator_dirty.store(false, Ordering::Relaxed);
            }


            fn is_dirty(&self) -> bool {
                self.dirty.load(Ordering::Relaxed)
            }

            fn chunk_data_nbt(&self) -> Option<pumpkin_nbt::compound::NbtCompound> {
                let mut nbt = pumpkin_nbt::compound::NbtCompound::new();
                nbt.put_short("cooking_total_time", self.get_cooking_total_time() as i16);
                nbt.put_short("cooking_time_spent", self.get_cooking_time_spent() as i16);
                nbt.put_short("lit_total_time", self.get_lit_total_time() as i16);
                nbt.put_short("lit_time_remaining", self.get_lit_time_remaining() as i16);

                if let Ok(recipes) = self.recipes_used.lock() {
                    if !recipes.is_empty() {
                        let mut recipes_compound = pumpkin_nbt::compound::NbtCompound::new();
                        for (recipe_id, count) in recipes.iter() {
                            recipes_compound.put(
                                recipe_id.as_str(),
                                pumpkin_nbt::tag::NbtTag::Int(*count as i32),
                            );
                        }
                        nbt.put(
                            "RecipesUsed",
                            pumpkin_nbt::tag::NbtTag::Compound(recipes_compound),
                        );
                    }
                }

                if let Ok(guard) = self.items.try_read() {
                    pumpkin_inventory::sync_write_items_to_nbt(&*guard, &mut nbt);
                }
                Some(nbt)
            }

            fn bedrock_block_actor_data(&self, _state_id: pumpkin_data::BlockStateId) -> Option<pumpkin_nbt::compound::NbtCompound> {
                let bedrock_id = match Self::RECIPE_KIND {
                    pumpkin_data::recipes::CookingRecipeKind::Smelting => "Furnace",
                    pumpkin_data::recipes::CookingRecipeKind::Blasting => "BlastFurnace",
                    pumpkin_data::recipes::CookingRecipeKind::Smoking => "Smoker",
                    _ => "Furnace",
                };
                Some($crate::block::entities::bedrock::build_furnace_actor(
                    bedrock_id,
                    self.position,
                    self.get_cooking_time_spent(),
                    self.get_lit_time_remaining(),
                    self.get_lit_total_time(),
                    0,
                    None,
                ))
            }

            fn as_any(&self) -> &dyn std::any::Any {
                self
            }

            fn to_property_delegate(
                self: Arc<Self>,
            ) -> Option<Arc<dyn $crate::block::entities::PropertyDelegate>> {
                Some(self as Arc<dyn $crate::block::entities::PropertyDelegate>)
            }

            fn to_experience_container(
                self: Arc<Self>,
            ) -> Option<
                Arc<dyn $crate::block::entities::furnace_like_block_entity::ExperienceContainer>,
            > {
                Some(
                    self as Arc<
                        dyn $crate::block::entities::furnace_like_block_entity::ExperienceContainer,
                    >,
                )
            }
        }
    };
}
