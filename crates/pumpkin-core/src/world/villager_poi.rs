//! Villager job sites: the professions' workstations and who holds each site's ticket.
//!
//! The sites are points of interest of the world (`pumpkin_world::poi`); the holder is
//! kept so a dead villager's site frees up (vanilla releases it from the villager's brain).

use std::{collections::HashMap, sync::Weak};

use pumpkin_data::poi_type::PoiType;
use pumpkin_data::tag::PointOfInterestType::MINECRAFT_ACQUIRABLE_JOB_SITE;
use pumpkin_data::tag::Taggable;
use pumpkin_data::{Block, villager::VillagerProfession};
use pumpkin_util::math::position::BlockPos;
use pumpkin_world::poi::{Occupancy, PoiStorage};

use crate::entity::EntityBase;

/// Vanilla `PoiTypes.MEETING`, used to place wandering traders near a village bell.
pub const POI_TYPE_MEETING: &str = "minecraft:meeting";

/// The villagers holding a job site ticket, by site.
#[derive(Default)]
pub struct VillagerPoiStorage {
    holders: HashMap<BlockPos, Weak<dyn EntityBase>>,
}

impl VillagerPoiStorage {
    fn live_holder(holder: &Weak<dyn EntityBase>) -> Option<std::sync::Arc<dyn EntityBase>> {
        holder.upgrade().filter(|entity| {
            entity
                .get_living_entity()
                .is_none_or(|living| living.health.load() > 0.0)
        })
    }

    /// Frees the tickets of the sites whose holder died.
    fn release_dead_holders(&mut self, poi: &mut PoiStorage) {
        self.holders.retain(|site, holder| {
            if Self::live_holder(holder).is_some() {
                return true;
            }
            poi.release_ticket(*site);
            false
        });
    }

    /// The point of interest at `position` changed: whoever held it lets go.
    pub fn forget(&mut self, position: &BlockPos) {
        self.holders.remove(position);
    }

    /// Vanilla `PoiManager.take` for a job site: the site's profession when `owner` holds
    /// its ticket afterwards; `None` when another living villager holds it, no ticket is
    /// free, or it is not a site of `expected_profession`. A taken ticket nobody here
    /// holds is a persisted claim from before a restart and goes to the claimant.
    pub fn claim(
        &mut self,
        poi: &mut PoiStorage,
        position: BlockPos,
        block: &Block,
        owner: Weak<dyn EntityBase>,
        expected_profession: Option<VillagerProfession>,
    ) -> Option<VillagerProfession> {
        let profession = profession_for_block(block)?;
        if expected_profession.is_some_and(|expected| expected != profession) {
            return None;
        }
        let owner_uuid = owner.upgrade()?.get_entity().entity_uuid;
        match self.holders.get(&position).and_then(Self::live_holder) {
            Some(holder) if holder.get_entity().entity_uuid == owner_uuid => {
                return Some(profession);
            }
            Some(_) => return None,
            None => {}
        }
        let known_holder = self.holders.remove(&position).is_some();
        if known_holder {
            poi.release_ticket(position);
        }
        poi.get_type(position)?;
        if !poi.acquire_ticket(position) && known_holder {
            return None;
        }
        self.holders.insert(position, owner);
        Some(profession)
    }

    /// Vanilla `PoiManager.release` by the site's holder.
    pub fn release(&mut self, poi: &mut PoiStorage, position: BlockPos, owner: uuid::Uuid) {
        let held_by_other = self
            .holders
            .get(&position)
            .and_then(Weak::upgrade)
            .is_some_and(|holder| holder.get_entity().entity_uuid != owner);
        if held_by_other {
            return;
        }
        if self.holders.remove(&position).is_some() {
            poi.release_ticket(position);
        }
    }

    /// Vanilla `AcquirePoi`'s search: the job sites with a free ticket within `radius`,
    /// closest first, of `expected_profession` when given.
    pub fn available_job_sites(
        &mut self,
        poi: &mut PoiStorage,
        origin: BlockPos,
        radius: i32,
        expected_profession: Option<VillagerProfession>,
    ) -> Vec<BlockPos> {
        self.release_dead_holders(poi);
        let wanted = expected_profession.and_then(poi_type_for_profession);
        let mut sites = poi.in_range(
            origin,
            radius,
            |kind| {
                kind.has_tag(&MINECRAFT_ACQUIRABLE_JOB_SITE)
                    && wanted.is_none_or(|wanted| wanted == kind)
            },
            Occupancy::HasSpace,
        );
        sites.sort_by_key(|(position, _)| {
            let delta = position.0 - origin.0;
            i64::from(delta.x).pow(2) + i64::from(delta.y).pow(2) + i64::from(delta.z).pow(2)
        });
        sites.into_iter().map(|(position, _)| position).collect()
    }

    /// The nearest job site within `radius`, held or not.
    pub fn get_nearest_job_site(
        poi: &mut PoiStorage,
        origin: BlockPos,
        radius: i32,
    ) -> Option<BlockPos> {
        poi.find_closest(
            origin,
            radius,
            |kind| kind.has_tag(&MINECRAFT_ACQUIRABLE_JOB_SITE),
            |_| true,
            Occupancy::Any,
        )
    }
}

/// The job site of a profession (vanilla `VillagerProfession.heldJobSite`).
#[must_use]
pub const fn poi_type_for_profession(profession: VillagerProfession) -> Option<&'static PoiType> {
    Some(match profession {
        VillagerProfession::Armorer => &PoiType::ARMORER,
        VillagerProfession::Butcher => &PoiType::BUTCHER,
        VillagerProfession::Cartographer => &PoiType::CARTOGRAPHER,
        VillagerProfession::Cleric => &PoiType::CLERIC,
        VillagerProfession::Farmer => &PoiType::FARMER,
        VillagerProfession::Fisherman => &PoiType::FISHERMAN,
        VillagerProfession::Fletcher => &PoiType::FLETCHER,
        VillagerProfession::Leatherworker => &PoiType::LEATHERWORKER,
        VillagerProfession::Librarian => &PoiType::LIBRARIAN,
        VillagerProfession::Mason => &PoiType::MASON,
        VillagerProfession::Shepherd => &PoiType::SHEPHERD,
        VillagerProfession::Toolsmith => &PoiType::TOOLSMITH,
        VillagerProfession::Weaponsmith => &PoiType::WEAPONSMITH,
        VillagerProfession::None | VillagerProfession::Nitwit => return None,
    })
}

#[must_use]
pub fn profession_for_block(block: &Block) -> Option<VillagerProfession> {
    match block {
        block if block == &Block::COMPOSTER => Some(VillagerProfession::Farmer),
        block if block == &Block::LECTERN => Some(VillagerProfession::Librarian),
        block if block == &Block::BLAST_FURNACE => Some(VillagerProfession::Armorer),
        block if block == &Block::SMOKER => Some(VillagerProfession::Butcher),
        block if block == &Block::CARTOGRAPHY_TABLE => Some(VillagerProfession::Cartographer),
        block if block == &Block::BREWING_STAND => Some(VillagerProfession::Cleric),
        block if block == &Block::BARREL => Some(VillagerProfession::Fisherman),
        block if block == &Block::FLETCHING_TABLE => Some(VillagerProfession::Fletcher),
        block
            if [
                &Block::CAULDRON,
                &Block::WATER_CAULDRON,
                &Block::LAVA_CAULDRON,
                &Block::POWDER_SNOW_CAULDRON,
            ]
            .contains(&block) =>
        {
            Some(VillagerProfession::Leatherworker)
        }
        block if block == &Block::STONECUTTER => Some(VillagerProfession::Mason),
        block if block == &Block::LOOM => Some(VillagerProfession::Shepherd),
        block if block == &Block::SMITHING_TABLE => Some(VillagerProfession::Toolsmith),
        block if block == &Block::GRINDSTONE => Some(VillagerProfession::Weaponsmith),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_villager_workstation_maps_to_its_profession() {
        let workstations = [
            (&Block::COMPOSTER, VillagerProfession::Farmer),
            (&Block::LECTERN, VillagerProfession::Librarian),
            (&Block::BLAST_FURNACE, VillagerProfession::Armorer),
            (&Block::SMOKER, VillagerProfession::Butcher),
            (&Block::CARTOGRAPHY_TABLE, VillagerProfession::Cartographer),
            (&Block::BREWING_STAND, VillagerProfession::Cleric),
            (&Block::BARREL, VillagerProfession::Fisherman),
            (&Block::FLETCHING_TABLE, VillagerProfession::Fletcher),
            (&Block::CAULDRON, VillagerProfession::Leatherworker),
            (&Block::STONECUTTER, VillagerProfession::Mason),
            (&Block::LOOM, VillagerProfession::Shepherd),
            (&Block::SMITHING_TABLE, VillagerProfession::Toolsmith),
            (&Block::GRINDSTONE, VillagerProfession::Weaponsmith),
        ];

        for (block, profession) in workstations {
            assert_eq!(profession_for_block(block), Some(profession));
        }
        assert_eq!(profession_for_block(&Block::DIRT), None);
    }

    #[test]
    fn available_job_sites_respect_profession_and_vanilla_search_radius() {
        let origin = BlockPos::new(0, 64, 0);
        let lectern = BlockPos::new(48, 64, 0);
        let too_far = BlockPos::new(49, 64, 0);
        let composter = BlockPos::new(1, 64, 0);
        let mut poi = PoiStorage::new(std::env::temp_dir().join("pumpkin_villager_poi_test"));
        let mut storage = VillagerPoiStorage::default();

        poi.add_type(lectern, &PoiType::LIBRARIAN);
        poi.add_type(too_far, &PoiType::LIBRARIAN);
        poi.add_type(composter, &PoiType::FARMER);

        assert_eq!(
            storage.available_job_sites(&mut poi, origin, 48, Some(VillagerProfession::Librarian)),
            vec![lectern]
        );
        assert_eq!(
            storage.available_job_sites(&mut poi, origin, 48, Some(VillagerProfession::Farmer)),
            vec![composter]
        );
    }
}
