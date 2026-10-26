use std::sync::RwLock;

use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::recipes::CookingRecipeKind;
use pumpkin_inventory::crafting::recipe_provider::RecipeProvider;

use crate::block::entities::furnace_like_block_entity::CookingMatch;
pub use pumpkin_protocol::codec::recipe::DynamicRecipe;

pub struct RecipeManager {
    /// Datapack recipes followed by plugin recipes.
    dynamic_recipes: RwLock<Vec<DynamicRecipe>>,
    /// Recipes plugins registered, kept so a datapack reload does not drop them.
    plugin_recipes: RwLock<Vec<DynamicRecipe>>,
}

impl Default for RecipeManager {
    fn default() -> Self {
        Self::new()
    }
}

impl RecipeManager {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            dynamic_recipes: RwLock::new(Vec::new()),
            plugin_recipes: RwLock::new(Vec::new()),
        }
    }

    /// Adds a plugin recipe. Plugin recipes survive [`Self::set_recipes`].
    pub fn add_recipe(&self, recipe: DynamicRecipe) {
        self.add_recipes([recipe]);
    }

    /// Adds plugin recipes. Plugin recipes survive [`Self::set_recipes`].
    pub fn add_recipes(&self, new_recipes: impl IntoIterator<Item = DynamicRecipe>) {
        let mut plugin_recipes = self
            .plugin_recipes
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut recipes = self
            .dynamic_recipes
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for recipe in new_recipes {
            recipes.push(recipe.clone());
            plugin_recipes.push(recipe);
        }
    }

    /// Replaces the datapack recipes; plugin recipes stay after them.
    pub fn set_recipes(&self, new_recipes: Vec<DynamicRecipe>) {
        let plugin_recipes = self
            .plugin_recipes
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut recipes = self
            .dynamic_recipes
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *recipes = new_recipes;
        recipes.extend(plugin_recipes.iter().cloned());
    }

    pub fn clear(&self) {
        let mut plugin_recipes = self
            .plugin_recipes
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut recipes = self
            .dynamic_recipes
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        plugin_recipes.clear();
        recipes.clear();
    }

    /// Finds the recipe `kind` cooks `input` with. As for crafting, dynamic recipes with stack
    /// ingredients come first, then the generated vanilla recipes, then the other dynamic ones.
    #[must_use]
    pub fn find_cooking_recipe(
        &self,
        input: &ItemStack,
        kind: &CookingRecipeKind,
    ) -> Option<CookingMatch> {
        if input.is_empty() {
            return None;
        }
        let recipes = self
            .dynamic_recipes
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let find_dynamic = |stack_ingredients: bool| {
            recipes.iter().find_map(|recipe| {
                let DynamicRecipe::Cooking(cooking) = recipe else {
                    return None;
                };
                let cooking = cooking.of_kind(kind)?;
                (cooking.ingredient.is_stack_ingredient() == stack_ingredients
                    && cooking.ingredient.match_stack(input))
                .then(|| CookingMatch::from_dynamic(cooking))
            })
        };
        find_dynamic(true)
            .or_else(|| CookingMatch::find_vanilla(input, kind))
            .or_else(|| find_dynamic(false))
    }

    /// The experience a cooking recipe gives, looked up by id like vanilla's `RecipesUsed`.
    #[must_use]
    pub fn cooking_experience(&self, recipe_id: &str) -> Option<f32> {
        let dynamic = self
            .dynamic_recipes
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .find_map(|recipe| {
                let DynamicRecipe::Cooking(cooking) = recipe else {
                    return None;
                };
                let cooking = cooking.recipe();
                (cooking.recipe_id == recipe_id).then_some(cooking.experience)
            });
        dynamic.or_else(|| pumpkin_data::recipes::get_recipe_experience(recipe_id))
    }

    pub fn get_dynamic_recipes_internal(&self) -> Vec<DynamicRecipe> {
        self.dynamic_recipes
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

impl RecipeProvider for RecipeManager {
    fn get_dynamic_recipes(&self) -> Vec<DynamicRecipe> {
        self.dynamic_recipes
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}
