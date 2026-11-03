use std::sync::{Arc, Weak, atomic::Ordering};

use crate::entity::mob::spawn::{EntitySpawnReason, SpawnGroupData, finalize_spawn};
use crate::entity::r#type::from_type;
use crate::world::World;
use pumpkin_data::BlockState;
use pumpkin_data::data_component_impl::EquipmentSlot;
use pumpkin_data::entity::EntityType;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::Sound;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::math::position::BlockPos;
use uuid::Uuid;

use crate::entity::passive::abstract_horse::{self as horse_base, AbstractHorse, HorseData};
use crate::entity::{
    Entity, EntityBase,
    ageable::{AgeableData, AgeableMob},
    ai::goal::{
        escape_danger::EscapeDangerGoal, look_around::RandomLookAroundGoal,
        look_at_entity::LookAtEntityGoal, swim::SwimGoal, wander_around::WanderAroundGoal,
    },
    mob::{Mob, MobEntity},
    passive::animal::Animal,
    player::Player,
};

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

        {
            let goal_selector = &mob_arc.mob_entity.goals_selector;

            goal_selector.add_goal(0, Box::new(SwimGoal::default()));
            goal_selector.add_goal(1, EscapeDangerGoal::new(1.2));
            goal_selector.add_goal(6, Box::new(WanderAroundGoal::new(0.7)));
            goal_selector.add_goal(
                7,
                LookAtEntityGoal::with_default(mob_weak, &EntityType::PLAYER, 6.0),
            );
            goal_selector.add_goal(8, Box::new(RandomLookAroundGoal::default()));
        };

        mob_arc
    }

    #[must_use]
    pub fn is_tame(&self) -> bool {
        horse_base::is_tamed(self)
    }

    pub fn set_tame(&self, val: bool) {
        horse_base::set_flag(self, horse_base::FLAG_TAME, val);
    }
}

impl AgeableMob for ZombieHorseEntity {
    fn get_ageable_data(&self) -> &AgeableData {
        &self.ageable_data
    }

    fn default_baby_spawn_chance(&self) -> f32 {
        crate::entity::ageable::ABSTRACT_HORSE_BABY_SPAWN_CHANCE
    }
}

impl Animal for ZombieHorseEntity {
    fn is_food(&self, _item_stack: &ItemStack) -> bool {
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
}

impl Mob for ZombieHorseEntity {
    /// Vanilla `AbstractHorse.isPushable`.
    fn mob_is_pushable(&self) -> bool {
        !self.get_entity().has_passengers()
    }

    /// Vanilla `supportQuadLeash`.
    fn mob_support_quad_leash(&self) -> bool {
        true
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
    fn get_ambient_sound(&self) -> Option<pumpkin_data::sound::Sound> {
        Some(pumpkin_data::sound::Sound::EntityZombieHorseAmbient)
    }

    /// Vanilla `ZombieHorse.removeWhenFarAway`.
    fn remove_when_far_away(&self, _distance_sq: f64) -> bool {
        true
    }

    fn is_tamed(&self) -> bool {
        self.is_tame()
    }

    fn mob_play_step_sound(&self, pos: &BlockPos, state: &BlockState) {
        horse_base::play_step_sound(self, pos, state, true);
    }

    fn as_ageable(&self) -> Option<&dyn AgeableMob> {
        Some(self)
    }

    fn sun_protection_slot(&self) -> pumpkin_data::data_component_impl::EquipmentSlot {
        pumpkin_data::data_component_impl::EquipmentSlot::BODY
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

    fn mob_interact(&self, player: &Arc<Player>, item_stack: &mut ItemStack) -> bool {
        // Vanilla `ZombieHorse.interact`: every click keeps it loaded.
        self.mob_entity
            .persistence_required
            .store(true, std::sync::atomic::Ordering::Relaxed);
        if horse_base::try_open_inventory(self, player) {
            return true;
        }
        if crate::entity::mob::saddle::try_equip_saddle(self, player, item_stack) {
            return true;
        }

        if !self.is_baby() {
            let world = player.world();
            let ent = &self.mob_entity.living_entity.entity;
            if let Some(vehicle) = world.get_entity_by_id(ent.entity_id)
                && let Some(passenger) = world.get_player_by_id(player.entity_id())
            {
                ent.add_passenger(vehicle, passenger as Arc<dyn EntityBase>);
                return true;
            }
        }

        self.animal_interact(player, item_stack, Sound::EntityZombieHorseAmbient)
    }
}
