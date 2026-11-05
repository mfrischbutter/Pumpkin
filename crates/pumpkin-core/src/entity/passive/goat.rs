use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use pumpkin_data::BlockState;
use pumpkin_data::attributes::Attributes;
use pumpkin_data::data_component_impl::InstrumentImpl;
use pumpkin_data::entity::EntityType;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::tag::{self, Taggable};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::math::boundingbox::EntityDimensions;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_util::random::RandomImpl;
use pumpkin_util::random::legacy_rand::LegacyRand;
use rand::RngExt;
use std::borrow::Cow;

use super::goat_ai;
use crate::entity::ai::brain::memory::{MemoryModuleId, PackedMemories, types};
use crate::entity::ai::brain::sensing::SensorType;
use crate::entity::ai::brain::{Brain, BrainProvider};
use crate::entity::ai::pathfinder::node::PathType;
use crate::entity::item::ItemEntity;
use crate::entity::mob::spawn::SpawnGroupData;
use crate::entity::{
    Entity, EntityBase,
    ageable::{AgeableData, AgeableMob},
    custom_sound::CustomSound,
    mob::{Mob, MobEntity},
    passive::animal::Animal,
    player::Player,
};
use crate::item::items::bucket::fill_bucket_stack;
use crate::world::World;

/// Vanilla `Goat.GOAT_FALL_DAMAGE_REDUCTION`.
const FALL_DAMAGE_REDUCTION: f32 = 10.0;
/// Vanilla `Goat.GOAT_SCREAMING_CHANCE`.
const SCREAMING_CHANCE: f64 = 0.02;
/// Vanilla `Goat.UNIHORN_CHANCE`.
const UNIHORN_CHANCE: f32 = 0.1;
/// Vanilla `Goat.ADULT_ATTACK_DAMAGE`.
const ADULT_ATTACK_DAMAGE: f64 = 2.0;
/// Vanilla `Goat.BABY_ATTACK_DAMAGE`.
const BABY_ATTACK_DAMAGE: f64 = 1.0;
/// Vanilla `Goat.BABY_DIMENSIONS`.
const BABY_DIMENSIONS: EntityDimensions = EntityDimensions {
    width: 0.45,
    height: 0.65,
    eye_height: 0.593_75,
};
/// The tag `minecraft:regular_goat_horns`, in the order of its data file.
const REGULAR_HORNS: [&str; 4] = [
    "minecraft:ponder_goat_horn",
    "minecraft:sing_goat_horn",
    "minecraft:seek_goat_horn",
    "minecraft:feel_goat_horn",
];
/// The tag `minecraft:screaming_goat_horns`, in the order of its data file.
const SCREAMING_HORNS: [&str; 4] = [
    "minecraft:admire_goat_horn",
    "minecraft:call_goat_horn",
    "minecraft:yearn_goat_horn",
    "minecraft:dream_goat_horn",
];

const MEMORIES: [MemoryModuleId; 15] = [
    types::LOOK_TARGET.id(),
    types::WALK_TARGET.id(),
    types::CANT_REACH_WALK_TARGET_SINCE.id(),
    types::PATH.id(),
    types::BREED_TARGET.id(),
    types::IS_PANICKING.id(),
    types::TEMPTING_PLAYER.id(),
    types::TEMPTATION_COOLDOWN_TICKS.id(),
    types::IS_TEMPTED.id(),
    types::LONG_JUMP_COOLDOWN_TICKS.id(),
    types::LONG_JUMP_MID_JUMP.id(),
    types::RAM_COOLDOWN_TICKS.id(),
    types::RAM_TARGET.id(),
    types::NEAREST_VISIBLE_LIVING_ENTITIES.id(),
    types::NEAREST_VISIBLE_ADULT.id(),
];

pub struct GoatEntity {
    pub mob_entity: MobEntity,
    pub ageable_data: AgeableData,
    pub is_screaming: AtomicBool,
    pub has_left_horn: AtomicBool,
    pub has_right_horn: AtomicBool,
}

impl GoatEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let mut mob_entity = MobEntity::new(entity);
        let navigator = mob_entity
            .navigator
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        navigator.set_can_float(true);
        navigator.set_pathfinding_malus(PathType::PowderSnow, -1.0);
        navigator.set_pathfinding_malus(PathType::DangerPowderSnow, -1.0);
        let living = &mob_entity.living_entity;
        living.set_attribute_base(&Attributes::MAX_HEALTH, 10.0);
        living.set_attribute_base(&Attributes::MOVEMENT_SPEED, 0.2);
        living.set_attribute_base(&Attributes::ATTACK_DAMAGE, ADULT_ATTACK_DAMAGE);
        Arc::new(Self {
            mob_entity,
            ageable_data: AgeableData::default(),
            is_screaming: AtomicBool::new(false),
            has_left_horn: AtomicBool::new(true),
            has_right_horn: AtomicBool::new(true),
        })
    }

    #[must_use]
    pub fn is_screaming(&self) -> bool {
        self.is_screaming.load(Ordering::Relaxed)
    }

    pub fn set_screaming(&self, screaming: bool) {
        self.is_screaming.store(screaming, Ordering::Relaxed);
        let entity = self.get_entity();
        entity.set_synced_data(
            pumpkin_data::tracked_data::goat::DATA_IS_SCREAMING_GOAT,
            screaming,
        );
    }

    #[must_use]
    pub fn has_left_horn(&self) -> bool {
        self.has_left_horn.load(Ordering::Relaxed)
    }

    pub fn set_has_left_horn(&self, has_horn: bool) {
        self.has_left_horn.store(has_horn, Ordering::Relaxed);
        let entity = self.get_entity();
        entity.set_synced_data(
            pumpkin_data::tracked_data::goat::DATA_HAS_LEFT_HORN,
            has_horn,
        );
    }

    #[must_use]
    pub fn has_right_horn(&self) -> bool {
        self.has_right_horn.load(Ordering::Relaxed)
    }

    pub fn set_has_right_horn(&self, has_horn: bool) {
        self.has_right_horn.store(has_horn, Ordering::Relaxed);
        let entity = self.get_entity();
        entity.set_synced_data(
            pumpkin_data::tracked_data::goat::DATA_HAS_RIGHT_HORN,
            has_horn,
        );
    }

    /// Vanilla `Goat.createHorn`: one goat always gives the same horn.
    fn create_horn(&self) -> ItemStack {
        let uuid = self.get_entity().entity_uuid.as_u128();
        let mixed = ((uuid >> 64) as u64) ^ (uuid as u64);
        let hash = ((mixed >> 32) as u32) ^ (mixed as u32);
        let mut random = LegacyRand::from_seed(u64::from(hash as i32 as u32));
        let horns = if self.is_screaming() {
            SCREAMING_HORNS
        } else {
            REGULAR_HORNS
        };
        let horn = horns[random.next_bounded_i32(4) as usize];
        let mut stack = ItemStack::new(1, &Item::GOAT_HORN);
        stack.set_data_component(InstrumentImpl {
            instrument: Cow::Borrowed(horn),
        });
        stack
    }

    /// Vanilla `Goat.dropHorn`.
    #[must_use]
    pub fn drop_horn(&self) -> bool {
        if self.is_baby() {
            return false;
        }
        let (left, right) = (self.has_left_horn(), self.has_right_horn());
        if !left && !right {
            return false;
        }
        let mut rng = self.get_random();
        let drop_left = if !left {
            false
        } else if !right {
            true
        } else {
            rng.random_bool(0.5)
        };
        if drop_left {
            self.set_has_left_horn(false);
        } else {
            self.set_has_right_horn(false);
        }
        let entity = self.get_entity();
        let velocity = Vector3::new(
            f64::from(rng.random_range(-0.2f32..0.2)),
            f64::from(rng.random_range(0.3f32..0.7)),
            f64::from(rng.random_range(-0.2f32..0.2)),
        );
        let world = entity.world.load_full();
        let item = ItemEntity::new_with_velocity(
            Entity::new(world.clone(), entity.pos.load(), &EntityType::ITEM),
            self.create_horn(),
            velocity,
            10,
        );
        world.spawn_entity(Arc::new(item));
        true
    }

    fn eating_sound(&self) -> Sound {
        if self.is_screaming() {
            Sound::EntityGoatScreamingEat
        } else {
            Sound::EntityGoatEat
        }
    }

    fn ambient_sound(&self) -> Sound {
        if self.is_screaming() {
            Sound::EntityGoatScreamingAmbient
        } else {
            Sound::EntityGoatAmbient
        }
    }
}

impl CustomSound for GoatEntity {
    fn hurt_sound(&self) -> Option<Sound> {
        Some(if self.is_screaming() {
            Sound::EntityGoatScreamingHurt
        } else {
            Sound::EntityGoatHurt
        })
    }

    fn death_sound(&self) -> Option<Sound> {
        Some(if self.is_screaming() {
            Sound::EntityGoatScreamingDeath
        } else {
            Sound::EntityGoatDeath
        })
    }
}

impl AgeableMob for GoatEntity {
    fn get_ageable_data(&self) -> &AgeableData {
        &self.ageable_data
    }

    /// Vanilla `Goat.ageBoundaryReached`, with `getDefaultDimensions` of a baby.
    fn age_boundary_reached(&self) {
        let living = &self.mob_entity.living_entity;
        let baby = self.is_baby();
        living.set_attribute_base(
            &Attributes::ATTACK_DAMAGE,
            if baby {
                BABY_ATTACK_DAMAGE
            } else {
                ADULT_ATTACK_DAMAGE
            },
        );
        living.entity.set_default_dimensions(if baby {
            BABY_DIMENSIONS
        } else {
            Entity::type_dimensions(living.entity.entity_type)
        });
    }
}

impl Animal for GoatEntity {
    /// Vanilla `Goat.playEatingSound`.
    fn play_eating_sound(&self) {
        let pitch = 0.8 + self.get_random().random::<f32>() * 0.4;
        self.play_entity_sound(self.eating_sound(), 1.0, pitch);
    }

    fn is_food(&self, item_stack: &ItemStack) -> bool {
        item_stack.item.has_tag(&tag::Item::MINECRAFT_GOAT_FOOD)
    }

    /// Vanilla `Goat.getBreedOffspring`.
    fn get_breed_offspring(&self, partner: &dyn EntityBase) -> Arc<dyn EntityBase> {
        let entity = self.get_entity();
        let baby = Self::new(Entity::new(
            entity.world.load_full(),
            entity.pos.load(),
            &EntityType::GOAT,
        ));
        let mut rng = self.get_random();
        let parent_screams = if rng.random_bool(0.5) {
            self.is_screaming()
        } else {
            partner
                .cast_any()
                .downcast_ref::<Self>()
                .is_some_and(Self::is_screaming)
        };
        baby.set_screaming(parent_screams || rng.random_bool(SCREAMING_CHANCE));
        baby.mob_entity
            .with_brain(baby.as_ref(), |brain| goat_ai::init_memories(brain));
        baby
    }
}

impl Mob for GoatEntity {
    fn make_brain(&self, packed: &PackedMemories) -> Brain {
        BrainProvider {
            memory_types: &MEMORIES,
            sensor_types: &[
                SensorType::NearestLivingEntities,
                SensorType::NearestPlayers,
                SensorType::NearestItems,
                SensorType::NearestAdult,
                SensorType::HurtBy,
                SensorType::FoodTemptations,
            ],
            activities: goat_ai::activities,
        }
        .make_brain(self, packed)
    }

    /// Vanilla `Goat.customServerAiStep`.
    fn custom_server_ai_step(&self, _caller: &dyn EntityBase) {
        self.mob_entity.tick_brain(self);
        self.mob_entity
            .with_brain(self, |brain| goat_ai::update_activity(brain));
    }

    /// Vanilla `Goat.finalizeSpawn`.
    fn finalize_spawn(
        &self,
        _world: &Arc<World>,
        group_data: Option<SpawnGroupData>,
    ) -> Option<SpawnGroupData> {
        self.mob_entity
            .with_brain(self, |brain| goat_ai::init_memories(brain));
        let mut rng = self.get_random();
        self.set_screaming(rng.random_bool(SCREAMING_CHANCE));
        self.age_boundary_reached();
        if !self.is_baby() && rng.random::<f32>() < UNIHORN_CHANCE {
            if rng.random_bool(0.5) {
                self.set_has_left_horn(false);
            } else {
                self.set_has_right_horn(false);
            }
        }
        self.finalize_spawn_super(group_data)
    }

    /// Vanilla `Goat.getMaxHeadYRot`.
    fn get_max_head_rotation(&self) -> f32 {
        15.0
    }

    /// Vanilla `Goat.calculateFallDamage`.
    fn mob_fall_damage_reduction(&self) -> f32 {
        FALL_DAMAGE_REDUCTION
    }

    /// Vanilla `Goat.getAmbientSound`.
    fn get_ambient_sound(&self) -> Option<Sound> {
        Some(self.ambient_sound())
    }

    fn as_custom_sound(&self) -> Option<&dyn CustomSound> {
        Some(self)
    }

    fn mob_play_step_sound(&self, _pos: &BlockPos, _state: &BlockState) {
        self.play_entity_sound(Sound::EntityGoatStep, 0.15, 1.0);
    }

    fn as_ageable(&self) -> Option<&dyn AgeableMob> {
        Some(self)
    }

    fn as_animal(&self) -> Option<&dyn Animal> {
        Some(self)
    }

    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        nbt.put_bool("IsScreamingGoat", self.is_screaming());
        nbt.put_bool("HasLeftHorn", self.has_left_horn());
        nbt.put_bool("HasRightHorn", self.has_right_horn());
    }

    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        if let Some(screaming) = nbt.get_bool("IsScreamingGoat") {
            self.set_screaming(screaming);
        }
        if let Some(left) = nbt.get_bool("HasLeftHorn") {
            self.set_has_left_horn(left);
        }
        if let Some(right) = nbt.get_bool("HasRightHorn") {
            self.set_has_right_horn(right);
        }
        self.age_boundary_reached();
    }

    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    fn mob_init_data_tracker(&self) {
        let entity = self.get_entity();
        let is_baby = entity.age.load(Ordering::Relaxed) < 0;
        if is_baby {
            entity.set_synced_data(pumpkin_data::tracked_data::goat::DATA_BABY_ID, true);
        }
        entity.set_synced_data(
            pumpkin_data::tracked_data::goat::DATA_IS_SCREAMING_GOAT,
            self.is_screaming(),
        );
        entity.set_synced_data(
            pumpkin_data::tracked_data::goat::DATA_HAS_LEFT_HORN,
            self.has_left_horn(),
        );
        entity.set_synced_data(
            pumpkin_data::tracked_data::goat::DATA_HAS_RIGHT_HORN,
            self.has_right_horn(),
        );
    }

    /// Vanilla `Goat.mobInteract`.
    fn mob_interact(&self, player: &Arc<Player>, item_stack: &mut ItemStack) -> bool {
        if item_stack.get_item() == &Item::BUCKET && !self.is_baby() {
            let sound = if self.is_screaming() {
                Sound::EntityGoatScreamingMilk
            } else {
                Sound::EntityGoatMilk
            };
            self.get_entity().world.load().play_sound_expect(
                player,
                sound,
                SoundCategory::Players,
                &player.position(),
            );
            fill_bucket_stack(item_stack, player, &Item::MILK_BUCKET);
            return true;
        }
        let food = self.is_food(item_stack);
        let used = self.animal_interact(player, item_stack);
        if used && food {
            self.play_eating_sound();
        }
        used
    }
}

/// Vanilla `Goat.LONG_JUMPING_DIMENSION_SCALE_FACTOR`.
pub const LONG_JUMPING_SCALE: f32 = 0.7;
