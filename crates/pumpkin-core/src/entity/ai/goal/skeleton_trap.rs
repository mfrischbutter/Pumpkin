//! Vanilla `SkeletonTrapGoal`: a trap horse springs when a player comes within ten blocks,
//! calling lightning and four skeleton horsemen.

use std::sync::{Arc, PoisonError, Weak, atomic::Ordering::Relaxed};

use pumpkin_data::data_component::DataComponent;
use pumpkin_data::data_component_impl::EquipmentSlot;
use pumpkin_data::enchantment_provider::EnchantmentProvider;
use pumpkin_data::entity::EntityType;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_util::math::vector3::Vector3;
use rand::RngExt;
use uuid::Uuid;

use crate::entity::ageable::AgeableMob;
use crate::entity::ai::goal::Goal;
use crate::entity::lightning::LightningBoltEntity;
use crate::entity::mob::equipment::RegionalDifficulty;
use crate::entity::mob::spawn::finalize_spawn;
use crate::entity::mob::{Mob, MobEntity};
use crate::entity::passive::skeleton_horse::SkeletonHorseEntity;
use crate::entity::r#type::from_type;
use crate::entity::{Entity, EntityBase};
use crate::world::World;

/// A player this close springs the trap.
const TRIGGER_RANGE: f64 = 10.0;
/// Vanilla `LIGHTNING_INVULNERABLE_TICKS`: the horsemen survive their own lightning.
const LIGHTNING_INVULNERABLE_TICKS: i32 = 60;
/// Horsemen besides the one on the trap horse.
const EXTRA_HORSEMEN: usize = 3;
/// Vanilla pushes each extra horse by `random.triangle(0.0, 1.1485)` on x and z.
const PUSH_SPREAD: f64 = 1.1485;

pub struct SkeletonTrapGoal {
    horse: Weak<SkeletonHorseEntity>,
}

impl SkeletonTrapGoal {
    #[must_use]
    pub const fn new(horse: Weak<SkeletonHorseEntity>) -> Self {
        Self { horse }
    }
}

impl Goal for SkeletonTrapGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        let Some(horse) = self.horse.upgrade() else {
            return false;
        };
        let entity = mob.get_entity();
        horse.is_trap()
            && entity
                .world
                .load()
                .has_nearby_alive_player(entity.pos.load(), TRIGGER_RANGE)
    }

    fn tick(&mut self, mob: &dyn Mob) {
        let Some(horse) = self.horse.upgrade() else {
            return;
        };
        let entity = mob.get_entity();
        let world = entity.world.load();
        let pos = entity.pos.load();
        let difficulty = RegionalDifficulty::at(&world, pos);
        horse.set_trap(false);
        horse.set_tame(true);
        horse.set_baby(false);
        let bolt = LightningBoltEntity::new(Entity::from_uuid(
            Uuid::new_v4(),
            world.clone(),
            pos,
            &EntityType::LIGHTNING_BOLT,
        ));
        bolt.set_visual_only(true);
        world.spawn_entity_non_save(Arc::new(bolt));
        let rider = create_skeleton(&world, &difficulty, pos);
        world.spawn_entity_non_save(rider.clone());
        entity.add_passenger(horse, rider);
        let mut rng = rand::rng();
        for _ in 0..EXTRA_HORSEMEN {
            let other_horse = create_horse(&world, pos);
            let other_rider = create_skeleton(&world, &difficulty, pos);
            world.spawn_entity_non_save(other_horse.clone());
            world.spawn_entity_non_save(other_rider.clone());
            other_horse
                .get_entity()
                .add_passenger(other_horse.clone(), other_rider);
            other_horse.get_entity().add_velocity(Vector3::new(
                triangle(&mut rng, PUSH_SPREAD),
                0.0,
                triangle(&mut rng, PUSH_SPREAD),
            ));
        }
    }
}

/// Vanilla `RandomSource.triangle(0.0, spread)`.
fn triangle<R: rand::Rng + ?Sized>(rng: &mut R, spread: f64) -> f64 {
    spread * (rng.random::<f64>() - rng.random::<f64>())
}

/// Vanilla `createSkeleton`: a persistent skeleton that survives the lightning, helmeted,
/// its weapon and helmet enchanted by the `mob_spawn_equipment` provider.
fn create_skeleton(
    world: &Arc<World>,
    difficulty: &RegionalDifficulty,
    pos: Vector3<f64>,
) -> Arc<dyn EntityBase> {
    let skeleton = from_type(&EntityType::SKELETON, pos, world, Uuid::new_v4());
    finalize_spawn(&skeleton, world, None);
    if let Some(mob) = skeleton.get_mob() {
        let mob_entity = mob.get_mob_entity();
        mob_entity
            .living_entity
            .hurt_cooldown
            .store(LIGHTNING_INVULNERABLE_TICKS, Relaxed);
        mob_entity.set_persistence_required();
        if equipped(mob_entity, &EquipmentSlot::HEAD).is_empty() {
            mob_entity.set_item_slot(&EquipmentSlot::HEAD, ItemStack::new(1, &Item::IRON_HELMET));
        }
        enchant(mob_entity, &EquipmentSlot::MAIN_HAND, difficulty);
        enchant(mob_entity, &EquipmentSlot::HEAD, difficulty);
    }
    skeleton
}

/// Vanilla `createHorse`: a persistent, tame adult skeleton horse that survives the lightning.
fn create_horse(world: &Arc<World>, pos: Vector3<f64>) -> Arc<dyn EntityBase> {
    let horse = SkeletonHorseEntity::new(Entity::from_uuid(
        Uuid::new_v4(),
        world.clone(),
        pos,
        &EntityType::SKELETON_HORSE,
    ));
    let entity: Arc<dyn EntityBase> = horse.clone();
    finalize_spawn(&entity, world, None);
    horse
        .mob_entity
        .living_entity
        .hurt_cooldown
        .store(LIGHTNING_INVULNERABLE_TICKS, Relaxed);
    horse.mob_entity.set_persistence_required();
    horse.set_tame(true);
    horse.set_baby(false);
    entity
}

fn equipped(mob: &MobEntity, slot: &EquipmentSlot) -> ItemStack {
    mob.living_entity
        .entity_equipment
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(slot)
}

/// Vanilla `SkeletonTrapGoal.enchant`: the slot's item loses its enchantments and gets the
/// provider's for the local difficulty.
fn enchant(mob: &MobEntity, slot: &EquipmentSlot, difficulty: &RegionalDifficulty) {
    let mut stack = equipped(mob, slot);
    stack.remove_data_component(DataComponent::Enchantments);
    EnchantmentProvider::MOB_SPAWN_EQUIPMENT
        .apply_to_stack(&mut stack, difficulty.special_multiplier);
    mob.set_item_slot(slot, stack);
}
