use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicI32, Ordering},
};

use pumpkin_data::BlockState;
use pumpkin_data::frog_variant::FrogVariant;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::Sound;
use pumpkin_data::tag::{self, Taggable};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_protocol::codec::optional_int::OptionalInt;
use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_util::math::position::BlockPos;
use rand::RngExt;

use super::frog_ai;
use crate::entity::ai::brain::memory::{MemoryModuleId, PackedMemories, types};
use crate::entity::ai::brain::sensing::SensorType;
use crate::entity::ai::brain::{Brain, BrainProvider};
use crate::entity::ai::control::smooth_swimming_move_control::SmoothSwimmingMoveControl;
use crate::entity::ai::pathfinder::{Navigator, node::PathType};
use crate::entity::mob::slime::SlimeEntity;
use crate::entity::mob::spawn::SpawnGroupData;
use crate::entity::{
    Entity, EntityBase,
    ageable::{AgeableData, AgeableMob},
    mob::{Mob, MobEntity},
    passive::animal::Animal,
    player::Player,
};
use crate::world::World;

const MEMORIES: [MemoryModuleId; 3] = [
    types::IS_PREGNANT.id(),
    types::LONG_JUMP_MID_JUMP.id(),
    types::LONG_JUMP_COOLDOWN_TICKS.id(),
];

/// Vanilla `Frog.canEat`: frog food, of a cube only the smallest.
#[must_use]
pub fn can_eat(entity: &dyn EntityBase) -> bool {
    let is_small = entity
        .cast_any()
        .downcast_ref::<SlimeEntity>()
        .is_none_or(|cube| cube.get_size() == 1);
    is_small
        && entity
            .get_entity()
            .entity_type
            .has_tag(&tag::EntityType::MINECRAFT_FROG_FOOD)
}

/// Represents a Frog, an amphibious mob that can eat small slimes and magma cubes.
///
/// Wiki: <https://minecraft.wiki/w/Frog>
pub struct FrogEntity {
    pub mob_entity: MobEntity,
    pub ageable_data: AgeableData,
    pub variant: AtomicI32,
    /// Bred, for the next brain step: breeding runs inside the brain tick.
    bred: AtomicBool,
}

impl FrogEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let mut mob_entity = MobEntity::new(entity);
        Self::set_up_movement(&mut mob_entity);
        Arc::new(Self {
            mob_entity,
            ageable_data: AgeableData::default(),
            variant: AtomicI32::new(FrogVariant::Temperate.id() as i32),
            bred: AtomicBool::new(false),
        })
    }

    /// Vanilla `Frog` constructor and `createNavigation`.
    fn set_up_movement(mob_entity: &mut MobEntity) {
        *mob_entity
            .move_control
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            Box::new(SmoothSwimmingMoveControl::new(85, 10, 0.02, 0.1, true));
        let navigator = mob_entity
            .navigator
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *navigator = Navigator::amphibious(true);
        navigator.set_pathfinding_malus(PathType::Water, 4.0);
        navigator.set_pathfinding_malus(PathType::Trapdoor, -1.0);
    }

    #[must_use]
    pub fn get_variant(&self) -> FrogVariant {
        FrogVariant::from_id(self.variant.load(Ordering::Relaxed) as u32)
    }

    pub fn set_variant(&self, variant: FrogVariant) {
        self.set_variant_id(variant.id() as i32);
    }

    pub fn set_variant_id(&self, raw_id: i32) {
        self.variant.store(raw_id, Ordering::Relaxed);
        let entity = self.get_entity();
        entity.set_synced_data(pumpkin_data::tracked_data::frog::VARIANT, VarInt(raw_id));
    }

    /// Vanilla `setTongueTarget` and `eraseTongueTarget`.
    pub fn set_tongue_target(&self, target: Option<i32>) {
        self.get_entity().set_synced_data(
            pumpkin_data::tracked_data::frog::DATA_TONGUE_TARGET_ID,
            OptionalInt(target),
        );
    }

    /// Vanilla `FrogVariants`: by the biome.
    fn variant_for_biome(world: &World, pos: &BlockPos) -> FrogVariant {
        let biome = world.get_biome(pos);
        if biome.has_tag(&tag::WorldgenBiome::MINECRAFT_SPAWNS_WARM_VARIANT_FROGS) {
            FrogVariant::Warm
        } else if biome.has_tag(&tag::WorldgenBiome::MINECRAFT_SPAWNS_COLD_VARIANT_FROGS) {
            FrogVariant::Cold
        } else {
            FrogVariant::Temperate
        }
    }
}

impl AgeableMob for FrogEntity {
    fn get_ageable_data(&self) -> &AgeableData {
        &self.ageable_data
    }

    fn can_be_a_baby(&self) -> bool {
        false
    }
}

impl Animal for FrogEntity {
    fn play_eating_sound(&self) {
        self.play_entity_sound(Sound::EntityFrogEat, 2.0, 1.0);
    }

    fn is_food(&self, item_stack: &ItemStack) -> bool {
        item_stack.item.has_tag(&tag::Item::MINECRAFT_FROG_FOOD)
    }

    /// Vanilla `Frog.spawnChildFromBreeding`: no baby, it lays spawn instead.
    fn breeds_eggs(&self) -> bool {
        true
    }

    fn on_bred_egg(&self) {
        self.bred.store(true, Ordering::Relaxed);
    }
}

impl Mob for FrogEntity {
    fn make_brain(&self, packed: &PackedMemories) -> Brain {
        BrainProvider {
            memory_types: &MEMORIES,
            sensor_types: &[
                SensorType::NearestLivingEntities,
                SensorType::HurtBy,
                SensorType::FrogAttackables,
                SensorType::FrogTemptations,
                SensorType::IsInWater,
            ],
            activities: frog_ai::activities,
        }
        .make_brain(self, packed)
    }

    /// Vanilla `Frog.customServerAiStep`.
    fn custom_server_ai_step(&self, _caller: &dyn EntityBase) {
        self.mob_entity.tick_brain(self);
        let bred = self.bred.swap(false, Ordering::Relaxed);
        let target = self.mob_entity.with_brain(self, |brain| {
            if bred {
                brain.set(types::IS_PREGNANT, ());
            }
            frog_ai::update_activity(brain);
            brain.get(types::ATTACK_TARGET).cloned()
        });
        // Vanilla `getTarget` reads the brain.
        self.mob_entity.set_target(target);
    }

    /// Vanilla `Frog.finalizeSpawn` with `FrogAi.initMemories`.
    fn finalize_spawn(
        &self,
        world: &Arc<World>,
        group_data: Option<SpawnGroupData>,
    ) -> Option<SpawnGroupData> {
        self.set_variant(Self::variant_for_biome(
            world,
            &self.get_entity().block_pos.load(),
        ));
        let cooldown = self
            .get_random()
            .random_range(frog_ai::TIME_BETWEEN_LONG_JUMPS);
        self.mob_entity.with_brain(self, |brain| {
            brain.set(types::LONG_JUMP_COOLDOWN_TICKS, cooldown);
        });
        self.finalize_spawn_super(group_data)
    }

    /// Through the frog itself: the loot of an eaten magma cube reads its variant.
    fn do_hurt_target(&self, target: &dyn EntityBase) -> bool {
        self.mob_entity.try_attack(self, target)
    }

    /// Vanilla `Frog.travelInWater`.
    fn travel_in_water(&self, caller: &dyn EntityBase, _gravity: f64, _falling: bool) {
        self.mob_entity.living_entity.travel_in_water_freely(caller);
    }

    /// Vanilla `Frog.isPushedByFluid`.
    fn mob_is_pushed_by_fluids(&self) -> bool {
        false
    }

    /// Vanilla `Frog.FROG_FALL_DAMAGE_REDUCTION`.
    fn mob_fall_damage_reduction(&self) -> f32 {
        5.0
    }

    /// Vanilla `Frog.getMaxHeadYRot`.
    fn get_max_head_rotation(&self) -> f32 {
        5.0
    }

    /// Vanilla `Frog.getAmbientSound`.
    fn get_ambient_sound(&self) -> Option<pumpkin_data::sound::Sound> {
        Some(pumpkin_data::sound::Sound::EntityFrogAmbient)
    }

    fn mob_play_step_sound(&self, _pos: &BlockPos, _state: &BlockState) {
        self.play_entity_sound(Sound::EntityFrogStep, 0.15, 1.0);
    }

    fn mob_get_component(
        &self,
        component: pumpkin_data::data_component::DataComponent,
    ) -> Option<Box<dyn pumpkin_data::data_component_impl::DataComponentImpl>> {
        use pumpkin_data::{data_component::DataComponent, data_component_impl::FrogVariantImpl};
        match component {
            DataComponent::FrogVariant => Some(Box::new(FrogVariantImpl {
                value: format!("minecraft:{}", self.get_variant().to_name()).into(),
            })),
            _ => None,
        }
    }

    fn as_ageable(&self) -> Option<&dyn AgeableMob> {
        Some(self)
    }

    fn as_animal(&self) -> Option<&dyn Animal> {
        Some(self)
    }

    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        let variant_id = self.variant.load(Ordering::Relaxed) as u32;
        let variant_str =
            self.resolve_dynamic_entry_id("minecraft:frog_variant", variant_id, |_id| {
                self.get_variant().asset_id().to_string()
            });
        nbt.put_string("variant", variant_str);
    }

    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        if let Some(variant_str) = nbt.get_string("variant")
            && let Some(id) =
                self.resolve_dynamic_raw_id("minecraft:frog_variant", variant_str, |n| {
                    FrogVariant::from_name(n).map(|v| v.id())
                })
        {
            self.set_variant_id(id as i32);
        }
    }

    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    fn mob_set_variant_name(&self, name: &str) {
        if let Some(id) = self.resolve_dynamic_raw_id("minecraft:frog_variant", name, |n| {
            FrogVariant::from_name(n).map(|v| v.id())
        }) {
            self.set_variant_id(id as i32);
        }
    }

    fn mob_init_data_tracker(&self) {
        let entity = self.get_entity();
        let is_baby = entity.age.load(Ordering::Relaxed) < 0;
        if is_baby {
            entity.set_synced_data(pumpkin_data::tracked_data::frog::BABY_ID, true);
        }
        entity.set_synced_data(
            pumpkin_data::tracked_data::frog::VARIANT,
            VarInt(self.variant.load(Ordering::Relaxed)),
        );
    }

    fn mob_interact(&self, player: &Arc<Player>, item_stack: &mut ItemStack) -> bool {
        self.animal_interact(player, item_stack)
    }
}
