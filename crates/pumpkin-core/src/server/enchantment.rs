use pumpkin_data::enchantment::{AttributeModifierSlot, Enchantment};
use pumpkin_data::item::Item;
use pumpkin_data::packet::CURRENT_MC_VERSION;
use pumpkin_nbt::NbtCompound;
use pumpkin_nbt::tag::NbtTag;
use pumpkin_util::identifier::Identifier;
use pumpkin_util::text::TextComponent;
use rustc_hash::FxHashMap;
use tokio::sync::RwLock;
use tracing::warn;

use crate::data::datapack::ENCHANTMENT_REGISTRY;
use crate::server::Server;

/// Vanilla `Enchantment.MAX_LEVEL`.
const MAX_LEVEL: u32 = 255;
/// The largest weight vanilla's enchantment definition codec accepts (from memory).
const MAX_WEIGHT: u32 = 1024;
/// Custom enchantments have no enchanting costs. Clients never evaluate them, the server does,
/// so the synced definition carries a constant cost.
const SYNCED_COST_BASE: i32 = 1;

#[derive(Clone, Debug)]
pub struct CustomEnchantmentEntry {
    pub id: String,
    pub description: TextComponent,
    pub max_level: u32,
    pub anvil_cost: u32,
    pub supported_items: String,
    pub weight: u32,
    pub slots: Vec<AttributeModifierSlot>,
    pub exclusive_set: Vec<String>,
}

impl CustomEnchantmentEntry {
    /// Builds the definition clients receive in the `minecraft:enchantment` registry. Fails for
    /// a definition a client could not read, which would keep it from joining.
    fn to_synced_definition(&self) -> Result<NbtCompound, String> {
        if !(1..=MAX_LEVEL).contains(&self.max_level) {
            return Err(format!(
                "max level {} is not in 1..={MAX_LEVEL}",
                self.max_level
            ));
        }
        if !(1..=MAX_WEIGHT).contains(&self.weight) {
            return Err(format!("weight {} is not in 1..={MAX_WEIGHT}", self.weight));
        }
        let mut definition = NbtCompound::new();
        definition.put(
            "description",
            self.description.to_nbt_tag_for_version(&CURRENT_MC_VERSION),
        );
        definition.put_string(
            "supported_items",
            synced_supported_items(&self.supported_items)?,
        );
        if let Some(exclusive_set) = self.synced_exclusive_set() {
            definition.put("exclusive_set", exclusive_set);
        }
        definition.put_int("weight", self.weight as i32);
        definition.put_int("max_level", self.max_level as i32);
        definition.put_int(
            "anvil_cost",
            i32::try_from(self.anvil_cost).unwrap_or(i32::MAX),
        );
        definition.put_compound("min_cost", synced_cost());
        definition.put_compound("max_cost", synced_cost());
        definition.put_list(
            "slots",
            self.slots
                .iter()
                .map(|slot| NbtTag::String(slot.to_name().into()))
                .collect(),
        );
        Ok(definition)
    }

    /// The exclusive set as a tag or a list of vanilla enchantments. A client fails on an
    /// enchantment id it does not know, so other ids are left out of the synced definition.
    fn synced_exclusive_set(&self) -> Option<NbtTag> {
        if let [tag] = self.exclusive_set.as_slice()
            && let Some(tag) = tag.strip_prefix('#')
        {
            return match synced_tag(tag) {
                Ok(tag) => Some(NbtTag::String(tag.into())),
                Err(error) => {
                    warn!(
                        "Custom enchantment {} is sent to clients without its exclusive set: {error}",
                        self.id
                    );
                    None
                }
            };
        }
        let mut enchantments = Vec::with_capacity(self.exclusive_set.len());
        for id in &self.exclusive_set {
            if let Some(enchantment) = Enchantment::from_name(id) {
                enchantments.push(NbtTag::String(enchantment.name.into()));
            } else {
                warn!(
                    "Custom enchantment {} is sent to clients without its exclusive entry {id}",
                    self.id
                );
            }
        }
        (!enchantments.is_empty()).then_some(NbtTag::List(enchantments))
    }
}

/// A tag reference (`#namespace:path`) for a tag id without the `#`.
fn synced_tag(tag: &str) -> Result<String, String> {
    let tag = Identifier::parse(tag).map_err(|error| error.to_string())?;
    Ok(format!("#{tag}"))
}

/// `supported_items` as an item tag (`#namespace:path`) or a vanilla item id.
fn synced_supported_items(supported_items: &str) -> Result<String, String> {
    if let Some(tag) = supported_items.strip_prefix('#') {
        return synced_tag(tag);
    }
    Item::from_registry_key(supported_items)
        .map(|item| format!("minecraft:{}", item.registry_key))
        .ok_or_else(|| format!("supported items {supported_items:?} are no item tag or item"))
}

fn synced_cost() -> NbtCompound {
    let mut cost = NbtCompound::new();
    cost.put_int("base", SYNCED_COST_BASE);
    cost.put_int("per_level_above_first", 0);
    cost
}

/// Adds a custom enchantment to the `minecraft:enchantment` registry clients receive, owned by
/// `plugin`. A definition clients could not read stays on the server only.
pub fn sync_custom_enchantment(
    server: &Server,
    plugin: &str,
    enchantment: &CustomEnchantmentEntry,
) {
    let synced = enchantment.to_synced_definition().and_then(|definition| {
        server.datapack_manager.insert_plugin_registry_entry(
            plugin,
            ENCHANTMENT_REGISTRY.to_string(),
            &enchantment.id,
            definition,
        )
    });
    if let Err(error) = synced {
        warn!(
            "Custom enchantment {} is not sent to clients: {error}",
            enchantment.id
        );
    }
}

pub struct EnchantmentManager {
    custom_enchantments: RwLock<FxHashMap<String, CustomEnchantmentEntry>>,
}

impl Default for EnchantmentManager {
    fn default() -> Self {
        Self::new()
    }
}

impl EnchantmentManager {
    #[must_use]
    pub fn new() -> Self {
        Self {
            custom_enchantments: RwLock::new(FxHashMap::default()),
        }
    }

    pub async fn register(&self, enchantment: CustomEnchantmentEntry) -> Result<(), String> {
        let mut map = self.custom_enchantments.write().await;
        if map.contains_key(&enchantment.id) {
            return Err(format!(
                "Enchantment '{}' is already registered",
                enchantment.id
            ));
        }
        map.insert(enchantment.id.clone(), enchantment);
        Ok(())
    }

    /// Removes a custom enchantment.
    pub async fn unregister(&self, id: &str) {
        self.custom_enchantments.write().await.remove(id);
    }

    pub async fn get(&self, id: &str) -> Option<CustomEnchantmentEntry> {
        let map = self.custom_enchantments.read().await;
        map.get(id).cloned()
    }

    pub async fn has(&self, id: &str) -> bool {
        let map = self.custom_enchantments.read().await;
        map.contains_key(id)
    }

    pub async fn get_all_ids(&self) -> Vec<String> {
        let map = self.custom_enchantments.read().await;
        map.keys().cloned().collect()
    }
}
