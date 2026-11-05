use std::sync::{Arc, Weak, atomic::Ordering};

use crate::entity::mob::spawn::{EntitySpawnReason, SpawnGroupData, finalize_spawn};
use crate::entity::r#type::from_type;
use crate::world::World;
use pumpkin_data::BlockState;
use pumpkin_data::attributes::Attributes;
use pumpkin_data::damage::DamageType;
use pumpkin_data::data_component_impl::EquipmentSlot;
use pumpkin_data::entity::EntityType;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::Sound;
use pumpkin_data::tag::{self, Taggable};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::math::position::BlockPos;
use rand::RngExt;
use uuid::Uuid;

use crate::entity::custom_sound::CustomSound;
use crate::entity::passive::abstract_horse::{self as horse_base, AbstractHorse, HorseData};
use crate::entity::{
    Entity, EntityBase,
    ageable::{AgeableData, AgeableMob},
    ai::goal::{swim::SwimGoal, tempt::TemptGoal},
    mob::{Mob, MobEntity},
    passive::animal::Animal,
    player::Player,
};

/// Vanilla `ZombieHorse.generateZombieHorseJumpStrength` and `generateZombieHorseSpeed`.
const BASE_JUMP_STRENGTH: f64 = 0.5;
const PER_RANDOM_JUMP_STRENGTH: f64 = 0.066_666_666_666_666_67;
const BASE_SPEED: f64 = 9.0;
const PER_RANDOM_SPEED: f64 = 1.0;
const SPEED_FACTOR: f32 = 42.16;

pub struct ZombieHorseEntity {
    pub mob_entity: MobEntity,
    pub ageable_data: AgeableData,
    horse: HorseData,
}

impl ZombieHorseEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let mob_entity = MobEntity::new(entity);
        let horse = Self {
            mob_entity,
            ageable_data: AgeableData::default(),
            horse: HorseData::default(),
        };
        let mob_arc = Arc::new(horse);
        let mob_weak: Weak<dyn Mob> = {
            let mob_arc: Arc<dyn Mob> = mob_arc.clone();
            Arc::downgrade(&mob_arc)
        };
        let goal_selector = &mob_arc.mob_entity.goals_selector;
        horse_base::register_horse_goals(goal_selector, mob_weak, true);
        // Vanilla `ZombieHorse.addBehaviourGoals`: floating and its own food, no panic.
        goal_selector.add_goal(0, Box::new(SwimGoal::default()));
        goal_selector.add_goal(
            3,
            Box::new(TemptGoal::with_tag(
                1.25,
                &tag::Item::MINECRAFT_ZOMBIE_HORSE_FOOD,
                false,
            )),
        );
        mob_arc
    }

    #[must_use]
    pub fn is_tame(&self) -> bool {
        horse_base::is_tamed(self)
    }

    pub fn set_tame(&self, val: bool) {
        horse_base::set_flag(self, horse_base::FLAG_TAME, val);
    }

    /// Vanilla `ZombieHorse.randomizeAttributes`: its own jump and speed, the health stays.
    fn randomize_attributes(&self) {
        let (jump, speed) = {
            let mut rng = self.get_random();
            let jump = BASE_JUMP_STRENGTH
                + rng.random::<f64>() * PER_RANDOM_JUMP_STRENGTH
                + rng.random::<f64>() * PER_RANDOM_JUMP_STRENGTH
                + rng.random::<f64>() * PER_RANDOM_JUMP_STRENGTH;
            let speed = (BASE_SPEED
                + rng.random::<f64>() * PER_RANDOM_SPEED
                + rng.random::<f64>() * PER_RANDOM_SPEED
                + rng.random::<f64>() * PER_RANDOM_SPEED)
                / f64::from(SPEED_FACTOR);
            (jump, speed)
        };
        let living = &self.mob_entity.living_entity;
        living.set_attribute_base(&Attributes::JUMP_STRENGTH, jump);
        living.set_attribute_base(&Attributes::MOVEMENT_SPEED, speed);
    }
}

impl AgeableMob for ZombieHorseEntity {
    fn get_ageable_data(&self) -> &AgeableData {
        &self.ageable_data
    }

    fn default_baby_spawn_chance(&self) -> f32 {
        crate::entity::ageable::ABSTRACT_HORSE_BABY_SPAWN_CHANCE
    }

    /// Vanilla `ZombieHorse.canAgeUp`: a foal stays one.
    fn can_age_up(&self) -> bool {
        false
    }

    fn age_boundary_reached(&self) {
        horse_base::refresh_age_dimensions(self);
    }
}

impl Animal for ZombieHorseEntity {
    /// Vanilla `ZombieHorse.isFood`.
    fn is_food(&self, item_stack: &ItemStack) -> bool {
        item_stack
            .item
            .has_tag(&tag::Item::MINECRAFT_ZOMBIE_HORSE_FOOD)
    }

    /// Vanilla `AbstractHorse.canMate`: it never breeds.
    fn can_mate(&self, _partner: &dyn EntityBase) -> bool {
        false
    }
}

impl AbstractHorse for ZombieHorseEntity {
    fn horse_data(&self) -> &HorseData {
        &self.horse
    }

    fn horse_eating_sound(&self) -> Option<Sound> {
        Some(Sound::EntityZombieHorseEat)
    }

    fn horse_angry_sound(&self) -> Option<Sound> {
        Some(Sound::EntityZombieHorseAngry)
    }

    /// Vanilla `ZombieHorse.isMobControlled`: a mob rides in front, like its zombie jockey.
    fn is_mob_controlled(&self) -> bool {
        self.get_entity()
            .passengers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .first()
            .is_some_and(|rider| rider.get_mob().is_some())
    }
}

impl CustomSound for ZombieHorseEntity {
    fn hurt_sound(&self) -> Option<Sound> {
        Some(Sound::EntityZombieHorseHurt)
    }

    fn death_sound(&self) -> Option<Sound> {
        Some(Sound::EntityZombieHorseDeath)
    }

    fn sound_volume(&self) -> f32 {
        horse_base::SOUND_VOLUME
    }
}

impl Mob for ZombieHorseEntity {
    /// Vanilla `ZombieHorse.chargeSpeedModifier`.
    fn charge_speed_modifier(&self) -> f32 {
        1.4f32
    }

    /// Vanilla `AbstractHorse.isPushable`.
    fn mob_is_pushable(&self) -> bool {
        !self.get_entity().has_passengers()
    }

    /// Vanilla `supportQuadLeash`.
    fn mob_support_quad_leash(&self) -> bool {
        true
    }

    /// Vanilla `ZombieHorse.canBeLeashed`: not while its jockey rides it wild.
    fn can_be_leashed(&self) -> bool {
        self.is_tame() || !self.is_mob_controlled()
    }

    /// Vanilla `AbstractHorse.finalizeSpawn`: its own jump and speed.
    fn finalize_spawn(
        &self,
        _world: &Arc<World>,
        group_data: Option<SpawnGroupData>,
    ) -> Option<SpawnGroupData> {
        self.randomize_attributes();
        self.finalize_spawn_super(group_data)
    }

    /// Vanilla `ZombieHorse.finalizeSpawn`: a natural zombie horse carries a zombie with an
    /// iron spear.
    fn after_finalize_spawn(
        &self,
        world: &Arc<World>,
        reason: EntitySpawnReason,
        group_data: Option<SpawnGroupData>,
    ) -> Option<SpawnGroupData> {
        if reason == EntitySpawnReason::Natural {
            let entity = self.get_entity();
            let zombie = from_type(
                &EntityType::ZOMBIE,
                entity.pos.load(),
                world,
                Uuid::new_v4(),
            );
            zombie.get_entity().set_rotation(entity.yaw.load(), 0.0);
            finalize_spawn(&zombie, world, reason, None);
            if let Some(mob) = zombie.get_mob() {
                mob.get_mob_entity().set_item_slot(
                    &EquipmentSlot::MAIN_HAND,
                    ItemStack::new(1, &Item::IRON_SPEAR),
                );
            }
            self.mob_entity.add_pending_rider(zombie);
        }
        group_data
    }

    /// Vanilla `ZombieHorse.getAmbientSound`.
    fn get_ambient_sound(&self) -> Option<Sound> {
        Some(Sound::EntityZombieHorseAmbient)
    }

    fn get_ambient_sound_interval(&self) -> i32 {
        horse_base::AMBIENT_SOUND_INTERVAL
    }

    fn as_custom_sound(&self) -> Option<&dyn CustomSound> {
        Some(self)
    }

    /// Vanilla `ZombieHorse.removeWhenFarAway`.
    fn remove_when_far_away(&self, _distance_sq: f64) -> bool {
        true
    }

    fn is_tamed(&self) -> bool {
        self.is_tame()
    }

    /// Vanilla `AbstractHorse.getOwnerReference`.
    fn get_owner_uuid(&self) -> Option<Uuid> {
        horse_base::owner(self)
    }

    fn mob_play_step_sound(&self, pos: &BlockPos, state: &BlockState) {
        horse_base::play_step_sound(self, pos, state, true);
    }

    fn as_ageable(&self) -> Option<&dyn AgeableMob> {
        Some(self)
    }

    /// Vanilla `ZombieHorse.sunProtectionSlot`: its armour keeps the sun off.
    fn sun_protection_slot(&self) -> EquipmentSlot {
        EquipmentSlot::BODY
    }

    fn as_animal(&self) -> Option<&dyn Animal> {
        Some(self)
    }

    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        self.write_ageable_nbt(nbt);
        horse_base::write_horse_nbt(self, nbt);
    }

    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        self.read_ageable_nbt(nbt);
        horse_base::read_horse_nbt(self, nbt);
    }

    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    fn mob_tick(&self, _caller: &dyn EntityBase) {
        horse_base::horse_tick(self);
        horse_base::horse_ai_step(self);
    }

    fn on_damage(&self, _damage_type: DamageType, _source: Option<&dyn EntityBase>) {
        horse_base::on_hurt(self);
    }

    fn on_elastic_leash_pull(&self) {
        horse_base::on_leash_pull(self);
    }

    fn mob_cause_fall_damage(&self, fall_distance: f32) {
        horse_base::play_land_sound(self, fall_distance);
    }

    fn as_abstract_horse(&self) -> Option<&dyn AbstractHorse> {
        Some(self)
    }

    fn is_mob_immobile(&self) -> bool {
        horse_base::is_immobile(self)
    }

    fn mob_start_rider_jump(&self, _strength: i32) {
        if self.is_saddled() {
            horse_base::on_rider_jump(self);
        }
    }

    /// Vanilla `AbstractHorse.canUseSlot(SADDLE)`: a tamed adult.
    fn can_be_saddled(&self) -> bool {
        self.get_entity().is_alive() && !self.is_baby() && self.is_tame()
    }

    /// Vanilla `AbstractHorse.getControllingPassenger`: a player in a saddle.
    fn mob_controlling_player(&self) -> Option<Arc<Player>> {
        horse_base::saddled_rider(self)
    }

    /// Vanilla `AbstractHorse.tickRidden`, the turn to the rider's look.
    fn mob_tick_ridden(&self, controller: &Player) {
        crate::entity::mob::saddle::face_rider(self, controller, 0.5);
    }

    fn mob_init_data_tracker(&self) {
        let entity = self.get_entity();
        let is_baby = entity.age.load(Ordering::Relaxed) < 0;
        if is_baby {
            entity.set_synced_data(pumpkin_data::tracked_data::zombie_horse::DATA_BABY_ID, true);
        }
        entity.set_synced_data(
            pumpkin_data::tracked_data::zombie_horse::DATA_ID_FLAGS,
            horse_base::flags(self) as i8,
        );
    }

    /// Vanilla `ZombieHorse.interact` and `mobInteract` with `AbstractHorse.mobInteract`: its
    /// food feeds it, a wild one gets mad, and it takes horse armour like a horse.
    fn mob_interact(&self, player: &Arc<Player>, item_stack: &mut ItemStack) -> bool {
        // Vanilla `ZombieHorse.interact`: every click keeps it loaded.
        self.mob_entity
            .persistence_required
            .store(true, Ordering::Relaxed);
        if let Some(result) = horse_base::important_interaction(self, player, item_stack) {
            return result;
        }
        if let Some(result) =
            horse_base::held_item_interaction(self, player, item_stack, false, |_| false)
        {
            return result;
        }
        // Vanilla `Animal.mobInteract` does nothing for it: it neither falls in love
        // (`canFallInLove`) nor ages up.
        if self.get_entity().has_passengers() || self.is_baby() {
            return false;
        }
        horse_base::abstract_horse_interact(self, player, item_stack)
    }
}
