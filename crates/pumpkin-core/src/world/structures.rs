//! Which structure a position lies in, for the spawn lists of vanilla
//! `ChunkGenerator.getMobsAt` and `NaturalSpawner.isInNetherFortressBounds`.

use std::sync::{Arc, LazyLock};

use pumpkin_data::entity::MobCategory;
use pumpkin_data::structures::{
    BoundingBoxType, SpawnEntry, Structure, StructureKeys, StructureSet,
};
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector2::Vector2;
use pumpkin_world::generation::generator::WorldGenerator;
use pumpkin_world::generation::structure::references::{StructureReference, structure_references};

use super::World;

/// The structure sets that hold a structure with spawn overrides: the only starts the
/// spawn lists need.
static SPAWN_OVERRIDE_SETS: LazyLock<Vec<usize>> = LazyLock::new(|| {
    StructureSet::ALL
        .iter()
        .enumerate()
        .filter(|(_, set)| {
            set.structures
                .iter()
                .any(|entry| !Structure::get(&entry.structure).spawn_overrides.is_empty())
        })
        .map(|(index, _)| index)
        .collect()
});

impl World {
    /// Vanilla's structure references of a chunk (`StructureManager.getAllStructuresAt`)
    /// for the structures with spawn overrides, recomputed from the placement for a loaded
    /// chunk and kept until it unloads.
    fn structure_references(&self, chunk_pos: Vector2<i32>) -> Arc<[StructureReference]> {
        if let Some(references) = self.structure_reference_cache.get(&chunk_pos) {
            return references.clone();
        }
        let world_gen = self.level.world_gen.load_full();
        let references = match &*world_gen {
            WorldGenerator::Noise(generator) => {
                let shape = &generator.settings.shape;
                let min_y = i32::from(shape.min_y).max(generator.dimension.min_y);
                let height = shape.height.min(generator.dimension.height as u16);
                let sets = SPAWN_OVERRIDE_SETS
                    .iter()
                    .copied()
                    .filter(|set_index| generator.enabled_structure_sets.contains(set_index));
                structure_references(generator, chunk_pos.x, chunk_pos.y, min_y, height, sets)
            }
            WorldGenerator::Flat(_) | WorldGenerator::Custom(_) => Vec::new(),
        };
        let references: Arc<[StructureReference]> = Arc::from(references);
        self.structure_reference_cache
            .insert(chunk_pos, references.clone());
        references
    }

    /// A chunk unloaded: its references go with it.
    pub(super) fn forget_structure_references(&self, chunk_pos: Vector2<i32>) {
        self.structure_reference_cache.remove(&chunk_pos);
    }

    /// Vanilla `ChunkGenerator.getMobsAt` before its biome fallback: the spawns of the
    /// first structure at `pos` with an override for `category`, when `pos` lies in its
    /// bounding box or, for a `piece` override, in one of its pieces.
    pub fn structure_spawn_override(
        &self,
        category: &'static MobCategory,
        pos: &BlockPos,
    ) -> Option<&'static [SpawnEntry]> {
        let references = self.structure_references(pos.chunk_position());
        for (key, collector) in references.iter() {
            let Some(spawn_override) = Structure::get(key)
                .spawn_overrides
                .iter()
                .find(|spawn_override| spawn_override.category == category.name)
            else {
                continue;
            };
            let mut collector = collector
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let inside = match spawn_override.bounding_box {
                BoundingBoxType::Piece => collector
                    .pieces
                    .iter()
                    .any(|piece| piece.bounding_box().contains_pos(&pos.0)),
                BoundingBoxType::Full => collector
                    .get_adjusted_bounding_box(key)
                    .contains_pos(&pos.0),
            };
            if inside {
                return Some(spawn_override.spawns);
            }
        }
        None
    }

    /// Vanilla `StructureManager.getStructureAt(pos, structure).isValid()`: a start of
    /// `key` whose bounding box holds `pos`.
    pub fn is_inside_structure(&self, key: StructureKeys, pos: &BlockPos) -> bool {
        self.structure_references(pos.chunk_position())
            .iter()
            .filter(|(reference, _)| *reference == key)
            .any(|(_, collector)| {
                collector
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .get_adjusted_bounding_box(&key)
                    .contains_pos(&pos.0)
            })
    }
}
