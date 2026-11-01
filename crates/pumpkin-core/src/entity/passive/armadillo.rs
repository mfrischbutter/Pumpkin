use std::sync::{
    Arc,
    atomic::{AtomicI32, AtomicU64, Ordering},
};

use pumpkin_data::BlockState;
use pumpkin_data::damage::DamageType;
use pumpkin_data::entity::EntityType;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::tag::{self, Taggable};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_util::math::position::BlockPos;

use pumpkin_data::entity::EntityStatus;
use pumpkin_data::environment_attribute::Activity;
use pumpkin_protocol::bedrock::server::actor_event::ActorEventID;
use rand::RngExt;

use crate::entity::ai::brain::{
    Brain, BrainProvider, BrainTick,
    activity::ActivityData,
    behavior::{
        AnimalMakeLove, AnimalPanic, CountDownCooldownTicks, DoNothing, FollowTemptation,
        LookAtTargetSink, MoveToTargetSink, OneShot, RandomLookAround, Swim, Timed,
        baby_follow_adult, random_stroll, run_one, run_one_with_conditions,
        set_entity_look_target_sometimes, set_walk_target_from_look_target, timed::Behavior,
    },
    memory::{MemoryModuleId, MemoryStatus, PackedMemories, types},
    sensing::SensorType,
};
use crate::entity::{
    Entity, EntityBase,
    ageable::{AgeableData, AgeableMob},
    item::ItemEntity,
    mob::{Mob, MobEntity},
    passive::animal::Animal,
    player::Player,
};

pub const ARMADILLO_FOOD: &[&Item] = &[&Item::SPIDER_EYE];
pub const ARMADILLO_BABY_START_AGE: i32 = -48000;
pub const SCARE_DISTANCE_HORIZONTAL: f64 = 7.0;
pub const SCARE_DISTANCE_VERTICAL: f64 = 2.0;

fn pick_next_scute_drop_time() -> i32 {
    let rand_ticks = (rand::random::<u32>() % 6000) as i32;
    rand_ticks + 6000
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(i32)]
pub enum ArmadilloState {
    #[default]
    Idle = 0,
    Rolling = 1,
    Scared = 2,
    Unrolling = 3,
}

impl ArmadilloState {
    #[must_use]
    pub const fn from_id(id: i32) -> Self {
        match id {
            1 => Self::Rolling,
            2 => Self::Scared,
            3 => Self::Unrolling,
            _ => Self::Idle,
        }
    }

    #[must_use]
    pub const fn id(self) -> i32 {
        self as i32
    }

    #[must_use]
    pub const fn is_threatened(self) -> bool {
        !matches!(self, Self::Idle)
    }

    #[must_use]
    pub const fn animation_duration(self) -> u64 {
        match self {
            Self::Idle => 0,
            Self::Rolling => 10,
            Self::Scared => 50,
            Self::Unrolling => 30,
        }
    }

    #[must_use]
    pub const fn should_hide_in_shell(self, ticks_in_state: u64) -> bool {
        match self {
            Self::Idle => false,
            Self::Rolling => ticks_in_state > 5,
            Self::Scared => true,
            Self::Unrolling => ticks_in_state < 26,
        }
    }

    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Rolling => "rolling",
            Self::Scared => "scared",
            Self::Unrolling => "unrolling",
        }
    }

    #[must_use]
    pub fn from_name(name: &str) -> Self {
        match name {
            "rolling" => Self::Rolling,
            "scared" => Self::Scared,
            "unrolling" => Self::Unrolling,
            _ => Self::Idle,
        }
    }
}

/// Represents an Armadillo, a passive entity that can roll into a ball when threatened.
///
/// Wiki: <https://minecraft.wiki/w/Armadillo>
pub struct ArmadilloEntity {
    pub mob_entity: MobEntity,
    pub ageable_data: AgeableData,
    pub state: AtomicI32,
    pub in_state_ticks: AtomicU64,
    pub scute_time: AtomicI32,
}

fn as_armadillo(mob: &dyn Mob) -> Option<&ArmadilloEntity> {
    mob.cast_any().downcast_ref::<ArmadilloEntity>()
}

impl ArmadilloEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let mob_entity = MobEntity::new(entity);
        let armadillo = Self {
            mob_entity,
            ageable_data: AgeableData::default(),
            state: AtomicI32::new(ArmadilloState::Idle.id()),
            in_state_ticks: AtomicU64::new(0),
            scute_time: AtomicI32::new(pick_next_scute_drop_time()),
        };
        Arc::new(armadillo)
    }

    #[must_use]
    pub fn get_state(&self) -> ArmadilloState {
        ArmadilloState::from_id(self.state.load(Ordering::Relaxed))
    }

    pub fn switch_to_state(&self, state: ArmadilloState) {
        self.state.store(state.id(), Ordering::Relaxed);
        self.in_state_ticks.store(0, Ordering::Relaxed);
        let entity = self.get_entity();
        entity.set_synced_data(
            pumpkin_data::tracked_data::armadillo::ARMADILLO_STATE,
            VarInt(state.id()),
        );
    }

    #[must_use]
    pub fn is_scared(&self) -> bool {
        self.get_state() != ArmadilloState::Idle
    }

    #[must_use]
    pub fn should_hide_in_shell(&self) -> bool {
        self.get_state()
            .should_hide_in_shell(self.in_state_ticks.load(Ordering::Relaxed))
    }

    #[must_use]
    pub fn should_switch_to_scared_state(&self) -> bool {
        self.get_state() == ArmadilloState::Rolling
            && self.in_state_ticks.load(Ordering::Relaxed)
                > ArmadilloState::Rolling.animation_duration()
    }

    pub fn can_stay_rolled_up(&self) -> bool {
        !self.is_panicking()
            && !self.mob_entity.living_entity.is_in_water()
            && !self.get_entity().has_vehicle()
    }

    pub fn roll_up(&self) {
        if !self.is_scared() {
            // Vanilla `stopInPlace`.
            self.mob_entity.steered().navigator().stop();
            self.mob_entity.reset_love_ticks();
            let entity = self.get_entity();
            let world = entity.world.load();
            world.play_sound(
                Sound::EntityArmadilloRoll,
                SoundCategory::Neutral,
                &entity.pos.load(),
            );
            self.switch_to_state(ArmadilloState::Rolling);
        }
    }

    pub fn roll_out(&self) {
        if self.is_scared() {
            let entity = self.get_entity();
            let world = entity.world.load();
            world.play_sound(
                Sound::EntityArmadilloUnrollFinish,
                SoundCategory::Neutral,
                &entity.pos.load(),
            );
            self.switch_to_state(ArmadilloState::Idle);
        }
    }

    pub fn brush_off_scute(&self, player: &Player, item_stack: &mut ItemStack) -> bool {
        if self.is_baby() || !self.get_entity().is_alive() {
            return false;
        }
        let entity = self.get_entity();
        let world = entity.world.load();
        let pos = entity.pos.load();
        let item_entity = Arc::new(ItemEntity::new(
            Entity::new(world.clone(), pos, &EntityType::ITEM),
            ItemStack::new(1, &Item::ARMADILLO_SCUTE),
        ));
        world.spawn_entity(item_entity);
        world.play_sound(Sound::EntityArmadilloBrush, SoundCategory::Neutral, &pos);
        if player.gamemode.load() != pumpkin_util::GameMode::Creative {
            let _ = item_stack.damage_item(16);
        }
        true
    }

    pub fn is_scared_by(&self, living_entity: &dyn EntityBase) -> bool {
        let entity = self.get_entity();
        let pos = entity.pos.load();
        let target_pos = living_entity.get_entity().pos.load();
        let dx = (pos.x - target_pos.x).abs();
        let dy = (pos.y - target_pos.y).abs();
        let dz = (pos.z - target_pos.z).abs();

        if dx > SCARE_DISTANCE_HORIZONTAL
            || dz > SCARE_DISTANCE_HORIZONTAL
            || dy > SCARE_DISTANCE_VERTICAL
        {
            return false;
        }

        let target_type = living_entity.get_entity().entity_type;
        if target_type.has_tag(&tag::EntityType::MINECRAFT_UNDEAD) {
            return true;
        }

        // Vanilla `isScaredBy`: whoever last hurt it too.
        if self
            .mob_entity
            .living_entity
            .get_last_hurt_by_mob()
            .is_some_and(|attacker| {
                attacker.get_entity().entity_id == living_entity.get_entity().entity_id
            })
        {
            return true;
        }

        if target_type == &EntityType::PLAYER {
            let target_ent = living_entity.get_entity();
            if target_ent.is_sprinting() || target_ent.has_vehicle() {
                return true;
            }
        }

        false
    }
}

impl AgeableMob for ArmadilloEntity {
    fn get_ageable_data(&self) -> &AgeableData {
        &self.ageable_data
    }

    fn get_baby_start_age(&self) -> i32 {
        ARMADILLO_BABY_START_AGE
    }
}

impl Animal for ArmadilloEntity {
    fn is_food(&self, item_stack: &ItemStack) -> bool {
        item_stack
            .item
            .has_tag(&tag::Item::MINECRAFT_ARMADILLO_FOOD)
            || ARMADILLO_FOOD.iter().any(|i| i.id == item_stack.item.id)
    }
}

impl Mob for ArmadilloEntity {
    /// Vanilla `Armadillo.getAmbientSound`.
    fn get_ambient_sound(&self) -> Option<pumpkin_data::sound::Sound> {
        (!self.is_scared()).then_some(pumpkin_data::sound::Sound::EntityArmadilloAmbient)
    }

    fn mob_play_step_sound(&self, _pos: &BlockPos, _state: &BlockState) {
        self.play_entity_sound(Sound::EntityArmadilloStep, 0.15, 1.0);
    }

    fn as_ageable(&self) -> Option<&dyn AgeableMob> {
        Some(self)
    }

    fn as_animal(&self) -> Option<&dyn Animal> {
        Some(self)
    }

    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        nbt.put_string("state", self.get_state().name().to_string());
        nbt.put_int("scute_time", self.scute_time.load(Ordering::Relaxed));
    }

    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        if let Some(state_name) = nbt.get_string("state") {
            self.switch_to_state(ArmadilloState::from_name(state_name));
        } else if let Some(state_id) = nbt.get_int("state") {
            self.switch_to_state(ArmadilloState::from_id(state_id));
        }
        if let Some(scute_time) = nbt.get_int("scute_time") {
            self.scute_time.store(scute_time, Ordering::Relaxed);
        }
    }

    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    fn modify_incoming_damage(&self, amount: f32, _damage_type: DamageType) -> f32 {
        if self.is_scared() {
            (amount - 1.0).max(0.0) / 2.0
        } else {
            amount
        }
    }

    /// Vanilla `Armadillo.actuallyHurt`: a hit by a mob rolls it up and keeps it wary for 80
    /// ticks; fire, lava and the like make it roll out and flee.
    fn on_damage(&self, damage_type: DamageType, source: Option<&dyn EntityBase>) {
        if self.mob_entity.is_no_ai() || !self.get_entity().is_alive() {
            return;
        }
        if source.is_some_and(|source| source.get_living_entity().is_some()) {
            self.mob_entity.with_brain(self, |brain| {
                brain.set_with_expiry(types::DANGER_DETECTED_RECENTLY, true, 80);
            });
            if self.can_stay_rolled_up() {
                self.roll_up();
            }
        } else if damage_type.has_tag(&tag::DamageType::MINECRAFT_PANIC_ENVIRONMENTAL_CAUSES) {
            self.roll_out();
        }
    }

    fn mob_tick(&self, _caller: &dyn EntityBase) {
        self.in_state_ticks.fetch_add(1, Ordering::Relaxed);
    }

    fn make_brain(&self, packed: &PackedMemories) -> Brain {
        make_brain(self, packed)
    }

    /// Vanilla `Armadillo.customServerAiStep`: the brain, then the scute it sheds.
    fn custom_server_ai_step(&self, _caller: &dyn EntityBase) {
        self.mob_entity.tick_brain(self);
        self.mob_entity.with_brain(self, |brain| {
            brain.set_active_activity_to_first_valid(&[Activity::Panic, Activity::Idle]);
        });

        let entity = self.get_entity();
        let world = entity.world.load();

        if entity.is_alive() && !self.is_baby() {
            let scute_time = self.scute_time.fetch_sub(1, Ordering::Relaxed) - 1;
            if scute_time <= 0 {
                let pos = entity.pos.load();
                let item_entity = Arc::new(ItemEntity::new(
                    Entity::new(world.clone(), pos, &EntityType::ITEM),
                    ItemStack::new(1, &Item::ARMADILLO_SCUTE),
                ));
                world.spawn_entity_non_save(item_entity as Arc<dyn EntityBase>);
                world.play_sound(
                    Sound::EntityArmadilloScuteDrop,
                    SoundCategory::Neutral,
                    &pos,
                );
                self.scute_time
                    .store(pick_next_scute_drop_time(), Ordering::Relaxed);
            }
        }
    }

    fn mob_init_data_tracker(&self) {
        let entity = self.get_entity();
        let is_baby = entity.age.load(Ordering::Relaxed) < 0;
        if is_baby {
            entity.set_synced_data(pumpkin_data::tracked_data::armadillo::BABY_ID, true);
        }
        entity.set_synced_data(
            pumpkin_data::tracked_data::armadillo::ARMADILLO_STATE,
            VarInt(self.get_state().id()),
        );
    }

    fn mob_interact(&self, player: &Arc<Player>, item_stack: &mut ItemStack) -> bool {
        if item_stack.item == &Item::BRUSH && self.brush_off_scute(player, item_stack) {
            return true;
        }
        if self.is_scared() {
            return false;
        }
        self.animal_interact(player, item_stack, Sound::EntityArmadilloAmbient)
    }
}

const MEMORIES: [MemoryModuleId; 0] = [];

/// Vanilla `ArmadilloAi`.
fn make_brain(armadillo: &ArmadilloEntity, packed: &PackedMemories) -> Brain {
    BrainProvider {
        memory_types: &MEMORIES,
        sensor_types: &[
            SensorType::NearestLivingEntities,
            SensorType::HurtBy,
            SensorType::FoodTemptations,
            SensorType::NearestAdult,
            SensorType::ArmadilloScareDetected,
        ],
        activities,
    }
    .make_brain(armadillo, packed)
}

fn is_scared(mob: &dyn Mob) -> bool {
    as_armadillo(mob).is_some_and(ArmadilloEntity::is_scared)
}

/// Vanilla `ArmadilloPanic`: it rolls out to run.
fn roll_out(mob: &dyn Mob) {
    if let Some(armadillo) = as_armadillo(mob) {
        armadillo.roll_out();
    }
}

/// Vanilla `ARMADILLO_ROLLING_OUT`: once the danger is gone, a curled up armadillo unrolls.
fn rolling_out() -> OneShot {
    OneShot::new(
        "ArmadilloRollingOut",
        vec![(
            types::DANGER_DETECTED_RECENTLY.id(),
            MemoryStatus::ValueAbsent,
        )],
        |tick| {
            let Some(armadillo) = as_armadillo(tick.mob) else {
                return false;
            };
            if !armadillo.is_scared() {
                return false;
            }
            armadillo.roll_out();
            true
        },
    )
}

/// Vanilla `MoveToTargetSink` with the armadillo's override: not while curled up.
struct ArmadilloMoveToTarget(MoveToTargetSink);

impl Behavior for ArmadilloMoveToTarget {
    fn entry_conditions(&self) -> &[(MemoryModuleId, MemoryStatus)] {
        self.0.entry_conditions()
    }

    fn min_duration(&self) -> i32 {
        self.0.min_duration()
    }

    fn max_duration(&self) -> i32 {
        self.0.max_duration()
    }

    fn check_extra_start_conditions(&mut self, tick: &mut BrainTick<'_>) -> bool {
        !is_scared(tick.mob) && self.0.check_extra_start_conditions(tick)
    }

    fn start(&mut self, tick: &mut BrainTick<'_>) {
        self.0.start(tick);
    }

    fn tick(&mut self, tick: &mut BrainTick<'_>) {
        self.0.tick(tick);
    }

    fn stop(&mut self, tick: &mut BrainTick<'_>) {
        self.0.stop(tick);
    }

    fn can_still_use(&mut self, tick: &mut BrainTick<'_>) -> bool {
        self.0.can_still_use(tick)
    }

    fn debug_name(&self) -> &'static str {
        self.0.debug_name()
    }
}

fn activities(_mob: &dyn Mob) -> Vec<ActivityData> {
    vec![
        ActivityData::with_priority_start(
            Activity::Core,
            0,
            vec![
                Box::new(Timed::new(Swim::new(0.8))),
                Box::new(Timed::new(
                    AnimalPanic::new(2.0)
                        .with_panic_causes(&tag::DamageType::MINECRAFT_PANIC_ENVIRONMENTAL_CAUSES)
                        .on_start(roll_out),
                )),
                Box::new(Timed::new(LookAtTargetSink::new(45, 90))),
                Box::new(Timed::new(ArmadilloMoveToTarget(
                    MoveToTargetSink::default(),
                ))),
                Box::new(CountDownCooldownTicks::new(
                    types::TEMPTATION_COOLDOWN_TICKS,
                )),
                Box::new(CountDownCooldownTicks::new(types::GAZE_COOLDOWN_TICKS)),
                Box::new(rolling_out()),
            ],
        ),
        ActivityData::with_priority_start(
            Activity::Idle,
            0,
            vec![
                Box::new(set_entity_look_target_sometimes::create(6.0, 30, 60)),
                Box::new(Timed::new(AnimalMakeLove::new())),
                Box::new(run_one(vec![
                    (
                        Box::new(Timed::new(FollowTemptation::new(
                            |_| 1.25,
                            |mob| {
                                if mob.get_entity().age.load(Ordering::Relaxed) < 0 {
                                    1.0
                                } else {
                                    2.0
                                }
                            },
                        ))),
                        1,
                    ),
                    (Box::new(baby_follow_adult(5, 16, 1.25)), 1),
                ])),
                Box::new(Timed::new(RandomLookAround::new(
                    (150, 250),
                    30.0,
                    0.0,
                    0.0,
                ))),
                Box::new(run_one_with_conditions(
                    vec![(types::WALK_TARGET.id(), MemoryStatus::ValueAbsent)],
                    vec![
                        (Box::new(random_stroll::stroll(1.0)), 1),
                        (
                            Box::new(set_walk_target_from_look_target::create(1.0, 3)),
                            1,
                        ),
                        (Box::new(DoNothing::new(30, 60)), 1),
                    ],
                )),
            ],
        ),
        ActivityData::with_priority_start_and_conditions(
            Activity::Panic,
            0,
            vec![Box::new(Timed::new(ArmadilloBallUp::default()))],
            vec![
                (
                    types::DANGER_DETECTED_RECENTLY.id(),
                    MemoryStatus::ValuePresent,
                ),
                (types::IS_PANICKING.id(), MemoryStatus::ValueAbsent),
            ],
        ),
    ]
}

/// Vanilla `ArmadilloAi.ArmadilloBallUp`: curled up while danger is near, peeking now and
/// then, unrolling once it is gone.
#[derive(Default)]
struct ArmadilloBallUp {
    next_peek_timer: i32,
    danger_was_around: bool,
}

impl ArmadilloBallUp {
    /// Vanilla `BALL_UP_STAY_IN_STATE`: five minutes.
    const STAY_IN_STATE: i32 = 5 * 60 * 20;

    fn pick_next_peek_timer(mob: &dyn Mob) -> i32 {
        ArmadilloState::Scared.animation_duration() as i32
            + mob.get_random().random_range(100..=400)
    }
}

impl Behavior for ArmadilloBallUp {
    fn entry_conditions(&self) -> &[(MemoryModuleId, MemoryStatus)] {
        &[]
    }

    fn min_duration(&self) -> i32 {
        Self::STAY_IN_STATE
    }

    fn max_duration(&self) -> i32 {
        Self::STAY_IN_STATE
    }

    fn check_extra_start_conditions(&mut self, tick: &mut BrainTick<'_>) -> bool {
        let entity = tick.mob.get_entity();
        entity.on_ground.load(Ordering::Relaxed)
            && !entity.is_in_water()
            && !entity.touching_lava.load(Ordering::Relaxed)
    }

    fn can_still_use(&mut self, tick: &mut BrainTick<'_>) -> bool {
        as_armadillo(tick.mob).is_some_and(|armadillo| armadillo.get_state().is_threatened())
    }

    fn start(&mut self, tick: &mut BrainTick<'_>) {
        if let Some(armadillo) = as_armadillo(tick.mob) {
            armadillo.roll_up();
        }
    }

    fn stop(&mut self, tick: &mut BrainTick<'_>) {
        if let Some(armadillo) = as_armadillo(tick.mob)
            && !armadillo.can_stay_rolled_up()
        {
            armadillo.roll_out();
        }
    }

    fn tick(&mut self, tick: &mut BrainTick<'_>) {
        let Some(armadillo) = as_armadillo(tick.mob) else {
            return;
        };
        if self.next_peek_timer > 0 {
            self.next_peek_timer -= 1;
        }
        let entity = armadillo.get_entity();
        if armadillo.should_switch_to_scared_state() {
            armadillo.switch_to_state(ArmadilloState::Scared);
            if entity.on_ground.load(Ordering::Relaxed) {
                armadillo.play_entity_sound(Sound::EntityArmadilloLand, 1.0, 1.0);
            }
            return;
        }
        let danger_ticks = tick
            .state
            .time_until_expiry(types::DANGER_DETECTED_RECENTLY.id())
            .unwrap_or(0);
        let danger_is_around = danger_ticks > 75;
        if danger_is_around != self.danger_was_around {
            self.next_peek_timer = Self::pick_next_peek_timer(tick.mob);
        }
        self.danger_was_around = danger_is_around;
        let unrolling = ArmadilloState::Unrolling.animation_duration() as i64;
        match armadillo.get_state() {
            ArmadilloState::Scared => {
                if self.next_peek_timer == 0
                    && entity.on_ground.load(Ordering::Relaxed)
                    && danger_is_around
                {
                    entity.world.load().send_entity_status(
                        entity,
                        EntityStatus::ArmadilloPeek,
                        ActorEventID::None,
                    );
                    self.next_peek_timer = Self::pick_next_peek_timer(tick.mob);
                }
                if danger_ticks < unrolling {
                    armadillo.play_entity_sound(Sound::EntityArmadilloUnrollStart, 1.0, 1.0);
                    armadillo.switch_to_state(ArmadilloState::Unrolling);
                }
            }
            ArmadilloState::Unrolling if danger_ticks > unrolling => {
                armadillo.switch_to_state(ArmadilloState::Scared);
            }
            _ => {}
        }
    }

    fn debug_name(&self) -> &'static str {
        "ArmadilloBallUp"
    }
}
