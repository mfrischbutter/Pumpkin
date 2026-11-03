use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};

use pumpkin_data::BlockState;
use pumpkin_data::damage::DamageType;
use pumpkin_data::sound::Sound;
use pumpkin_data::tag::{self, Taggable};
use pumpkin_data::tracked_data;
use pumpkin_data::world::WorldEvent;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_world::chunk::ChunkHeightmapType;
use rand::RngExt;

use crate::entity::ai::target_predicate::TargetPredicate;
use crate::entity::custom_sound::CustomSound;
use crate::entity::mob::{Mob, MobEntity};
use crate::entity::{Entity, EntityBase};
use crate::world::World;
use crate::world::level_accessor::LevelAccessor;

const ROOSTING_FLAG: u8 = 1;
const CLOSE_PLAYER_DISTANCE: f64 = 4.0;

pub struct BatEntity {
    pub mob_entity: MobEntity,
    hanging_position: Mutex<Option<BlockPos>>,
    roosting: AtomicBool,
}

impl BatEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let mob_entity = MobEntity::new(entity);
        let bat = Self {
            mob_entity,
            hanging_position: Mutex::new(None),
            roosting: AtomicBool::new(true),
        };
        Arc::new(bat)
    }

    #[must_use]
    pub fn check_bat_spawn_rules(level: &dyn LevelAccessor, pos: &BlockPos) -> bool {
        // Vanilla `getHeightmapPos` is the block above the top one.
        if pos.0.y
            > level.get_level().get_heightmap_height(
                ChunkHeightmapType::WorldSurface,
                pos.0.x,
                pos.0.z,
            )
        {
            return false;
        }
        if rand::random_bool(0.5) {
            return false;
        }
        if level.get_max_local_raw_brightness(pos) > rand::random_range(0..4) {
            return false;
        }
        let below_pos = BlockPos::new(pos.0.x, pos.0.y - 1, pos.0.z);
        if !level
            .get_block(&below_pos)
            .has_tag(&tag::Block::MINECRAFT_BATS_SPAWNABLE_ON)
        {
            return false;
        }
        true
    }

    #[must_use]
    pub fn is_roosting(&self) -> bool {
        self.roosting.load(Relaxed)
    }

    pub fn set_roosting(&self, roosting: bool) {
        self.roosting.store(roosting, Relaxed);
        let flags: u8 = if roosting { ROOSTING_FLAG } else { 0 };
        self.mob_entity
            .living_entity
            .entity
            .set_synced_data(tracked_data::bat::DATA_ID_FLAGS, flags);
    }

    /// Vanilla `levelEvent(null, 1025, pos, 0)`, unless the bat is silent.
    fn play_takeoff(&self, world: &World) {
        let entity = self.get_entity();
        if !entity.is_silent() {
            world.sync_world_event(WorldEvent::SoundBatLiftoff, entity.block_pos.load(), 0);
        }
    }

    /// Vanilla `level.getNearestPlayer(BAT_RESTING_TARGETING, this)`, within four blocks.
    fn player_is_near(&self, world: &World, pos: Vector3<f64>) -> bool {
        let targeting = TargetPredicate::create_non_attackable().set_base_max_distance(4.0);
        world
            .get_nearby_players(pos, CLOSE_PLAYER_DISTANCE)
            .iter()
            .any(|player| targeting.test(world, Some(self), player.as_ref()))
    }

    /// The resting half of vanilla `customServerAiStep`.
    fn tick_resting(&self, world: &World, above: &BlockPos, pos: Vector3<f64>) {
        let entity = self.get_entity();
        if !world.get_block_state(above).is_solid_block() {
            self.set_roosting(false);
            self.play_takeoff(world);
            return;
        }
        let head_yaw = {
            let mut rng = rand::rng();
            (rng.random_range(0u32..200) == 0).then(|| rng.random_range(0i32..360) as f32)
        };
        if let Some(head_yaw) = head_yaw {
            entity.head_yaw.store(head_yaw);
        }
        if self.player_is_near(world, pos) {
            self.set_roosting(false);
            self.play_takeoff(world);
        }
    }

    /// Vanilla `targetPosition.closerToCenterThan(position(), 2.0)`.
    fn is_close_to(target: BlockPos, pos: Vector3<f64>) -> bool {
        target.to_centered_f64().squared_distance_to_vec(&pos) < 4.0
    }

    /// The target of the flying half of vanilla `customServerAiStep`.
    fn next_target(&self, world: &World, pos: Vector3<f64>) -> BlockPos {
        let mut target = self
            .hanging_position
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if target.is_some_and(|hp| {
            !world.get_block_state(&hp).is_air() || hp.0.y <= world.dimension.min_y
        }) {
            *target = None;
        }
        let mut rng = rand::rng();
        match *target {
            Some(hp) if rng.random_range(0u32..30) != 0 && !Self::is_close_to(hp, pos) => hp,
            _ => {
                let chosen = BlockPos::floored(
                    pos.x + f64::from(rng.random_range(0i32..7) - rng.random_range(0i32..7)),
                    pos.y + f64::from(rng.random_range(0i32..6)) - 2.0,
                    pos.z + f64::from(rng.random_range(0i32..7) - rng.random_range(0i32..7)),
                );
                *target = Some(chosen);
                chosen
            }
        }
    }

    /// The flying half of vanilla `customServerAiStep`.
    fn tick_flying(&self, world: &World, above: &BlockPos, pos: Vector3<f64>) {
        let entity = self.get_entity();
        let target = self.next_target(world, pos);
        let velocity = entity.velocity.load();
        let step = |wanted: f64, current: f64| (wanted - current) * 0.1;
        let new_velocity = Vector3::new(
            velocity.x
                + step(
                    (f64::from(target.0.x) + 0.5 - pos.x).signum() * 0.5,
                    velocity.x,
                ),
            velocity.y
                + step(
                    (f64::from(target.0.y) + 0.1 - pos.y).signum() * 0.7,
                    velocity.y,
                ),
            velocity.z
                + step(
                    (f64::from(target.0.z) + 0.5 - pos.z).signum() * 0.5,
                    velocity.z,
                ),
        );
        entity.velocity.store(new_velocity);
        let yaw = (new_velocity.z.atan2(new_velocity.x) as f32).to_degrees() - 90.0;
        let yaw_diff = pumpkin_util::math::wrap_degrees(yaw - entity.yaw.load());
        self.mob_entity
            .living_entity
            .movement_input
            .store(Vector3::new(0.0, 0.0, 0.5));
        entity.yaw.store(entity.yaw.load() + yaw_diff);
        if rand::rng().random_range(0u32..100) == 0 && world.get_block_state(above).is_solid_block()
        {
            self.set_roosting(true);
        }
    }
}

impl CustomSound for BatEntity {
    fn hurt_sound(&self) -> Option<Sound> {
        Some(Sound::EntityBatHurt)
    }

    fn death_sound(&self) -> Option<Sound> {
        Some(Sound::EntityBatDeath)
    }

    /// Vanilla `Bat.getSoundVolume`.
    fn sound_volume(&self) -> f32 {
        0.1
    }
}

impl Mob for BatEntity {
    fn mob_is_pushable(&self) -> bool {
        false
    }

    fn mob_is_flapping(&self) -> bool {
        !self.is_roosting() && self.get_entity().tick_count.load(Relaxed) % 10 == 0
    }

    fn mob_init_data_tracker(&self) {
        let entity = self.get_entity();
        let flags: u8 = if self.is_roosting() { ROOSTING_FLAG } else { 0 };
        entity.set_synced_data(tracked_data::bat::DATA_ID_FLAGS, flags);
    }

    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        let flags: u8 = if self.is_roosting() { ROOSTING_FLAG } else { 0 };
        nbt.put_byte("BatFlags", i8::try_from(flags).unwrap_or(0));
    }

    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        let flags = u8::try_from(nbt.get_byte("BatFlags").unwrap_or(0)).unwrap_or(0);
        let roosting = (flags & ROOSTING_FLAG) != 0;
        self.set_roosting(roosting);
    }

    fn mob_metadata(&self) -> Option<crate::entity::mob::MobMetadata> {
        let mut meta = crate::entity::mob::MobMetadata::new();
        if self.is_roosting() {
            meta = meta.bat_roosting(true);
        }
        Some(meta)
    }

    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    fn custom_server_ai_step(&self, _caller: &dyn EntityBase) {
        let entity = &self.mob_entity.living_entity.entity;
        let block_pos = entity.block_pos.load();
        let above_pos = BlockPos::new(block_pos.0.x, block_pos.0.y + 1, block_pos.0.z);
        let world = entity.world.load();
        let pos = entity.pos.load();

        if self.is_roosting() {
            self.tick_resting(&world, &above_pos, pos);
        } else {
            self.tick_flying(&world, &above_pos, pos);
        }
    }

    /// Vanilla `Bat.getAmbientSound`.
    fn get_ambient_sound(&self) -> Option<Sound> {
        (!self.is_roosting() || rand::rng().random_range(0..4) == 0)
            .then_some(Sound::EntityBatAmbient)
    }

    /// Vanilla `getSoundVolume` and `getVoicePitch` of the bat.
    fn play_ambient_sound(&self) {
        if let Some(sound) = self.get_ambient_sound() {
            let pitch = self.mob_entity.living_entity.get_pitch() * 0.95;
            self.play_entity_sound(sound, 0.1, pitch);
        }
    }

    fn as_custom_sound(&self) -> Option<&dyn CustomSound> {
        Some(self)
    }

    /// Vanilla `Bat.getMovementEmission`: only game events, no step sounds.
    fn mob_play_step_sound(&self, _pos: &BlockPos, _state: &BlockState) {}

    /// Vanilla `Bat.checkFallDamage`.
    fn should_check_fall_damage(&self) -> bool {
        false
    }

    fn post_tick(&self) {
        if self.is_roosting() {
            let entity = &self.mob_entity.living_entity.entity;
            entity.velocity.store(Vector3::new(0.0, 0.0, 0.0));
            let pos = entity.pos.load();
            let snapped_y = (pos.y.floor()) + 1.0 - f64::from(entity.height());
            entity.set_pos(Vector3::new(pos.x, snapped_y, pos.z));
        }
    }

    fn get_mob_y_velocity_drag(&self) -> Option<f64> {
        Some(0.6)
    }

    /// Vanilla `Bat.hurtServer`: being hurt wakes it, without the takeoff sound.
    fn on_damage(&self, _damage_type: DamageType, _source: Option<&dyn EntityBase>) {
        if self.is_roosting() {
            self.set_roosting(false);
        }
    }
}
