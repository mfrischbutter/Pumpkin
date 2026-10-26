use super::AccessorExt;
use crate::pumpkin::plugin::recipe::HostRecipeManagerWithStore;

use crate::pumpkin::plugin::recipe::{
    CookingRecipe as WitCookingRecipe, CookingType as WitCookingType, Host as RecipeHost,
    HostRecipeManager, Ingredient as WitIngredient, RecipeCategory as WitRecipeCategory,
    RecipeManager as WitRecipeManager, ShapedRecipe as WitShapedRecipe,
    ShapelessRecipe as WitShapelessRecipe,
};
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::recipes::RecipeCategoryTypes;
use pumpkin_protocol::codec::recipe::{
    DynamicRecipe, OwnedCookingRecipe, OwnedCookingRecipeType, OwnedCraftingRecipe,
    OwnedRecipeIngredient, OwnedRecipeResult,
};
use pumpkin_wasm_host_common::state::PluginHostState;
use wasmtime::component::{Accessor, HasSelf, Resource};

impl RecipeHost for PluginHostState {}

impl HostRecipeManager for PluginHostState {
    fn drop(&mut self, rep: Resource<WitRecipeManager>) -> wasmtime::Result<()> {
        self.drop(rep)
    }
}

impl HostRecipeManagerWithStore<PluginHostState> for HasSelf<PluginHostState> {
    async fn register_shaped(
        accessor: &Accessor<PluginHostState, Self>,
        _res: Resource<WitRecipeManager>,
        id: String,
        recipe: WitShapedRecipe,
    ) -> wasmtime::Result<()> {
        let mut key = Vec::with_capacity(recipe.key.len());
        for (symbol, ing) in recipe.key {
            let ingredient = to_owned_ingredient(accessor, ing).await?;
            key.push((symbol.chars().next().unwrap_or(' '), ingredient));
        }
        let result_stack = accessor.take_res(recipe.output)?;
        let result_stack = result_stack.lock().await;

        let category = recipe
            .category
            .map_or(RecipeCategoryTypes::Misc, to_data_category);

        let owned_recipe = OwnedCraftingRecipe::Shaped {
            recipe_id: Some(id),
            category,
            group: recipe.group,
            show_notification: recipe.show_notification.unwrap_or(true),
            key,
            pattern: recipe.pattern,
            result: to_owned_result(&result_stack),
        };

        let server = accessor.server()?;
        server
            .recipe_manager
            .add_recipe(DynamicRecipe::Crafting(owned_recipe));
        Ok(())
    }

    async fn register_shapeless(
        accessor: &Accessor<PluginHostState, Self>,
        _res: Resource<WitRecipeManager>,
        id: String,
        recipe: WitShapelessRecipe,
    ) -> wasmtime::Result<()> {
        let mut ingredients = Vec::with_capacity(recipe.ingredients.len());
        for ing in recipe.ingredients {
            ingredients.push(to_owned_ingredient(accessor, ing).await?);
        }
        let result_stack = accessor.take_res(recipe.output)?;
        let result_stack = result_stack.lock().await;

        let category = recipe
            .category
            .map_or(RecipeCategoryTypes::Misc, to_data_category);

        let owned_recipe = OwnedCraftingRecipe::Shapeless {
            recipe_id: Some(id),
            category,
            group: recipe.group,
            ingredients,
            result: to_owned_result(&result_stack),
        };

        let server = accessor.server()?;
        server
            .recipe_manager
            .add_recipe(DynamicRecipe::Crafting(owned_recipe));
        Ok(())
    }

    async fn register_cooking(
        accessor: &Accessor<PluginHostState, Self>,
        _res: Resource<WitRecipeManager>,
        id: String,
        station_type: WitCookingType,
        recipe: WitCookingRecipe,
    ) -> wasmtime::Result<()> {
        let ingredient = to_owned_ingredient(accessor, recipe.ingredient).await?;
        let result_stack = accessor.take_res(recipe.output)?;
        let result_stack = result_stack.lock().await;

        let category = recipe
            .category
            .map_or(RecipeCategoryTypes::Misc, to_data_category);

        let owned_cooking = OwnedCookingRecipe {
            recipe_id: id,
            category,
            group: recipe.group,
            ingredient,
            cooking_time: recipe.cooking_time as i32,
            experience: recipe.experience,
            result: to_owned_result(&result_stack),
        };

        let dynamic_recipe = match station_type {
            WitCookingType::Smelting => {
                DynamicRecipe::Cooking(OwnedCookingRecipeType::Smelting(owned_cooking))
            }
            WitCookingType::Blasting => {
                DynamicRecipe::Cooking(OwnedCookingRecipeType::Blasting(owned_cooking))
            }
            WitCookingType::Smoking => {
                DynamicRecipe::Cooking(OwnedCookingRecipeType::Smoking(owned_cooking))
            }
            WitCookingType::Campfire => {
                DynamicRecipe::Cooking(OwnedCookingRecipeType::CampfireCooking(owned_cooking))
            }
        };

        let server = accessor.server()?;
        server.recipe_manager.add_recipe(dynamic_recipe);
        Ok(())
    }
}

const fn to_data_category(cat: WitRecipeCategory) -> RecipeCategoryTypes {
    match cat {
        WitRecipeCategory::Building => RecipeCategoryTypes::Building,
        WitRecipeCategory::Redstone => RecipeCategoryTypes::Restone,
        WitRecipeCategory::Equipment => RecipeCategoryTypes::Equipment,
        WitRecipeCategory::Misc => RecipeCategoryTypes::Misc,
        WitRecipeCategory::Food => RecipeCategoryTypes::Food,
        WitRecipeCategory::Blocks => RecipeCategoryTypes::Blocks,
    }
}

fn to_owned_result(stack: &ItemStack) -> OwnedRecipeResult {
    OwnedRecipeResult {
        item_id: stack.item.registry_key.to_string(),
        count: stack.item_count,
        components: stack.patch.clone(),
    }
}

/// Converts a WIT ingredient, taking the stack of an `exact` ingredient from the table.
async fn to_owned_ingredient(
    accessor: &Accessor<PluginHostState, HasSelf<PluginHostState>>,
    ing: WitIngredient,
) -> wasmtime::Result<OwnedRecipeIngredient> {
    Ok(match ing {
        WitIngredient::Item(id) => OwnedRecipeIngredient::Simple(id),
        WitIngredient::Tag(tag) => OwnedRecipeIngredient::Tagged(tag),
        WitIngredient::OneOf(items) => OwnedRecipeIngredient::OneOf(items),
        WitIngredient::Exact(stack) => {
            OwnedRecipeIngredient::Exact(accessor.take_res(stack)?.lock().await.clone())
        }
        WitIngredient::CustomData(ing) => OwnedRecipeIngredient::CustomData {
            item: ing.item,
            namespace: ing.namespace,
            key: ing.key,
            value: ing.value,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use pumpkin_core::server::RecipeManager;
    use std::sync::Arc;

    #[test]
    fn guest_drop_releases_recipe_manager_resource() {
        let mut state = PluginHostState::new();
        let manager = Arc::new(RecipeManager::new());
        let weak_manager = Arc::downgrade(&manager);
        let resource = state
            .add::<WitRecipeManager>(manager)
            .expect("insert recipe manager resource");
        let rep = resource.rep();

        HostRecipeManager::drop(&mut state, resource).expect("drop recipe manager resource");

        assert!(weak_manager.upgrade().is_none());
        assert!(
            state
                .get(&Resource::<WitRecipeManager>::new_borrow(rep))
                .is_err()
        );
    }
}
