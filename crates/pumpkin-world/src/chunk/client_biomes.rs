//! Biomes players are shown in place of the stored ones.

use std::borrow::Cow;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use pumpkin_data::biome_registry::biome_network_id;
use rustc_hash::FxHashMap;

use super::palette::BiomePalette;

/// A 4x4x4 biome cell of a chunk: x and z from 0 to 3, y counted in cells from the bottom of the
/// chunk.
pub type BiomeCell = (usize, usize, usize);

/// A biome players are shown in one cell instead of the stored one.
#[derive(Clone, Debug)]
pub struct ClientBiome {
    /// The namespaced id of the biome in the synced `minecraft:worldgen/biome` registry.
    pub biome: Arc<str>,
    /// Who set it, so all cells of one owner can be cleared together.
    pub owner: Arc<str>,
}

/// The cells of a chunk that players are shown with another biome than the stored one. They are
/// set at runtime (by plugins), never saved, and gone once the chunk unloads.
#[derive(Default)]
pub struct ClientBiomes {
    /// Whether any cell is set, so encoding a chunk without client biomes takes no lock.
    any: AtomicBool,
    cells: Mutex<FxHashMap<BiomeCell, ClientBiome>>,
}

impl ClientBiomes {
    fn cells(&self) -> MutexGuard<'_, FxHashMap<BiomeCell, ClientBiome>> {
        self.cells.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(crate) fn set(&self, cell: BiomeCell, biome: ClientBiome) {
        let mut cells = self.cells();
        cells.insert(cell, biome);
        self.any.store(true, Ordering::Relaxed);
    }

    pub(crate) fn clear(&self, cell: BiomeCell) {
        let mut cells = self.cells();
        cells.remove(&cell);
        self.any.store(!cells.is_empty(), Ordering::Relaxed);
    }

    /// Returns whether no cell is set.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        !self.any.load(Ordering::Relaxed)
    }

    /// Clears every cell `owner` set. Returns whether there was one.
    #[must_use]
    pub fn clear_owner(&self, owner: &str) -> bool {
        if self.is_empty() {
            return false;
        }
        let mut cells = self.cells();
        let before = cells.len();
        cells.retain(|_, biome| &*biome.owner != owner);
        self.any.store(!cells.is_empty(), Ordering::Relaxed);
        cells.len() != before
    }

    /// The biome sections players are shown: `stored` with the set cells on top. A cell whose
    /// biome is no longer registered shows the stored biome. Borrows `stored` when no cell is
    /// set.
    #[must_use]
    pub fn apply<'a>(&self, stored: &'a [BiomePalette]) -> Cow<'a, [BiomePalette]> {
        if self.is_empty() {
            return Cow::Borrowed(stored);
        }
        let mut sections = stored.to_vec();
        let cells = self.cells();
        for (&(x, y, z), biome) in &*cells {
            if let Some(id) = biome_network_id(&biome.biome)
                && let Some(section) = sections.get_mut(y / BiomePalette::SIZE)
            {
                section.set(x, y % BiomePalette::SIZE, z, id);
            }
        }
        Cow::Owned(sections)
    }
}

#[cfg(test)]
mod tests {
    use pumpkin_data::biome::Biome;

    use super::ClientBiome;
    use crate::chunk::ChunkSections;

    #[test]
    fn client_biomes_cover_the_stored_cell_until_cleared() {
        let sections = ChunkSections::new(24, -64);
        let swamp = ClientBiome {
            biome: "minecraft:swamp".into(),
            owner: "a".into(),
        };
        // Block (5, -44, 9) lies in section 1, cell (1, 1, 2).
        assert!(sections.set_client_biome_absolute_y(5, -44, 9, swamp));
        assert!(!sections.set_client_biome_absolute_y(
            5,
            -65,
            9,
            ClientBiome {
                biome: "minecraft:swamp".into(),
                owner: "a".into(),
            }
        ));

        let stored = sections.biome_sections.read().unwrap();
        let shown = sections.client_biomes.apply(&stored);
        assert_eq!(shown[1].get(1, 1, 2), Biome::SWAMP.id);
        assert_ne!(stored[1].get(1, 1, 2), Biome::SWAMP.id);
        assert_eq!(shown[0].get(1, 1, 2), stored[0].get(1, 1, 2));

        assert!(!sections.client_biomes.clear_owner("b"));
        assert!(sections.client_biomes.clear_owner("a"));
        assert!(sections.client_biomes.is_empty());
    }
}
