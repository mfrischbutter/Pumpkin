//! Vanilla `RandomizableContainer`: a container keeps its loot table until it is opened or its
//! items are first read or changed, which rolls the loot into it.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use pumpkin_data::attributes::Attributes;
use pumpkin_data::data_component_impl::ContainerLootImpl;
use pumpkin_data::loot_table::get_loot_table;
use pumpkin_inventory::Inventory;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::math::position::BlockPos;

use crate::entity::EntityBase;
use crate::entity::player::Player;
use crate::entity::player::advancement::trigger::AdvancementTrigger;
use crate::world::World;
use crate::world::loot::{LootContextParameters, LootTableHandle, fill_chest_inventory_handle};

const LOOT_TABLE_TAG: &str = "LootTable";
const LOOT_TABLE_SEED_TAG: &str = "LootTableSeed";

/// A container's loot table and seed, kept until its loot is rolled.
#[derive(Default)]
pub struct ContainerLootTable(Mutex<Option<(String, i64)>>);

impl ContainerLootTable {
    #[must_use]
    pub const fn new() -> Self {
        Self(Mutex::new(None))
    }

    /// Vanilla `RandomizableContainer.tryLoadLootTable`. The container reads its items only if
    /// there is no loot table.
    #[must_use]
    pub fn from_nbt(nbt: &NbtCompound) -> Self {
        let loot_table = nbt.get_string(LOOT_TABLE_TAG).map(|loot_table| {
            (
                loot_table.to_owned(),
                nbt.get_long(LOOT_TABLE_SEED_TAG).unwrap_or(0),
            )
        });
        Self(Mutex::new(loot_table))
    }

    /// Vanilla `RandomizableContainer.trySaveLootTable`. Returns `false` once the loot is rolled;
    /// only then does the container write its items.
    pub fn try_write_nbt(&self, nbt: &mut NbtCompound) -> bool {
        let Some((loot_table, seed)) = self.lock().clone() else {
            return false;
        };
        nbt.put_string(LOOT_TABLE_TAG, loot_table);
        if seed != 0 {
            nbt.put_long(LOOT_TABLE_SEED_TAG, seed);
        }
        true
    }

    /// Whether the loot is not rolled yet.
    #[must_use]
    pub fn is_pending(&self) -> bool {
        self.lock().is_some()
    }

    /// Takes the loot table and its seed out, so the loot is rolled once.
    #[must_use]
    pub fn take(&self) -> Option<(String, i64)> {
        self.lock().take()
    }

    /// Vanilla `RandomizableContainerBlockEntity.applyImplicitComponents` for `container_loot`.
    pub fn apply_component(&self, loot: &ContainerLootImpl) {
        *self.lock() = Some((loot.loot_table.clone(), loot.seed));
    }

    fn lock(&self) -> MutexGuard<'_, Option<(String, i64)>> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The part of vanilla `RandomizableContainer.unpackLootTable` after the loot table is taken:
/// rolls `loot_table` with `seed` into `inventory`. `player` is the one opening the container,
/// `None` for any other access.
pub fn fill_with_loot(
    world: &World,
    position: BlockPos,
    loot_table: &str,
    seed: i64,
    inventory: &Arc<dyn Inventory>,
    player: Option<&Arc<Player>>,
) {
    let Some(table) = get_loot_table(loot_table) else {
        return;
    };
    if let Some(player) = player {
        player.trigger_advancement(AdvancementTrigger::PlayerGeneratesContainerLoot {
            loot_table_id: loot_table.to_string(),
        });
    }
    fill_chest_inventory_handle(
        inventory,
        &LootTableHandle::Static(table),
        seed,
        &LootContextParameters {
            world: Some(world),
            position: Some(position.to_centered_f64()),
            this_entity: player.map(|player| &**player as &dyn EntityBase),
            luck: player.map_or(0.0, |player| {
                player.living_entity.get_attribute_value(&Attributes::LUCK) as f32
            }),
            ..Default::default()
        },
        |items| {
            world.fire_loot_generate(
                loot_table,
                Some(position),
                None,
                player.map(Arc::as_ref),
                items,
            )
        },
    );
    inventory.mark_dirty();
}
