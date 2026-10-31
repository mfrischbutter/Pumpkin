use std::sync::{
    Arc, Weak,
    atomic::{AtomicI32, AtomicU8, Ordering},
};

use pumpkin_data::BlockState;
use pumpkin_data::attributes::Attributes;
use pumpkin_data::damage::DamageType;
use pumpkin_data::effect::StatusEffect;
use pumpkin_data::entity::EntityType;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::Sound;
use pumpkin_data::tag::{self, Taggable};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::Difficulty;
use pumpkin_util::math::position::BlockPos;
use rand::RngExt;

use crate::entity::ai::behavior::hunting::{self, Hunt};
use crate::entity::ai::target_match::TargetMatch;
use crate::entity::{
    Entity, EntityBase,
    ageable::{AgeableData, AgeableMob},
    ai::control::{
        Control, LookControlTrait, flying_move_control::FlyingMoveControl,
        look_control::LookControl,
    },
    ai::goal::{
        active_target::TargetCondition, breed::BreedGoal, follow_parent::FollowParentGoal,
        look_around::RandomLookAroundGoal, look_at_entity::LookAtEntityGoal,
        melee_attack::MeleeAttackGoal, reset_universal_anger::ResetUniversalAngerGoal,
        revenge::RevengeGoal, swim::SwimGoal, tempt::TemptGoal, wander_around::WanderAroundGoal,
    },
    ai::pathfinder::Navigator,
    custom_sound::CustomSound,
    mob::{
        Mob, MobEntity,
        neutral::{NeutralData, NeutralMob},
    },
    passive::animal::Animal,
    player::Player,
};

pub const FLAG_ROLL: u8 = 2;
pub const FLAG_HAS_STUNG: u8 = 4;
pub const FLAG_HAS_NECTAR: u8 = 8;

/// Vanilla `Bee.TICKS_PER_FLAP`, `Mth.ceil(1.4959966F)`.
const TICKS_PER_FLAP: i32 = 2;
/// Vanilla `Bee.getSoundVolume`.
const SOUND_VOLUME: f32 = 0.4;
/// Vanilla `Bee.customServerAiStep`: ticks in water before it drowns.
const MAX_UNDER_WATER_TICKS: i32 = 20;
/// Vanilla `Bee.customServerAiStep`: a bee that stung dies within this many ticks.
const STING_DEATH_TICKS: i32 = 1200;

fn is_angry(mob: &dyn Mob) -> bool {
    mob.as_neutral().is_some_and(NeutralMob::is_angry)
}

/// Angry and still holding its stinger.
fn can_sting(mob: &dyn Mob) -> bool {
    is_angry(mob)
        && !mob
            .cast_any()
            .downcast_ref::<BeeEntity>()
            .is_some_and(BeeEntity::has_stung)
}

const BEE_HUNTS: &[Hunt] = &[Hunt::new(2, TargetMatch::Type(&EntityType::PLAYER), true)
    .when(TargetCondition::AngryAt)
    .gated_by(can_sting)];

pub struct BeeEntity {
    pub mob_entity: MobEntity,
    pub ageable_data: AgeableData,
    pub neutral_data: NeutralData,
    pub flags: AtomicU8,
    pub ticks_without_nectar: AtomicI32,
    pub cannot_enter_hive_ticks: AtomicI32,
    pub crops_grown_since_pollination: AtomicI32,
    pub time_since_sting: AtomicI32,
    under_water_ticks: AtomicI32,
}

impl BeeEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let mut mob_entity = MobEntity::new(entity);
        *mob_entity
            .move_control
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            Box::new(FlyingMoveControl::new(20, true));
        *mob_entity
            .look_control
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            Box::new(BeeLookControl::default());
        let navigator = mob_entity
            .navigator
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *navigator = Navigator::flying();
        navigator.set_can_open_doors(false);
        navigator.set_can_float(false);
        navigator.set_required_path_length(48.0);
        let bee = Self {
            mob_entity,
            ageable_data: AgeableData::default(),
            neutral_data: NeutralData::default(),
            flags: AtomicU8::new(0),
            ticks_without_nectar: AtomicI32::new(0),
            cannot_enter_hive_ticks: AtomicI32::new(0),
            crops_grown_since_pollination: AtomicI32::new(0),
            time_since_sting: AtomicI32::new(0),
            under_water_ticks: AtomicI32::new(0),
        };
        let mob_arc = Arc::new(bee);
        let mob_weak: Weak<dyn Mob> = {
            let mob_arc: Arc<dyn Mob> = mob_arc.clone();
            Arc::downgrade(&mob_arc)
        };

        {
            let goal_selector = &mob_arc.mob_entity.goals_selector;

            goal_selector.add_goal(
                0,
                Box::new(MeleeAttackGoal::new(1.4, true).gated_by(can_sting)),
            );
            goal_selector.add_goal(2, BreedGoal::new(1.0));
            goal_selector.add_goal(
                3,
                Box::new(TemptGoal::with_tag(
                    1.25,
                    &tag::Item::MINECRAFT_BEE_FOOD,
                    false,
                )),
            );
            goal_selector.add_goal(5, Box::new(FollowParentGoal::new(1.25)));
            goal_selector.add_goal(8, Box::new(WanderAroundGoal::new(1.0)));
            goal_selector.add_goal(9, Box::new(SwimGoal::default()));
            goal_selector.add_goal(
                10,
                LookAtEntityGoal::with_default(mob_weak, &EntityType::PLAYER, 6.0),
            );
            goal_selector.add_goal(11, Box::new(RandomLookAroundGoal::default()));
        };

        {
            let target_selector = &mob_arc.mob_entity.target_selector;

            // Vanilla BeeHurtByOtherGoal.
            target_selector.add_goal(
                1,
                Box::new(
                    RevengeGoal::new(true)
                        .alerting_others()
                        .alerting_in_sight()
                        .continuing_while(is_angry),
                ),
            );
            hunting::apply(target_selector, &mob_arc.mob_entity, BEE_HUNTS);
            target_selector.add_goal(3, ResetUniversalAngerGoal::new(true));
        };

        mob_arc
    }

    #[must_use]
    pub fn has_flag(&self, flag: u8) -> bool {
        (self.flags.load(Ordering::Relaxed) & flag) != 0
    }

    pub fn set_flag(&self, flag: u8, val: bool) {
        let current = self.flags.load(Ordering::Relaxed);
        let new_flags = if val { current | flag } else { current & !flag };
        self.flags.store(new_flags, Ordering::Relaxed);
        let entity = self.get_entity();
        entity.set_synced_data(
            pumpkin_data::tracked_data::bee::DATA_FLAGS_ID,
            new_flags as i8,
        );
    }

    #[must_use]
    pub fn has_nectar(&self) -> bool {
        self.has_flag(FLAG_HAS_NECTAR)
    }

    pub fn set_has_nectar(&self, val: bool) {
        self.set_flag(FLAG_HAS_NECTAR, val);
    }

    #[must_use]
    pub fn has_stung(&self) -> bool {
        self.has_flag(FLAG_HAS_STUNG)
    }

    pub fn set_has_stung(&self, val: bool) {
        self.set_flag(FLAG_HAS_STUNG, val);
    }

    #[must_use]
    pub fn is_rolling(&self) -> bool {
        self.has_flag(FLAG_ROLL)
    }

    pub fn set_rolling(&self, val: bool) {
        self.set_flag(FLAG_ROLL, val);
    }

    /// Vanilla `Bee.customServerAiStep`: a bee long in water drowns, and one that stung dies
    /// at a random point within a minute.
    fn tick_drowning_and_sting(&self) {
        let entity = self.get_entity();
        let under_water = if entity.is_in_water() {
            self.under_water_ticks.fetch_add(1, Ordering::Relaxed) + 1
        } else {
            self.under_water_ticks.store(0, Ordering::Relaxed);
            0
        };
        if under_water > MAX_UNDER_WATER_TICKS {
            self.damage(self, 1.0, DamageType::DROWN);
        }
        if self.has_stung() {
            let time = self.time_since_sting.fetch_add(1, Ordering::Relaxed) + 1;
            if time % 5 == 0
                && self
                    .get_random()
                    .random_range(0..(STING_DEATH_TICKS - time).clamp(1, STING_DEATH_TICKS))
                    == 0
            {
                let health = self.mob_entity.living_entity.health.load();
                self.damage(self, health, DamageType::GENERIC);
            }
        }
        if !self.has_nectar() {
            self.ticks_without_nectar.fetch_add(1, Ordering::Relaxed);
        }
    }
}

impl AgeableMob for BeeEntity {
    fn get_ageable_data(&self) -> &AgeableData {
        &self.ageable_data
    }
}

impl Animal for BeeEntity {
    fn is_food(&self, item_stack: &ItemStack) -> bool {
        item_stack.item.has_tag(&tag::Item::MINECRAFT_BEE_FOOD)
    }
}

impl CustomSound for BeeEntity {
    fn death_sound(&self) -> Option<Sound> {
        None
    }

    fn hurt_sound(&self) -> Option<Sound> {
        None
    }

    fn sound_volume(&self) -> f32 {
        SOUND_VOLUME
    }
}

impl NeutralMob for BeeEntity {
    fn updates_anger_in_ai_step(&self) -> bool {
        true
    }

    fn get_neutral_data(&self) -> &NeutralData {
        &self.neutral_data
    }

    /// Bees calm down once the target is gone, unlike the other neutral mobs.
    fn stays_angry_with_target(&self) -> bool {
        false
    }
}

#[derive(Default)]
struct BeeLookControl(LookControl);

impl Control for BeeLookControl {}

impl LookControlTrait for BeeLookControl {
    fn tick(&mut self, mob: &dyn Mob) {
        if !is_angry(mob) {
            self.0.tick(mob);
        }
    }

    fn look_at_with_range(
        &mut self,
        x: f64,
        y: f64,
        z: f64,
        max_yaw_change: f32,
        max_pitch_change: f32,
    ) {
        self.0
            .look_at_with_range(x, y, z, max_yaw_change, max_pitch_change);
    }

    fn wanted_position(&self) -> pumpkin_util::math::vector3::Vector3<f64> {
        self.0.wanted_position()
    }

    fn is_looking_at_target(&self) -> bool {
        self.0.is_looking_at_target()
    }
}

impl Mob for BeeEntity {
    fn mob_is_flapping(&self) -> bool {
        let entity = self.get_entity();
        !entity.on_ground.load(Ordering::Relaxed)
            && entity.tick_count.load(Ordering::Relaxed) % TICKS_PER_FLAP == 0
    }

    /// Vanilla `Bee` steps silently.
    fn mob_play_step_sound(&self, _pos: &BlockPos, _state: &BlockState) {}

    fn omnidirectional_air_mover(&self) -> bool {
        true
    }

    fn as_custom_sound(&self) -> Option<&dyn CustomSound> {
        Some(self)
    }

    /// Vanilla `Bee.doHurtTarget`: a sting for the attack damage, poison on normal and hard,
    /// then the bee loses its stinger and calms down.
    fn do_hurt_target(&self, target: &dyn EntityBase) -> bool {
        let mob = &self.mob_entity;
        // Vanilla rounds the attack damage down to whole points.
        let damage = mob
            .living_entity
            .get_attribute_value(&Attributes::ATTACK_DAMAGE) as i32;
        let hurt = mob.try_attack_with_damage(self, target, damage as f32, DamageType::STING);
        if !hurt {
            return false;
        }
        if let Some(living) = target.get_living_entity() {
            let poison_seconds = match self.get_entity().world.load().level_info.load().difficulty {
                Difficulty::Normal => 10,
                Difficulty::Hard => 18,
                _ => 0,
            };
            if poison_seconds > 0 {
                living.add_effect(pumpkin_data::potion::Effect {
                    effect_type: &StatusEffect::POISON,
                    duration: poison_seconds * 20,
                    amplifier: 0,
                    ambient: false,
                    show_particles: true,
                    show_icon: true,
                    blend: true,
                });
            }
        }
        self.set_has_stung(true);
        self.stop_being_angry();
        self.play_entity_sound(Sound::EntityBeeSting, 1.0, 1.0);
        self.on_attack(target);
        true
    }

    fn as_ageable(&self) -> Option<&dyn AgeableMob> {
        Some(self)
    }

    fn as_animal(&self) -> Option<&dyn Animal> {
        Some(self)
    }

    fn as_neutral(&self) -> Option<&dyn NeutralMob> {
        Some(self)
    }

    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        self.write_ageable_nbt(nbt);
        nbt.put_bool("HasNectar", self.has_nectar());
        nbt.put_bool("HasStung", self.has_stung());
        nbt.put_int(
            "TicksSincePollination",
            self.ticks_without_nectar.load(Ordering::Relaxed),
        );
        nbt.put_int(
            "CannotEnterHiveTicks",
            self.cannot_enter_hive_ticks.load(Ordering::Relaxed),
        );
        nbt.put_int(
            "CropsGrownSincePollination",
            self.crops_grown_since_pollination.load(Ordering::Relaxed),
        );
    }

    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        self.read_ageable_nbt(nbt);
        if let Some(nectar) = nbt.get_bool("HasNectar") {
            self.set_has_nectar(nectar);
        }
        if let Some(stung) = nbt.get_bool("HasStung") {
            self.set_has_stung(stung);
        }
        if let Some(ticks) = nbt.get_int("TicksSincePollination") {
            self.ticks_without_nectar.store(ticks, Ordering::Relaxed);
        }
        if let Some(cannot) = nbt.get_int("CannotEnterHiveTicks") {
            self.cannot_enter_hive_ticks
                .store(cannot, Ordering::Relaxed);
        }
        if let Some(crops) = nbt.get_int("CropsGrownSincePollination") {
            self.crops_grown_since_pollination
                .store(crops, Ordering::Relaxed);
        }
    }

    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    fn custom_server_ai_step(&self, _caller: &dyn EntityBase) {
        self.tick_drowning_and_sting();
        self.update_persistent_anger();
    }

    fn mob_init_data_tracker(&self) {
        let entity = self.get_entity();
        let is_baby = entity.age.load(Ordering::Relaxed) < 0;
        if is_baby {
            entity.set_synced_data(pumpkin_data::tracked_data::bee::DATA_BABY_ID, true);
        }
        entity.set_synced_data(
            pumpkin_data::tracked_data::bee::DATA_FLAGS_ID,
            self.flags.load(Ordering::Relaxed) as i8,
        );
    }

    fn mob_interact(&self, player: &Arc<Player>, item_stack: &mut ItemStack) -> bool {
        self.animal_interact(player, item_stack, Sound::EntityBeePollinate)
    }
}
