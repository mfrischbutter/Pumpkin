//! The world's points of interest: vanilla `ServerLevel`'s hooks into the `PoiManager`
//! (`pumpkin_world::poi`) and its village tests.

use std::sync::{PoisonError, Weak};

use pumpkin_data::Block;
use pumpkin_data::BlockStateId;
use pumpkin_data::poi_type::PoiType;
use pumpkin_data::villager::VillagerProfession;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector2::Vector2;
use pumpkin_world::chunk::palette::BlockPalette;
use pumpkin_world::poi::{MAX_VILLAGE_DISTANCE, section_of};

use super::World;
use crate::entity::EntityBase;

impl World {
    /// A villager takes a job site's ticket (see `VillagerPoiStorage::claim`).
    pub fn claim_job_site(
        &self,
        position: BlockPos,
        block: &Block,
        owner: Weak<dyn EntityBase>,
        expected_profession: Option<VillagerProfession>,
    ) -> Option<VillagerProfession> {
        let mut poi = self
            .portal_poi
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        self.villager_poi
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .claim(&mut poi, position, block, owner, expected_profession)
    }

    /// A villager gives a job site's ticket back.
    pub fn release_job_site(&self, position: BlockPos, owner: uuid::Uuid) {
        let mut poi = self
            .portal_poi
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        self.villager_poi
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .release(&mut poi, position, owner);
    }

    /// The job sites with a free ticket within `radius`, closest first.
    #[must_use]
    pub fn available_job_sites(
        &self,
        origin: BlockPos,
        radius: i32,
        expected_profession: Option<VillagerProfession>,
    ) -> Vec<BlockPos> {
        let mut poi = self
            .portal_poi
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        self.villager_poi
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .available_job_sites(&mut poi, origin, radius, expected_profession)
    }

    /// The nearest job site within `radius`, held or not.
    #[must_use]
    pub fn nearest_job_site(&self, origin: BlockPos, radius: i32) -> Option<BlockPos> {
        super::villager_poi::VillagerPoiStorage::get_nearest_job_site(
            &mut self
                .portal_poi
                .lock()
                .unwrap_or_else(PoisonError::into_inner),
            origin,
            radius,
        )
    }

    /// Vanilla `PoiManager.checkConsistencyWithBlocks` for a chunk that loaded: a section
    /// without point of interest data gets the points its blocks hold.
    pub(super) fn check_poi_consistency(&self, chunk_pos: Vector2<i32>) {
        let Some(chunk) = self.level.read_chunk_sync(&chunk_pos, Clone::clone) else {
            return;
        };
        let min_section_y = chunk.section.min_y >> 4;
        let palettes = chunk
            .section
            .block_sections
            .read()
            .unwrap_or_else(PoisonError::into_inner);
        let mut poi = self
            .portal_poi
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        for (index, palette) in palettes.iter().enumerate() {
            let section_y = min_section_y + index as i32;
            let section = (chunk_pos.x, section_y, chunk_pos.y);
            if poi.section_has_data(section)
                || !palette.contains_matching(|state| PoiType::has_poi(state.as_u16()))
            {
                continue;
            }
            poi.refresh_section(section, section_points(palette, chunk_pos, section_y));
        }
    }

    /// Vanilla `ServerLevel.updatePOIOnBlockStateChange`: a changed point of interest
    /// leaves the storage and a new one enters it with every ticket free.
    pub(super) fn update_poi_on_block_state_change(
        &self,
        position: &BlockPos,
        old_state: BlockStateId,
        new_state: BlockStateId,
    ) {
        let old_kind = PoiType::from_state_id(old_state.as_u16());
        let new_kind = PoiType::from_state_id(new_state.as_u16());
        if old_kind == new_kind {
            return;
        }
        let mut poi = self
            .portal_poi
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if old_kind.is_some() {
            poi.remove(position);
        }
        if let Some(kind) = new_kind {
            poi.add_type(*position, kind);
        }
        drop(poi);
        self.villager_poi
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .forget(position);
    }

    /// Vanilla `ServerLevel.isVillage`.
    #[must_use]
    pub fn is_village(&self, pos: &BlockPos) -> bool {
        self.is_close_to_village(pos, 1)
    }

    /// Vanilla `ServerLevel.isCloseToVillage`: a village section within `section_distance`
    /// sections; never true beyond `MAX_VILLAGE_DISTANCE`.
    #[must_use]
    pub fn is_close_to_village(&self, pos: &BlockPos, section_distance: i32) -> bool {
        section_distance <= MAX_VILLAGE_DISTANCE
            && self
                .portal_poi
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .sections_to_village(section_of(pos))
                <= section_distance
    }
}

/// The points of interest in one chunk section, by block state.
fn section_points(
    palette: &BlockPalette,
    chunk_pos: Vector2<i32>,
    section_y: i32,
) -> Vec<(BlockPos, &'static PoiType)> {
    let mut points = Vec::new();
    for x in 0..16usize {
        for y in 0..16usize {
            for z in 0..16usize {
                if let Some(kind) = PoiType::from_state_id(palette.get(x, y, z).as_u16()) {
                    points.push((
                        BlockPos::new(
                            (chunk_pos.x << 4) + x as i32,
                            (section_y << 4) + y as i32,
                            (chunk_pos.y << 4) + z as i32,
                        ),
                        kind,
                    ));
                }
            }
        }
    }
    points
}
