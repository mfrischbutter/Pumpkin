use std::sync::{
    Arc,
    atomic::{AtomicI32, AtomicI64, Ordering},
};

use crate::entity::mob::spawn::SpawnGroupData;
use crate::world::World;
use crate::world::game_event::GameEventContext;
use pumpkin_data::BlockState;
use pumpkin_data::attributes::Attributes;
use pumpkin_data::damage::DamageType;
use pumpkin_data::entity::{EntityPose, EntityType};
use pumpkin_data::environment_attribute::Activity;
use pumpkin_data::game_event::GameEvent;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::Sound;
use pumpkin_data::tag::{self, Taggable};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_protocol::codec::var_long::VarLong;
use pumpkin_util::math::boundingbox::{BoundingBox, EntityDimensions};
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_util::math::wrap_degrees;
use rand::RngExt;
use uuid::Uuid;

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
/// Vanilla `Camel.MAX_HEAD_Y_ROT`.
const MAX_HEAD_Y_ROT: f32 = 30.0;
/// Vanilla `Camel.BABY_SCALE`, its `getAgeScale` for a calf.
const BABY_SCALE: f32 = 0.6;
/// Vanilla `Camel.SITTING_HEIGHT_DIFFERENCE`.
const SITTING_HEIGHT_DIFFERENCE: f32 = 1.43;
/// Vanilla `Camel.getPassengerAttachmentPoint`: the driver sits forward, a second rider back,
/// an animal a bit further forward than a player.
const DRIVER_SEAT_OFFSET: f32 = 0.5;
const BACK_SEAT_OFFSET: f32 = -0.7;
const ANIMAL_SEAT_SHIFT: f32 = 0.2;
const REMOVED_SEAT_HEIGHT: f64 = 0.01;
/// Vanilla `canCamelChangePose`'s `deflate(1.0E-7)`.
const POSE_CHECK_DEFLATE: f64 = 1.0E-7;

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

/// Vanilla `isMobControlled`, for `CamelPanic` and `CamelHusk.canBeLeashed`.
fn is_mob_controlled(mob: &dyn Mob) -> bool {
    mob.as_abstract_horse()
        .is_some_and(AbstractHorse::is_mob_controlled)
}

fn as_camel(mob: &dyn Mob) -> Option<&CamelEntity> {
    mob.cast_any().downcast_ref::<CamelEntity>()
}

impl CamelEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let mut mob_entity = MobEntity::new(entity);
        let navigator = mob_entity
            .navigator
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        navigator.set_can_float(true);
        navigator.set_can_walk_over_fences(true);
        let camel = Arc::new(Self {
            mob_entity,
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
            horse_base::SOUND_VOLUME,
            self.voice_pitch(),
        );
        self.change_pose(true);
        self.set_last_pose_tick(-self.game_time());
    }

    /// Vanilla `standUp`.
    fn stand_up(&self) {
        if !self.is_camel_sitting() {
            return;
        }
        self.play_entity_sound(
            self.sound(Sound::EntityCamelStand, Sound::EntityCamelHuskStand),
            horse_base::SOUND_VOLUME,
            self.voice_pitch(),
        );
        self.change_pose(false);
        self.set_last_pose_tick(self.game_time());
    }

    /// Vanilla `standUpInstantly`: standing as if the transition were over.
    fn stand_up_instantly(&self) {
        self.change_pose(false);
        self.set_last_pose_tick((self.game_time() - STAND_UP_TICKS - 1).max(0));
    }

    /// Vanilla `setPose` with `gameEvent(ENTITY_ACTION)`, the size following the pose.
    fn change_pose(&self, sitting: bool) {
        let entity = self.get_entity();
        entity.set_default_dimensions(self.dimensions_for(sitting));
        entity.set_pose(if sitting {
            EntityPose::Sitting
        } else {
            EntityPose::Standing
        });
        self.emit_own_game_event(GameEvent::EntityAction);
    }

    /// Vanilla `gameEvent(event)`: at the camel, with it as the source.
    fn emit_own_game_event(&self, event: GameEvent) {
        let entity = self.get_entity();
        entity.world.load().emit_game_event_with_context(
            event.name(),
            entity.pos.load(),
            &GameEventContext {
                source_entity: Some(self),
            },
        );
    }

    /// Vanilla `Camel.getDefaultDimensions`: lower while sitting, smaller as a calf.
    fn dimensions_for(&self, sitting: bool) -> EntityDimensions {
        let entity_type = self.get_entity().entity_type;
        let baby = self.is_baby();
        sitting
            .then(|| entity_type.pose_dimensions(EntityPose::Sitting, baby))
            .flatten()
            .map_or_else(
                || Entity::age_dimensions(entity_type, baby),
                |pose| pose.dimensions,
            )
    }

    /// Vanilla `canCamelChangePose`: room for the other pose's size.
    fn can_camel_change_pose(&self) -> bool {
        let entity = self.get_entity();
        let dimensions = self.dimensions_for(!self.is_camel_sitting());
        let pos = entity.pos.load();
        let target = BoundingBox::new_from_pos(pos.x, pos.y, pos.z, &dimensions);
        entity
            .world
            .load()
            .is_space_empty(target.contract_all(POSE_CHECK_DEFLATE))
    }

    /// Vanilla `Mob.clampHeadRotationToBody`: the head stays within 30 degrees of the body.
    fn clamp_head_rotation_to_body(&self) {
        let entity = self.get_entity();
        let head = entity.head_yaw.load();
        let delta = wrap_degrees(entity.body_yaw.load() - head);
        let clamped = delta.clamp(-MAX_HEAD_Y_ROT, MAX_HEAD_Y_ROT);
        entity.head_yaw.store(head + delta - clamped);
    }

    /// Vanilla `Camel.getAgeScale`.
    fn age_scale(&self) -> f32 {
        if self.is_baby() { BABY_SCALE } else { 1.0 }
    }

    /// Vanilla `getBodyAnchorAnimationYOffset`: the saddle's height above the feet, following
    /// the sit and stand animations.
    fn body_anchor_y_offset(&self, front: bool, height: f32, scale: f32) -> f64 {
        let age_sit_y_offset = if self.is_baby() { 0.093_75 } else { 0.375 };
        let mut offset = f64::from(height) - age_sit_y_offset;
        let sitting_height_difference = scale * SITTING_HEIGHT_DIFFERENCE;
        let vertical_drop = sitting_height_difference - scale * 0.2;
        let bottom_point = sitting_height_difference - vertical_drop;
        let sitting = self.is_camel_sitting();
        let in_transition = self.is_in_pose_transition();
        if in_transition {
            let (duration, half_point, flex_point_offset) = match (sitting, front) {
                (true, true) => (SIT_DOWN_TICKS, 28, 0.5),
                (true, false) => (SIT_DOWN_TICKS, 28, 0.1),
                (false, true) => (STAND_UP_TICKS, 24, 0.6),
                (false, false) => (STAND_UP_TICKS, 32, 0.35),
            };
            let pose_time = (self.pose_time() as f32).clamp(0.0, duration as f32);
            let half_point = half_point as f32;
            let first_part = pose_time < half_point;
            let part = if first_part {
                pose_time / half_point
            } else {
                (pose_time - half_point) / (duration as f32 - half_point)
            };
            let flex_point = sitting_height_difference - flex_point_offset * vertical_drop;
            let (from, to) = match (sitting, first_part) {
                (true, true) => (sitting_height_difference, flex_point),
                (true, false) => (flex_point, bottom_point),
                (false, true) => (
                    bottom_point - sitting_height_difference,
                    bottom_point - flex_point,
                ),
                (false, false) => (bottom_point - flex_point, 0.0),
            };
            offset += f64::from(part.mul_add(to - from, from));
        }
        if sitting && !in_transition {
            offset += f64::from(bottom_point);
        }
        offset
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

    /// Vanilla `Camel.tick`, the server part: the dash ends and recharges, a camel that does
    /// not move keeps its head to its body, and water gets a sitting camel up.
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
        if self.refuse_to_move() {
            self.clamp_head_rotation_to_body();
        }
        if self.is_camel_sitting() && entity.is_in_water() {
            self.stand_up_instantly();
        }
    }

    /// Vanilla `Camel.handleEating`: food heals 2, makes a tame adult fall in love and ages a
    /// calf by 10 seconds.
    fn feed(&self, player: &Arc<Player>, stack: &mut ItemStack) -> bool {
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
            self.set_in_love(player);
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
        self.emit_own_game_event(GameEvent::Eat);
        stack.decrement_unless_creative(player.gamemode.load(), 1);
        true
    }
}

impl AgeableMob for CamelEntity {
    fn get_ageable_data(&self) -> &AgeableData {
        &self.ageable_data
    }

    /// Vanilla `AbstractHorse.finalizeSpawn`'s group data when there is none.
    fn default_baby_spawn_chance(&self) -> f32 {
        crate::entity::ageable::ABSTRACT_HORSE_BABY_SPAWN_CHANCE
    }

    /// Vanilla `CamelHusk.canBeABaby`.
    fn can_be_a_baby(&self) -> bool {
        !self.is_husk()
    }

    fn age_boundary_reached(&self) {
        self.get_entity()
            .set_default_dimensions(self.dimensions_for(self.is_camel_sitting()));
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

    /// Vanilla `CamelHusk.isMobControlled`: a mob rides in front; a camel never counts.
    fn is_mob_controlled(&self) -> bool {
        self.is_husk()
            && self
                .get_entity()
                .passengers
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .first()
                .is_some_and(|rider| rider.get_mob().is_some())
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

    fn sound_volume(&self) -> f32 {
        horse_base::SOUND_VOLUME
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

    fn get_ambient_sound_interval(&self) -> i32 {
        horse_base::AMBIENT_SOUND_INTERVAL
    }

    fn as_custom_sound(&self) -> Option<&dyn CustomSound> {
        Some(self)
    }

    fn is_tamed(&self) -> bool {
        true
    }

    /// Vanilla `AbstractHorse.getOwnerReference`.
    fn get_owner_uuid(&self) -> Option<Uuid> {
        horse_base::owner(self)
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
        !is_mob_controlled(self)
    }

    /// Vanilla `Camel.travel`: no walking while it sits or gets up or down on the ground.
    fn travel(&self, _caller: &dyn EntityBase) -> bool {
        let entity = self.get_entity();
        if self.refuse_to_move() && entity.on_ground.load(Ordering::Relaxed) {
            let velocity = entity.velocity.load();
            entity.velocity.store(Vector3::new(0.0, velocity.y, 0.0));
            let living = &self.mob_entity.living_entity;
            let input = living.movement_input.load();
            living.movement_input.store(Vector3::new(0.0, input.y, 0.0));
        }
        false
    }

    /// Vanilla `Camel.getMaxHeadYRot`.
    fn get_max_head_rotation(&self) -> f32 {
        MAX_HEAD_Y_ROT
    }

    /// Vanilla `CamelLookControl`: a rider turns it, not its own look.
    fn ticks_look_control(&self) -> bool {
        self.get_controlling_passenger().is_none()
    }

    /// Vanilla `CamelBodyRotationControl`: the body keeps still while the camel will not move.
    fn ticks_body_rotation(&self) -> bool {
        !self.refuse_to_move()
    }

    /// Vanilla `Camel.getPassengerAttachmentPoint`: two seats on the hump, rising and sinking
    /// with the sit and stand animations.
    fn mob_passenger_attachment_point(
        &self,
        passenger: &dyn EntityBase,
        index: usize,
    ) -> Option<Vector3<f64>> {
        let entity = self.get_entity();
        let driver = index == 0;
        let scale_attribute = self
            .mob_entity
            .living_entity
            .get_attribute_value(&Attributes::SCALE) as f32;
        let scale = scale_attribute * self.age_scale();
        let height = self.dimensions_for(self.is_camel_sitting()).height * scale_attribute;
        let seat_height = if entity.is_removed() {
            REMOVED_SEAT_HEIGHT
        } else {
            self.body_anchor_y_offset(driver, height, scale)
        };
        let mut offset = DRIVER_SEAT_OFFSET;
        let riders = entity
            .passengers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len();
        if riders > 1 {
            if !driver {
                offset = BACK_SEAT_OFFSET;
            }
            if passenger
                .get_mob()
                .and_then(|mob| mob.as_animal())
                .is_some()
            {
                offset += ANIMAL_SEAT_SHIFT;
            }
        }
        let yaw = entity.yaw.load().to_radians();
        let forward = f64::from(offset * scale);
        Some(Vector3::new(
            -forward * f64::from(yaw.sin()),
            seat_height,
            forward * f64::from(yaw.cos()),
        ))
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

    /// Vanilla `Camel.customServerAiStep` with `CamelAi.updateActivity`, then
    /// `CamelMoveControl`: a sitting camel told to walk gets up first.
    fn custom_server_ai_step(&self, _caller: &dyn EntityBase) {
        self.mob_entity.tick_brain(self);
        self.mob_entity.with_brain(self, |brain| {
            brain.set_active_activity_to_first_valid(&[Activity::Idle]);
        });
        let wants_to_move = self
            .mob_entity
            .move_control
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .has_wanted();
        if wants_to_move
            && !self.get_entity().is_leashed()
            && self.is_camel_sitting()
            && !self.is_in_pose_transition()
            && self.can_camel_change_pose()
        {
            self.stand_up();
        }
    }

    fn mob_tick(&self, _caller: &dyn EntityBase) {
        self.tick_camel();
        horse_base::horse_tick(self);
    }

    /// Vanilla `Camel.actuallyHurt`: a hit gets it up at once.
    fn on_damage(&self, _damage_type: DamageType, _source: Option<&dyn EntityBase>) {
        self.stand_up_instantly();
    }

    /// Vanilla `Camel.onElasticLeashPull`: a sitting camel gets up when pulled.
    fn on_elastic_leash_pull(&self) {
        horse_base::on_leash_pull(self);
        if self.is_camel_sitting() && !self.is_in_pose_transition() && self.can_camel_change_pose()
        {
            self.stand_up();
        }
    }

    fn mob_cause_fall_damage(&self, fall_distance: f32) {
        horse_base::play_land_sound(self, fall_distance);
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
            horse_base::SOUND_VOLUME,
            self.voice_pitch(),
        );
        self.emit_own_game_event(GameEvent::EntityAction);
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
            let entity = self.get_entity();
            entity.set_default_dimensions(self.dimensions_for(true));
            entity.set_pose(EntityPose::Sitting);
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
        if let Some(result) = horse_base::important_interaction(self, player, item_stack) {
            return result;
        }
        if player.get_entity().is_sneaking() && !self.is_baby() {
            horse_inventory::open_custom_inventory_screen(self, player);
            return true;
        }
        if saddle::try_equip_saddle(self, player, item_stack) {
            return true;
        }
        // Vanilla `fedFood`: food it does not take is no click to ride either.
        if self.is_food(item_stack) {
            return self.feed(player, item_stack);
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
                Box::new(set_entity_look_target_sometimes::create_for(
                    &EntityType::PLAYER,
                    6.0,
                    30,
                    60,
                )),
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
/// down or stands up where it has the room.
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
            && camel.can_camel_change_pose()
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
