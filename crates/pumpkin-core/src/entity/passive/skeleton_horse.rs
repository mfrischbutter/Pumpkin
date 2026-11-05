use std::sync::{
    Arc, Weak,
    atomic::{AtomicBool, AtomicI32, Ordering},
};

use pumpkin_data::BlockState;
use pumpkin_data::attributes::Attributes;
use pumpkin_data::damage::DamageType;
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
    ai::goal::skeleton_trap::SkeletonTrapGoal,
    mob::{Mob, MobEntity, spawn::SpawnGroupData},
    passive::animal::Animal,
    player::Player,
};
use crate::world::World;

/// An untouched trap horse leaves after this many ticks (vanilla `SkeletonHorse.aiStep`).
const TRAP_DESPAWN_TICKS: i32 = 18_000;
/// Vanilla `SkeletonHorse.playSwimSound`: its splashes on the bottom are this loud.
const GROUND_SWIM_VOLUME: f32 = 0.3;

pub struct SkeletonHorseEntity {
    pub mob_entity: MobEntity,
    pub ageable_data: AgeableData,
    horse: HorseData,
    /// Vanilla `isTrap`: a horse lightning left, waiting for a player to come close.
    pub trap: AtomicBool,
    /// Vanilla `trapTime`: the ticks spent waiting as a trap.
    pub trap_time: AtomicI32,
}

impl SkeletonHorseEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let mob_entity = MobEntity::new(entity);
        let horse = Self {
            mob_entity,
            ageable_data: AgeableData::default(),
            horse: HorseData::default(),
            trap: AtomicBool::new(false),
            trap_time: AtomicI32::new(0),
        };
        let mob_arc = Arc::new(horse);
        let mob_weak: Weak<dyn Mob> = {
            let mob_arc: Arc<dyn Mob> = mob_arc.clone();
            Arc::downgrade(&mob_arc)
        };
        // Vanilla `SkeletonHorse.addBehaviourGoals` adds nothing: it neither floats, panics
        // nor follows food.
        let goal_selector = &mob_arc.mob_entity.goals_selector;
        horse_base::register_horse_goals(goal_selector, mob_weak, true);
        // Vanilla adds this goal while the horse is a trap; here it only starts then.
        goal_selector.add_goal(1, Box::new(SkeletonTrapGoal::new(Arc::downgrade(&mob_arc))));
        mob_arc
    }

    #[must_use]
    pub fn is_tame(&self) -> bool {
        horse_base::is_tamed(self)
    }

    pub fn set_tame(&self, val: bool) {
        horse_base::set_flag(self, horse_base::FLAG_TAME, val);
    }

    /// Vanilla `SkeletonHorse.isTrap`.
    #[must_use]
    pub fn is_trap(&self) -> bool {
        self.trap.load(Ordering::Relaxed)
    }

    /// Vanilla `SkeletonHorse.setTrap`.
    pub fn set_trap(&self, trap: bool) {
        self.trap.store(trap, Ordering::Relaxed);
    }
}

impl AgeableMob for SkeletonHorseEntity {
    fn get_ageable_data(&self) -> &AgeableData {
        &self.ageable_data
    }

    fn default_baby_spawn_chance(&self) -> f32 {
        crate::entity::ageable::ABSTRACT_HORSE_BABY_SPAWN_CHANCE
    }

    /// Vanilla `SkeletonHorse.canAgeUp`: a foal stays one.
    fn can_age_up(&self) -> bool {
        false
    }

    fn age_boundary_reached(&self) {
        horse_base::refresh_age_dimensions(self);
    }
}

impl Animal for SkeletonHorseEntity {
    /// Vanilla `AbstractHorse.isFood`.
    fn is_food(&self, item_stack: &ItemStack) -> bool {
        item_stack.item.has_tag(&tag::Item::MINECRAFT_HORSE_FOOD)
    }

    /// Vanilla `AbstractHorse.canMate`: it never breeds.
    fn can_mate(&self, _partner: &dyn EntityBase) -> bool {
        false
    }
}

impl AbstractHorse for SkeletonHorseEntity {
    fn horse_data(&self) -> &HorseData {
        &self.horse
    }

    /// Vanilla `SkeletonHorse.playJumpSound`: another sound in water.
    fn horse_jump_sound(&self) -> Sound {
        if self.get_entity().is_in_water() {
            Sound::EntitySkeletonHorseJumpWater
        } else {
            Sound::EntityHorseJump
        }
    }
}

impl CustomSound for SkeletonHorseEntity {
    fn hurt_sound(&self) -> Option<Sound> {
        Some(Sound::EntitySkeletonHorseHurt)
    }

    fn death_sound(&self) -> Option<Sound> {
        Some(Sound::EntitySkeletonHorseDeath)
    }

    fn sound_volume(&self) -> f32 {
        horse_base::SOUND_VOLUME
    }
}

impl Mob for SkeletonHorseEntity {
    /// Vanilla `AbstractHorse.isPushable`.
    fn mob_is_pushable(&self) -> bool {
        !self.get_entity().has_passengers()
    }

    /// Vanilla `supportQuadLeash`.
    fn mob_support_quad_leash(&self) -> bool {
        true
    }

    /// Vanilla `SkeletonHorse.getAmbientSound`.
    fn get_ambient_sound(&self) -> Option<Sound> {
        Some(if self.get_entity().is_submerged_in_water() {
            Sound::EntitySkeletonHorseAmbientWater
        } else {
            Sound::EntitySkeletonHorseAmbient
        })
    }

    fn get_ambient_sound_interval(&self) -> i32 {
        horse_base::AMBIENT_SOUND_INTERVAL
    }

    fn as_custom_sound(&self) -> Option<&dyn CustomSound> {
        Some(self)
    }

    fn is_tamed(&self) -> bool {
        self.is_tame()
    }

    /// Vanilla `AbstractHorse.getOwnerReference`.
    fn get_owner_uuid(&self) -> Option<Uuid> {
        horse_base::owner(self)
    }

    fn mob_tick(&self, _caller: &dyn EntityBase) {
        horse_base::horse_tick(self);
        horse_base::horse_ai_step(self);
        // Vanilla `SkeletonHorse.aiStep`: a trap nobody springs leaves after its time.
        if !self.mob_entity.persistence_required.load(Ordering::Relaxed)
            && self.is_trap()
            && self.trap_time.fetch_add(1, Ordering::Relaxed) >= TRAP_DESPAWN_TICKS
        {
            self.get_entity().remove();
        }
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

    /// Vanilla `SkeletonHorse.randomizeAttributes` through `AbstractHorse.finalizeSpawn`: only
    /// the jump is rolled.
    fn finalize_spawn(
        &self,
        _world: &Arc<World>,
        group_data: Option<SpawnGroupData>,
    ) -> Option<SpawnGroupData> {
        let jump = {
            let mut rng = self.get_random();
            horse_base::generate_jump_strength(|| rng.random())
        };
        self.mob_entity
            .living_entity
            .set_attribute_base(&Attributes::JUMP_STRENGTH, jump);
        self.finalize_spawn_super(group_data)
    }

    fn mob_play_step_sound(&self, pos: &BlockPos, state: &BlockState) {
        horse_base::play_step_sound(self, pos, state, true);
    }

    /// Vanilla `SkeletonHorse.getSwimSound`: on the bottom it steps, and gallops under a rider.
    fn mob_get_swim_sound(&self) -> Option<Sound> {
        let entity = self.get_entity();
        if entity.on_ground.load(Ordering::Relaxed) {
            if !entity.has_passengers() {
                return Some(Sound::EntitySkeletonHorseStepWater);
            }
            let gallop = horse_base::next_gallop_step(self);
            if gallop > 5 && gallop % 3 == 0 {
                return Some(Sound::EntitySkeletonHorseGallopWater);
            }
            if gallop <= 5 {
                return Some(Sound::EntitySkeletonHorseStepWater);
            }
        }
        Some(Sound::EntitySkeletonHorseSwim)
    }

    fn mob_play_swim_sound(&self, volume: f32) {
        if self.get_entity().on_ground.load(Ordering::Relaxed) {
            self.play_default_swim_sound(GROUND_SWIM_VOLUME);
        } else {
            self.play_default_swim_sound((volume * 25.0).min(0.1));
        }
    }

    fn as_ageable(&self) -> Option<&dyn AgeableMob> {
        Some(self)
    }

    fn as_animal(&self) -> Option<&dyn Animal> {
        Some(self)
    }

    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        self.write_ageable_nbt(nbt);
        horse_base::write_horse_nbt(self, nbt);
        nbt.put_bool("SkeletonTrap", self.is_trap());
        nbt.put_int("SkeletonTrapTime", self.trap_time.load(Ordering::Relaxed));
    }

    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        self.read_ageable_nbt(nbt);
        horse_base::read_horse_nbt(self, nbt);
        self.set_trap(nbt.get_bool("SkeletonTrap").unwrap_or(false));
        self.trap_time.store(
            nbt.get_int("SkeletonTrapTime").unwrap_or(0),
            Ordering::Relaxed,
        );
    }

    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
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
            entity.set_synced_data(
                pumpkin_data::tracked_data::skeleton_horse::DATA_BABY_ID,
                true,
            );
        }
        entity.set_synced_data(
            pumpkin_data::tracked_data::skeleton_horse::DATA_ID_FLAGS,
            horse_base::flags(self) as i8,
        );
    }

    /// Vanilla `SkeletonHorse.mobInteract`: a wild one ignores the player, a tamed one is a
    /// horse (`AbstractHorse.mobInteract`).
    fn mob_interact(&self, player: &Arc<Player>, item_stack: &mut ItemStack) -> bool {
        if let Some(result) = horse_base::important_interaction(self, player, item_stack) {
            return result;
        }
        self.is_tame() && horse_base::abstract_horse_interact(self, player, item_stack)
    }
}
