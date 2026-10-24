//! Biomes registered at runtime, next to the generated vanilla [`Biome`]s.
//!
//! Registered biomes only exist on clients: the server stores, simulates and saves a vanilla
//! biome in their place and shows them where a plugin asks for it. The configuration sync sends
//! them after the vanilla ones, sorted by id, so a registered biome's network id is the vanilla
//! biome count plus its index in that order. That holds while these are the only custom biomes:
//! datapacks do not load biomes.

use std::collections::BTreeSet;
use std::sync::{PoisonError, RwLock};

use crate::biome::Biome;

static DYNAMIC_BIOMES: RwLock<BTreeSet<String>> = RwLock::new(BTreeSet::new());

/// Registers a biome under its namespaced id.
pub fn register_dynamic_biome(id: String) {
    DYNAMIC_BIOMES
        .write()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(id);
}

/// Removes a registered biome.
pub fn unregister_dynamic_biome(id: &str) {
    DYNAMIC_BIOMES
        .write()
        .unwrap_or_else(PoisonError::into_inner)
        .remove(id);
}

/// The number of registered biomes.
#[must_use]
pub fn dynamic_biome_count() -> usize {
    DYNAMIC_BIOMES
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .len()
}

/// Returns the network id of the vanilla or registered biome with this namespaced id.
#[must_use]
pub fn biome_network_id(id: &str) -> Option<u8> {
    if let Some(biome) = id.strip_prefix("minecraft:").and_then(Biome::from_name) {
        return Some(biome.id);
    }
    let biomes = DYNAMIC_BIOMES
        .read()
        .unwrap_or_else(PoisonError::into_inner);
    if !biomes.contains(id) {
        return None;
    }
    let index = biomes
        .range::<str, _>((std::ops::Bound::Unbounded, std::ops::Bound::Excluded(id)))
        .count();
    u8::try_from(Biome::ALL.len() + index).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registered_biomes_follow_the_vanilla_ids_in_id_order() {
        register_dynamic_biome("test_b:biome".to_string());
        register_dynamic_biome("test_a:biome".to_string());

        let vanilla_count = Biome::ALL.len();
        assert_eq!(biome_network_id("test_a:biome").map(usize::from), Some(vanilla_count));
        assert_eq!(biome_network_id("test_b:biome").map(usize::from), Some(vanilla_count + 1));
        assert_eq!(biome_network_id("minecraft:swamp"), Some(Biome::SWAMP.id));

        unregister_dynamic_biome("test_a:biome");
        unregister_dynamic_biome("test_b:biome");
        assert_eq!(biome_network_id("test_a:biome"), None);
    }
}
