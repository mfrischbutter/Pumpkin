//! Vanilla `AbstractChestedHorse` with `Donkey` and `Mule`: horses that carry a chest.

use std::sync::{
    Arc, Weak,
    atomic::{AtomicBool, Ordering},
};

use pumpkin_data::BlockState;
use pumpkin_data::damage::DamageType;
use pumpkin_data::entity::EntityType;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::Sound;
use pumpkin_data::tag::{self, Taggable};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::math::position::BlockPos;
use uuid::Uuid;

use crate::entity::custom_sound::CustomSound;
use crate::entity::passive::abstract_horse::{self as horse_base, AbstractHorse, HorseData};
use crate::entity::passive::horse_inventory;
use crate::entity::{
    Entity, EntityBase,
    ageable::{AgeableData, AgeableMob},
    mob::{Mob, MobEntity, spawn::SpawnGroupData},
    passive::animal::Animal,
    player::Player,
};
use crate::world::World;

/// What a donkey and a mule do differently.
pub struct ChestedKind {
    pub entity_type: &'static EntityType,
    ambient: Sound,
    angry: Sound,
    eat: Sound,
    hurt: Sound,
    death: Sound,
    chest: Sound,
    jump: Sound,
    /// Vanilla `canMate`: donkeys breed with donkeys and horses, mules not at all.
    breeds: bool,
}

pub static DONKEY: ChestedKind = ChestedKind {
    entity_type: &EntityType::DONKEY,
    ambient: Sound::EntityDonkeyAmbient,
    angry: Sound::EntityDonkeyAngry,
    eat: Sound::EntityDonkeyEat,
    hurt: Sound::EntityDonkeyHurt,
    death: Sound::EntityDonkeyDeath,
    chest: Sound::EntityDonkeyChest,
    jump: Sound::EntityDonkeyJump,
    breeds: true,
};

pub static MULE: ChestedKind = ChestedKind {
    entity_type: &EntityType::MULE,
    ambient: Sound::EntityMuleAmbient,
    angry: Sound::EntityMuleAngry,
    eat: Sound::EntityMuleEat,
    hurt: Sound::EntityMuleHurt,
    death: Sound::EntityMuleDeath,
    chest: Sound::EntityMuleChest,
    jump: Sound::EntityMuleJump,
    breeds: false,
};

pub struct ChestedHorseEntity {
    pub mob_entity: MobEntity,
    pub ageable_data: AgeableData,
    pub kind: &'static ChestedKind,
    horse: HorseData,
    has_chest: AtomicBool,
}

impl ChestedHorseEntity {
    pub fn new(entity: Entity, kind: &'static ChestedKind) -> Arc<Self> {
        let chested = Self {
            mob_entity: MobEntity::new(entity),
            ageable_data: AgeableData::default(),
            kind,
            horse: HorseData::default(),
            has_chest: AtomicBool::new(false),
        };
        let mob_arc = Arc::new(chested);
        let mob_weak: Weak<dyn Mob> = {
            let mob_arc: Arc<dyn Mob> = mob_arc.clone();
            Arc::downgrade(&mob_arc)
        };
        let goal_selector = &mob_arc.mob_entity.goals_selector;
        horse_base::register_horse_goals(goal_selector, mob_weak, true);
        horse_base::add_behaviour_goals(goal_selector);
        mob_arc
    }

    /// Vanilla `setChest`.
    pub fn set_chest(&self, chest: bool) {
        self.has_chest.store(chest, Ordering::Relaxed);
        self.get_entity()
            .set_synced_data(pumpkin_data::tracked_data::donkey::DATA_ID_CHEST, chest);
    }

    #[must_use]
    pub fn is_tame(&self) -> bool {
        horse_base::is_tamed(self)
    }

    /// Vanilla `equipChest`.
    fn equip_chest(&self, player: &Player, stack: &mut ItemStack) {
        self.set_chest(true);
        let pitch = {
            use rand::RngExt;
            let mut rng = self.get_random();
            (rng.random::<f32>() - rng.random::<f32>()).mul_add(0.2, 1.0)
        };
        self.play_entity_sound(self.kind.chest, 1.0, pitch);
        stack.decrement_unless_creative(player.gamemode.load(), 1);
        horse_inventory::create_inventory(self);
    }
}

impl AgeableMob for ChestedHorseEntity {
    fn get_ageable_data(&self) -> &AgeableData {
        &self.ageable_data
    }

    fn default_baby_spawn_chance(&self) -> f32 {
        crate::entity::ageable::ABSTRACT_HORSE_BABY_SPAWN_CHANCE
    }

    fn age_boundary_reached(&self) {
        horse_base::refresh_age_dimensions(self);
    }
}

impl Animal for ChestedHorseEntity {
    /// Vanilla `AbstractHorse.isFood`.
    fn is_food(&self, item_stack: &ItemStack) -> bool {
        item_stack.item.has_tag(&tag::Item::MINECRAFT_HORSE_FOOD)
    }

    /// Vanilla `Donkey.canMate`: with a donkey or a horse, both ready to parent.
    fn can_mate(&self, partner: &dyn EntityBase) -> bool {
        let Some(other) = partner.get_mob().and_then(|mob| mob.as_abstract_horse()) else {
            return false;
        };
        let kind = partner.get_entity().entity_type;
        self.kind.breeds
            && (kind == &EntityType::DONKEY || kind == &EntityType::HORSE)
            && horse_base::can_parent(self)
            && horse_base::can_parent(other)
    }

    /// Vanilla `Donkey.getBreedOffspring`: a mule with a horse, else a donkey.
    fn get_breed_offspring(&self, partner: &dyn EntityBase) -> Arc<dyn EntityBase> {
        let kind = if partner.get_entity().entity_type == &EntityType::HORSE {
            &MULE
        } else {
            &DONKEY
        };
        let entity = self.get_entity();
        let foal = Self::new(
            Entity::new(
                entity.world.load_full(),
                entity.pos.load(),
                kind.entity_type,
            ),
            kind,
        );
        horse_base::set_offspring_attributes(self, partner, foal.as_ref());
        foal
    }
}

impl AbstractHorse for ChestedHorseEntity {
    fn horse_data(&self) -> &HorseData {
        &self.horse
    }

    fn horse_eating_sound(&self) -> Option<Sound> {
        Some(self.kind.eat)
    }

    fn horse_angry_sound(&self) -> Option<Sound> {
        Some(self.kind.angry)
    }

    fn horse_jump_sound(&self) -> Sound {
        self.kind.jump
    }

    fn has_chest(&self) -> bool {
        self.has_chest.load(Ordering::Relaxed)
    }

    /// Vanilla `AbstractChestedHorse.getInventoryColumns`.
    fn inventory_columns(&self) -> i32 {
        if self.has_chest() { 5 } else { 0 }
    }
}

impl CustomSound for ChestedHorseEntity {
    fn hurt_sound(&self) -> Option<Sound> {
        Some(self.kind.hurt)
    }

    fn death_sound(&self) -> Option<Sound> {
        Some(self.kind.death)
    }

    fn sound_volume(&self) -> f32 {
        horse_base::SOUND_VOLUME
    }
}

impl Mob for ChestedHorseEntity {
    fn get_ambient_sound(&self) -> Option<Sound> {
        Some(self.kind.ambient)
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

    /// Vanilla `AbstractChestedHorse`: `canGallop` is off.
    fn mob_play_step_sound(&self, pos: &BlockPos, state: &BlockState) {
        horse_base::play_step_sound(self, pos, state, false);
    }

    fn as_ageable(&self) -> Option<&dyn AgeableMob> {
        Some(self)
    }

    fn as_animal(&self) -> Option<&dyn Animal> {
        Some(self)
    }

    fn as_abstract_horse(&self) -> Option<&dyn AbstractHorse> {
        Some(self)
    }

    fn is_mob_immobile(&self) -> bool {
        horse_base::is_immobile(self)
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

    fn mob_start_rider_jump(&self, _strength: i32) {
        if self.is_saddled() {
            horse_base::on_rider_jump(self);
        }
    }

    /// Vanilla `AbstractChestedHorse.randomizeAttributes`: only the health is rolled.
    fn finalize_spawn(
        &self,
        _world: &Arc<World>,
        group_data: Option<SpawnGroupData>,
    ) -> Option<SpawnGroupData> {
        horse_base::randomize_max_health(self);
        self.finalize_spawn_super(group_data)
    }

    /// Vanilla `AbstractChestedHorse.dropEquipment`.
    fn custom_death_loot(&self) -> Vec<ItemStack> {
        horse_inventory::inventory_drops(self)
    }

    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        self.write_ageable_nbt(nbt);
        horse_base::write_horse_nbt(self, nbt);
        horse_inventory::write_chest_nbt(self, nbt);
    }

    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        self.read_ageable_nbt(nbt);
        horse_base::read_horse_nbt(self, nbt);
        self.set_chest(nbt.get_bool("ChestedHorse").unwrap_or(false));
        horse_inventory::read_chest_items(self, nbt);
        crate::entity::mob::saddle::read_legacy_saddle(self, nbt);
    }

    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
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
        if entity.age.load(Ordering::Relaxed) < 0 {
            entity.set_synced_data(pumpkin_data::tracked_data::donkey::DATA_BABY_ID, true);
        }
        entity.set_synced_data(
            pumpkin_data::tracked_data::donkey::DATA_ID_FLAGS,
            horse_base::flags(self) as i8,
        );
        entity.set_synced_data(
            pumpkin_data::tracked_data::donkey::DATA_ID_CHEST,
            self.has_chest(),
        );
    }

    /// Vanilla `AbstractChestedHorse.mobInteract` with `AbstractHorse.mobInteract`: a tamed
    /// adult also takes a chest.
    fn mob_interact(&self, player: &Arc<Player>, item_stack: &mut ItemStack) -> bool {
        horse_base::horse_interact(self, player, item_stack, |stack| {
            if !self.has_chest() && stack.item == &Item::CHEST {
                self.equip_chest(player, stack);
                return true;
            }
            false
        })
    }
}
