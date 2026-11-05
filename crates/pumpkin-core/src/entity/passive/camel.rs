use std::sync::{
    Arc,
    atomic::{AtomicI32, AtomicI64, Ordering},
};

use crate::entity::mob::spawn::SpawnGroupData;
use crate::world::World;
use pumpkin_data::BlockState;
use pumpkin_data::damage::DamageType;
use pumpkin_data::entity::{EntityPose, EntityType};
use pumpkin_data::environment_attribute::Activity;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::Sound;
use pumpkin_data::tag::{self, Taggable};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_protocol::codec::var_long::VarLong;
use pumpkin_util::math::position::BlockPos;
use rand::RngExt;

use crate::entity::ai::brain::{
    Brain, BrainProvider, BrainTick,
    activity::ActivityData,
    behavior::{
        AnimalMakeLove, AnimalPanic, CountDownCooldownTicks, DoNothing, FollowTemptation,
        LookAtTargetSink, MoveToTargetSink, RandomLookAround, Swim, Timed, baby_follow_adult,
        random_stroll, run_one, run_one_with_conditions, set_entity_look_target_sometimes,
        set_walk_target_from_look_target, timed::Behavior,
    },
    memory::{MemoryModuleId, MemoryStatus, PackedMemories, types},
    sensing::SensorType,
};
use crate::entity::custom_sound::CustomSound;
use crate::entity::passive::abstract_horse::{self as horse_base, AbstractHorse, HorseData};
use crate::entity::passive::horse_inventory;
use crate::entity::{
    Entity, EntityBase,
    ageable::{AgeableData, AgeableMob},
    mob::{Mob, MobEntity, saddle},
    passive::animal::Animal,
    player::Player,
};

/// Vanilla `Camel` pose times: sitting down takes 40 ticks, standing up 52.
const SIT_DOWN_TICKS: i64 = 40;
const STAND_UP_TICKS: i64 = 52;
/// Vanilla `Camel.DASH_COOLDOWN_TICKS`.
const DASH_COOLDOWN: i32 = 55;
/// Vanilla `Camel.MAX_PASSENGERS`.
const MAX_PASSENGERS: usize = 2;

pub struct CamelEntity {
    pub mob_entity: MobEntity,
    pub ageable_data: AgeableData,
    horse: HorseData,
    /// Vanilla `LAST_POSE_CHANGE_TICK`: the game time of the last change, negative while
    /// sitting.
    last_pose_tick: AtomicI64,
    dash_cooldown: AtomicI32,
    dashing: std::sync::atomic::AtomicBool,
}

/// Vanilla `isMobControlled`: a mob rides in front.
fn is_mob_controlled(mob: &dyn Mob) -> bool {
    mob.get_entity()
        .passengers
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .first()
        .is_some_and(|rider| rider.get_mob().is_some())
}

fn as_camel(mob: &dyn Mob) -> Option<&CamelEntity> {
    mob.cast_any().downcast_ref::<CamelEntity>()
}

impl CamelEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let camel = Arc::new(Self {
            mob_entity: MobEntity::new(entity),
            ageable_data: AgeableData::default(),
            horse: HorseData::default(),
            last_pose_tick: AtomicI64::new(0),
            dash_cooldown: AtomicI32::new(0),
            dashing: std::sync::atomic::AtomicBool::new(false),
        });
        // Vanilla `Camel.isTamed` is always true.
        horse_base::set_flag(camel.as_ref(), horse_base::FLAG_TAME, true);
        camel
    }

    /// Vanilla `CamelHusk`, which shares the camel's body.
    fn is_husk(&self) -> bool {
        self.get_entity().entity_type == &EntityType::CAMEL_HUSK
    }

    fn sound(&self, camel: Sound, husk: Sound) -> Sound {
        if self.is_husk() { husk } else { camel }
    }

    fn game_time(&self) -> i64 {
        self.get_entity().world.load().get_world_age()
    }

    fn set_last_pose_tick(&self, tick: i64) {
        self.last_pose_tick.store(tick, Ordering::Relaxed);
        self.get_entity().set_synced_data(
            pumpkin_data::tracked_data::camel::LAST_POSE_CHANGE_TICK,
            VarLong(tick),
        );
    }

    /// Vanilla `isCamelSitting`.
    #[must_use]
    pub fn is_camel_sitting(&self) -> bool {
        self.last_pose_tick.load(Ordering::Relaxed) < 0
    }

    /// Vanilla `getPoseTime`.
    fn pose_time(&self) -> i64 {
        self.game_time() - self.last_pose_tick.load(Ordering::Relaxed).abs()
    }

    /// Vanilla `isInPoseTransition`.
    fn is_in_pose_transition(&self) -> bool {
        let limit = if self.is_camel_sitting() {
            SIT_DOWN_TICKS
        } else {
            STAND_UP_TICKS
        };
        self.pose_time() < limit
    }

    /// Vanilla `refuseToMove`: sitting, or getting up or down.
    #[must_use]
    pub fn refuse_to_move(&self) -> bool {
        self.is_camel_sitting() || self.is_in_pose_transition()
    }

    /// Vanilla `sitDown`.
    fn sit_down(&self) {
        if self.is_camel_sitting() {
            return;
        }
        self.play_entity_sound(
            self.sound(Sound::EntityCamelSit, Sound::EntityCamelHuskSit),
            1.0,
            self.voice_pitch(),
        );
        self.get_entity().set_pose(EntityPose::Sitting);
        self.set_last_pose_tick(-self.game_time());
    }

    /// Vanilla `standUp`.
    fn stand_up(&self) {
        if !self.is_camel_sitting() {
            return;
        }
        self.play_entity_sound(
            self.sound(Sound::EntityCamelStand, Sound::EntityCamelHuskStand),
            1.0,
            self.voice_pitch(),
        );
        self.get_entity().set_pose(EntityPose::Standing);
        self.set_last_pose_tick(self.game_time());
    }

    /// Vanilla `standUpInstantly`: standing as if the transition were over.
    fn stand_up_instantly(&self) {
        self.get_entity().set_pose(EntityPose::Standing);
        self.set_last_pose_tick((self.game_time() - STAND_UP_TICKS - 1).max(0));
    }

    fn voice_pitch(&self) -> f32 {
        self.mob_entity.living_entity.get_pitch()
    }

    fn set_dashing(&self, dashing: bool) {
        self.dashing.store(dashing, Ordering::Relaxed);
        self.get_entity()
            .set_synced_data(pumpkin_data::tracked_data::camel::DASH, dashing);
        // Vanilla `onSyncedDataUpdated(DASH)`.
        if self.dash_cooldown.load(Ordering::Relaxed) == 0 {
            self.dash_cooldown.store(DASH_COOLDOWN, Ordering::Relaxed);
        }
    }

    /// Vanilla `Camel.tick`, the server part: the dash ends and recharges, and water gets a
    /// sitting camel up.
    fn tick_camel(&self) {
        let entity = self.get_entity();
        let cooldown = self.dash_cooldown.load(Ordering::Relaxed);
        if self.dashing.load(Ordering::Relaxed)
            && cooldown < 50
            && (entity.on_ground.load(Ordering::Relaxed)
                || entity.is_in_water()
                || entity.touching_lava.load(Ordering::Relaxed)
                || entity.has_vehicle())
        {
            self.set_dashing(false);
        }
        if cooldown > 0 {
            self.dash_cooldown.store(cooldown - 1, Ordering::Relaxed);
            if cooldown == 1 {
                self.play_entity_sound(
                    self.sound(Sound::EntityCamelDashReady, Sound::EntityCamelHuskDashReady),
                    1.0,
                    1.0,
                );
            }
        }
        if self.is_camel_sitting() && entity.is_in_water() {
            self.stand_up_instantly();
        }
    }

    /// Vanilla `Camel.handleEating`: food heals 2, makes a tame adult fall in love and ages a
    /// calf by 10 seconds.
    fn feed(&self, player: &Player, stack: &mut ItemStack) -> bool {
        if !self.is_food(stack) {
            return false;
        }
        let mob_entity = &self.mob_entity;
        let living = &mob_entity.living_entity;
        let heal = living.health.load() < living.get_max_health();
        if heal {
            living.heal(2.0);
        }
        let love = !self.is_husk()
            && self.get_entity().age.load(Ordering::Relaxed) == 0
            && mob_entity.is_breeding_ready()
            && !mob_entity.is_in_love();
        if love {
            mob_entity.set_love_ticks(600, Some(player.gameprofile.id));
        }
        let age_up = self.is_baby() && !self.is_age_locked();
        if age_up {
            self.age_up(10, false);
        }
        if !heal && !love && !age_up {
            return false;
        }
        let pitch = {
            let mut rng = self.get_random();
            (rng.random::<f32>() - rng.random::<f32>()).mul_add(0.2, 1.0)
        };
        self.play_entity_sound(
            self.sound(Sound::EntityCamelEat, Sound::EntityCamelHuskEat),
            1.0,
            pitch,
        );
        stack.decrement_unless_creative(player.gamemode.load(), 1);
        true
    }
}

impl AgeableMob for CamelEntity {
    fn get_ageable_data(&self) -> &AgeableData {
        &self.ageable_data
    }

    /// Vanilla `CamelHusk.canBeABaby`.
    fn can_be_a_baby(&self) -> bool {
        !self.is_husk()
    }
}

impl Animal for CamelEntity {
    /// Vanilla `Camel.isFood` and `CamelHusk.isFood`.
    fn is_food(&self, item_stack: &ItemStack) -> bool {
        item_stack.item.has_tag(if self.is_husk() {
            &tag::Item::MINECRAFT_CAMEL_HUSK_FOOD
        } else {
            &tag::Item::MINECRAFT_CAMEL_FOOD
        })
    }

    /// Vanilla `Camel.canMate`; a camel husk does not.
    fn can_mate(&self, partner: &dyn EntityBase) -> bool {
        !self.is_husk()
            && partner.get_entity().entity_type == &EntityType::CAMEL
            && partner
                .get_mob()
                .and_then(|mob| mob.as_abstract_horse())
                .is_some_and(horse_base::can_parent)
            && horse_base::can_parent(self)
    }
}

impl AbstractHorse for CamelEntity {
    fn horse_data(&self) -> &HorseData {
        &self.horse
    }

    fn horse_eating_sound(&self) -> Option<Sound> {
        Some(self.sound(Sound::EntityCamelEat, Sound::EntityCamelHuskEat))
    }

    fn can_perform_rearing(&self) -> bool {
        false
    }

    /// Vanilla `Camel.openCustomInventoryScreen` asks nothing.
    fn may_open_inventory(&self, _player: &Player) -> bool {
        true
    }
}

impl CustomSound for CamelEntity {
    fn hurt_sound(&self) -> Option<Sound> {
        Some(self.sound(Sound::EntityCamelHurt, Sound::EntityCamelHuskHurt))
    }

    fn death_sound(&self) -> Option<Sound> {
        Some(self.sound(Sound::EntityCamelDeath, Sound::EntityCamelHuskDeath))
    }
}

impl Mob for CamelEntity {
    /// Vanilla `CamelHusk.chargeSpeedModifier`.
    fn charge_speed_modifier(&self) -> f32 {
        if self.is_husk() { 4.0f32 } else { 1.0f32 }
    }

    /// Vanilla `AbstractHorse.isPushable`.
    fn mob_is_pushable(&self) -> bool {
        !self.get_entity().has_passengers()
    }

    /// Vanilla `supportQuadLeash`.
    fn mob_support_quad_leash(&self) -> bool {
        true
    }

    /// Vanilla `Camel.finalizeSpawn`: the pose timer reads as a finished stand already
    /// (`resetLastPoseChangeTickToFullStand`); `CamelAi.initMemories` sets nothing in 26.3.
    fn finalize_spawn(
        &self,
        _world: &Arc<World>,
        group_data: Option<SpawnGroupData>,
    ) -> Option<SpawnGroupData> {
        self.set_last_pose_tick((self.game_time() - STAND_UP_TICKS - 1).max(0));
        self.finalize_spawn_super(group_data)
    }

    fn get_ambient_sound(&self) -> Option<Sound> {
        Some(self.sound(Sound::EntityCamelAmbient, Sound::EntityCamelHuskAmbient))
    }

    fn as_custom_sound(&self) -> Option<&dyn CustomSound> {
        Some(self)
    }

    fn is_tamed(&self) -> bool {
        true
    }

    fn mob_play_step_sound(&self, _pos: &BlockPos, state: &BlockState) {
        let sound = if state
            .id
            .to_block()
            .has_tag(&tag::Block::MINECRAFT_CAMEL_SAND_STEP_SOUND_BLOCKS)
        {
            self.sound(Sound::EntityCamelStepSand, Sound::EntityCamelHuskStepSand)
        } else {
            self.sound(Sound::EntityCamelStep, Sound::EntityCamelHuskStep)
        };
        // Vanilla `CamelHusk.playStepSound` is quieter.
        self.play_entity_sound(sound, if self.is_husk() { 0.4 } else { 1.0 }, 1.0);
    }

    /// Vanilla `CamelHusk.removeWhenFarAway`.
    fn remove_when_far_away(&self, _distance_sq: f64) -> bool {
        self.is_husk()
    }

    /// Vanilla `CamelHusk.canBeLeashed`.
    fn can_be_leashed(&self) -> bool {
        !self.is_husk() || !is_mob_controlled(self)
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

    fn make_brain(&self, packed: &PackedMemories) -> Brain {
        make_brain(self, packed)
    }

    /// Vanilla `Camel.customServerAiStep` with `CamelAi.updateActivity`.
    fn custom_server_ai_step(&self, _caller: &dyn EntityBase) {
        self.mob_entity.tick_brain(self);
        self.mob_entity.with_brain(self, |brain| {
            brain.set_active_activity_to_first_valid(&[Activity::Idle]);
        });
    }

    fn mob_tick(&self, _caller: &dyn EntityBase) {
        self.tick_camel();
        horse_base::horse_tick(self);
    }

    /// Vanilla `Camel.actuallyHurt`: a hit gets it up at once.
    fn on_damage(&self, _damage_type: DamageType, _source: Option<&dyn EntityBase>) {
        self.stand_up_instantly();
    }

    /// Vanilla `Camel.handleStartJump`: the rider's dash.
    fn mob_start_rider_jump(&self, _strength: i32) {
        if !self.is_saddled()
            || self.refuse_to_move()
            || self.dash_cooldown.load(Ordering::Relaxed) > 0
        {
            return;
        }
        self.play_entity_sound(
            self.sound(Sound::EntityCamelDash, Sound::EntityCamelHuskDash),
            1.0,
            self.voice_pitch(),
        );
        self.set_dashing(true);
    }

    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        self.write_ageable_nbt(nbt);
        horse_base::write_horse_nbt(self, nbt);
        nbt.put_long("LastPoseTick", self.last_pose_tick.load(Ordering::Relaxed));
    }

    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        self.read_ageable_nbt(nbt);
        horse_base::read_horse_nbt(self, nbt);
        horse_base::set_flag(self, horse_base::FLAG_TAME, true);
        let pose_tick = nbt.get_long("LastPoseTick").unwrap_or(0);
        if pose_tick < 0 {
            self.get_entity().set_pose(EntityPose::Sitting);
        }
        self.set_last_pose_tick(pose_tick);
        saddle::read_legacy_saddle(self, nbt);
    }

    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    /// Vanilla `AbstractHorse.canUseSlot(SADDLE)`; camels count as tamed.
    fn can_be_saddled(&self) -> bool {
        self.get_entity().is_alive() && !self.is_baby()
    }

    fn saddle_sound(&self) -> Sound {
        self.sound(Sound::EntityCamelSaddle, Sound::EntityCamelHuskSaddle)
    }

    /// Vanilla `AbstractHorse.getControllingPassenger`: a player in a saddle.
    fn mob_controlling_player(&self) -> Option<Arc<Player>> {
        horse_base::saddled_rider(self)
    }

    /// Vanilla `Camel.tickRidden`: it turns with its rider unless it sits, and gets up once
    /// the rider steers it.
    fn mob_tick_ridden(&self, controller: &Player) {
        if self.is_camel_sitting() && !self.is_in_pose_transition() {
            self.stand_up();
        }
        if !self.refuse_to_move() {
            saddle::face_rider(self, controller, 0.5);
        }
    }

    fn mob_init_data_tracker(&self) {
        let entity = self.get_entity();
        if entity.age.load(Ordering::Relaxed) < 0 {
            entity.set_synced_data(pumpkin_data::tracked_data::camel::DATA_BABY_ID, true);
        }
        entity.set_synced_data(
            pumpkin_data::tracked_data::camel::DATA_ID_FLAGS,
            horse_base::flags(self) as i8,
        );
        entity.set_synced_data(
            pumpkin_data::tracked_data::camel::DASH,
            self.dashing.load(Ordering::Relaxed),
        );
        entity.set_synced_data(
            pumpkin_data::tracked_data::camel::LAST_POSE_CHANGE_TICK,
            VarLong(self.last_pose_tick.load(Ordering::Relaxed)),
        );
    }

    /// Vanilla `Camel.mobInteract`: the inventory, saddle, food, then up to two riders.
    fn mob_interact(&self, player: &Arc<Player>, item_stack: &mut ItemStack) -> bool {
        if self.is_husk() {
            // Vanilla `CamelHusk.interact`: every click keeps it loaded.
            self.mob_entity
                .persistence_required
                .store(true, Ordering::Relaxed);
        }
        if player.get_entity().is_sneaking() && !self.is_baby() {
            horse_inventory::open_custom_inventory_screen(self, player);
            return true;
        }
        if saddle::try_equip_saddle(self, player, item_stack) || self.feed(player, item_stack) {
            return true;
        }
        let entity = self.get_entity();
        let riders = entity
            .passengers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len();
        if riders < MAX_PASSENGERS && !self.is_baby() {
            let world = entity.world.load();
            if let Some(vehicle) = world.get_entity_by_id(entity.entity_id)
                && let Some(passenger) = world.get_player_by_id(player.entity_id())
            {
                passenger
                    .get_entity()
                    .set_rotation(entity.yaw.load(), entity.pitch.load());
                entity.add_passenger(vehicle, passenger as Arc<dyn EntityBase>);
            }
            return true;
        }
        false
    }
}

const MEMORIES: [MemoryModuleId; 0] = [];

/// Vanilla `CamelAi`: panic, temptation, love, following adults, strolling and sitting.
fn make_brain(camel: &CamelEntity, packed: &PackedMemories) -> Brain {
    BrainProvider {
        memory_types: &MEMORIES,
        sensor_types: &[
            SensorType::NearestLivingEntities,
            SensorType::HurtBy,
            SensorType::FoodTemptations,
            SensorType::NearestAdult,
        ],
        activities,
    }
    .make_brain(camel, packed)
}

/// Vanilla `CamelPanic.start`: a panicking camel is up at once.
fn stand_up_to_flee(mob: &dyn Mob) {
    if let Some(camel) = as_camel(mob) {
        camel.stand_up_instantly();
    }
}

fn refuses_to_move(mob: &dyn Mob) -> bool {
    as_camel(mob).is_some_and(CamelEntity::refuse_to_move)
}

fn moves(mob: &dyn Mob) -> bool {
    !refuses_to_move(mob)
}

fn activities(_mob: &dyn Mob) -> Vec<ActivityData> {
    vec![
        ActivityData::with_priority_start(
            Activity::Core,
            0,
            vec![
                Box::new(Timed::new(Swim::new(0.8))),
                Box::new(Timed::new(
                    AnimalPanic::new(4.0)
                        .on_start(stand_up_to_flee)
                        .unless(is_mob_controlled),
                )),
                Box::new(Timed::new(LookAtTargetSink::new(45, 90))),
                Box::new(Timed::new(MoveToTargetSink::default())),
                Box::new(CountDownCooldownTicks::new(
                    types::TEMPTATION_COOLDOWN_TICKS,
                )),
                Box::new(CountDownCooldownTicks::new(types::GAZE_COOLDOWN_TICKS)),
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
                            |_| 2.5,
                            |mob| {
                                if mob.get_entity().age.load(Ordering::Relaxed) < 0 {
                                    2.5
                                } else {
                                    3.5
                                }
                            },
                        ))),
                        1,
                    ),
                    (
                        Box::new(baby_follow_adult(5, 16, |_| 2.5).only_if(moves)),
                        1,
                    ),
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
                        (Box::new(random_stroll::stroll(2.0).only_if(moves)), 1),
                        (
                            Box::new(
                                set_walk_target_from_look_target::create(2.0, 3).only_if(moves),
                            ),
                            1,
                        ),
                        (Box::new(Timed::new(RandomSitting)), 1),
                        (Box::new(DoNothing::new(30, 60)), 1),
                    ],
                )),
            ],
        ),
    ]
}

/// Vanilla `CamelAi.RandomSitting`: after 20 seconds in a pose on dry ground, a camel sits
/// down or stands up.
struct RandomSitting;

impl Behavior for RandomSitting {
    fn entry_conditions(&self) -> &[(MemoryModuleId, MemoryStatus)] {
        &[]
    }

    fn check_extra_start_conditions(&mut self, tick: &mut BrainTick<'_>) -> bool {
        let Some(camel) = as_camel(tick.mob) else {
            return false;
        };
        let entity = camel.get_entity();
        !entity.is_in_water()
            && camel.pose_time() >= 20 * 20
            && !entity.is_leashed()
            && entity.on_ground.load(Ordering::Relaxed)
            && camel.mob_controlling_player().is_none()
    }

    fn start(&mut self, tick: &mut BrainTick<'_>) {
        let Some(camel) = as_camel(tick.mob) else {
            return;
        };
        if camel.is_camel_sitting() {
            camel.stand_up();
        } else if !tick.mob.is_panicking() {
            camel.sit_down();
        }
    }

    fn debug_name(&self) -> &'static str {
        "RandomSitting"
    }
}
