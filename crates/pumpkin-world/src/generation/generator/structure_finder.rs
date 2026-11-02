use pumpkin_data::structures::{
    ConcentricRingsStructurePlacement, RandomSpreadStructurePlacement, StructurePlacement,
    StructurePlacementType, StructureSet,
};
use pumpkin_util::math::{floor_div, position::BlockPos};

use crate::generation::structure::height_sampler::NoiseHeightSampler;
use crate::generation::structure::placement::{
    GlobalStructureCache, StrongholdLayout, get_structure_chunk_in_region,
    should_generate_structure,
};
use crate::generation::structure::references::claiming_structure_start;

use super::WorldGenerator;

/// Block-level position of a found structure plus squared distance from the
/// search origin, used internally to track the running nearest candidate.
#[derive(Debug, Clone)]
pub struct FoundStructure {
    pub pos: BlockPos,
    pub distance_sq: f64,
}

/// Finds the block position of the nearest structure whose placement is listed
/// in `placements`, within `max_search_radius` chunk-region rings.
///
/// Each placement comes with the salt the world places it with
/// ([`WorldGenerator::structure_salt`]); `structure_seed` is the world's
/// [`WorldGenerator::structure_seed`].
///
/// Mirrors the two-pass logic in vanilla's
/// `ChunkGenerator.findNearestMapStructure`:
///
/// 1. **Concentric-rings** placements (strongholds) are resolved in one pass
///    from the pre-computed [`GlobalStructureCache`].
/// 2. **Random-spread** placements are searched ring-by-ring outward, stopping
///    at the first radius that produces any result.
///
/// The best candidate from both passes is returned.
///
/// Returns [`StructureSearch::Pending`] while the stronghold positions are still
/// being computed, so the caller can retry instead of reporting a wrong result.
pub fn find_nearest_structure(
    origin: BlockPos,
    placements: &[(&StructurePlacement, u32)],
    max_search_radius: i32,
    structure_seed: i64,
    global_cache: &GlobalStructureCache,
) -> StructureSearch {
    if placements.is_empty() {
        return StructureSearch::NotFound;
    }

    let mut nearest: Option<FoundStructure> = None;

    // ── Pass 1: Concentric-rings (strongholds) ──────────────────────────────
    for (p, _) in placements {
        if let StructurePlacementType::ConcentricRings(rings) = &p.placement_type {
            match find_nearest_concentric(origin, rings, global_cache) {
                ConcentricSearch::Pending => return StructureSearch::Pending,
                ConcentricSearch::Done(Some(found))
                    if nearest
                        .as_ref()
                        .is_none_or(|n| found.distance_sq < n.distance_sq) =>
                {
                    nearest = Some(found);
                }
                ConcentricSearch::Done(_) => {}
            }
        }
    }

    let random_spread: Vec<(&RandomSpreadStructurePlacement, u32)> = placements
        .iter()
        .filter_map(|(p, salt)| {
            if let StructurePlacementType::RandomSpread(r) = &p.placement_type {
                Some((r, *salt))
            } else {
                None
            }
        })
        .collect();

    if !random_spread.is_empty() {
        let chunk_origin_x = origin.0.x >> 4;
        let chunk_origin_z = origin.0.z >> 4;

        'radius: for radius in 0..=max_search_radius {
            for (placement, salt) in &random_spread {
                if let Some(found) = find_nearest_random_spread_at_radius(
                    origin,
                    chunk_origin_x,
                    chunk_origin_z,
                    radius,
                    structure_seed,
                    placement,
                    *salt,
                ) {
                    if nearest
                        .as_ref()
                        .is_none_or(|n| found.distance_sq < n.distance_sq)
                    {
                        nearest = Some(found);
                    }
                    break 'radius;
                }
            }
        }
    }

    nearest.map_or(StructureSearch::NotFound, |f| StructureSearch::Found(f.pos))
}

/// Result of [`find_nearest_structure`].
#[derive(Debug, Clone)]
pub enum StructureSearch {
    Found(BlockPos),
    NotFound,
    Pending,
}

/// Finds the nearest candidate that actually produces one of `target_structures`.
///
/// Returns the position along with which of the targets was found. Explorer maps
/// and `/locate structure` use this instead of pointing at a placement-only
/// candidate whose biome may reject the requested structure.
#[must_use]
pub fn find_nearest_structure_start(
    origin: BlockPos,
    set_index: usize,
    target_structures: &[pumpkin_data::structures::StructureKeys],
    max_search_radius: i32,
    generator: &WorldGenerator,
) -> Option<(BlockPos, pumpkin_data::structures::StructureKeys)> {
    let WorldGenerator::Noise(noise_generator) = generator else {
        return None;
    };
    let structure_set = &StructureSet::ALL[set_index];
    let StructurePlacementType::RandomSpread(placement) = &structure_set.placement.placement_type
    else {
        return None;
    };

    let chunk_origin_x = origin.0.x >> 4;
    let chunk_origin_z = origin.0.z >> 4;
    let region_origin_x = floor_div(chunk_origin_x, placement.spacing);
    let region_origin_z = floor_div(chunk_origin_z, placement.spacing);
    let world_seed = noise_generator.random_config.structure_seed as i64;
    let salt = noise_generator.structure_salts[set_index];
    let global_cache = &noise_generator.global_structure_cache;
    if !noise_generator.enabled_structure_sets.contains(&set_index) {
        return None;
    }
    let mut height_sampler = NoiseHeightSampler::new(noise_generator);

    for radius in 0..=max_search_radius {
        let mut nearest: Option<(FoundStructure, pumpkin_data::structures::StructureKeys)> = None;
        for region_x_offset in -radius..=radius {
            for region_z_offset in -radius..=radius {
                if region_x_offset.abs() != radius && region_z_offset.abs() != radius {
                    continue;
                }
                let (chunk_x, chunk_z) = get_structure_chunk_in_region(
                    placement,
                    world_seed,
                    region_origin_x + region_x_offset,
                    region_origin_z + region_z_offset,
                    salt,
                );
                if !should_generate_structure(
                    set_index,
                    &noise_generator.structure_calculator,
                    &noise_generator.structure_salts,
                    chunk_x,
                    chunk_z,
                    global_cache,
                ) {
                    continue;
                }

                // The chunk's start is the one chunk generation places there (a set's
                // structures displace each other), computed into the cache generation reads.
                let Some((key, _)) = claiming_structure_start(
                    noise_generator,
                    structure_set,
                    chunk_x,
                    chunk_z,
                    &mut height_sampler,
                ) else {
                    continue;
                };
                if !target_structures.contains(&key) {
                    continue;
                }
                // Vanilla getLocatePos: chunkPos.getMinBlockX(), 0, chunkPos.getMinBlockZ()
                let locate_x = chunk_x << 4;
                let locate_z = chunk_z << 4;
                let position = BlockPos::new(locate_x, 0, locate_z);
                let dx = f64::from(position.0.x - origin.0.x);
                let dz = f64::from(position.0.z - origin.0.z);
                let found = FoundStructure {
                    pos: position,
                    distance_sq: dx * dx + dz * dz,
                };
                if nearest
                    .as_ref()
                    .is_none_or(|(current, _)| found.distance_sq < current.distance_sq)
                {
                    nearest = Some((found, key));
                }
            }
        }
        if let Some((found, key)) = nearest {
            return Some((found.pos, key));
        }
    }
    None
}

fn find_nearest_concentric(
    origin: BlockPos,
    // Kept for potential future bounds / distance validation.
    _rings: &ConcentricRingsStructurePlacement,
    global_cache: &GlobalStructureCache,
) -> ConcentricSearch {
    // No block, callers run in server tasks and the tick, which shutdown waits on
    let strongholds = match global_cache.try_get_stronghold_chunks() {
        StrongholdLayout::Ready(strongholds) => strongholds,
        StrongholdLayout::Pending => return ConcentricSearch::Pending,
        StrongholdLayout::Failed => return ConcentricSearch::Done(None),
    };

    let ox = origin.0.x as f64;
    let oz = origin.0.z as f64;

    strongholds
        .iter()
        .map(|(cx, cz)| {
            // Vanilla getLocatePos: chunkPos.getMinBlockX(), 0, chunkPos.getMinBlockZ()
            let bx = cx << 4;
            let bz = cz << 4;
            let dx = bx as f64 - ox;
            let dz = bz as f64 - oz;
            FoundStructure {
                pos: BlockPos::new(bx, 0, bz),
                distance_sq: dx * dx + dz * dz,
            }
        })
        .min_by(|a, b| a.distance_sq.total_cmp(&b.distance_sq))
        .map_or(ConcentricSearch::Done(None), |found| {
            ConcentricSearch::Done(Some(found))
        })
}

/// Outcome of a stronghold lookup: the ring positions may still be computing.
enum ConcentricSearch {
    Pending,
    Done(Option<FoundStructure>),
}

fn find_nearest_random_spread_at_radius(
    origin: BlockPos,
    chunk_origin_x: i32,
    chunk_origin_z: i32,
    radius: i32,
    world_seed: i64,
    placement: &RandomSpreadStructurePlacement,
    salt: u32,
) -> Option<FoundStructure> {
    let spacing = placement.spacing;
    let ox = origin.0.x as f64;
    let oz = origin.0.z as f64;

    let mut best: Option<FoundStructure> = None;

    for rx_off in -radius..=radius {
        for rz_off in -radius..=radius {
            if rx_off.abs() != radius && rz_off.abs() != radius {
                continue;
            }

            let rx = floor_div(chunk_origin_x, spacing) + rx_off;
            let rz = floor_div(chunk_origin_z, spacing) + rz_off;

            let (struct_cx, struct_cz) =
                get_structure_chunk_in_region(placement, world_seed, rx, rz, salt);

            // Vanilla getLocatePos: chunkPos.getMinBlockX(), 0, chunkPos.getMinBlockZ()
            let bx = struct_cx << 4;
            let bz = struct_cz << 4;
            let dx = bx as f64 - ox;
            let dz = bz as f64 - oz;
            let dist_sq = dx * dx + dz * dz;

            if best.as_ref().is_none_or(|b| dist_sq < b.distance_sq) {
                best = Some(FoundStructure {
                    pos: BlockPos::new(bx, 0, bz),
                    distance_sq: dist_sq,
                });
            }
        }
    }

    best
}

#[cfg(test)]
mod tests {
    use pumpkin_data::dimension::Dimension;
    use pumpkin_data::structures::{StructureKeys, StructureSet};
    use pumpkin_util::math::position::BlockPos;
    use pumpkin_util::world_seed::Seed;

    use super::super::{GeneratorInit, VanillaGenerator, WorldGenerator};
    use super::find_nearest_structure_start;

    /// A nether structure resolves through the nether's biome source: with the
    /// overworld's, no candidate passed the fortress biome test, `/locate` found no
    /// fortress and cached the miss for chunk generation.
    #[test]
    fn nether_structure_starts_resolve() {
        let generator = WorldGenerator::Noise(Box::new(VanillaGenerator::new(
            Seed(13579),
            Dimension::THE_NETHER,
        )));
        let set_index = StructureSet::NAMES
            .iter()
            .position(|name| *name == "nether_complexes")
            .expect("nether_complexes is a structure set");
        let found = find_nearest_structure_start(
            BlockPos::new(0, 64, 0),
            set_index,
            &[StructureKeys::Fortress],
            8,
            &generator,
        );
        assert!(matches!(found, Some((_, StructureKeys::Fortress))));
    }

    /// A set's structures displace each other: where the weighted draw gives the chunk to
    /// the bastion remnant, the search must not report a fortress (the finder once resolved
    /// every requested structure on its own, so `/locate fortress` could lead to a bastion).
    #[test]
    fn a_chunk_another_structure_of_the_set_claims_is_not_found() {
        let generator = WorldGenerator::Noise(Box::new(VanillaGenerator::new(
            Seed(13579),
            Dimension::THE_NETHER,
        )));
        let set_index = StructureSet::NAMES
            .iter()
            .position(|name| *name == "nether_complexes")
            .expect("nether_complexes is a structure set");
        let origin = BlockPos::new(0, 64, 0);
        let fortress = find_nearest_structure_start(
            origin,
            set_index,
            &[StructureKeys::Fortress],
            8,
            &generator,
        );
        let bastion = find_nearest_structure_start(
            origin,
            set_index,
            &[StructureKeys::BastionRemnant],
            8,
            &generator,
        );
        assert!(matches!(fortress, Some((_, StructureKeys::Fortress))));
        assert!(matches!(bastion, Some((_, StructureKeys::BastionRemnant))));
        assert_ne!(
            fortress.map(|(pos, _)| pos),
            bastion.map(|(pos, _)| pos),
            "a chunk holds one structure of its set"
        );
    }
}
