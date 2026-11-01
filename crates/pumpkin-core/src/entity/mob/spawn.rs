//! Vanilla `Mob.finalizeSpawn`: the step run on freshly created mobs before they enter the world.

use std::sync::Arc;

use pumpkin_data::attributes::Attributes;
use pumpkin_data::effect::StatusEffect;
use pumpkin_data::entity_variant::RabbitVariant;
use rand::RngExt;

use crate::entity::EntityBase;
use crate::entity::attributes::{Modifier, ModifierOperation};
use crate::entity::mob::MobEntity;
use crate::entity::mob::equipment::{EQUIPMENT_REGISTRY, RegionalDifficulty, equip_mob_on_spawn};
use crate::entity::passive::fox::FoxVariant;
use crate::entity::passive::llama::LlamaVariant;
use crate::entity::r#type::from_type;
use crate::world::World;
use crate::world::level_accessor::LevelAccessor;
use pumpkin_data::entity::EntityType;
use pumpkin_util::math::boundingbox::{BoundingBox, EntityDimensions};
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_util::math::wrap_degrees;
use uuid::Uuid;

const RANDOM_SPAWN_BONUS_ID: &str = "minecraft:random_spawn_bonus";

/// Why a mob is being created (vanilla `EntitySpawnReason`). The placement rules relax
/// their floor and light tests for spawner blocks and trial spawners.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntitySpawnReason {
    Natural,
    ChunkGeneration,
    Spawner,
    Structure,
    Breeding,
    MobSummoned,
    Jockey,
    Event,
    Conversion,
    Reinforcement,
    Triggered,
    Bucket,
    SpawnItemUse,
    Command,
    Dispenser,
    Patrol,
    TrialSpawner,
    Load,
    DimensionTravel,
}

impl EntitySpawnReason {
    /// Vanilla `EntitySpawnReason.isSpawner`.
    #[must_use]
    pub const fn is_spawner(self) -> bool {
        matches!(self, Self::Spawner | Self::TrialSpawner)
    }

    /// Vanilla `EntitySpawnReason.ignoresLightRequirements`.
    #[must_use]
    pub const fn ignores_light_requirements(self) -> bool {
        matches!(self, Self::TrialSpawner)
    }
}

/// State shared by every mob of one spawn group (vanilla `SpawnGroupData`).
pub enum SpawnGroupData {
    Ageable {
        group_size: u32,
        should_spawn_baby: bool,
        baby_spawn_chance: f32,
    },
    School(std::sync::Weak<dyn EntityBase>),
    TropicalFish {
        leader: std::sync::Weak<dyn EntityBase>,
        variant: i32,
    },
    /// The effect the first spider of a group rolled on hard difficulty.
    SpiderEffects(Option<&'static StatusEffect>),
    /// Vanilla `FoxGroupData`: the group's variant and how many joined so far.
    Fox {
        variant: FoxVariant,
        size: u32,
    },
    /// Vanilla `RabbitGroupData`: the group's variant and how many joined so far.
    Rabbit {
        variant: RabbitVariant,
        size: u32,
    },
    /// Vanilla `LlamaGroupData`: the group's variant and how many joined so far.
    Llama {
        variant: LlamaVariant,
        group_size: u32,
    },
}

impl MobEntity {
    /// Vanilla `Mob.finalizeSpawn`: random follow range bonus and left-handedness.
    pub fn finalize_spawn_base(&self) {
        let mut rng = rand::rng();
        {
            let mut attributes = self
                .living_entity
                .attributes
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(follow_range) = attributes.get_mut(&Attributes::FOLLOW_RANGE.id)
                && !follow_range
                    .modifiers
                    .iter()
                    .any(|modifier| modifier.id == RANDOM_SPAWN_BONUS_ID)
            {
                follow_range.add_or_replace_permanent_modifier(Modifier {
                    id: RANDOM_SPAWN_BONUS_ID.to_string(),
                    // Triangle distribution around 0.
                    amount: 0.114_85 * f64::from(rng.random::<f32>() - rng.random::<f32>()),
                    operation: ModifierOperation::MultiplyBase,
                    // Vanilla `Mob.finalizeSpawn` adds this as a permanent modifier.
                });
            }
        }
        self.set_left_handed(rng.random::<f32>() < 0.05);
        self.populate_spawn_equipment();
    }

    /// Loot pickup and spawn equipment of the mobs in `EQUIPMENT_REGISTRY`, rolled once in
    /// `finalizeSpawn` like vanilla (`setCanPickUpLoot`, `populateDefaultEquipmentSlots`), never
    /// when a saved mob is loaded.
    fn populate_spawn_equipment(&self) {
        let entity = &self.living_entity.entity;
        let Some(def) = EQUIPMENT_REGISTRY.get(entity.entity_type.resource_name) else {
            return;
        };
        let world = entity.world.load();
        if def.can_pick_up_loot {
            let difficulty = RegionalDifficulty::at(&world, entity.pos.load());
            self.set_can_pick_up_loot(rand::random::<f32>() < 0.55 * difficulty.special_multiplier);
        }
        equip_mob_on_spawn(self, &world);
    }

    /// Queues a mob to be added and mounted onto this one when it enters the world.
    pub fn add_pending_rider(&self, rider: Arc<dyn EntityBase>) {
        self.pending_riders
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(rider);
    }

    pub(crate) fn take_pending_riders(&self) -> Vec<Arc<dyn EntityBase>> {
        std::mem::take(
            &mut *self
                .pending_riders
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        )
    }
}

/// Finalizes `entity` if it is a mob, and returns the group data for the next mob of the group.
///
/// `reason` is vanilla `finalizeSpawn`'s spawn reason: the shared `PatrollingMonster` step
/// reads it; the species' own finalization does not yet.
pub fn finalize_spawn(
    entity: &Arc<dyn EntityBase>,
    world: &Arc<World>,
    reason: EntitySpawnReason,
    group_data: Option<SpawnGroupData>,
) -> Option<SpawnGroupData> {
    let Some(mob) = entity.get_mob() else {
        return group_data;
    };
    let group_data = mob.finalize_spawn(world, group_data);
    // Vanilla `PatrollingMonster.finalizeSpawn`, part of every patrolling monster's chain.
    if let Some(patroller) = mob.as_patrolling_monster() {
        patroller.finalize_patrol_spawn(reason);
    }
    group_data
}

impl MobEntity {
    /// Baby state kept only as a negative age and the shared baby flag (hoglins, zoglins and
    /// zombified piglins, which are not `AgeableMob`s here).
    pub fn set_baby_by_age(&self) {
        let entity = &self.living_entity.entity;
        entity
            .age
            .store(-24000, std::sync::atomic::Ordering::Relaxed);
        entity.set_synced_data(pumpkin_data::tracked_data::ageable_mob::DATA_BABY_ID, true);
    }

    /// Baby state kept as a mob's own flag and synced key (vanilla zombies and piglins), so it
    /// never ages up.
    pub fn set_baby_flag(
        &self,
        flag: &std::sync::atomic::AtomicBool,
        tracked: pumpkin_data::tracked_data::TrackedData,
        baby: bool,
    ) {
        flag.store(baby, std::sync::atomic::Ordering::Relaxed);
        let entity = &self.living_entity.entity;
        entity.set_synced_data(tracked, baby);
        // Pumpkin's negative age is what reports the Bedrock baby flag.
        entity.age.store(
            if baby { -24000 } else { 0 },
            std::sync::atomic::Ordering::Relaxed,
        );
    }
}

/// Vanilla `VoxelShape.collideX`'s tolerance.
const COLLISION_EPSILON: f64 = 1.0E-7;

/// Vanilla `EntityType.getYOffset`: how far a mob set one block above `spawn_pos` sinks onto
/// the block collision shapes of that block (and of the one below when it moved up a face)
/// before it rests: `1 + Shapes.collide(Y, box, shapes, -1 or -2)`.
fn spawn_y_offset(
    level: &dyn LevelAccessor,
    spawn_pos: &BlockPos,
    moved_up: bool,
    entity_box: &BoundingBox,
) -> f64 {
    let (lowest, mut distance): (BlockPos, f64) = if moved_up {
        (spawn_pos.down(), -2.0)
    } else {
        (*spawn_pos, -1.0)
    };
    for pos in BlockPos::iterate(lowest, *spawn_pos) {
        let state = level.get_block_state(&pos);
        for shape in state
            .get_block_collision_shapes_at(&pos)
            .map(|shape| shape.at_pos(pos))
        {
            if distance.abs() < COLLISION_EPSILON {
                return 1.0;
            }
            let overlaps = shape.min.x < entity_box.max.x - COLLISION_EPSILON
                && shape.max.x > entity_box.min.x + COLLISION_EPSILON
                && shape.min.z < entity_box.max.z - COLLISION_EPSILON
                && shape.max.z > entity_box.min.z + COLLISION_EPSILON;
            if !overlaps {
                continue;
            }
            // The box only moves down to the top of a shape below its bottom.
            let to_top = shape.max.y - entity_box.min.y;
            if to_top <= COLLISION_EPSILON {
                distance = distance.max(to_top);
            }
        }
    }
    1.0 + distance
}

/// Vanilla `EntityType.create` before its `finalizeSpawn`.
///
/// The entity at the block's bottom centre with a random yaw. With `try_move_down` it is
/// set one block up and sinks onto the block's collision shapes, two blocks far when it
/// `moved_up` onto a top face.
#[must_use]
pub fn create_at(
    entity_type: &'static EntityType,
    world: &Arc<World>,
    spawn_pos: &BlockPos,
    try_move_down: bool,
    moved_up: bool,
) -> Arc<dyn EntityBase> {
    let x = f64::from(spawn_pos.0.x) + 0.5;
    let z = f64::from(spawn_pos.0.z) + 0.5;
    let y_offset = if try_move_down {
        let dimensions = EntityDimensions::new(
            entity_type.dimension[0],
            entity_type.dimension[1],
            entity_type.eye_height,
        );
        let raised = BoundingBox::new_from_pos(x, f64::from(spawn_pos.0.y + 1), z, &dimensions);
        spawn_y_offset(world.as_ref(), spawn_pos, moved_up, &raised)
    } else {
        0.0
    };
    let entity = from_type(
        entity_type,
        Vector3::new(x, f64::from(spawn_pos.0.y) + y_offset, z),
        world,
        Uuid::new_v4(),
    );
    entity
        .get_entity()
        .set_rotation(wrap_degrees(rand::random::<f32>() * 360.0), 0.0);
    entity
}
