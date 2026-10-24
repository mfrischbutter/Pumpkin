use std::sync::RwLock;

use pumpkin_inventory::crafting::recipe_provider::RecipeProvider;
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
