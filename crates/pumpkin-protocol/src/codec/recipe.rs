use pumpkin_data::recipes::{CookingRecipeKind, RecipeCategoryTypes};

use pumpkin_data::data_component::DataComponent;
use pumpkin_data::data_component_impl::DataComponentImpl;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::tag::Taggable;

#[derive(Clone, Debug)]
pub enum OwnedRecipeIngredient {
    Simple(String),
    Tagged(String),
    OneOf(Vec<String>),
    /// A stack with the same item and components; the count is ignored.
    Exact(ItemStack),
    /// An item whose custom data has `key` in the `namespace` compound, holding the string
    /// `value` when one is given.
    CustomData {
        item: String,
        namespace: String,
        key: String,
        value: Option<String>,
    },
}

impl OwnedRecipeIngredient {
    /// Checks the item only. Stack ingredients match every stack of their item here.
    #[must_use]
    pub fn match_item(&self, item: &Item) -> bool {
        match self {
            Self::Simple(id) | Self::CustomData { item: id, .. } => {
                let name = format!("minecraft:{}", item.registry_key);
                name == *id
            }
            Self::Tagged(tag) => item.is_tagged_with(tag).unwrap_or(false),
            Self::OneOf(ids) => {
                let name = format!("minecraft:{}", item.registry_key);
                ids.contains(&name)
            }
            Self::Exact(stack) => stack.item.id == item.id,
        }
    }

    /// Checks the whole stack, including the components that stack ingredients look at.
    #[must_use]
    pub fn match_stack(&self, stack: &ItemStack) -> bool {
        match self {
            Self::Exact(expected) => expected.are_items_and_components_equal(stack),
            Self::CustomData {
                namespace,
                key,
                value,
                ..
            } => {
                self.match_item(stack.item)
                    && stack.get_custom_data(namespace, key).is_some_and(|tag| {
                        value
                            .as_deref()
                            .is_none_or(|value| tag.extract_string() == Some(value))
                    })
            }
            Self::Simple(_) | Self::Tagged(_) | Self::OneOf(_) => self.match_item(stack.item),
        }
    }

    /// Whether this ingredient looks at the components of a stack, not just its item.
    #[must_use]
    pub const fn is_stack_ingredient(&self) -> bool {
        matches!(self, Self::Exact(_) | Self::CustomData { .. })
    }
}

#[derive(Clone, Debug)]
pub struct OwnedRecipeResult {
    pub item_id: String,
    pub count: u8,
    /// Component patch of the result on top of the item's defaults (vanilla `ItemStackTemplate`).
    pub components: Vec<(DataComponent, Option<Box<dyn DataComponentImpl>>)>,
}

impl OwnedRecipeResult {
    /// Creates the stack this result describes, or an empty stack for an unknown item.
    #[must_use]
    pub fn to_item_stack(&self) -> ItemStack {
        let key = self
            .item_id
            .strip_prefix("minecraft:")
            .unwrap_or(&self.item_id);
        let item = Item::from_registry_key(key).unwrap_or(&Item::AIR);
        ItemStack::new_with_component(self.count, item, self.components.clone())
    }
}

#[derive(Clone, Debug)]
pub enum OwnedCraftingRecipe {
    Shaped {
        recipe_id: Option<String>,
        category: RecipeCategoryTypes,
        group: Option<String>,
        show_notification: bool,
        key: Vec<(char, OwnedRecipeIngredient)>,
        pattern: Vec<String>,
        result: OwnedRecipeResult,
    },
    Shapeless {
        recipe_id: Option<String>,
        category: RecipeCategoryTypes,
        group: Option<String>,
        ingredients: Vec<OwnedRecipeIngredient>,
        result: OwnedRecipeResult,
    },
}

impl OwnedCraftingRecipe {
    /// Whether any ingredient looks at stack components. Such recipes are matched before the
    /// vanilla ones, so a stack a plugin recipe claims never crafts as its plain item.
    #[must_use]
    pub fn has_stack_ingredients(&self) -> bool {
        match self {
            Self::Shaped { key, .. } => key.iter().any(|(_, ing)| ing.is_stack_ingredient()),
            Self::Shapeless { ingredients, .. } => ingredients
                .iter()
                .any(OwnedRecipeIngredient::is_stack_ingredient),
        }
    }
}

#[derive(Clone, Debug)]
pub struct OwnedCookingRecipe {
    pub recipe_id: String,
    pub category: RecipeCategoryTypes,
    pub group: Option<String>,
    pub ingredient: OwnedRecipeIngredient,
    pub cooking_time: i32,
    pub experience: f32,
    pub result: OwnedRecipeResult,
}

#[derive(Clone, Debug)]
pub enum OwnedCookingRecipeType {
    Blasting(OwnedCookingRecipe),
    Smelting(OwnedCookingRecipe),
    Smoking(OwnedCookingRecipe),
    CampfireCooking(OwnedCookingRecipe),
}

impl OwnedCookingRecipeType {
    /// The recipe, whatever station cooks it.
    #[must_use]
    pub const fn recipe(&self) -> &OwnedCookingRecipe {
        match self {
            Self::Blasting(recipe)
            | Self::Smelting(recipe)
            | Self::Smoking(recipe)
            | Self::CampfireCooking(recipe) => recipe,
        }
    }

    /// The recipe if `kind` cooks it.
    #[must_use]
    pub const fn of_kind(&self, kind: &CookingRecipeKind) -> Option<&OwnedCookingRecipe> {
        match (self, kind) {
            (Self::Blasting(recipe), CookingRecipeKind::Blasting)
            | (Self::Smelting(recipe), CookingRecipeKind::Smelting)
            | (Self::Smoking(recipe), CookingRecipeKind::Smoking)
            | (Self::CampfireCooking(recipe), CookingRecipeKind::CampfireCooking) => Some(recipe),
            _ => None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct OwnedBrewingRecipe {
    pub recipe_id: String,
    pub input_item: String,
    pub input_potion: Option<String>,
    pub reagent: String,
    pub output_item: String,
    pub output_potion: Option<String>,
}

#[derive(Clone, Debug)]
pub struct OwnedStonecutterRecipe {
    pub recipe_id: String,
    pub group: Option<String>,
    pub ingredient: OwnedRecipeIngredient,
    pub result: OwnedRecipeResult,
}

#[derive(Clone, Debug)]
pub struct OwnedSmithingTransformRecipe {
    pub recipe_id: String,
    pub template: OwnedRecipeIngredient,
    pub base: OwnedRecipeIngredient,
    pub addition: OwnedRecipeIngredient,
    pub result: OwnedRecipeResult,
}

#[derive(Clone, Debug)]
pub struct OwnedSmithingTrimRecipe {
    pub recipe_id: String,
    pub template: OwnedRecipeIngredient,
    pub base: OwnedRecipeIngredient,
    pub addition: OwnedRecipeIngredient,
    pub pattern: String,
}

#[derive(Clone, Debug)]
pub struct OwnedSpecialCraftingRecipe {
    pub recipe_id: String,
    pub category: RecipeCategoryTypes,
}

#[derive(Clone, Debug)]
pub enum DynamicRecipe {
    Crafting(OwnedCraftingRecipe),
    Cooking(OwnedCookingRecipeType),
    Brewing(OwnedBrewingRecipe),
    Stonecutting(OwnedStonecutterRecipe),
    SmithingTransform(OwnedSmithingTransformRecipe),
    SmithingTrim(OwnedSmithingTrimRecipe),
    SpecialCrafting(OwnedSpecialCraftingRecipe),
}

impl DynamicRecipe {
    #[must_use]
    pub fn recipe_id(&self) -> Option<&str> {
        match self {
            Self::Crafting(
                OwnedCraftingRecipe::Shaped { recipe_id, .. }
                | OwnedCraftingRecipe::Shapeless { recipe_id, .. },
            ) => recipe_id.as_deref(),
            Self::Cooking(
                OwnedCookingRecipeType::Blasting(r)
                | OwnedCookingRecipeType::Smelting(r)
                | OwnedCookingRecipeType::Smoking(r)
                | OwnedCookingRecipeType::CampfireCooking(r),
            ) => Some(&r.recipe_id),
            Self::Brewing(r) => Some(&r.recipe_id),
            Self::Stonecutting(r) => Some(&r.recipe_id),
            Self::SmithingTransform(r) => Some(&r.recipe_id),
            Self::SmithingTrim(r) => Some(&r.recipe_id),
            Self::SpecialCrafting(r) => Some(&r.recipe_id),
        }
    }
}
