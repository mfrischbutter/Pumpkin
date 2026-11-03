use pumpkin_data::BlockState;
use pumpkin_data::sound::Sound;
use pumpkin_util::math::position::BlockPos;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::entity::mob::zombie::ZombieEntityBase;
use crate::entity::{
    Entity, EntityBase,
    mob::{Mob, MobEntity},
};
use pumpkin_data::data_component_impl::EquipmentSlot;
use pumpkin_data::entity::EntityType;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::math::boundingbox::BoundingBox;
use rand::RngExt;
use uuid::Uuid;

use crate::entity::mob::spawn::{EntitySpawnReason, SpawnGroupData, finalize_spawn};
use crate::entity::r#type::from_type;
use crate::world::World;

/// Vanilla `Husk.finalizeSpawn`: the chance of a group of husks to come on a camel husk.
const CAMEL_HUSK_CHANCE: f32 = 0.1;

pub struct HuskEntity {
    /// It rolled a camel husk, which it mounts once it is in the world itself.
    wants_camel: AtomicBool,
    entity: Arc<ZombieEntityBase>,
}

impl HuskEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let entity = ZombieEntityBase::new(entity);
        let zombie = Self {
            wants_camel: AtomicBool::new(false),
            entity,
        };
        Arc::new(zombie)
    }

    #[must_use]
    pub fn with_can_break_doors(entity: Entity, can_break_doors: bool) -> Arc<Self> {
        let entity = ZombieEntityBase::with_can_break_doors(entity, can_break_doors);
        let zombie = Self {
            wants_camel: AtomicBool::new(false),
            entity,
        };
        Arc::new(zombie)
    }
}

impl HuskEntity {
    /// The rest of vanilla `Husk.finalizeSpawn`: a spear, a camel husk and a parched behind it.
    fn mount_camel_husk(&self) {
        let entity = self.get_entity();
        let world = entity.world.load_full();
        let Some(this) = world.get_entity_by_id(entity.entity_id) else {
            return;
        };
        self.get_mob_entity().set_item_slot(
            &EquipmentSlot::MAIN_HAND,
            ItemStack::new(1, &Item::IRON_SPEAR),
        );
        let pos = entity.pos.load();
        let camel = from_type(&EntityType::CAMEL_HUSK, pos, &world, Uuid::new_v4());
        finalize_spawn(&camel, &world, EntitySpawnReason::Natural, None);
        if !world.spawn_entity(camel.clone()) {
            return;
        }
        camel.get_entity().add_passenger(camel.clone(), this);
        let parched = from_type(&EntityType::PARCHED, pos, &world, Uuid::new_v4());
        finalize_spawn(&parched, &world, EntitySpawnReason::Natural, None);
        if world.spawn_entity(parched.clone()) {
            camel.get_entity().add_passenger(camel.clone(), parched);
        }
    }
}

impl Mob for HuskEntity {
    /// Vanilla `Husk.getAmbientSound`.
    fn get_ambient_sound(&self) -> Option<pumpkin_data::sound::Sound> {
        Some(pumpkin_data::sound::Sound::EntityHuskAmbient)
    }

    fn mob_play_step_sound(&self, _pos: &BlockPos, _state: &BlockState) {
        self.play_entity_sound(Sound::EntityHuskStep, 0.15, 1.0);
    }

    fn get_mob_entity(&self) -> &MobEntity {
        &self.entity.mob_entity
    }

    fn spawn_as_baby(&self) -> bool {
        self.entity.spawn_as_baby()
    }

    fn finalize_spawn(
        &self,
        world: &Arc<World>,
        group_data: Option<SpawnGroupData>,
    ) -> Option<SpawnGroupData> {
        self.entity.finalize_spawn(world, group_data)
    }

    /// Vanilla `Husk.finalizeSpawn`: one natural group may come mounted.
    fn after_finalize_spawn(
        &self,
        world: &Arc<World>,
        reason: EntitySpawnReason,
        group_data: Option<SpawnGroupData>,
    ) -> Option<SpawnGroupData> {
        if reason != EntitySpawnReason::Natural
            || matches!(group_data, Some(SpawnGroupData::HuskTriedCamel))
        {
            return group_data;
        }
        let pos = self.get_entity().block_pos.load().0;
        let room = BoundingBox::new_from_pos(
            f64::from(pos.x) + 0.5,
            f64::from(pos.y),
            f64::from(pos.z) + 0.5,
            &Entity::type_dimensions(&EntityType::CAMEL_HUSK),
        );
        if !world.is_space_empty(room) {
            return group_data;
        }
        if self.get_random().random::<f32>() < CAMEL_HUSK_CHANCE {
            self.wants_camel.store(true, Ordering::Relaxed);
        }
        Some(SpawnGroupData::HuskTriedCamel)
    }

    fn mob_tick(&self, caller: &dyn EntityBase) {
        if self.wants_camel.swap(false, Ordering::Relaxed) {
            self.mount_camel_husk();
        }
        self.entity.mob_tick(caller);
    }

    fn mob_init_data_tracker(&self) {
        self.entity.mob_init_data_tracker();
    }

    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        self.entity.mob_write_nbt(nbt);
    }

    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        self.entity.mob_read_nbt(nbt);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entity::ai::test_support::world;
    use pumpkin_util::math::vector2::Vector2;
    use pumpkin_util::math::vector3::Vector3;
    use pumpkin_world::chunk::ChunkData;

    #[tokio::test]
    async fn a_husk_that_rolled_a_camel_rides_it_with_a_parched_behind() {
        let folder = tempfile::tempdir().expect("a temporary folder");
        let world = world(&folder);
        world
            .level
            .loaded_chunks
            .insert(Vector2::new(0, 0), ChunkData::empty_sync(0, 0));
        let husk = HuskEntity::new(Entity::new(
            world.clone(),
            Vector3::new(4.5, 70.0, 4.5),
            &EntityType::HUSK,
        ));
        assert!(world.spawn_entity(husk.clone()));

        husk.mount_camel_husk();

        let camel = husk.get_entity().get_vehicle().expect("the husk rides");
        assert_eq!(camel.get_entity().entity_type, &EntityType::CAMEL_HUSK);
        let riders = camel
            .get_entity()
            .passengers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .map(|rider| rider.get_entity().entity_type.resource_name)
            .collect::<Vec<_>>();
        assert_eq!(riders, ["husk", "parched"]);
        assert_eq!(
            husk.get_mob_entity()
                .living_entity
                .entity_equipment
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .get(&EquipmentSlot::MAIN_HAND)
                .item,
            &Item::IRON_SPEAR
        );
    }
}
