use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use pumpkin_data::entity::EntityType;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::particle::Particle;
use pumpkin_data::sound::Sound;
use pumpkin_data::tag::{self, Taggable};
use pumpkin_data::tracked_data;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::math::vector3::Vector3;

use super::abstract_fish::{AbstractFish, FishData};
use super::frog_ai;
use crate::entity::ai::brain::memory::{MemoryModuleId, PackedMemories};
use crate::entity::ai::brain::sensing::SensorType;
use crate::entity::ai::brain::{Brain, BrainProvider};
use crate::entity::ai::control::smooth_swimming_look_control::SmoothSwimmingLookControl;
use crate::entity::ai::control::smooth_swimming_move_control::SmoothSwimmingMoveControl;
use crate::entity::ai::pathfinder::{Navigator, node::PathType};
use crate::entity::bucketable::{load_age_from_bucket_tag, save_age_to_bucket_tag};
use crate::entity::mob::conversion::{ConversionParams, convert_to};
use crate::entity::mob::spawn::SpawnGroupData;
use crate::entity::{
    Entity, EntityBase,
    ageable::{AgeableData, AgeableMob},
    mob::{Mob, MobEntity},
    player::Player,
};
use crate::world::World;

/// Vanilla `Tadpole.ticksToBeFrog`.
const TICKS_TO_BE_FROG: i32 = 24000;

const MEMORIES: [MemoryModuleId; 0] = [];

/// A tadpole: a fish that grows up. Its age counts up from 0 to the frog, and it stays until
/// then wherever it is (vanilla `Tadpole.fromBucket` is always true).
pub struct TadpoleEntity {
    pub mob_entity: MobEntity,
    pub ageable_data: AgeableData,
    fish_data: FishData,
}

impl TadpoleEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let mut mob_entity = MobEntity::new(entity);
        Self::set_up_movement(&mut mob_entity);
        Arc::new(Self {
            mob_entity,
            ageable_data: AgeableData::default(),
            fish_data: FishData {
                from_bucket: AtomicBool::new(true),
            },
        })
    }

    /// Vanilla `Tadpole` constructor and `createNavigation`.
    fn set_up_movement(mob_entity: &mut MobEntity) {
        *mob_entity
            .move_control
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            Box::new(SmoothSwimmingMoveControl::new(85, 10, 0.02, 0.1, true));
        *mob_entity
            .look_control
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            Box::new(SmoothSwimmingLookControl::new(10));
        let navigator = mob_entity
            .navigator
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *navigator = Navigator::water_bound(false);
        navigator.set_pathfinding_malus(PathType::Water, 0.0);
    }

    fn is_food(item_stack: &ItemStack) -> bool {
        item_stack.item.has_tag(&tag::Item::MINECRAFT_FROG_FOOD)
    }

    /// Vanilla `Tadpole.feed`.
    fn feed(&self, player: &Arc<Player>, item_stack: &mut ItemStack) {
        item_stack.decrement_unless_creative(player.gamemode.load(), 1);
        let ticks_left = (TICKS_TO_BE_FROG - self.get_age()).max(0);
        let seconds = (ticks_left as f32 / 20.0 * 0.1) as i32;
        self.set_age(self.get_age() + seconds * 20);

        let entity = self.get_entity();
        entity.world.load().spawn_particle(
            entity.pos.load() + Vector3::new(0.0, f64::from(entity.height()), 0.0),
            Vector3::new(0.5, 0.5, 0.5),
            1.0,
            7,
            Particle::HappyVillager,
        );
    }

    /// Vanilla `Tadpole.ageUp`.
    fn grow_into_frog(&self) {
        let world = self.get_entity().world.load_full();
        let _ = convert_to(
            self,
            &EntityType::FROG,
            ConversionParams::single(false, false),
            |frog| {
                frog.finalize_spawn(&world, None);
                frog.get_mob_entity().set_persistence_required();
                self.play_entity_sound(Sound::EntityTadpoleGrowUp, 0.15, 1.0);
            },
        );
    }
}

impl AgeableMob for TadpoleEntity {
    fn get_ageable_data(&self) -> &AgeableData {
        &self.ageable_data
    }

    /// For the golden dandelion: it works on a tadpole of any age.
    fn is_baby(&self) -> bool {
        true
    }

    /// The age the golden dandelion sets back to.
    fn get_baby_start_age(&self) -> i32 {
        0
    }

    /// Vanilla `Tadpole.setAge`.
    fn set_age(&self, new_age: i32) {
        self.get_entity().age.store(new_age, Ordering::Relaxed);
        if new_age >= TICKS_TO_BE_FROG {
            self.grow_into_frog();
        }
    }

    /// Vanilla `Tadpole.setAgeLocked`.
    fn set_age_locked(&self, locked: bool) {
        self.ageable_data
            .age_locked
            .store(locked, Ordering::Relaxed);
        self.get_entity()
            .set_synced_data(tracked_data::tadpole::AGE_LOCKED, locked);
    }

    /// Vanilla `Tadpole.aiStep`.
    fn ageable_ai_step(&self) {
        self.tick_age_lock_particle_timer();
        if !self.is_age_locked() {
            self.set_age(self.get_age() + 1);
        }
    }
}

impl Mob for TadpoleEntity {
    crate::fish_mob_hooks!();

    fn as_ageable(&self) -> Option<&dyn AgeableMob> {
        Some(self)
    }

    /// A spawn egg on a tadpole gives none: vanilla's is no `AgeableMob`.
    fn spawn_as_baby(&self) -> bool {
        false
    }

    fn make_brain(&self, packed: &PackedMemories) -> Brain {
        BrainProvider {
            memory_types: &MEMORIES,
            sensor_types: &[
                SensorType::NearestLivingEntities,
                SensorType::NearestPlayers,
                SensorType::HurtBy,
                SensorType::FrogTemptations,
            ],
            activities: frog_ai::tadpole_activities,
        }
        .make_brain(self, packed)
    }

    /// Vanilla `Tadpole.customServerAiStep`.
    fn custom_server_ai_step(&self, _caller: &dyn EntityBase) {
        self.mob_entity.tick_brain(self);
        self.mob_entity.with_brain(self, |brain| {
            brain.set_active_activity_to_first_valid(&[
                pumpkin_data::environment_attribute::Activity::Idle,
            ]);
        });
    }

    fn finalize_spawn(
        &self,
        _world: &Arc<World>,
        group_data: Option<SpawnGroupData>,
    ) -> Option<SpawnGroupData> {
        self.mob_entity.finalize_spawn_base();
        group_data
    }

    fn should_drop_experience(&self) -> bool {
        false
    }

    fn mob_init_data_tracker(&self) {
        self.set_age_locked(self.is_age_locked());
    }

    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        self.fish_write_nbt(nbt);
    }

    /// Vanilla `Tadpole.mobInteract` past the bucket; a locked one does not eat.
    fn mob_interact(&self, player: &Arc<Player>, item_stack: &mut ItemStack) -> bool {
        if Self::is_food(item_stack) && !self.is_age_locked() {
            self.feed(player, item_stack);
            return true;
        }
        false
    }
}

crate::impl_abstract_fish!(
    TadpoleEntity,
    bucket: &Item::TADPOLE_BUCKET,
    flop: Sound::EntityTadpoleFlop,
    ambient: None,
    pickup: Sound::ItemBucketFillTadpole,
    bucket_extra: {
        /// Vanilla `Tadpole.saveToBucketTag`: the age and its lock go along.
        fn save_to_bucket_tag(&self, bucket: &mut ItemStack) {
            self.save_default_data_to_bucket_tag(bucket);
            save_age_to_bucket_tag(self, bucket);
        }

        /// Vanilla `Tadpole.loadFromBucketTag`.
        fn load_from_bucket_tag(&self, nbt: &NbtCompound) {
            self.load_default_data_from_bucket_tag(nbt);
            load_age_from_bucket_tag(self, nbt);
        }
    }
);
