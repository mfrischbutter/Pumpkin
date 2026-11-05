//! Vanilla `AbstractHorse`: what horses, donkeys, mules and skeleton and zombie horses share,
//! from taming by riding to rearing, grazing and the stats a foal inherits.

use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};

use crossbeam::atomic::AtomicCell;
use pumpkin_data::attributes::Attributes;
use pumpkin_data::block_state::SoundType;
use pumpkin_data::data_component_impl::{EquipmentSlot, EquippableImpl};
use pumpkin_data::entity::EntityStatus;
use pumpkin_data::game_event::GameEvent;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::Sound;
use pumpkin_data::{Block, BlockState};
use pumpkin_inventory::SimpleInventory;
use pumpkin_inventory::mount_screen_handler::MountScreenHandler;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_protocol::bedrock::server::actor_event::ActorEventID;
use pumpkin_util::math::position::BlockPos;
use rand::RngExt;
use uuid::Uuid;

use crate::entity::ageable::AgeableMob;
use crate::entity::ai::goal::Goal;
use crate::entity::ai::target_match::TargetMatch;
use crate::entity::ai::target_predicate::TargetPredicate;
use crate::entity::mob::Mob;
use crate::entity::passive::animal::Animal;
use crate::entity::passive::horse_food::{self, FoodEffect};
use crate::entity::passive::horse_inventory;
use crate::entity::player::Player;
use crate::entity::player::advancement::trigger::AdvancementTrigger;
use crate::entity::{Entity, EntityBase, mob::saddle};

pub const FLAG_TAME: u8 = 2;
pub const FLAG_BRED: u8 = 8;
pub const FLAG_EATING: u8 = 16;
pub const FLAG_STANDING: u8 = 32;
pub const FLAG_OPEN_MOUTH: u8 = 64;
/// Vanilla `AbstractHorse.getSoundVolume`.
pub const SOUND_VOLUME: f32 = 0.8;
/// Vanilla `AbstractHorse.getAmbientSoundInterval`, also its `getAmbientStandInterval`.
pub const AMBIENT_SOUND_INTERVAL: i32 = 400;
/// Vanilla `MOMMY_TARGETING`'s range.
const MOMMY_RANGE: f64 = 16.0;
const MOMMY_MIN_DISTANCE_SQ: f64 = 4.0;
/// Vanilla `playStepSound` and `playGallopSound`: hooves sound at this share of the block's volume.
const STEP_VOLUME_SCALE: f32 = 0.15;
/// Vanilla `causeFallDamage` and `playJumpSound`.
const LAND_VOLUME: f32 = 0.4;
const JUMP_VOLUME: f32 = 0.4;
/// Vanilla `PlayerRideableJumping.getPlayerJumpPendingScale`: a jump charged this long is full.
const MAX_JUMP_AMOUNT: i32 = 90;

/// The state vanilla `AbstractHorse` keeps beside its synced flags.
pub struct HorseData {
    flags: AtomicU8,
    temper: AtomicI32,
    owner: AtomicCell<Option<Uuid>>,
    eating_counter: AtomicI32,
    mouth_counter: AtomicI32,
    stand_counter: AtomicI32,
    allow_stand_sliding: AtomicBool,
    gallop_sound_counter: AtomicI32,
    /// Vanilla `inventory`: the chest, replaced as a whole when its size changes.
    pub(super) inventory: Mutex<Arc<SimpleInventory>>,
}

impl Default for HorseData {
    fn default() -> Self {
        Self {
            flags: AtomicU8::new(0),
            temper: AtomicI32::new(0),
            owner: AtomicCell::new(None),
            eating_counter: AtomicI32::new(0),
            mouth_counter: AtomicI32::new(0),
            stand_counter: AtomicI32::new(0),
            allow_stand_sliding: AtomicBool::new(false),
            gallop_sound_counter: AtomicI32::new(0),
            inventory: Mutex::new(Arc::new(SimpleInventory::new(0))),
        }
    }
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

    /// Vanilla `playGallopSound`.
    fn play_gallop_sound(&self, sound_type: &SoundType) {
        play_gallop(self, sound_type);
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

    /// Vanilla `isMobControlled`: ridden by a mob, like a zombie horse's jockey.
    fn is_mob_controlled(&self) -> bool {
        false
    }

    /// Vanilla `handleEating`'s table.
    fn food_effect(&self, item: &Item) -> FoodEffect {
        horse_food::horse_food_effect(item)
    }

    /// Vanilla `eating`; `Llama.handleEating` only plays the sound.
    fn opens_mouth_to_eat(&self) -> bool {
        true
    }

    /// Vanilla `followMommy`'s own overrides, past the bred baby check.
    fn can_follow_mommy(&self) -> bool {
        true
    }

    /// Vanilla `doPlayerRide`'s own overrides.
    fn can_player_ride(&self) -> bool {
        true
    }

    /// Vanilla `AbstractChestedHorse.hasChest`.
    fn has_chest(&self) -> bool {
        false
    }

    /// Vanilla `getInventoryColumns`.
    fn inventory_columns(&self) -> i32 {
        0
    }

    /// Vanilla `openCustomInventoryScreen`'s check, which a camel leaves out.
    fn may_open_inventory(&self, player: &Player) -> bool {
        let entity = self.get_entity();
        (!entity.has_passengers() || entity.has_passenger(player.entity_id())) && self.is_tamed()
    }

    /// The mount screen slot Bedrock shows as its one equipment slot.
    fn bedrock_equip_slot(&self) -> usize {
        MountScreenHandler::SLOT_SADDLE
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
pub fn modify_temper(horse: &dyn AbstractHorse, amount: i32) {
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
        horse.play_entity_sound(
            sound,
            SOUND_VOLUME,
            horse.get_mob_entity().living_entity.get_pitch(),
        );
    }
}

/// Vanilla `openMouth`.
pub(super) fn open_mouth(horse: &dyn AbstractHorse) {
    horse.horse_data().mouth_counter.store(1, Ordering::Relaxed);
    set_flag(horse, FLAG_OPEN_MOUTH, true);
}

/// Vanilla `tameWithName`: the rider becomes the owner.
pub fn tame_with_name(horse: &dyn AbstractHorse, player: &Player) {
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

/// Vanilla `followMommy`: a bred foal plans a path to the nearest bred horse of the family 16
/// blocks around. It only plans (`createPath`, no `moveTo`); `FollowParentGoal` does the walking.
/// Breeding does not set the bred flag (vanilla `setBred` is only read from saves).
fn follow_mommy(horse: &dyn AbstractHorse) {
    if !horse.can_follow_mommy()
        || !has_flag(horse, FLAG_BRED)
        || !horse.is_baby()
        || is_eating(horse)
    {
        return;
    }
    let entity = horse.get_entity();
    let world = entity.world.load();
    let pos = entity.pos.load();
    let tester: &dyn EntityBase = horse;
    let mommy_targeting = TargetPredicate::create_non_attackable()
        .set_base_max_distance(MOMMY_RANGE)
        .ignore_visibility();
    let area = entity
        .bounding_box
        .load()
        .expand(MOMMY_RANGE, MOMMY_RANGE, MOMMY_RANGE);
    let mommy = world
        .get_entities_at_box(&area)
        .into_iter()
        .filter(|other| other.get_entity().entity_id != entity.entity_id)
        .filter(|other| {
            other
                .get_mob()
                .and_then(|mob| mob.as_abstract_horse())
                .is_some_and(|other| has_flag(other, FLAG_BRED))
                && mommy_targeting.test(&world, Some(tester), other.as_ref())
        })
        .min_by(|a, b| {
            let da = a.get_entity().pos.load().squared_distance_to_vec(&pos);
            let db = b.get_entity().pos.load().squared_distance_to_vec(&pos);
            da.total_cmp(&db)
        });
    if let Some(mommy) = mommy {
        let target = mommy.get_entity().pos.load();
        if target.squared_distance_to_vec(&pos) > MOMMY_MIN_DISTANCE_SQ {
            let steered = horse.get_mob_entity().steered();
            let owner = &steered.mob_entity().living_entity;
            steered.navigator().create_path(owner, target, 0);
        }
    }
}

/// Vanilla `doPlayerRide`.
fn do_player_ride(horse: &dyn AbstractHorse, player: &Arc<Player>) {
    if !horse.can_player_ride() {
        return;
    }
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

/// Vanilla `Entity.interact`: the rope's holder lets go, a lead in hand ties up unless another
/// player holds the rope.
fn is_leash_click(horse: &dyn AbstractHorse, player: &Player, stack: &ItemStack) -> bool {
    let holder = horse.get_entity().get_leash_holder();
    let held_by_player = holder
        .as_ref()
        .is_some_and(|holder| holder.get_player().is_some());
    holder.is_some_and(|holder| holder.get_entity().entity_id == player.entity_id())
        || (stack.item == &Item::LEAD && !held_by_player)
}

/// Vanilla `Player.isHolding`: in either hand.
fn is_holding(player: &Player, item: &Item) -> bool {
    let inventory = player.inventory();
    inventory.held_item().item == item || inventory.off_hand_item().item == item
}

/// Vanilla `Mob.interact` before the mob's own `mobInteract`.
///
/// Name tags and spawn eggs are left to their items, and the rope's holder or a lead in hand
/// deals with the leash. `Some` when the click was one of those.
pub fn important_interaction(
    horse: &dyn AbstractHorse,
    player: &Arc<Player>,
    stack: &mut ItemStack,
) -> Option<bool> {
    if stack.item == &Item::NAME_TAG || stack.item.registry_key.ends_with("_spawn_egg") {
        return Some(false);
    }
    if is_leash_click(horse, player, stack)
        && horse
            .get_mob_entity()
            .mob_interact(player, stack, horse.can_be_leashed())
    {
        return Some(true);
    }
    None
}

/// The held-item part of vanilla's horse interactions, before `AbstractHorse.mobInteract`.
///
/// Vanilla `Horse.mobInteract`, `AbstractChestedHorse.mobInteract` and `ZombieHorse.mobInteract`:
/// a held item feeds, angers a wild horse (a foal too) or, through `extra`, goes on as a chest.
/// Skipped for a ridden horse, a sneaking click on a tamed adult and, where
/// `golden_dandelion_skips`, a foal while the player holds a golden dandelion. `None` passes the
/// click on.
pub fn held_item_interaction(
    horse: &dyn AbstractHorse,
    player: &Arc<Player>,
    stack: &mut ItemStack,
    golden_dandelion_skips: bool,
    extra: impl FnOnce(&mut ItemStack) -> bool,
) -> Option<bool> {
    let baby = horse.is_baby();
    let opens_inventory = !baby && is_tamed(horse) && player.get_entity().is_sneaking();
    if horse.get_entity().has_passengers()
        || opens_inventory
        || stack.is_empty()
        || (golden_dandelion_skips && baby && is_holding(player, &Item::GOLDEN_DANDELION))
    {
        return None;
    }
    if horse.is_food(stack) {
        return Some(horse_food::fed_food(horse, player, stack));
    }
    if !is_tamed(horse) {
        make_mad(horse);
        return Some(true);
    }
    extra(stack).then_some(true)
}

/// Vanilla `AbstractHorse.mobInteract`: a foal or a ridden horse is only fed like any animal, a
/// sneaking click opens a tamed horse's inventory, a saddle or body armour goes on, else the
/// player mounts.
pub fn abstract_horse_interact(
    horse: &dyn AbstractHorse,
    player: &Arc<Player>,
    stack: &mut ItemStack,
) -> bool {
    if horse.get_entity().has_passengers() || horse.is_baby() {
        return horse.animal_interact(player, stack);
    }
    if is_tamed(horse) && player.get_entity().is_sneaking() {
        horse_inventory::open_custom_inventory_screen(horse, player);
        return true;
    }
    if !stack.is_empty()
        && (saddle::try_equip_saddle(horse, player, stack)
            || try_equip_body_armor(horse, player, stack))
    {
        return true;
    }
    do_player_ride(horse, player);
    true
}

/// Vanilla `Horse.mobInteract` and `AbstractChestedHorse.mobInteract` with `AbstractHorse`'s.
///
/// [`important_interaction`], [`held_item_interaction`] with `extra` (a chest for donkeys, mules
/// and llamas), then [`abstract_horse_interact`].
pub fn horse_interact(
    horse: &dyn AbstractHorse,
    player: &Arc<Player>,
    stack: &mut ItemStack,
    extra: impl FnOnce(&mut ItemStack) -> bool,
) -> bool {
    if let Some(result) = important_interaction(horse, player, stack) {
        return result;
    }
    if let Some(result) = held_item_interaction(horse, player, stack, true, extra) {
        return result;
    }
    abstract_horse_interact(horse, player, stack)
}

/// Vanilla `RandomStandGoal`: now and then a horse rears with its ambient sound.
pub struct RandomStandGoal {
    next_stand: i32,
}

impl RandomStandGoal {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            next_stand: -AMBIENT_SOUND_INTERVAL,
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
            // Vanilla `getAmbientStandInterval`.
            self.next_stand = -mob.get_ambient_sound_interval();
            return !mob.is_immobile() && rng.random_range(0..10) == 0;
        }
        false
    }

    fn should_continue(&mut self, _mob: &dyn Mob) -> bool {
        false
    }

    /// Vanilla `start` with `playStandSound`, through `Entity.playSound` at full volume.
    fn start(&mut self, mob: &dyn Mob) {
        let Some(horse) = as_horse(mob) else {
            return;
        };
        stand_if_possible(horse);
        if let Some(sound) = mob.get_ambient_sound() {
            mob.play_entity_sound(sound, 1.0, 1.0);
        }
    }

    fn should_run_every_tick(&self) -> bool {
        true
    }
}

/// Vanilla `AbstractHorse.isImmobile`: eating, rearing or ridden with a saddle while dying.
pub fn is_immobile(horse: &dyn AbstractHorse) -> bool {
    let living = &horse.get_mob_entity().living_entity;
    (living.is_dead_or_dying() && horse.get_entity().has_passengers() && horse.is_saddled())
        || is_eating(horse)
        || is_standing(horse)
}

/// Vanilla `hurtServer`: a hurt horse rears one time in three.
pub fn on_hurt(horse: &dyn AbstractHorse) {
    if horse.get_random().random_range(0..3) == 0 {
        stand_if_possible(horse);
    }
}

/// Vanilla `onElasticLeashPull`: a pulled horse stops grazing.
pub fn on_leash_pull(horse: &dyn AbstractHorse) {
    if is_eating(horse) {
        set_flag(horse, FLAG_EATING, false);
    }
}

/// Vanilla `handleStartJump`: a rider's jump makes it rear with the jump sound.
pub fn on_rider_jump(horse: &dyn AbstractHorse) {
    horse
        .horse_data()
        .allow_stand_sliding
        .store(true, Ordering::Relaxed);
    stand_if_possible(horse);
    horse.play_entity_sound(horse.horse_jump_sound(), JUMP_VOLUME, 1.0);
}

/// Vanilla `PlayerRideableJumping.getPlayerJumpPendingScale`: how strong a jump charged for
/// `jump_amount` ticks is.
#[must_use]
pub const fn player_jump_pending_scale(jump_amount: i32) -> f32 {
    if jump_amount >= MAX_JUMP_AMOUNT {
        1.0
    } else {
        0.4 + 0.4 * jump_amount as f32 / MAX_JUMP_AMOUNT as f32
    }
}

/// Vanilla `causeFallDamage`'s landing sound, after more than a block of falling. The damage
/// itself takes the shared path.
pub fn play_land_sound(horse: &dyn AbstractHorse, fall_distance: f32) {
    if fall_distance > 1.0 {
        let sound = if horse.is_baby() {
            Sound::EntityBabyHorseLand
        } else {
            Sound::EntityHorseLand
        };
        horse.play_entity_sound(sound, LAND_VOLUME, 1.0);
    }
}

/// Vanilla `getDefaultDimensions`: a foal has its own size.
pub fn refresh_age_dimensions(horse: &dyn AbstractHorse) {
    let entity = horse.get_entity();
    entity.set_default_dimensions(Entity::age_dimensions(entity.entity_type, horse.is_baby()));
}

/// Vanilla `generateMaxHealth`: 15 to 30.
fn generate_max_health(mut next_int: impl FnMut(i32) -> i32) -> f64 {
    15.0 + f64::from(next_int(8) + next_int(9))
}

/// Vanilla `generateJumpStrength`, from three rolls in 0 to 1.
pub fn generate_jump_strength(mut probability: impl FnMut() -> f64) -> f64 {
    f64::from(0.4f32) + probability() * 0.2 + probability() * 0.2 + probability() * 0.2
}

/// Vanilla `generateSpeed`, from three rolls in 0 to 1.
fn generate_speed(mut probability: impl FnMut() -> f64) -> f64 {
    (f64::from(0.45f32) + probability() * 0.3 + probability() * 0.3 + probability() * 0.3) * 0.25
}

/// Vanilla `Horse.randomizeAttributes`: a new horse's own health, speed and jump.
pub fn randomize_attributes(horse: &dyn AbstractHorse) {
    randomize_max_health(horse);
    let living = &horse.get_mob_entity().living_entity;
    let (speed, jump) = {
        let mut rng = horse.get_random();
        let speed = generate_speed(|| rng.random());
        (speed, generate_jump_strength(|| rng.random()))
    };
    living.set_attribute_base(&Attributes::MOVEMENT_SPEED, speed);
    living.set_attribute_base(&Attributes::JUMP_STRENGTH, jump);
}

/// Vanilla `AbstractChestedHorse.randomizeAttributes`: the health only, also for llamas.
pub fn randomize_max_health(horse: &dyn AbstractHorse) {
    let living = &horse.get_mob_entity().living_entity;
    let health = {
        let mut rng = horse.get_random();
        generate_max_health(|bound| rng.random_range(0..bound))
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

/// Vanilla keeps the extremes a new horse can roll (`MIN_HEALTH` to `MAX_MOVEMENT_SPEED`) as
/// floats.
fn as_float(value: f64) -> f64 {
    f64::from(value as f32)
}

/// Vanilla `setOffspringAttributes`: health, jump and speed from both parents, within the
/// ranges a new horse rolls.
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
        (
            &Attributes::MAX_HEALTH,
            generate_max_health(|_| 0),
            generate_max_health(|bound| bound - 1),
        ),
        (
            &Attributes::JUMP_STRENGTH,
            as_float(generate_jump_strength(|| 0.0)),
            as_float(generate_jump_strength(|| 1.0)),
        ),
        (
            &Attributes::MOVEMENT_SPEED,
            as_float(generate_speed(|| 0.0)),
            as_float(generate_speed(|| 1.0)),
        ),
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

/// Vanilla `equipBodyArmor`: body armour this horse can wear goes into the body slot, with
/// `LivingEntity.onEquipItem`'s sound and game event.
pub fn try_equip_body_armor(
    horse: &dyn AbstractHorse,
    player: &Player,
    stack: &mut ItemStack,
) -> bool {
    let entity = horse.get_entity();
    let is_armor = crate::entity::mob::equipment::is_equippable_in_slot(
        entity.entity_type,
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
    let armor = stack.copy_with_count(1);
    let equip_sound = armor
        .get_data_component::<EquippableImpl>()
        .map(|equippable| equippable.equip_sound.clone());
    mob_entity.set_item_slot_and_drop_when_killed(&EquipmentSlot::BODY, armor);
    stack.decrement_unless_creative(player.gamemode.load(), 1);
    let world = entity.world.load();
    let pos = entity.pos.load();
    if let Some(sound) = equip_sound
        && !entity.is_silent()
    {
        world.play_sound_event(&sound, horse.mob_sound_source(), &pos);
    }
    world.emit_game_event(GameEvent::Equip.name(), pos);
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
    let volume = sound_type.volume * STEP_VOLUME_SCALE;
    if entity.has_passengers() && can_gallop {
        let gallop = next_gallop_step(horse);
        if gallop > 5 && gallop % 3 == 0 {
            horse.play_gallop_sound(sound_type);
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

/// Vanilla `gallopSoundCounter`, one step more under a rider. Only the rider's client resets
/// it, so the server keeps galloping once the first steps are past.
pub fn next_gallop_step(horse: &dyn AbstractHorse) -> i32 {
    let counter = &horse.horse_data().gallop_sound_counter;
    counter.fetch_add(1, Ordering::Relaxed) + 1
}

/// Vanilla `AbstractHorse.playGallopSound`.
pub fn play_gallop<H: EntityBase + ?Sized>(horse: &H, sound_type: &SoundType) {
    horse.play_entity_sound(
        Sound::EntityHorseGallop,
        sound_type.volume * STEP_VOLUME_SCALE,
        sound_type.pitch,
    );
}

/// Vanilla `AbstractHorse.isWoodSoundType`.
fn is_wood_sound_type(sound_type: &SoundType) -> bool {
    matches!(
        sound_type.name,
        "WOOD" | "NETHER_WOOD" | "STEM" | "CHERRY_WOOD" | "BAMBOO_WOOD"
    )
}

/// Vanilla `AbstractHorse.registerGoals`, without `addBehaviourGoals`. The breed goal looks at
/// every horse kind (`AbstractHorse.class`); the species' `canMate` decides.
pub fn register_horse_goals(
    goal_selector: &crate::entity::ai::goal::goal_selector::GoalSelector,
    mob_weak: std::sync::Weak<dyn Mob>,
    can_perform_rearing: bool,
) {
    use crate::entity::ai::goal::{
        breed::BreedGoal, follow_parent::FollowParentGoal, look_around::RandomLookAroundGoal,
        look_at_entity::LookAtEntityGoal, run_around_like_crazy::RunAroundLikeCrazyGoal,
        wander_around::WanderAroundGoal,
    };
    goal_selector.add_goal(1, Box::new(RunAroundLikeCrazyGoal::new(1.2)));
    goal_selector.add_goal(
        2,
        BreedGoal::with_partners(1.0, TargetMatch::ABSTRACT_HORSE),
    );
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
}

/// Vanilla `AbstractHorse.addBehaviourGoals`: floating, panicking (`MountPanicGoal`, which only
/// differs for horses a mob controls) and following tempting food.
pub fn add_behaviour_goals(goal_selector: &crate::entity::ai::goal::goal_selector::GoalSelector) {
    use crate::entity::ai::goal::{
        escape_danger::EscapeDangerGoal, swim::SwimGoal, tempt::TemptGoal,
    };
    goal_selector.add_goal(0, Box::new(SwimGoal::default()));
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
