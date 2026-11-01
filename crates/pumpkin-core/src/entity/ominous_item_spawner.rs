//! Vanilla `OminousItemSpawner`: the floating marker an ominous trial spawner puts above a
//! player of the trial or one of its mobs; after 60 to 120 ticks it drops or shoots its item
//! and is gone.

use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use pumpkin_data::damage::DamageType;
use pumpkin_data::entity::EntityType;
use pumpkin_data::game_event::GameEvent;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::tracked_data::ominous_item_spawner::DATA_ITEM;
use pumpkin_data::world::WorldEvent;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_protocol::codec::item_stack_seralizer::ItemStackSerializer;
use pumpkin_util::math::vector3::Vector3;
use rand::RngExt;

use super::item::ItemEntity;
use super::projectile::projectile_item::{dispense_config, shoot_projectile_item};
use super::{Entity, EntityBase, living::LivingEntity};
use crate::server::Server;
use crate::world::World;

pub struct OminousItemSpawnerEntity {
    pub entity: Entity,
    item: Mutex<ItemStack>,
    /// Vanilla `tickCount`: the ticks since the spawner appeared.
    tick_count: AtomicI64,
    /// Vanilla `spawnItemAfterTicks`, saved as `spawn_item_after_ticks`.
    spawn_item_after_ticks: AtomicI64,
}

impl OminousItemSpawnerEntity {
    const SPAWN_ITEM_DELAY_MIN: i64 = 60;
    const SPAWN_ITEM_DELAY_MAX: i64 = 120;
    /// Vanilla `TICKS_BEFORE_ABOUT_TO_SPAWN_SOUND`.
    const TICKS_BEFORE_ABOUT_TO_SPAWN_SOUND: i64 = 36;
    const TAG_ITEM: &str = "item";
    const TAG_SPAWN_ITEM_AFTER_TICKS: &str = "spawn_item_after_ticks";

    /// A spawner holding nothing yet: the saved data fills it.
    pub fn new(entity: Entity) -> Self {
        entity.no_physics.store(true, Ordering::Relaxed);
        Self {
            entity,
            item: Mutex::new(ItemStack::EMPTY.clone()),
            tick_count: AtomicI64::new(0),
            spawn_item_after_ticks: AtomicI64::new(0),
        }
    }

    /// Vanilla `OminousItemSpawner.create`: a spawner for `item` at `position`, due after 60
    /// to 120 ticks.
    pub fn create(world: Arc<World>, position: Vector3<f64>, item: ItemStack) -> Self {
        let spawner = Self::new(Entity::new(
            world,
            position,
            &EntityType::OMINOUS_ITEM_SPAWNER,
        ));
        spawner.spawn_item_after_ticks.store(
            rand::rng().random_range(Self::SPAWN_ITEM_DELAY_MIN..=Self::SPAWN_ITEM_DELAY_MAX),
            Ordering::Relaxed,
        );
        spawner.set_item(item);
        spawner
    }

    #[must_use]
    pub fn item(&self) -> ItemStack {
        self.item
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn set_item(&self, item: ItemStack) {
        self.entity
            .set_synced_data(DATA_ITEM, ItemStackSerializer::from(item.clone()));
        *self.item.lock().unwrap_or_else(PoisonError::into_inner) = item;
    }

    /// Vanilla `tickServer`: the warning sound 36 ticks before the item, then the item and
    /// the end of the spawner.
    fn tick_server(&self, world: &Arc<World>) {
        let tick_count = self.tick_count.fetch_add(1, Ordering::Relaxed) + 1;
        let spawn_item_after_ticks = self.spawn_item_after_ticks.load(Ordering::Relaxed);
        if tick_count == spawn_item_after_ticks - Self::TICKS_BEFORE_ABOUT_TO_SPAWN_SOUND {
            world.play_block_sound(
                Sound::BlockTrialSpawnerAboutToSpawnItem,
                SoundCategory::Neutral,
                self.entity.block_pos.load(),
            );
        }
        if tick_count >= spawn_item_after_ticks {
            self.spawn_item(world);
            self.entity.remove();
        }
    }

    /// Vanilla `spawnItem`: a projectile item is shot straight down the way a dispenser
    /// launches it, anything else drops as an item; the particles and the place event follow.
    fn spawn_item(&self, world: &Arc<World>) {
        let item = self.item();
        if item.is_empty() {
            return;
        }
        let position = self.entity.pos.load();
        let block_pos = self.entity.block_pos.load();
        if let Some(config) = dispense_config(item.item) {
            if let Some(event) = config.override_dispense_event {
                world.sync_world_event(event, block_pos, 0);
            }
            let down = Vector3::new(0.0, -1.0, 0.0);
            if let Some(projectile) = shoot_projectile_item(
                world,
                position,
                &item,
                down,
                down,
                &config,
                Some(&self.entity),
            ) {
                world.spawn_entity(projectile);
            }
        } else {
            let entity = Entity::new(world.clone(), position, &EntityType::ITEM);
            world.spawn_entity(Arc::new(ItemEntity::new(entity, item)));
        }
        world.sync_world_event(WorldEvent::ParticlesTrialSpawnerSpawnItem, block_pos, 1);
        world.emit_game_event(GameEvent::EntityPlace.name(), position);
        self.set_item(ItemStack::EMPTY.clone());
    }
}

impl EntityBase for OminousItemSpawnerEntity {
    fn tick(&self, caller: &dyn EntityBase, server: &Server) {
        self.entity.tick(caller, server);
        let world = self.entity.world.load_full();
        self.tick_server(&world);
    }

    fn init_data_tracker(&self) {
        self.entity
            .set_synced_data(DATA_ITEM, ItemStackSerializer::from(self.item()));
    }

    fn write_custom_nbt(&self, nbt: &mut NbtCompound) {
        let item = self.item();
        if !item.is_empty() {
            let mut item_compound = NbtCompound::new();
            item.write_item_stack(&mut item_compound);
            nbt.put_compound(Self::TAG_ITEM, item_compound);
        }
        nbt.put_long(
            Self::TAG_SPAWN_ITEM_AFTER_TICKS,
            self.spawn_item_after_ticks.load(Ordering::Relaxed),
        );
    }

    fn read_custom_nbt(&self, nbt: &NbtCompound) {
        self.set_item(
            nbt.get_compound(Self::TAG_ITEM)
                .and_then(ItemStack::read_item_stack)
                .unwrap_or_else(|| ItemStack::EMPTY.clone()),
        );
        self.spawn_item_after_ticks.store(
            nbt.get_long(Self::TAG_SPAWN_ITEM_AFTER_TICKS).unwrap_or(0),
            Ordering::Relaxed,
        );
    }

    /// Vanilla `hurtServer` returns false: nothing hurts the spawner.
    fn damage_with_context(
        &self,
        _caller: &dyn EntityBase,
        _amount: f32,
        _damage_type: DamageType,
        _position: Option<Vector3<f64>>,
        _source: Option<&dyn EntityBase>,
        _cause: Option<&dyn EntityBase>,
    ) -> bool {
        false
    }

    fn get_entity(&self) -> &Entity {
        &self.entity
    }

    fn get_living_entity(&self) -> Option<&LivingEntity> {
        None
    }

    fn cast_any(&self) -> &dyn std::any::Any {
        self
    }
}
