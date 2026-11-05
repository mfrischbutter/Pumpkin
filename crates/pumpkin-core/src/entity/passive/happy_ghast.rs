use crate::world::level_accessor::LevelAccessor;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicI32, AtomicUsize, Ordering},
};

use pumpkin_data::BlockState;
use pumpkin_data::data_component_impl::EquipmentSlot;
use pumpkin_data::entity::EntityType;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::tag::{self, Taggable};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::math::boundingbox::{BoundingBox, EntityDimensions};
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_util::math::wrap_degrees;

use super::happy_ghast_ai;
use crate::entity::ai::brain::memory::{MemoryModuleId, PackedMemories};
use crate::entity::ai::brain::sensing::SensorType;
use crate::entity::ai::brain::{Brain, BrainProvider};
use crate::entity::ai::control::flying_move_control::FlyingMoveControl;
use crate::entity::ai::control::ghast_move_control::GhastMoveControl;
use crate::entity::ai::control::look_control::LookControl;
use crate::entity::ai::control::still_look_control::StillLookControl;
use crate::entity::ai::goal::{Controls, Goal};
use crate::entity::ai::pathfinder::Navigator;
use crate::entity::custom_sound::CustomSound;
use crate::entity::mob::ghast::RandomFloatAroundGoal;
use crate::entity::{
    Entity, EntityBase,
    ageable::{AgeableData, AgeableMob},
    ai::goal::{swim::SwimGoal, tempt::TemptGoal},
    mob::{Mob, MobEntity, equipment},
    passive::animal::Animal,
    player::Player,
};

/// How many riders vanilla lets on.
const MAX_PASSENGERS: usize = 4;
/// Vanilla `STILL_TIMEOUT_ON_LOAD_GRACE_PERIOD`.
const STILL_TIMEOUT_ON_LOAD_GRACE_PERIOD: i32 = 60;
/// Vanilla `MAX_STILL_TIMEOUT`.
const MAX_STILL_TIMEOUT: i32 = 10;
/// Vanilla `getAgeScale` for a baby and the eye of `BABY_DIMENSIONS`.
const BABY_SCALE: f32 = 0.2375;
const BABY_EYE_HEIGHT: f32 = 0.46875;
/// Vanilla `travel`: `FLYING_SPEED * 5.0 / 3.0`.
const FLYING_SPEED_SCALE: f32 = 5.0 / 3.0;
/// Vanilla `tickRidden`.
const RIDDEN_TURN_SPEED: f32 = 0.08;
/// Vanilla `getRiddenRotation`.
const RIDDEN_PITCH_SCALE: f32 = 0.5;
/// How far above the world's cloud height vanilla `isInClouds` reaches.
const CLOUD_DEPTH: f64 = 4.0;

const MEMORIES: [MemoryModuleId; 0] = [];

fn as_happy_ghast(mob: &dyn Mob) -> Option<&HappyGhastEntity> {
    mob.cast_any().downcast_ref::<HappyGhastEntity>()
}

fn holds_still(mob: &dyn Mob) -> bool {
    as_happy_ghast(mob).is_some_and(HappyGhastEntity::is_on_still_timeout)
}

/// Vanilla's `TemptGoal` predicate: a harness draws a bare adult, a snowball anyone else.
fn is_tempted_by(mob: &dyn Mob, stack: &ItemStack) -> bool {
    let tag = if as_happy_ghast(mob)
        .is_some_and(|ghast| !ghast.is_wearing_harness() && !ghast.is_baby())
    {
        &tag::Item::MINECRAFT_HAPPY_GHAST_TEMPT_ITEMS
    } else {
        &tag::Item::MINECRAFT_HAPPY_GHAST_FOOD
    };
    stack.item.has_tag(tag)
}

/// Vanilla `HappyGhast.HappyGhastFloatGoal`: it does not rise while it holds still.
#[derive(Default)]
struct FloatUnlessStill(SwimGoal);

impl Goal for FloatUnlessStill {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        !holds_still(mob) && self.0.can_start(mob)
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        self.0.should_continue(mob)
    }

    fn start(&mut self, mob: &dyn Mob) {
        self.0.start(mob);
    }

    fn tick(&mut self, mob: &dyn Mob) {
        self.0.tick(mob);
    }

    fn stop(&mut self, mob: &dyn Mob) {
        self.0.stop(mob);
    }

    fn controls(&self) -> Controls {
        self.0.controls()
    }
}

/// Represents a Happy Ghast, a passive flying mob a player can ride in a harness.
///
/// Wiki: <https://minecraft.wiki/w/Happy_Ghast>
pub struct HappyGhastEntity {
    pub mob_entity: MobEntity,
    pub ageable_data: AgeableData,
    pub server_still_timeout: AtomicI32,
    pub leash_holder_time: AtomicI32,
    pub is_leash_holder: AtomicBool,
    pub stays_still: AtomicBool,
    /// What the controls and goals are set up for: vanilla does it in `ageBoundaryReached`.
    set_up_as_baby: AtomicBool,
    /// The riders last seen, for vanilla's `addPassenger` and `removePassenger`.
    last_passengers: AtomicUsize,
}

impl HappyGhastEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let ghast = Arc::new(Self {
            mob_entity: MobEntity::new(entity),
            ageable_data: AgeableData::default(),
            server_still_timeout: AtomicI32::new(0),
            leash_holder_time: AtomicI32::new(0),
            is_leash_holder: AtomicBool::new(false),
            stays_still: AtomicBool::new(false),
            set_up_as_baby: AtomicBool::new(false),
            last_passengers: AtomicUsize::new(0),
        });
        ghast.set_adult_controls();
        ghast.register_goals();
        ghast
    }

    /// Vanilla `registerGoals`.
    fn register_goals(&self) {
        let goals = &self.mob_entity.goals_selector;
        goals.add_goal(3, Box::new(FloatUnlessStill::default()));
        goals.add_goal(
            4,
            Box::new(TemptGoal::for_non_pathfinders(
                1.0,
                is_tempted_by,
                false,
                7.0,
            )),
        );
        goals.add_goal(5, Box::new(RandomFloatAroundGoal::keeping_near_blocks(16)));
    }

    fn set_adult_controls(&self) {
        *self
            .mob_entity
            .move_control
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            Box::new(GhastMoveControl::careful(holds_still));
        *self
            .mob_entity
            .look_control
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            Box::new(StillLookControl::new(holds_still));
    }

    /// Vanilla `adultGhastSetup`.
    fn adult_setup(&self) {
        self.set_adult_controls();
        *self
            .mob_entity
            .navigator
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Navigator::default();
        self.mob_entity.goals_selector.clear(self);
        self.register_goals();
        self.mob_entity.reset_brain(self);
        let entity = self.get_entity();
        entity.set_default_dimensions(Entity::type_dimensions(&EntityType::HAPPY_GHAST));
    }

    /// Vanilla `babyGhastSetup`.
    fn baby_setup(&self) {
        *self
            .mob_entity
            .move_control
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            Box::new(FlyingMoveControl::new(180, true));
        *self
            .mob_entity
            .look_control
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Box::new(LookControl::default());
        let mut navigator = self
            .mob_entity
            .navigator
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *navigator = Navigator::flying();
        navigator.set_can_open_doors(false);
        navigator.set_can_float(true);
        navigator.set_required_path_length(48.0);
        drop(navigator);
        self.set_server_still_timeout(0);
        self.mob_entity.goals_selector.clear(self);
        let full = Entity::type_dimensions(&EntityType::HAPPY_GHAST);
        self.get_entity()
            .set_default_dimensions(EntityDimensions::new(
                full.width * BABY_SCALE,
                full.height * BABY_SCALE,
                BABY_EYE_HEIGHT,
            ));
    }

    pub fn set_server_still_timeout(&self, timeout: i32) {
        self.server_still_timeout.store(timeout, Ordering::Relaxed);
        self.sync_stay_still_flag();
    }

    #[must_use]
    pub fn is_on_still_timeout(&self) -> bool {
        self.stays_still.load(Ordering::Relaxed)
            || self.server_still_timeout.load(Ordering::Relaxed) > 0
    }

    fn set_leash_holder(&self, is_leash_holder: bool) {
        if self
            .is_leash_holder
            .swap(is_leash_holder, Ordering::Relaxed)
            != is_leash_holder
        {
            self.get_entity().set_synced_data(
                pumpkin_data::tracked_data::happy_ghast::IS_LEASH_HOLDER,
                is_leash_holder,
            );
        }
    }

    fn sync_stay_still_flag(&self) {
        let stays_still = self.server_still_timeout.load(Ordering::Relaxed) > 0;
        if self.stays_still.swap(stays_still, Ordering::Relaxed) != stays_still {
            self.get_entity().set_synced_data(
                pumpkin_data::tracked_data::happy_ghast::STAYS_STILL,
                stays_still,
            );
        }
    }

    /// Vanilla `isWearingBodyArmor`.
    #[must_use]
    pub fn is_wearing_harness(&self) -> bool {
        !self
            .mob_entity
            .living_entity
            .entity_equipment
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&EquipmentSlot::BODY)
            .is_empty()
    }

    fn passenger_count(&self) -> usize {
        self.get_entity()
            .passengers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len()
    }

    /// Vanilla `scanPlayerAboveGhast`: a player stands on it, in a box as wide as it plus a block
    /// and half as high.
    fn scan_player_above(&self) -> bool {
        let entity = self.get_entity();
        let bounds = entity.bounding_box.load();
        let above = BoundingBox::new(
            Vector3::new(
                bounds.min.x - 1.0,
                bounds.max.y - 1.0e-5,
                bounds.min.z - 1.0,
            ),
            Vector3::new(
                bounds.max.x + 1.0,
                bounds.max.y + (bounds.max.y - bounds.min.y) / 2.0,
                bounds.max.z + 1.0,
            ),
        );
        entity.world.load().players.load().iter().any(|player| {
            if player.is_spectator() {
                return false;
            }
            let player_entity = player.get_entity();
            let root_pos = match player_entity.get_root_vehicle() {
                Some(root) if root.cast_any().downcast_ref::<Self>().is_some() => return false,
                Some(root) => root.get_entity().pos.load(),
                None => player_entity.pos.load(),
            };
            (above.min.x..above.max.x).contains(&root_pos.x)
                && (above.min.y..above.max.y).contains(&root_pos.y)
                && (above.min.z..above.max.z).contains(&root_pos.z)
        })
    }

    /// Vanilla `addPassenger` and `removePassenger`, seen a tick later.
    fn notice_passengers(&self) {
        let now = self.passenger_count();
        let before = self.last_passengers.swap(now, Ordering::Relaxed);
        if now > before {
            if before == 0 {
                self.play_entity_sound(Sound::EntityHappyGhastHarnessGogglesDown, 1.0, 1.0);
            }
            if !self.scan_player_above() {
                self.set_server_still_timeout(0);
            } else if self.server_still_timeout.load(Ordering::Relaxed) > MAX_STILL_TIMEOUT {
                self.set_server_still_timeout(MAX_STILL_TIMEOUT);
            }
        } else if now < before {
            self.set_server_still_timeout(MAX_STILL_TIMEOUT);
            if now == 0 {
                self.mob_entity.clear_home();
                self.play_entity_sound(Sound::EntityHappyGhastHarnessGogglesUp, 1.0, 1.0);
            }
        }
    }

    fn tick_still_timeout(&self) {
        let timeout = self.server_still_timeout.load(Ordering::Relaxed);
        if timeout > 0 {
            let loaded = self.get_entity().tick_count.load(Ordering::Relaxed)
                > STILL_TIMEOUT_ON_LOAD_GRACE_PERIOD;
            self.set_server_still_timeout(if loaded { timeout - 1 } else { timeout });
        }
        if self.scan_player_above() {
            self.set_server_still_timeout(MAX_STILL_TIMEOUT);
        }
    }

    /// Vanilla `getHappyGhastRestrictionRadius`.
    fn restriction_radius(&self) -> i32 {
        if !self.is_baby() && !self.is_wearing_harness() {
            64
        } else {
            32
        }
    }

    /// Vanilla `checkRestriction`.
    fn check_restriction(&self) {
        let entity = self.get_entity();
        if entity.is_leashed() || self.passenger_count() > 0 {
            return;
        }
        let radius = self.restriction_radius();
        let mob = &self.mob_entity;
        let pos = entity.block_pos.load();
        let margin = radius + 16;
        if !mob.has_position_target()
            || mob.position_target.load().squared_distance(&pos) >= margin * margin
            || radius != mob.position_target_range.load(Ordering::Relaxed)
        {
            mob.set_home_to(pos, radius);
        }
    }

    /// Vanilla `isInClouds`.
    fn is_in_clouds(&self) -> bool {
        let entity = self.get_entity();
        let Some(bottom) = entity.world.load().dimension.cloud_height.map(f64::from) else {
            return false;
        };
        let y = entity.pos.load().y;
        y + f64::from(entity.height()) >= bottom && y <= bottom + CLOUD_DEPTH
    }

    /// Vanilla `continuousHeal`: a point every 30 seconds, every second in clouds or rain.
    fn continuous_heal(&self) {
        let entity = self.get_entity();
        let living = &self.mob_entity.living_entity;
        if !entity.is_alive() || living.health.load() >= living.get_max_health() {
            return;
        }
        let fast =
            self.is_in_clouds() || entity.world.load().is_raining_at(&entity.block_pos.load());
        if entity.tick_count.load(Ordering::Relaxed) % (if fast { 20 } else { 600 }) == 0 {
            living.heal(1.0);
        }
    }

    /// The equip part of vanilla `Equippable.equipOnInteract`.
    fn try_equip_harness(&self, player: &Player, stack: &mut ItemStack) -> bool {
        if self.is_baby()
            || self.is_wearing_harness()
            || !equipment::is_equippable_in_slot(
                self.get_entity().entity_type,
                stack,
                &EquipmentSlot::BODY,
            )
        {
            return false;
        }
        self.mob_entity
            .set_item_slot_and_drop_when_killed(&EquipmentSlot::BODY, stack.copy_with_count(1));
        stack.decrement_unless_creative(player.gamemode.load(), 1);
        self.play_entity_sound(Sound::EntityHappyGhastEquip, 1.0, 1.0);
        true
    }

    /// Vanilla `doPlayerRide`.
    fn start_riding(&self, player: &Player) -> bool {
        if self.passenger_count() >= MAX_PASSENGERS {
            return false;
        }
        let entity = self.get_entity();
        let world = entity.world.load();
        if let Some(vehicle) = world.get_entity_by_id(entity.entity_id)
            && let Some(passenger) = world.get_player_by_id(player.entity_id())
        {
            entity.add_passenger(vehicle, passenger as Arc<dyn EntityBase>);
        }
        true
    }
}

impl AgeableMob for HappyGhastEntity {
    fn get_ageable_data(&self) -> &AgeableData {
        &self.ageable_data
    }
}

impl Animal for HappyGhastEntity {
    fn is_food(&self, item_stack: &ItemStack) -> bool {
        item_stack
            .item
            .has_tag(&tag::Item::MINECRAFT_HAPPY_GHAST_FOOD)
    }
}

impl CustomSound for HappyGhastEntity {
    fn death_sound(&self) -> Option<Sound> {
        Some(if self.is_baby() {
            Sound::EntityGhastlingDeath
        } else {
            Sound::EntityHappyGhastDeath
        })
    }

    fn hurt_sound(&self) -> Option<Sound> {
        Some(if self.is_baby() {
            Sound::EntityGhastlingHurt
        } else {
            Sound::EntityHappyGhastHurt
        })
    }

    fn sound_volume(&self) -> f32 {
        if self.is_baby() { 1.0 } else { 4.0 }
    }

    fn sound_category(&self) -> SoundCategory {
        SoundCategory::Neutral
    }
}

impl Mob for HappyGhastEntity {
    /// Vanilla `supportQuadLeashAsHolder`.
    fn mob_support_quad_leash_as_holder(&self) -> bool {
        true
    }

    fn make_brain(&self, packed: &PackedMemories) -> Brain {
        BrainProvider {
            memory_types: &MEMORIES,
            sensor_types: &[
                SensorType::NearestLivingEntities,
                SensorType::HurtBy,
                SensorType::FoodTemptations,
                SensorType::NearestAdultAnyType,
                SensorType::NearestPlayers,
            ],
            activities: happy_ghast_ai::activities,
        }
        .make_brain(self, packed)
    }

    /// Vanilla `ageBoundaryReached`, caught before the mob thinks, also for an age from NBT.
    fn before_mob_ai_step(&self) {
        let baby = self.is_baby();
        if self.set_up_as_baby.swap(baby, Ordering::Relaxed) != baby {
            if baby {
                self.baby_setup();
            } else {
                self.adult_setup();
            }
        }
    }

    /// Vanilla `HappyGhast.customServerAiStep`.
    fn custom_server_ai_step(&self, _caller: &dyn EntityBase) {
        if self.is_baby() {
            self.mob_entity.tick_brain(self);
            self.mob_entity
                .with_brain(self, |brain| happy_ghast_ai::update_activity(brain));
        }
        self.check_restriction();
    }

    /// Vanilla `HappyGhast.travel`.
    fn travel(&self, caller: &dyn EntityBase) -> bool {
        let living = &self.mob_entity.living_entity;
        let flying_speed =
            living.get_attribute_value(&pumpkin_data::attributes::Attributes::FLYING_SPEED) as f32;
        living.travel_flying_at(caller, flying_speed * FLYING_SPEED_SCALE);
        true
    }

    /// Vanilla `checkFallDamage` does nothing.
    fn should_check_fall_damage(&self) -> bool {
        false
    }

    /// Vanilla `getControllingPassenger`: in a harness the first rider steers.
    fn mob_controlling_player(&self) -> Option<Arc<Player>> {
        if !self.is_wearing_harness() || self.is_on_still_timeout() {
            return None;
        }
        let entity = self.get_entity();
        let first_id = entity
            .passengers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .first()
            .map(|rider| rider.get_entity().entity_id)?;
        entity.world.load().get_player_by_id(first_id)
    }

    /// Vanilla `tickRidden`.
    fn mob_tick_ridden(&self, controller: &Player) {
        let entity = self.get_entity();
        let rider = controller.get_entity();
        let yaw = entity.yaw.load();
        let turned = yaw + wrap_degrees(rider.yaw.load() - yaw) * RIDDEN_TURN_SPEED;
        entity.set_rotation(turned, rider.pitch.load() * RIDDEN_PITCH_SCALE);
        entity.body_yaw.store(turned);
        entity.head_yaw.store(turned);
    }

    /// Vanilla `getDismountLocationForPassenger`: on top of it.
    fn dismount_location(&self) -> Option<Vector3<f64>> {
        let entity = self.get_entity();
        let pos = entity.pos.load();
        Some(Vector3::new(pos.x, entity.bounding_box.load().max.y, pos.z))
    }

    /// Vanilla `HappyGhast.getWalkTargetValue`: nothing inside a block; in the air 10 with air
    /// below and a block under that, else 5.
    fn get_walk_target_value(&self, pos: &BlockPos, level: &dyn LevelAccessor) -> f32 {
        if !level.get_block_state(pos).is_air() {
            return 0.0;
        }
        if level.get_block_state(&pos.down()).is_air()
            && !level.get_block_state(&pos.down_height(2)).is_air()
        {
            10.0
        } else {
            5.0
        }
    }

    /// Vanilla `HappyGhast.getAmbientSound`.
    fn get_ambient_sound(&self) -> Option<Sound> {
        Some(if self.is_baby() {
            Sound::EntityGhastlingAmbient
        } else {
            Sound::EntityHappyGhastAmbient
        })
    }

    /// Vanilla `getAmbientSoundInterval`: it is quieter with riders.
    fn get_ambient_sound_interval(&self) -> i32 {
        let interval = MobEntity::DEFAULT_AMBIENT_SOUND_INTERVAL;
        if self.passenger_count() > 0 {
            interval * 6
        } else {
            interval
        }
    }

    fn as_custom_sound(&self) -> Option<&dyn CustomSound> {
        Some(self)
    }

    /// Vanilla `HappyGhast` steps silently.
    fn mob_play_step_sound(&self, _pos: &BlockPos, _state: &BlockState) {}

    fn as_ageable(&self) -> Option<&dyn AgeableMob> {
        Some(self)
    }

    fn as_animal(&self) -> Option<&dyn Animal> {
        Some(self)
    }

    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        nbt.put_int(
            "still_timeout",
            self.server_still_timeout.load(Ordering::Relaxed),
        );
    }

    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        if let Some(timeout) = nbt.get_int("still_timeout") {
            self.set_server_still_timeout(timeout);
        }
    }

    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    /// Vanilla `HappyGhast.tick` and the heal of its `aiStep`.
    fn mob_tick(&self, _caller: &dyn EntityBase) {
        let leash_time = self.leash_holder_time.load(Ordering::Relaxed);
        if leash_time > 0 {
            self.leash_holder_time
                .store(leash_time - 1, Ordering::Relaxed);
        }
        self.set_leash_holder(leash_time > 1);
        self.tick_still_timeout();
        self.notice_passengers();
        self.continuous_heal();
    }

    fn mob_init_data_tracker(&self) {
        let entity = self.get_entity();
        let is_baby = entity.age.load(Ordering::Relaxed) < 0;
        if is_baby {
            entity.set_synced_data(pumpkin_data::tracked_data::happy_ghast::BABY_ID, true);
        }
        entity.set_synced_data(
            pumpkin_data::tracked_data::happy_ghast::IS_LEASH_HOLDER,
            self.is_leash_holder.load(Ordering::Relaxed),
        );
        entity.set_synced_data(
            pumpkin_data::tracked_data::happy_ghast::STAYS_STILL,
            self.stays_still.load(Ordering::Relaxed),
        );
    }

    /// Vanilla `HappyGhast.mobInteract` with the item's own interactions before it.
    fn mob_interact(&self, player: &Arc<Player>, item_stack: &mut ItemStack) -> bool {
        if self.is_baby() {
            return self.animal_interact(player, item_stack);
        }
        if self.try_equip_harness(player, item_stack) {
            return true;
        }
        self.is_wearing_harness() && !player.get_entity().is_sneaking() && self.start_riding(player)
    }
}
