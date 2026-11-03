//! Vanilla `SulfurCubeArchetype`, read from the `minecraft:sulfur_cube_archetype` registry so
//! datapacks change it like they do in vanilla.

use std::io::Cursor;

use pumpkin_data::attributes::Attributes;
use pumpkin_data::damage::DamageType;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::packet::CURRENT_MC_VERSION;
use pumpkin_data::registry::{Registry, RegistryEntryData};
use pumpkin_data::sound::Sound;
use pumpkin_data::tag::Taggable;
use pumpkin_nbt::Nbt;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_nbt::deserializer::NbtReadHelperJava;
use pumpkin_nbt::tag::NbtTag;
use rand::RngExt;

use crate::entity::attributes::ModifierOperation;
use crate::world::World;

const SULFUR_CUBE_ARCHETYPE_REGISTRY: &str = "minecraft:sulfur_cube_archetype";

/// Vanilla `SulfurCubeArchetype.DEFAULT_KNOCKBACK_MODIFIERS`.
pub const DEFAULT_KNOCKBACK_MODIFIERS: KnockbackModifiers = KnockbackModifiers {
    horizontal_power: 0.33,
    vertical_power: 0.06,
};

/// Vanilla `SulfurCubeArchetype.DEFAULT_SOUND_SETTINGS`.
pub const DEFAULT_SOUND_SETTINGS: SoundSettings = SoundSettings {
    hit_sound: Sound::EntitySulfurCubeRegularHit,
    push_sound: Sound::EntitySulfurCubeRegularPush,
    push_sound_impulse_threshold: 0.2,
    push_sound_cooldown: 0.5,
};

/// Vanilla `SulfurCubeArchetype.KnockbackModifiers`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct KnockbackModifiers {
    pub horizontal_power: f32,
    pub vertical_power: f32,
}

/// Vanilla `SulfurCubeArchetype.SoundSettings`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SoundSettings {
    pub hit_sound: Sound,
    pub push_sound: Sound,
    pub push_sound_impulse_threshold: f32,
    pub push_sound_cooldown: f32,
}

/// Vanilla `SulfurCubeArchetype.ExplosionData`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExplosionData {
    pub power: i32,
    pub causes_fire: bool,
    pub fuse: i32,
}

/// The `FloatProvider` of a contact damage: the kinds a datapack is likely to use.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ContactAmount {
    Constant(f32),
    Uniform {
        min_inclusive: f32,
        max_exclusive: f32,
    },
}

impl ContactAmount {
    /// Vanilla `FloatProvider.sample`.
    #[must_use]
    pub fn sample(self) -> f32 {
        match self {
            Self::Constant(value) => value,
            Self::Uniform {
                min_inclusive,
                max_exclusive,
            } => {
                if min_inclusive >= max_exclusive {
                    min_inclusive
                } else {
                    rand::rng().random_range(min_inclusive..max_exclusive)
                }
            }
        }
    }

    fn parse(tag: &NbtTag) -> Option<Self> {
        if let Some(value) = tag.as_numeric_float() {
            return Some(Self::Constant(value));
        }
        let compound = tag.extract_compound()?;
        let kind = compound.get_string("type")?;
        match kind.strip_prefix("minecraft:").unwrap_or(kind) {
            "constant" => Some(Self::Constant(numeric_float(compound, "value")?)),
            "uniform" => Some(Self::Uniform {
                min_inclusive: numeric_float(compound, "min_inclusive")?,
                max_exclusive: numeric_float(compound, "max_exclusive")?,
            }),
            other => {
                tracing::warn!("Sulfur cube contact damage: unsupported float provider {other}");
                None
            }
        }
    }
}

/// Vanilla `SulfurCubeArchetype.ContactDamage`.
#[derive(Clone, Copy, Debug)]
pub struct ContactDamage {
    pub damage_type: DamageType,
    pub amount: ContactAmount,
    pub attribute_to_source: bool,
}

/// Vanilla `SulfurCubeArchetype.AttributeEntry`.
#[derive(Clone, Debug)]
pub struct AttributeEntry {
    pub attribute: &'static Attributes,
    pub id: String,
    pub amount: f64,
    pub operation: ModifierOperation,
}

/// Vanilla `SulfurCubeArchetype`: what a block in a sulfur cube's body does to it.
#[derive(Clone, Debug)]
pub struct SulfurCubeArchetype {
    items: Vec<String>,
    pub attribute_modifiers: Vec<AttributeEntry>,
    pub buoyant: bool,
    pub explosion: Option<ExplosionData>,
    pub contact_damage: Option<ContactDamage>,
    pub knockback_modifiers: KnockbackModifiers,
    pub sound_settings: SoundSettings,
}

impl SulfurCubeArchetype {
    /// `stack.is(arch.items())`.
    fn matches(&self, item: &Item) -> bool {
        self.items.iter().any(|holder| {
            if holder.starts_with('#') {
                item.is_tagged_with(holder).unwrap_or(false)
            } else {
                holder.strip_prefix("minecraft:").unwrap_or(holder) == item.registry_key
            }
        })
    }

    /// `DIRECT_CODEC`, from the unnamed NBT of a registry entry. `None` for an entry that does
    /// not decode, which vanilla refuses at load.
    fn parse(data: &[u8]) -> Option<Self> {
        let root = Nbt::read_unnamed(&mut NbtReadHelperJava::new(Cursor::new(data)))
            .ok()?
            .root_tag;
        Some(Self {
            items: parse_holder_set(root.get("items")?),
            attribute_modifiers: root
                .get_list("attribute_modifiers")?
                .iter()
                .filter_map(parse_attribute_entry)
                .collect(),
            buoyant: root.get_bool("buoyant").unwrap_or(false),
            explosion: root.get_compound("explosion").and_then(parse_explosion),
            contact_damage: root
                .get_compound("contact_damage")
                .and_then(parse_contact_damage),
            knockback_modifiers: parse_knockback(root.get_compound("knockback_modifiers")?)?,
            sound_settings: parse_sound_settings(root.get_compound("sound_settings")?)?,
        })
    }
}

/// The state vanilla `SulfurCube.collectEquipmentChanges` derives from the body item.
#[derive(Clone, Debug)]
pub struct BodyEffects {
    pub floats_in_liquids: bool,
    pub explosion: Option<ExplosionData>,
    pub contact_damages: Vec<ContactDamage>,
    pub knockback_modifier: KnockbackModifiers,
    pub sound_settings: SoundSettings,
}

impl Default for BodyEffects {
    fn default() -> Self {
        Self {
            floats_in_liquids: false,
            explosion: None,
            contact_damages: Vec::new(),
            knockback_modifier: DEFAULT_KNOCKBACK_MODIFIERS,
            sound_settings: DEFAULT_SOUND_SETTINGS,
        }
    }
}

/// Vanilla `SulfurCube.matchingArchetypes`.
#[must_use]
pub fn matching_archetypes(world: &World, stack: &ItemStack) -> Vec<SulfurCubeArchetype> {
    if stack.is_empty() {
        return Vec::new();
    }
    archetype_entries(world)
        .iter()
        .filter_map(|entry| SulfurCubeArchetype::parse(entry.data.as_deref()?))
        .filter(|archetype| archetype.matches(stack.item))
        .collect()
}

/// The registry with the datapacks merged in, or vanilla's own without a server.
fn archetype_entries(world: &World) -> Vec<RegistryEntryData> {
    world.server.upgrade().map_or_else(
        || {
            Registry::get_synced(CURRENT_MC_VERSION)
                .into_iter()
                .find(|registry| registry.registry_id == SULFUR_CUBE_ARCHETYPE_REGISTRY)
                .map_or_else(Vec::new, |registry| registry.registry_entries)
        },
        |server| {
            server
                .datapack_manager
                .dynamic_registry_manager
                .get_registry_entries(SULFUR_CUBE_ARCHETYPE_REGISTRY)
        },
    )
}

fn numeric_float(compound: &NbtCompound, name: &str) -> Option<f32> {
    compound.get(name)?.as_numeric_float()
}

/// `RegistryCodecs.holderSet`: a tag, an id or a list of ids.
fn parse_holder_set(tag: &NbtTag) -> Vec<String> {
    tag.extract_list().map_or_else(
        || {
            tag.extract_string()
                .map(str::to_string)
                .into_iter()
                .collect()
        },
        |list| {
            list.iter()
                .filter_map(NbtTag::extract_string)
                .map(str::to_string)
                .collect()
        },
    )
}

fn parse_attribute_entry(tag: &NbtTag) -> Option<AttributeEntry> {
    let compound = tag.extract_compound()?;
    let name = compound.get_string("attribute")?;
    let attribute = Attributes::ALL.iter().find(|attribute| {
        attribute.name == name || attribute.name.strip_prefix("minecraft:") == Some(name)
    })?;
    Some(AttributeEntry {
        attribute,
        id: compound.get_string("id")?.to_string(),
        amount: compound.get("amount")?.as_numeric_double()?,
        operation: ModifierOperation::from_name(compound.get_string("operation")?)?,
    })
}

fn parse_explosion(compound: &NbtCompound) -> Option<ExplosionData> {
    Some(ExplosionData {
        power: compound.get_int("power")?,
        causes_fire: compound.get_bool("causes_fire")?,
        fuse: compound.get_int("fuse")?,
    })
}

fn parse_contact_damage(compound: &NbtCompound) -> Option<ContactDamage> {
    let name = compound.get_string("damage_type")?;
    Some(ContactDamage {
        damage_type: DamageType::from_name(name.strip_prefix("minecraft:").unwrap_or(name))?,
        amount: ContactAmount::parse(compound.get("amount")?)?,
        attribute_to_source: compound.get_bool("attribute_to_source")?,
    })
}

fn parse_knockback(compound: &NbtCompound) -> Option<KnockbackModifiers> {
    Some(KnockbackModifiers {
        horizontal_power: numeric_float(compound, "horizontal_power")?,
        vertical_power: numeric_float(compound, "vertical_power")?,
    })
}

/// `SoundEvent.CODEC`: an id, or a compound with `sound_id`. Only sounds of the game play.
fn parse_sound(tag: &NbtTag) -> Option<Sound> {
    let name = tag
        .extract_string()
        .or_else(|| tag.extract_compound()?.get_string("sound_id"))?;
    Sound::from_name(name.strip_prefix("minecraft:").unwrap_or(name))
}

fn parse_sound_settings(compound: &NbtCompound) -> Option<SoundSettings> {
    Some(SoundSettings {
        hit_sound: parse_sound(compound.get("hit_sound")?)?,
        push_sound: parse_sound(compound.get("push_sound")?)?,
        push_sound_impulse_threshold: numeric_float(compound, "push_sound_impulse_threshold")?,
        push_sound_cooldown: numeric_float(compound, "push_sound_cooldown")?,
    })
}
