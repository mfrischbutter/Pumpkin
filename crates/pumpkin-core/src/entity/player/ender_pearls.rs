//! The ender pearls a player has in flight, which are saved and loaded with the player: vanilla
//! `ServerPlayer.enderPearls`, `saveEnderPearls` and `loadAndSpawnEnderPearls`.

use std::sync::{Arc, Mutex, PoisonError, Weak};

use pumpkin_nbt::compound::NbtCompound;
use pumpkin_nbt::tag::NbtTag;
use pumpkin_util::math::vector3::Vector3;
use tracing::warn;

use super::Player;
use crate::entity::EntityBase;
use crate::entity::projectile::ender_pearl::EnderPearlEntity;
use crate::entity::r#type::load_entity_from_nbt;

const PEARLS_KEY: &str = "ender_pearls";
const DIMENSION_KEY: &str = "ender_pearl_dimension";

#[derive(Default)]
pub struct EnderPearls {
    flying: Mutex<Vec<Weak<dyn EntityBase>>>,
    /// Read with the player's data, spawned once the player is in a world.
    to_load: Mutex<Vec<NbtCompound>>,
}

impl Player {
    /// Vanilla `registerEnderPearl`.
    pub(crate) fn register_ender_pearl(&self, pearl: &Arc<dyn EntityBase>) {
        let mut flying = self
            .ender_pearls
            .flying
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        flying.retain(|known| known.strong_count() > 0);
        let known = flying
            .iter()
            .filter_map(Weak::upgrade)
            .any(|known| known.get_entity().entity_id == pearl.get_entity().entity_id);
        if !known {
            flying.push(Arc::downgrade(pearl));
        }
    }

    /// The registered pearls still in a world; vanilla deregisters them in `onRemoval`.
    fn live_ender_pearls(&self) -> Vec<Arc<dyn EntityBase>> {
        self.ender_pearls
            .flying
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .filter_map(Weak::upgrade)
            .filter(|pearl| !pearl.get_entity().is_removed())
            .collect()
    }

    /// Vanilla `saveEnderPearls`.
    pub(super) fn save_ender_pearls(&self, nbt: &mut NbtCompound) {
        let pearls: Vec<NbtTag> = self
            .live_ender_pearls()
            .iter()
            .map(|pearl| {
                let mut pearl_nbt = NbtCompound::new();
                EntityBase::write_nbt(&**pearl, &mut pearl_nbt);
                pearl_nbt.put_string(
                    DIMENSION_KEY,
                    pearl
                        .get_entity()
                        .world
                        .load()
                        .dimension
                        .minecraft_name
                        .to_string(),
                );
                NbtTag::Compound(pearl_nbt)
            })
            .collect();
        if !pearls.is_empty() {
            nbt.put_list(PEARLS_KEY, pearls);
        }
    }

    /// Keeps the saved pearls for [`Self::load_and_spawn_ender_pearls`].
    pub(super) fn read_ender_pearls(&self, nbt: &NbtCompound) {
        let pearls: Vec<NbtCompound> = nbt
            .get_list(PEARLS_KEY)
            .unwrap_or_default()
            .iter()
            .filter_map(|pearl| pearl.extract_compound().cloned())
            .collect();
        *self
            .ender_pearls
            .to_load
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = pearls;
    }

    /// Vanilla `PlayerList.remove`: the pearls leave with the player, already saved, and do
    /// not go to their chunks.
    pub fn unload_ender_pearls(&self) {
        for pearl in self.live_ender_pearls() {
            pearl
                .get_entity()
                .world
                .load()
                .remove_entity(pearl.as_ref());
        }
    }

    /// Vanilla `loadAndSpawnEnderPearls`.
    pub fn load_and_spawn_ender_pearls(&self) {
        let pearls = std::mem::take(
            &mut *self
                .ender_pearls
                .to_load
                .lock()
                .unwrap_or_else(PoisonError::into_inner),
        );
        let Some(server) = self.world().server.upgrade() else {
            return;
        };
        let worlds = server.worlds.load();
        for pearl_nbt in &pearls {
            let dimension = pearl_nbt.get_string(DIMENSION_KEY);
            let Some(world) = worlds
                .iter()
                .find(|world| Some(world.dimension.minecraft_name) == dimension)
            else {
                warn!(
                    "Trying to load ender pearl without level ({dimension:?}) being loaded, skipping"
                );
                continue;
            };
            let Some(pearl) = load_entity_from_nbt(pearl_nbt, Vector3::default(), world) else {
                warn!("Failed to spawn player ender pearl in level ({dimension:?}), skipping");
                continue;
            };
            pearl.init_data_tracker();
            if !world.add_entity_silent(pearl.clone()) {
                continue;
            }
            if let Some(entity) = pearl.cast_any().downcast_ref::<EnderPearlEntity>() {
                entity.place_ticket();
            }
            self.register_ender_pearl(&pearl);
        }
    }
}
