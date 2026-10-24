//! Biomes players are shown in place of the stored ones: biomes plugins register for clients,
//! and the biome cells that show them.

use std::io::Cursor;

use pumpkin_data::biome::Biome;
use pumpkin_data::environment_attribute::EnvironmentAttribute;
use pumpkin_data::packet::CURRENT_MC_VERSION;
use pumpkin_data::registry::Registry;
use pumpkin_nbt::Nbt;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_nbt::deserializer::NbtReadHelperJava;
use pumpkin_world::chunk::palette::BIOME_NETWORK_MAX_BITS;

use crate::data::datapack::BIOME_REGISTRY;

/// Chunk data writes direct biome palettes with this many bits per cell, and clients read them
/// with as many bits as their biome registry needs, so it may hold at most this many biomes.
pub(crate) const MAX_SYNCED_BIOMES: usize = 1 << BIOME_NETWORK_MAX_BITS;
/// The largest color the biome codec reads as RGB.
const MAX_RGB_COLOR: u32 = 0x00FF_FFFF;

/// How a plugin biome looks. Colors are RGB (`0xRRGGBB`); a color left out keeps the base
/// biome's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BiomeEffects {
    pub water_color: u32,
    pub water_fog_color: u32,
    pub grass_color: Option<u32>,
    pub foliage_color: Option<u32>,
    pub sky_color: Option<u32>,
    pub fog_color: Option<u32>,
}

/// Builds the synced definition of a plugin biome: the definition of the vanilla biome `base`,
/// with the colors of `effects`.
pub(crate) fn biome_definition(
    base: &Biome,
    effects: &BiomeEffects,
) -> Result<NbtCompound, String> {
    let base_id = format!("minecraft:{}", base.registry_id);
    let data = Registry::get_synced(CURRENT_MC_VERSION)
        .into_iter()
        .find(|registry| registry.registry_id == BIOME_REGISTRY)
        .and_then(|registry| {
            registry
                .registry_entries
                .into_iter()
                .find(|entry| entry.entry_id == base_id)
        })
        .and_then(|entry| entry.data)
        .ok_or_else(|| format!("Biome {base_id} has no synced definition"))?;
    let mut definition = Nbt::read_unnamed(&mut NbtReadHelperJava::new(Cursor::new(&data[..])))
        .map_err(|error| format!("Biome {base_id} has an unreadable definition: {error}"))?
        .root_tag;

    let mut visuals = definition
        .get_compound("effects")
        .cloned()
        .unwrap_or_default();
    visuals.put_string("water_color", rgb_color(effects.water_color)?);
    if let Some(grass_color) = effects.grass_color {
        visuals.put_string("grass_color", rgb_color(grass_color)?);
        // A modifier (swamp, dark forest) replaces the grass color on the client.
        visuals.child_tags.remove("grass_color_modifier");
    }
    if let Some(foliage_color) = effects.foliage_color {
        visuals.put_string("foliage_color", rgb_color(foliage_color)?);
    }
    definition.put_compound("effects", visuals);

    let mut attributes = definition
        .get_compound("attributes")
        .cloned()
        .unwrap_or_default();
    for (attribute, color) in [
        (
            EnvironmentAttribute::VisualWaterFogColor,
            Some(effects.water_fog_color),
        ),
        (EnvironmentAttribute::VisualSkyColor, effects.sky_color),
        (EnvironmentAttribute::VisualFogColor, effects.fog_color),
    ] {
        if let Some(color) = color {
            attributes.put_string(attribute.resource_location(), rgb_color(color)?);
        }
    }
    definition.put_compound("attributes", attributes);
    Ok(definition)
}

/// A color in the form the vanilla biome data uses, `#rrggbb`.
fn rgb_color(color: u32) -> Result<String, String> {
    if color > MAX_RGB_COLOR {
        return Err(format!("Biome color {color:#x} is not an RGB color"));
    }
    Ok(format!("#{color:06x}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugin_biome_keeps_the_base_climate_and_takes_its_own_colors() {
        let effects = BiomeEffects {
            water_color: 0x12_3456,
            water_fog_color: 0x65_4321,
            grass_color: Some(0xab_cdef),
            foliage_color: None,
            sky_color: None,
            fog_color: None,
        };
        let definition = biome_definition(&Biome::SWAMP, &effects).unwrap();

        assert!(definition.has("temperature"));
        assert!(definition.has("has_precipitation"));
        let visuals = definition.get_compound("effects").unwrap();
        assert_eq!(visuals.get_string("water_color"), Some("#123456"));
        assert_eq!(visuals.get_string("grass_color"), Some("#abcdef"));
        assert!(!visuals.has("grass_color_modifier"));
        assert!(visuals.has("foliage_color"), "the swamp's foliage color stays");
        let attributes = definition.get_compound("attributes").unwrap();
        assert_eq!(
            attributes.get_string("minecraft:visual/water_fog_color"),
            Some("#654321")
        );
        assert!(attributes.has("minecraft:audio/background_music"));

        assert!(
            biome_definition(
                &Biome::SWAMP,
                &BiomeEffects {
                    water_color: 0x0100_0000,
                    ..effects
                }
            )
            .is_err()
        );
    }
}
