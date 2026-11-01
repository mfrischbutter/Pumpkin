//! Vanilla `AbstractHorse`: what horses, donkeys, mules and skeleton and zombie horses share,
//! from taming by riding to rearing, grazing and the stats a foal inherits.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU8, Ordering};

use crossbeam::atomic::AtomicCell;
use pumpkin_data::attributes::Attributes;
use pumpkin_data::block_state::SoundType;
use pumpkin_data::data_component_impl::EquipmentSlot;
use pumpkin_data::entity::EntityStatus;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::Sound;
use pumpkin_data::{Block, BlockState};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_protocol::bedrock::server::actor_event::ActorEventID;
use pumpkin_util::math::position::BlockPos;
use rand::RngExt;
use uuid::Uuid;

use crate::entity::ageable::AgeableMob;
use crate::entity::ai::goal::{Controls, Goal, to_goal_ticks};
use crate::entity::ai::pathfinder::NavigatorGoal;
use crate::entity::ai::util::default_random_pos;
use crate::entity::mob::Mob;
use crate::entity::passive::animal::Animal;
use crate::entity::player::Player;
use crate::entity::player::advancement::trigger::AdvancementTrigger;
use crate::entity::{EntityBase, mob::saddle};

pub const FLAG_TAME: u8 = 2;
pub const FLAG_BRED: u8 = 8;
pub const FLAG_EATING: u8 = 16;
pub const FLAG_STANDING: u8 = 32;
pub const FLAG_OPEN_MOUTH: u8 = 64;

/// The state vanilla `AbstractHorse` keeps beside its synced flags.
#[derive(Default)]
pub struct HorseData {
    flags: AtomicU8,
    temper: AtomicI32,
    owner: AtomicCell<Option<Uuid>>,
    eating_counter: AtomicI32,
    mouth_counter: AtomicI32,
    stand_counter: AtomicI32,
    allow_stand_sliding: AtomicBool,
    gallop_sound_counter: AtomicI32,
}

/// A member of the horse family. Species fill in what differs; the free functions below are
/// the shared vanilla behaviour.
pub trait AbstractHorse: Animal + AgeableMob {
    fn horse_data(&self) -> &HorseData;

    /// Vanilla `getEatingSound`.
    fn horse_eating_sound(&self) -> Option<Sound> {
        None
    }

    /// Vanilla `getAngrySound`.
    fn horse_angry_sound(&self) -> Option<Sound> {
        None
    }

    /// Vanilla `playJumpSound`.
    fn horse_jump_sound(&self) -> Sound {
        Sound::EntityHorseJump
    }

    /// Vanilla `canPerformRearing`.
    fn can_perform_rearing(&self) -> bool {
        true
    }

    /// Vanilla `getMaxTemper`.
    fn max_temper(&self) -> i32 {
        100
    }

    /// Vanilla `canEatGrass`.
    fn can_eat_grass(&self) -> bool {
        true
    }

    /// Vanilla `isMobControlled`: ridden by a mob, like a skeleton trap's horses.
    fn is_mob_controlled(&self) -> bool {
        false
    }
}

fn as_horse(mob: &dyn Mob) -> Option<&dyn AbstractHorse> {
    mob.as_abstract_horse()
}

pub fn has_flag(horse: &dyn AbstractHorse, flag: u8) -> bool {
    horse.horse_data().flags.load(Ordering::Relaxed) & flag != 0
}

pub fn set_flag(horse: &dyn AbstractHorse, flag: u8, value: bool) {
    let flags = &horse.horse_data().flags;
    let old = flags.load(Ordering::Relaxed);
    let new = if value { old | flag } else { old & !flag };
    if new != old {
        flags.store(new, Ordering::Relaxed);
        horse
            .get_entity()
            .set_synced_data(pumpkin_data::tracked_data::horse::DATA_ID_FLAGS, new as i8);
    }
}

/// The synced flag byte, for a horse's first data tracker update.
pub fn flags(horse: &dyn AbstractHorse) -> u8 {
    horse.horse_data().flags.load(Ordering::Relaxed)
}

pub fn is_tamed(horse: &dyn AbstractHorse) -> bool {
    has_flag(horse, FLAG_TAME)
}

pub fn is_eating(horse: &dyn AbstractHorse) -> bool {
    has_flag(horse, FLAG_EATING)
}

pub fn is_standing(horse: &dyn AbstractHorse) -> bool {
    has_flag(horse, FLAG_STANDING)
}

pub fn temper(horse: &dyn AbstractHorse) -> i32 {
    horse.horse_data().temper.load(Ordering::Relaxed)
}

pub fn owner(horse: &dyn AbstractHorse) -> Option<Uuid> {
    horse.horse_data().owner.load()
}

/// Vanilla `modifyTemper`.
fn modify_temper(horse: &dyn AbstractHorse, amount: i32) {
    let temper = (temper(horse) + amount).clamp(0, horse.max_temper());
    horse.horse_data().temper.store(temper, Ordering::Relaxed);
}

/// Vanilla `setStanding`.
fn set_standing(horse: &dyn AbstractHorse, ticks: i32) {
    set_flag(horse, FLAG_EATING, false);
    set_flag(horse, FLAG_STANDING, true);
    horse
        .horse_data()
        .stand_counter
        .store(ticks, Ordering::Relaxed);
}

/// Vanilla `clearStanding`.
fn clear_standing(horse: &dyn AbstractHorse) {
    set_flag(horse, FLAG_STANDING, false);
    horse.horse_data().stand_counter.store(0, Ordering::Relaxed);
}

/// Vanilla `standIfPossible`.
pub fn stand_if_possible(horse: &dyn AbstractHorse) {
    if horse.can_perform_rearing() {
        set_standing(horse, 20);
    }
}

/// Vanilla `makeMad`: it rears and complains.
pub fn make_mad(horse: &dyn AbstractHorse) {
    if is_standing(horse) {
        return;
    }
    stand_if_possible(horse);
    if let Some(sound) = horse.horse_angry_sound() {
        horse.play_entity_sound(sound, 0.8, horse.get_mob_entity().living_entity.get_pitch());
    }
}

/// Vanilla `openMouth`.
fn open_mouth(horse: &dyn AbstractHorse) {
    horse.horse_data().mouth_counter.store(1, Ordering::Relaxed);
    set_flag(horse, FLAG_OPEN_MOUTH, true);
}

/// Vanilla `tameWithName`: the rider becomes the owner.
fn tame_with_name(horse: &dyn AbstractHorse, player: &Player) {
    horse.horse_data().owner.store(Some(player.gameprofile.id));
    set_flag(horse, FLAG_TAME, true);
    let entity = horse.get_entity();
    player.trigger_advancement(AdvancementTrigger::TameAnimal {
        entity_type: format!("minecraft:{}", entity.entity_type.resource_name),
    });
    entity.world.load().send_entity_status(
        entity,
        EntityStatus::TamingSucceeded,
        ActorEventID::TamingSucceeded,
    );
}

/// Vanilla `AbstractHorse.tick`, the server part: the mouth closes after 30 ticks and a
/// rearing horse comes down when its time is up.
pub fn horse_tick(horse: &dyn AbstractHorse) {
    let data = horse.horse_data();
    let mouth = data.mouth_counter.load(Ordering::Relaxed);
    if mouth > 0 {
        if mouth + 1 > 30 {
            data.mouth_counter.store(0, Ordering::Relaxed);
            set_flag(horse, FLAG_OPEN_MOUTH, false);
        } else {
            data.mouth_counter.store(mouth + 1, Ordering::Relaxed);
        }
    }
    let stand = data.stand_counter.load(Ordering::Relaxed);
    if stand > 0 {
        data.stand_counter.store(stand - 1, Ordering::Relaxed);
        if stand - 1 <= 0 {
            clear_standing(horse);
        }
    }
}

/// Vanilla `AbstractHorse.aiStep`, the server part: slow healing, grazing on grass and a bred
/// foal following its parents.
pub fn horse_ai_step(horse: &dyn AbstractHorse) {
    let entity = horse.get_entity();
    let living = &horse.get_mob_entity().living_entity;
    if !entity.is_alive() {
        return;
    }
    let (heal, graze) = {
        let mut rng = horse.get_random();
        (rng.random_range(0..900) == 0, rng.random_range(0..300) == 0)
    };
    if heal && living.death_time.load(Ordering::Relaxed) == 0 {
        living.heal(1.0);
    }
    if horse.can_eat_grass() {
        let data = horse.horse_data();
        if !is_eating(horse)
            && !entity.has_passengers()
            && graze
            && entity
                .world
                .load()
                .get_block(&entity.block_pos.load().down())
                == &Block::GRASS_BLOCK
        {
            set_flag(horse, FLAG_EATING, true);
        }
        if is_eating(horse) && data.eating_counter.fetch_add(1, Ordering::Relaxed) + 1 > 50 {
            data.eating_counter.store(0, Ordering::Relaxed);
            set_flag(horse, FLAG_EATING, false);
        }
    }
    follow_mommy(horse);
}

/// Vanilla `followMommy`: a bred foal walks to the nearest bred horse 16 blocks around.
fn follow_mommy(horse: &dyn AbstractHorse) {
    if !has_flag(horse, FLAG_BRED) || !horse.is_baby() || is_eating(horse) {
        return;
    }
    let entity = horse.get_entity();
    let pos = entity.pos.load();
    let area = entity.bounding_box.load().expand(16.0, 16.0, 16.0);
    let mommy = entity
        .world
        .load()
        .get_entities_at_box(&area)
        .into_iter()
        .filter(|other| other.get_entity().entity_id != entity.entity_id)
        .filter(|other| {
            other
                .get_mob()
                .and_then(|mob| mob.as_abstract_horse())
                .is_some_and(|other| has_flag(other, FLAG_BRED))
        })
        .min_by(|a, b| {
            let da = a.get_entity().pos.load().squared_distance_to_vec(&pos);
            let db = b.get_entity().pos.load().squared_distance_to_vec(&pos);
            da.total_cmp(&db)
        });
    if let Some(mommy) = mommy {
        let target = mommy.get_entity().pos.load();
        if target.squared_distance_to_vec(&pos) > 4.0 {
            horse
                .get_mob_entity()
                .steered()
                .navigator()
                .set_progress(NavigatorGoal::new(pos, target, 1.0));
        }
    }
}

/// Vanilla `handleEating`: heals, ages up and calms a horse, golden food makes a tame adult
/// fall in love. `true` when it ate.
pub fn handle_eating(horse: &dyn AbstractHorse, player: &Player, stack: &ItemStack) -> bool {
    let item = stack.item;
    let golden = item == &Item::GOLDEN_CARROT
        || item == &Item::GOLDEN_APPLE
        || item == &Item::ENCHANTED_GOLDEN_APPLE;
    let (heal, age_up, temper) = match item {
        i if i == &Item::WHEAT => (2.0, 20, 3),
        i if i == &Item::SUGAR => (1.0, 30, 3),
        i if i == &Item::HAY_BLOCK => (20.0, 180, 0),
        i if i == &Item::APPLE || i == &Item::CARROT => (3.0, 60, 3),
        i if i == &Item::RED_MUSHROOM => (3.0, 0, 3),
        i if i == &Item::GOLDEN_CARROT => (4.0, 60, 5),
        i if i == &Item::GOLDEN_APPLE || i == &Item::ENCHANTED_GOLDEN_APPLE => (10.0, 240, 10),
        _ => (0.0, 0, 0),
    };
    let mob_entity = horse.get_mob_entity();
    let living = &mob_entity.living_entity;
    let entity = &living.entity;
    let mut used = false;
    if golden
        && is_tamed(horse)
        && entity.age.load(Ordering::Relaxed) == 0
        && !mob_entity.is_in_love()
    {
        used = true;
        mob_entity.set_love_ticks(600, Some(player.gameprofile.id));
        entity.world.load().send_entity_status(
            entity,
            EntityStatus::InLoveHearts,
            ActorEventID::InLoveHearts,
        );
    }
    if living.health.load() < living.get_max_health() && heal > 0.0 {
        living.heal(heal);
        used = true;
    }
    if horse.is_baby() && age_up > 0 && !horse.is_age_locked() {
        horse.age_up(age_up, false);
        used = true;
    }
    if temper > 0 && (used || !is_tamed(horse)) && self::temper(horse) < horse.max_temper() {
        modify_temper(horse, temper);
        used = true;
    }
    if used {
        eating(horse);
    }
    used
}

/// Vanilla `eating`: the mouth opens and the eating sound plays.
fn eating(horse: &dyn AbstractHorse) {
    open_mouth(horse);
    if let Some(sound) = horse.horse_eating_sound() {
        let pitch = {
            let mut rng = horse.get_random();
            (rng.random::<f32>() - rng.random::<f32>()).mul_add(0.2, 1.0)
        };
        horse.play_entity_sound(sound, 1.0, pitch);
    }
    let entity = horse.get_entity();
    entity.world.load().emit_game_event(
        pumpkin_data::game_event::GameEvent::Eat.name(),
        entity.pos.load(),
    );
}

/// Vanilla `doPlayerRide`.
fn do_player_ride(horse: &dyn AbstractHorse, player: &Arc<Player>) {
    set_flag(horse, FLAG_EATING, false);
    clear_standing(horse);
    let entity = horse.get_entity();
    let world = entity.world.load();
    if let Some(vehicle) = world.get_entity_by_id(entity.entity_id)
        && let Some(passenger) = world.get_player_by_id(player.entity_id())
    {
        passenger
            .get_entity()
            .set_rotation(entity.yaw.load(), entity.pitch.load());
        entity.add_passenger(vehicle, passenger as Arc<dyn EntityBase>);
    }
}

/// Vanilla `Horse.mobInteract` and `AbstractChestedHorse.mobInteract`.
///
/// With `AbstractHorse.mobInteract`: feeding, a wild horse rearing at items, `extra` (armour
/// for horses, a chest for donkeys and mules), saddles, then riding.
pub fn horse_interact(
    horse: &dyn AbstractHorse,
    player: &Arc<Player>,
    stack: &mut ItemStack,
    extra: impl FnOnce(&mut ItemStack) -> bool,
) -> bool {
    let entity = horse.get_entity();
    let sneaking = player.get_entity().is_sneaking();
    let baby = horse.is_baby();
    if entity.has_passengers() || baby {
        // Vanilla `Animal.mobInteract`: foals take food.
        return baby && horse.is_food(stack) && feed(horse, player, stack);
    }
    if is_tamed(horse) && sneaking {
        // TODO: the horse inventory screen (`openCustomInventoryScreen`).
        return false;
    }
    if !stack.is_empty() {
        if horse.is_food(stack) {
            return feed(horse, player, stack);
        }
        if !is_tamed(horse) {
            make_mad(horse);
            return true;
        }
        if extra(stack) || saddle::try_equip_saddle(horse, player, stack) {
            return true;
        }
    }
    do_player_ride(horse, player);
    true
}

/// Vanilla `fedFood`.
fn feed(horse: &dyn AbstractHorse, player: &Player, stack: &mut ItemStack) -> bool {
    let ate = handle_eating(horse, player, stack);
    if ate {
        stack.decrement_unless_creative(player.gamemode.load(), 1);
    }
    ate
}

/// Vanilla `RunAroundLikeCrazyGoal`: a wild horse with a rider bucks around; now and then it
/// either takes the rider as owner, more likely the calmer it got, or throws them off.
pub struct RunAroundLikeCrazyGoal {
    speed: f64,
    target: Option<pumpkin_util::math::vector3::Vector3<f64>>,
}

impl RunAroundLikeCrazyGoal {
    #[must_use]
    pub const fn new(speed: f64) -> Self {
        Self {
            speed,
            target: None,
        }
    }
}

impl Goal for RunAroundLikeCrazyGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        let Some(horse) = as_horse(mob) else {
            return false;
        };
        if horse.is_mob_controlled() || is_tamed(horse) || !mob.get_entity().has_passengers() {
            return false;
        }
        self.target = default_random_pos::get_pos(mob, 5, 4);
        self.target.is_some()
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        as_horse(mob).is_some_and(|horse| !is_tamed(horse))
            && !mob.is_navigator_idle()
            && mob.get_entity().has_passengers()
    }

    fn start(&mut self, mob: &dyn Mob) {
        if let Some(target) = self.target {
            let pos = mob.get_entity().pos.load();
            mob.get_mob_entity()
                .steered()
                .navigator()
                .set_progress(NavigatorGoal::new(pos, target, self.speed));
        }
    }

    fn tick(&mut self, mob: &dyn Mob) {
        let Some(horse) = as_horse(mob) else {
            return;
        };
        if is_tamed(horse) || mob.get_random().random_range(0..to_goal_ticks(50)) != 0 {
            return;
        }
        let entity = mob.get_entity();
        let Some(rider) = saddle::first_passenger_player(mob) else {
            if entity.has_passengers() {
                eject_passengers(horse);
            }
            return;
        };
        let max = horse.max_temper();
        if max > 0 && mob.get_random().random_range(0..max) < temper(horse) {
            tame_with_name(horse, &rider);
            return;
        }
        modify_temper(horse, 5);
        eject_passengers(horse);
        make_mad(horse);
        entity.world.load().send_entity_status(
            entity,
            EntityStatus::TamingFailed,
            ActorEventID::TamingFailed,
        );
    }

    fn controls(&self) -> Controls {
        Controls::MOVE
    }
}

fn eject_passengers(horse: &dyn AbstractHorse) {
    let entity = horse.get_entity();
    let passengers: Vec<i32> = entity
        .passengers
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .iter()
        .map(|passenger| passenger.get_entity().entity_id)
        .collect();
    for id in passengers {
        entity.remove_passenger(id);
    }
}

/// Vanilla `RandomStandGoal`: now and then a horse rears with its ambient sound.
pub struct RandomStandGoal {
    next_stand: i32,
}

impl RandomStandGoal {
    /// Vanilla `getAmbientStandInterval`.
    const STAND_INTERVAL: i32 = 400;

    #[must_use]
    pub const fn new() -> Self {
        Self {
            next_stand: -Self::STAND_INTERVAL,
        }
    }
}

impl Default for RandomStandGoal {
    fn default() -> Self {
        Self::new()
    }
}

impl Goal for RandomStandGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        self.next_stand += 1;
        let mut rng = mob.get_random();
        if self.next_stand > 0 && rng.random_range(0..1000) < self.next_stand {
            self.next_stand = -Self::STAND_INTERVAL;
            return !mob.is_immobile() && rng.random_range(0..10) == 0;
        }
        false
    }

    fn should_continue(&mut self, _mob: &dyn Mob) -> bool {
        false
    }

    fn start(&mut self, mob: &dyn Mob) {
        let Some(horse) = as_horse(mob) else {
            return;
        };
        stand_if_possible(horse);
        if let Some(sound) = mob.get_ambient_sound() {
            mob.play_entity_sound(sound, 0.8, mob.get_mob_entity().living_entity.get_pitch());
        }
    }

    fn should_run_every_tick(&self) -> bool {
        true
    }
}

/// Vanilla `AbstractHorse.isImmobile`: eating, rearing or ridden with a saddle.
pub fn is_immobile(horse: &dyn AbstractHorse) -> bool {
    let living = &horse.get_mob_entity().living_entity;
    (living.health.load() <= 0.0 && horse.get_entity().has_passengers() && horse.is_saddled())
        || is_eating(horse)
        || is_standing(horse)
}

/// Vanilla `hurtServer`: a hurt horse rears one time in three.
pub fn on_hurt(horse: &dyn AbstractHorse) {
    if horse.get_random().random_range(0..3) == 0 {
        stand_if_possible(horse);
    }
}

/// Vanilla `handleStartJump`: a rider's jump makes it rear with the jump sound.
pub fn on_rider_jump(horse: &dyn AbstractHorse) {
    horse
        .horse_data()
        .allow_stand_sliding
        .store(true, Ordering::Relaxed);
    stand_if_possible(horse);
    horse.play_entity_sound(horse.horse_jump_sound(), 0.4, 1.0);
}

/// Vanilla `generateMaxHealth`, `generateJumpStrength` and `generateSpeed` with random rolls:
/// a new horse's own stats.
pub fn randomize_attributes(horse: &dyn AbstractHorse) {
    randomize_max_health(horse);
    let living = &horse.get_mob_entity().living_entity;
    let (jump, speed) = {
        let mut rng = horse.get_random();
        let jump =
            0.4 + rng.random::<f64>() * 0.2 + rng.random::<f64>() * 0.2 + rng.random::<f64>() * 0.2;
        let speed = (0.45
            + rng.random::<f64>() * 0.3
            + rng.random::<f64>() * 0.3
            + rng.random::<f64>() * 0.3)
            * 0.25;
        (jump, speed)
    };
    living.set_attribute_base(&Attributes::JUMP_STRENGTH, jump);
    living.set_attribute_base(&Attributes::MOVEMENT_SPEED, speed);
}

/// Vanilla `generateMaxHealth`: 15 to 30.
pub fn randomize_max_health(horse: &dyn AbstractHorse) {
    let living = &horse.get_mob_entity().living_entity;
    let health = {
        let mut rng = horse.get_random();
        15.0 + f64::from(rng.random_range(0..8) + rng.random_range(0..9))
    };
    living.set_attribute_base(&Attributes::MAX_HEALTH, health);
    living.set_health(health as f32);
}

/// Vanilla `createOffspringAttribute`: around the parents' average, spread by their
/// difference plus 15% of the range, mirrored back into the range.
fn offspring_attribute(a: f64, b: f64, min: f64, max: f64, quality: f64) -> f64 {
    let (a, b) = (a.clamp(min, max), b.clamp(min, max));
    let margin = 0.15 * (max - min);
    let range = (a - b).abs() + margin * 2.0;
    let value = range.mul_add(quality, f64::midpoint(a, b));
    if value > max {
        max - (value - max)
    } else if value < min {
        min + (min - value)
    } else {
        value
    }
}

/// Vanilla `setOffspringAttributes`: health, jump and speed from both parents.
pub fn set_offspring_attributes(
    parent: &dyn AbstractHorse,
    partner: &dyn EntityBase,
    baby: &dyn AbstractHorse,
) {
    let Some(partner) = partner.get_living_entity() else {
        return;
    };
    let mine = &parent.get_mob_entity().living_entity;
    let born = &baby.get_mob_entity().living_entity;
    let ranges = [
        (&Attributes::MAX_HEALTH, 15.0, 30.0),
        (&Attributes::JUMP_STRENGTH, 0.4, 1.0),
        (&Attributes::MOVEMENT_SPEED, 0.1125, 0.3375),
    ];
    for (attribute, min, max) in ranges {
        let quality = {
            let mut rng = parent.get_random();
            (rng.random::<f64>() + rng.random::<f64>() + rng.random::<f64>()) / 3.0 - 0.5
        };
        let value = offspring_attribute(
            mine.get_attribute_base(attribute),
            partner.get_attribute_base(attribute),
            min,
            max,
            quality,
        );
        born.set_attribute_base(attribute, value);
        if attribute == &Attributes::MAX_HEALTH {
            born.set_health(value as f32);
        }
    }
}

/// Vanilla `canParent`.
pub fn can_parent(horse: &dyn AbstractHorse) -> bool {
    let entity = horse.get_entity();
    let living = &horse.get_mob_entity().living_entity;
    !entity.has_passengers()
        && !entity.has_vehicle()
        && is_tamed(horse)
        && !horse.is_baby()
        && living.health.load() >= living.get_max_health()
        && horse.get_mob_entity().is_in_love()
}

/// Vanilla `AbstractHorse.addAdditionalSaveData`.
pub fn write_horse_nbt(horse: &dyn AbstractHorse, nbt: &mut NbtCompound) {
    nbt.put_bool("EatingHaystack", is_eating(horse));
    nbt.put_bool("Bred", has_flag(horse, FLAG_BRED));
    nbt.put_int("Temper", temper(horse));
    nbt.put_bool("Tame", is_tamed(horse));
    if let Some(owner) = owner(horse) {
        nbt.put_uuid("Owner", owner);
    }
}

/// Vanilla `AbstractHorse.readAdditionalSaveData`.
pub fn read_horse_nbt(horse: &dyn AbstractHorse, nbt: &NbtCompound) {
    set_flag(
        horse,
        FLAG_EATING,
        nbt.get_bool("EatingHaystack").unwrap_or(false),
    );
    set_flag(horse, FLAG_BRED, nbt.get_bool("Bred").unwrap_or(false));
    horse
        .horse_data()
        .temper
        .store(nbt.get_int("Temper").unwrap_or(0), Ordering::Relaxed);
    set_flag(horse, FLAG_TAME, nbt.get_bool("Tame").unwrap_or(false));
    horse.horse_data().owner.store(nbt.get_uuid("Owner"));
}

/// Marks a foal as bred, vanilla's `setBred` in `getBreedOffspring`'s callers.
pub fn set_bred(horse: &dyn AbstractHorse) {
    set_flag(horse, FLAG_BRED, true);
}

/// Horse armour goes into the body slot, vanilla `equipBodyArmor`.
pub fn try_equip_body_armor(
    horse: &dyn AbstractHorse,
    player: &Player,
    stack: &mut ItemStack,
) -> bool {
    let is_armor = crate::entity::mob::equipment::is_equippable_in_slot(
        horse.get_entity().entity_type,
        stack,
        &EquipmentSlot::BODY,
    );
    let mob_entity = horse.get_mob_entity();
    let wearing = !mob_entity
        .living_entity
        .entity_equipment
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(&EquipmentSlot::BODY)
        .is_empty();
    if !is_armor || wearing {
        return false;
    }
    mob_entity.set_item_slot_and_drop_when_killed(&EquipmentSlot::BODY, stack.copy_with_count(1));
    stack.decrement_unless_creative(player.gamemode.load(), 1);
    true
}

/// Vanilla `AbstractHorse.getControllingPassenger`: the first passenger steers a saddled
/// horse when it is a player.
pub fn saddled_rider(horse: &dyn Mob) -> Option<Arc<Player>> {
    if !horse.is_saddled() {
        return None;
    }
    saddle::first_passenger_player(horse)
}

/// Vanilla `AbstractHorse.playStepSound`: hooves on wood, and a gallop under a rider.
pub fn play_step_sound(
    horse: &dyn AbstractHorse,
    pos: &BlockPos,
    state: &BlockState,
    can_gallop: bool,
) {
    if state.is_liquid() {
        return;
    }
    let entity = horse.get_entity();
    let above = entity.world.load().get_block_state(&pos.up());
    let sound_type = if above.id.to_block() == &Block::SNOW {
        above.get_sound_type()
    } else {
        state.get_sound_type()
    };
    let volume = sound_type.volume * 0.15;
    if entity.has_passengers() && can_gallop {
        let counter = &horse.horse_data().gallop_sound_counter;
        let gallop = counter.fetch_add(1, Ordering::Relaxed) + 1;
        if gallop > 5 && gallop % 3 == 0 {
            horse.play_entity_sound(Sound::EntityHorseGallop, volume, sound_type.pitch);
        } else if gallop <= 5 {
            horse.play_entity_sound(Sound::EntityHorseStepWood, volume, sound_type.pitch);
        }
        return;
    }
    let sound = if is_wood_sound_type(sound_type) {
        Sound::EntityHorseStepWood
    } else if horse.is_baby() {
        Sound::EntityBabyHorseStep
    } else {
        Sound::EntityHorseStep
    };
    horse.play_entity_sound(sound, volume, sound_type.pitch);
}

/// Vanilla `AbstractHorse.isWoodSoundType`.
fn is_wood_sound_type(sound_type: &SoundType) -> bool {
    matches!(
        sound_type.name,
        "WOOD" | "NETHER_WOOD" | "STEM" | "CHERRY_WOOD" | "BAMBOO_WOOD"
    )
}

/// Vanilla `AbstractHorse.registerGoals` with `addBehaviourGoals`.
pub fn register_horse_goals(
    goal_selector: &crate::entity::ai::goal::goal_selector::GoalSelector,
    mob_weak: std::sync::Weak<dyn Mob>,
    can_perform_rearing: bool,
) {
    use crate::entity::ai::goal::{
        breed::BreedGoal, escape_danger::EscapeDangerGoal, follow_parent::FollowParentGoal,
        look_around::RandomLookAroundGoal, look_at_entity::LookAtEntityGoal, swim::SwimGoal,
        tempt::TemptGoal, wander_around::WanderAroundGoal,
    };
    goal_selector.add_goal(1, Box::new(RunAroundLikeCrazyGoal::new(1.2)));
    goal_selector.add_goal(2, BreedGoal::new(1.0));
    goal_selector.add_goal(4, Box::new(FollowParentGoal::new(1.0)));
    goal_selector.add_goal(6, Box::new(WanderAroundGoal::water_avoiding(0.7)));
    goal_selector.add_goal(
        7,
        LookAtEntityGoal::with_default(mob_weak, &pumpkin_data::entity::EntityType::PLAYER, 6.0),
    );
    goal_selector.add_goal(8, Box::new(RandomLookAroundGoal::default()));
    if can_perform_rearing {
        goal_selector.add_goal(9, Box::new(RandomStandGoal::new()));
    }
    goal_selector.add_goal(0, Box::new(SwimGoal::default()));
    // Vanilla `MountPanicGoal`: no panic while a mob rides it.
    goal_selector.add_goal(1, EscapeDangerGoal::new(1.2));
    goal_selector.add_goal(
        3,
        Box::new(TemptGoal::with_tag(
            1.25,
            &pumpkin_data::tag::Item::MINECRAFT_HORSE_TEMPT_ITEMS,
            false,
        )),
    );
}
