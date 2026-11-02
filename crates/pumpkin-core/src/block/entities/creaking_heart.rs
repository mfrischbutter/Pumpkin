use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicI64, Ordering};

use crossbeam::atomic::AtomicCell;
use pumpkin_data::block_properties::{
    Axis, CreakingHeartLikeProperties, CreakingHeartState, GlowLichenLikeProperties,
    PaleOakWoodLikeProperties,
};
use pumpkin_data::dimension::Dimension;
use pumpkin_data::entity::EntityType;
use pumpkin_data::game_event::GameEvent;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::tag::{self, Taggable};
use pumpkin_data::{Block, BlockDirection, BlockStateId};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_protocol::java::client::play::CEntityStatus;
use pumpkin_util::Difficulty;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_world::world::BlockFlags;
use rand::RngExt;
use rand::seq::SliceRandom;
use uuid::Uuid;

use crate::entity::mob::creaking::CreakingEntity;
use crate::entity::spawn_util::{SpawnStrategy, try_spawn_mob};
use crate::entity::{EntityBase, mob::Mob};
use crate::world::World;

use super::BlockEntity;

/// Vanilla `PLAYER_DETECTION_RANGE`.
const PLAYER_DETECTION_RANGE: f64 = 32.0;
/// Vanilla `CREAKING_ROAMING_RADIUS`, the distance the comparator output scales over.
const CREAKING_ROAMING_RADIUS: f64 = 32.0;
/// Vanilla `DISTANCE_CREAKING_TOO_FAR`.
const DISTANCE_CREAKING_TOO_FAR: f64 = 34.0;
const SPAWN_RANGE_XZ: i32 = 16;
const SPAWN_RANGE_Y: i32 = 8;
const ATTEMPTS_PER_SPAWN: i32 = 5;
const UPDATE_TICKS: i32 = 20;
const UPDATE_TICKS_VARIANCE: i32 = 5;
const HURT_CALL_TOTAL_TICKS: i32 = 100;
const HURT_CALL_INTERVAL: i32 = 10;
const MAX_DEPTH: i32 = 2;
const MAX_COUNT: usize = 64;
const TICKS_GRACE_PERIOD: i64 = 30;
/// Vanilla's `Timelines` overworld track of `CREAKING_ACTIVE`.
const ACTIVE_FROM: i64 = 12600;
const ACTIVE_UNTIL: i64 = 23401;
/// The entity event of a spawned creaking, vanilla's byte 60.
const EVENT_SPAWN_POOF: i8 = 60;

const ALL_DIRECTIONS: [BlockDirection; 6] = [
    BlockDirection::Down,
    BlockDirection::Up,
    BlockDirection::North,
    BlockDirection::South,
    BlockDirection::West,
    BlockDirection::East,
];

/// Vanilla `EnvironmentAttributes.CREAKING_ACTIVE` at a heart: at night in the overworld.
#[must_use]
pub fn creaking_active(world: &World) -> bool {
    if world.dimension != Dimension::OVERWORLD {
        return false;
    }
    let time = world
        .level_time
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .time_of_day
        .rem_euclid(24000);
    (ACTIVE_FROM..ACTIVE_UNTIL).contains(&time)
}

/// Vanilla `CreakingHeartBlock.hasRequiredLogs`.
#[must_use]
pub fn has_required_logs(world: &World, pos: &BlockPos, axis: Axis) -> bool {
    let (a, b) = match axis {
        Axis::X => (pos.west(), pos.east()),
        Axis::Y => (pos.down(), pos.up()),
        Axis::Z => (pos.north(), pos.south()),
    };
    <[BlockPos; 2]>::from((a, b)).iter().all(|neighbour| {
        let id = world.get_block_state_id(neighbour);
        id.to_block().has_tag(&tag::Block::MINECRAFT_PALE_OAK_LOGS)
            && PaleOakWoodLikeProperties::from_state_id(id).axis == axis
    })
}

/// Runs `read` on the heart at `pos`, when there is one.
pub fn with_heart<R>(
    world: &World,
    pos: &BlockPos,
    read: impl FnOnce(&CreakingHeartBlockEntity) -> R,
) -> Option<R> {
    let block_entity = world.get_block_entity(pos)?;
    block_entity
        .as_any()
        .downcast_ref::<CreakingHeartBlockEntity>()
        .map(read)
}

pub struct CreakingHeartBlockEntity {
    pub position: BlockPos,
    pub creaking_uuid: AtomicCell<Option<Uuid>>,
    ticks_existed: AtomicI64,
    ticker: AtomicI32,
    emitter: AtomicI32,
    emitter_target: AtomicCell<Option<Vector3<f64>>>,
    output_signal: AtomicI32,
    dirty: AtomicBool,
}

impl BlockEntity for CreakingHeartBlockEntity {
    fn resource_location(&self) -> &'static str {
        Self::ID
    }

    fn get_position(&self) -> BlockPos {
        self.position
    }

    fn from_nbt(nbt: &pumpkin_nbt::compound::NbtCompound, position: BlockPos) -> Self
    where
        Self: Sized,
    {
        let creaking_uuid = nbt
            .get_uuid("creaking")
            .or_else(|| {
                nbt.get_string("creaking")
                    .and_then(|uuid_str| Uuid::parse_str(uuid_str).ok())
            })
            .or_else(|| {
                nbt.get_string("creaking_uuid")
                    .and_then(|uuid_str| Uuid::parse_str(uuid_str).ok())
            });
        let heart = Self::new(position);
        heart.creaking_uuid.store(creaking_uuid);
        heart
    }

    fn write_nbt(&self, nbt: &mut NbtCompound) {
        if let Some(uuid) = self.creaking_uuid.load() {
            nbt.put_uuid("creaking", uuid);
        }
    }

    fn chunk_data_nbt(&self) -> Option<NbtCompound> {
        let mut nbt = NbtCompound::new();
        if let Some(uuid) = self.creaking_uuid.load() {
            nbt.put_uuid("creaking", uuid);
        }
        Some(nbt)
    }

    /// Vanilla `CreakingHeartBlockEntity.serverTick`, while the heart is rooted.
    fn tick(&self, world: &Arc<World>) {
        self.ticks_existed.fetch_add(1, Ordering::Relaxed);
        let state_id = world.get_block_state_id(&self.position);
        if state_id.to_block() != &Block::CREAKING_HEART
            || CreakingHeartLikeProperties::from_state_id(state_id).creaking_heart_state
                == CreakingHeartState::Uprooted
        {
            return;
        }
        let signal = self.compute_output_signal(world);
        if self.output_signal.swap(signal, Ordering::Relaxed) != signal {
            world.update_neighbour_for_output_signal(&self.position, &Block::CREAKING_HEART);
        }
        self.tick_emitter(world);
        if self.ticker.fetch_sub(1, Ordering::Relaxed) >= 0 {
            return;
        }
        let variance = rand::rng().random_range(0..UPDATE_TICKS_VARIANCE);
        self.ticker
            .store(UPDATE_TICKS + variance, Ordering::Relaxed);
        self.update(world, state_id);
    }

    fn is_dirty(&self) -> bool {
        self.dirty.load(Ordering::Relaxed)
    }

    fn clear_dirty(&self) {
        self.dirty.store(false, Ordering::Relaxed);
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

impl CreakingHeartBlockEntity {
    pub const ID: &'static str = "minecraft:creaking_heart";

    #[must_use]
    pub const fn new(position: BlockPos) -> Self {
        Self {
            position,
            creaking_uuid: AtomicCell::new(None),
            ticks_existed: AtomicI64::new(0),
            ticker: AtomicI32::new(0),
            emitter: AtomicI32::new(0),
            emitter_target: AtomicCell::new(None),
            output_signal: AtomicI32::new(0),
            dirty: AtomicBool::new(false),
        }
    }

    pub fn is_protector(&self, creaking_uuid: Uuid) -> bool {
        self.creaking_uuid.load() == Some(creaking_uuid)
    }

    pub fn set_creaking_uuid(&self, uuid: Option<Uuid>) {
        self.creaking_uuid.store(uuid);
        self.dirty.store(true, Ordering::Relaxed);
    }

    /// Vanilla `getCreakingProtector`: a grace period lets a loading chunk bring it back.
    fn protector(&self, world: &World) -> Option<Arc<dyn EntityBase>> {
        let uuid = self.creaking_uuid.load()?;
        let found = world
            .get_entity_by_uuid(uuid)
            .filter(|entity| !entity.get_entity().is_removed());
        if found.is_none() && self.ticks_existed.load(Ordering::Relaxed) >= TICKS_GRACE_PERIOD {
            self.set_creaking_uuid(None);
        }
        found
    }

    fn with_protector<R>(
        &self,
        world: &World,
        read: impl FnOnce(&CreakingEntity) -> R,
    ) -> Option<R> {
        let found = self.protector(world)?;
        found.cast_any().downcast_ref::<CreakingEntity>().map(read)
    }

    fn distance_to_creaking(&self, world: &World) -> f64 {
        let center = self.position.to_centered_f64();
        let bottom = Vector3::new(center.x, f64::from(self.position.0.y), center.z);
        self.with_protector(world, |creaking| {
            creaking
                .get_entity()
                .pos
                .load()
                .squared_distance_to_vec(&bottom)
                .sqrt()
        })
        .unwrap_or(0.0)
    }

    /// Vanilla `computeAnalogOutputSignal`.
    #[must_use]
    pub fn compute_output_signal(&self, world: &World) -> i32 {
        if self.protector(world).is_none() {
            return 0;
        }
        let scaled = self
            .distance_to_creaking(world)
            .clamp(0.0, CREAKING_ROAMING_RADIUS)
            / CREAKING_ROAMING_RADIUS;
        15 - (scaled * 15.0).floor() as i32
    }

    /// Vanilla `getAnalogOutputSignal`.
    #[must_use]
    pub fn output_signal(&self) -> i32 {
        self.output_signal.load(Ordering::Relaxed)
    }

    /// The hurt call of vanilla `serverTick`: a sound that walks from the creaking to the heart.
    fn tick_emitter(&self, world: &Arc<World>) {
        let emitter = self.emitter.load(Ordering::Relaxed);
        if emitter <= 0 {
            return;
        }
        if emitter % HURT_CALL_INTERVAL == 0
            && let Some(target) = self.emitter_target.load()
        {
            let target = self
                .with_protector(world, |creaking| creaking.get_entity().pos.load())
                .unwrap_or(target);
            self.emitter_target.store(Some(target));
            let heart = self.position.to_centered_f64();
            let progress = 0.2
                + 0.8 * f64::from(HURT_CALL_TOTAL_TICKS - emitter)
                    / f64::from(HURT_CALL_TOTAL_TICKS);
            let location = target + (heart - target) * progress;
            let volume = emitter as f32 / 2.0 / HURT_CALL_TOTAL_TICKS as f32 + 0.5;
            world.play_sound_fine(
                Sound::BlockCreakingHeartHurt,
                SoundCategory::Blocks,
                &location,
                volume,
                1.0,
            );
        }
        self.emitter.store(emitter - 1, Ordering::Relaxed);
    }

    /// The periodic part of vanilla `serverTick`.
    fn update(&self, world: &Arc<World>, state_id: BlockStateId) {
        let mut props = CreakingHeartLikeProperties::from_state_id(state_id);
        let has_creaking = self.creaking_uuid.load().is_some();
        let new_state = if !has_required_logs(world, &self.position, props.axis) && !has_creaking {
            CreakingHeartState::Uprooted
        } else if creaking_active(world) {
            CreakingHeartState::Awake
        } else {
            CreakingHeartState::Dormant
        };
        if props.creaking_heart_state != new_state {
            props.creaking_heart_state = new_state;
            world.set_block_state(
                &self.position,
                props.to_state_id(&Block::CREAKING_HEART),
                BlockFlags::NOTIFY_ALL,
            );
            if new_state == CreakingHeartState::Uprooted {
                return;
            }
        }
        if has_creaking {
            self.check_protector(world);
        } else if new_state == CreakingHeartState::Awake {
            self.try_spawn_protector(world);
        }
    }

    /// Vanilla: the day, a long way from home or a player stuck in it end the protector.
    fn check_protector(&self, world: &Arc<World>) {
        let ends = self.with_protector(world, |creaking| {
            (!creaking_active(world)
                && !creaking
                    .get_mob_entity()
                    .persistence_required
                    .load(Ordering::Relaxed))
                || creaking.player_is_stuck_in_you()
        });
        if ends == Some(true)
            || (ends.is_some() && self.distance_to_creaking(world) > DISTANCE_CREAKING_TOO_FAR)
        {
            self.remove_protector(world);
        }
    }

    fn try_spawn_protector(&self, world: &Arc<World>) {
        if !world.should_spawn_monsters()
            || world.level_info.load().difficulty == Difficulty::Peaceful
            || world
                .get_nearby_players(self.position.to_f64(), PLAYER_DETECTION_RANGE)
                .is_empty()
        {
            return;
        }
        let Some(creaking) = try_spawn_mob(
            &EntityType::CREAKING,
            CreakingEntity::new,
            world,
            &self.position,
            ATTEMPTS_PER_SPAWN,
            SPAWN_RANGE_XZ,
            SPAWN_RANGE_Y,
            SpawnStrategy::OnTopOfColliderNoLeaves,
            true,
        ) else {
            return;
        };
        let entity = creaking.get_entity();
        world.emit_game_event(GameEvent::EntityPlace.name(), entity.pos.load());
        world.broadcast_to_chunk(
            entity.chunk_pos.load(),
            &CEntityStatus::new(entity.entity_id, EVENT_SPAWN_POOF),
        );
        creaking.set_transient(self.position);
        self.set_creaking_uuid(Some(entity.entity_uuid));
        creaking.play_entity_sound(Sound::EntityCreakingSpawn, 1.0, 1.0);
        world.play_sound(
            Sound::BlockCreakingHeartSpawn,
            SoundCategory::Blocks,
            &self.position.to_f64(),
        );
    }

    /// Vanilla `removeProtector(null)`: the creaking crumbles away.
    pub fn remove_protector(&self, world: &World) {
        self.with_protector(world, CreakingEntity::tear_down);
        self.set_creaking_uuid(None);
    }

    /// Vanilla `creakingHurt`: the heart answers with a call and grows resin while it is awake.
    pub fn creaking_hurt(&self, world: &Arc<World>) {
        let Some(creaking_pos) =
            self.with_protector(world, |creaking| creaking.get_entity().pos.load())
        else {
            return;
        };
        if self.emitter.load(Ordering::Relaxed) > 0 {
            return;
        }
        let state_id = world.get_block_state_id(&self.position);
        if CreakingHeartLikeProperties::from_state_id(state_id).creaking_heart_state
            == CreakingHeartState::Awake
        {
            let clumps = rand::rng().random_range(2..=3);
            for _ in 0..clumps {
                if let Some(pos) = self.spread_resin(world) {
                    world.play_sound(Sound::BlockResinPlace, SoundCategory::Blocks, &pos.to_f64());
                    world.emit_game_event(GameEvent::BlockPlace.name(), pos.to_f64());
                }
            }
        }
        self.emitter.store(HURT_CALL_TOTAL_TICKS, Ordering::Relaxed);
        self.emitter_target.store(Some(creaking_pos));
    }

    /// Vanilla `spreadResin`: one clump on a free side of the logs around the heart.
    fn spread_resin(&self, world: &Arc<World>) -> Option<BlockPos> {
        let mut queue = vec![(self.position, 0)];
        let mut seen = vec![self.position];
        let mut index = 0;
        while index < queue.len() && seen.len() <= MAX_COUNT {
            let (pos, depth) = queue[index];
            index += 1;
            let mut directions = ALL_DIRECTIONS;
            directions.shuffle(&mut rand::rng());
            let is_log = world
                .get_block(&pos)
                .has_tag(&tag::Block::MINECRAFT_PALE_OAK_LOGS);
            if is_log && let Some(placed) = place_resin_beside(world, pos, directions) {
                return Some(placed);
            }
            if depth >= MAX_DEPTH {
                continue;
            }
            for direction in directions {
                let neighbour = pos.offset(direction.to_offset());
                if !seen.contains(&neighbour)
                    && world
                        .get_block(&neighbour)
                        .has_tag(&tag::Block::MINECRAFT_PALE_OAK_LOGS)
                {
                    seen.push(neighbour);
                    queue.push((neighbour, depth + 1));
                }
            }
        }
        None
    }
}

/// The inner loop of vanilla `spreadResin`.
fn place_resin_beside(
    world: &Arc<World>,
    log: BlockPos,
    directions: [BlockDirection; 6],
) -> Option<BlockPos> {
    let resin = &Block::RESIN_CLUMP;
    for direction in directions {
        let pos = log.offset(direction.to_offset());
        let state_id = world.get_block_state_id(&pos);
        let block = state_id.to_block();
        let source_water = block == &Block::WATER && state_id == Block::WATER.default_state.id;
        let mut props = if block == resin {
            GlowLichenLikeProperties::from_state_id(state_id)
        } else if state_id.to_state().is_air() || source_water {
            let mut fresh = GlowLichenLikeProperties::default(resin);
            fresh.waterlogged = source_water;
            fresh
        } else {
            continue;
        };
        let face = direction.opposite();
        let has_face = match face {
            BlockDirection::Down => &mut props.down,
            BlockDirection::Up => &mut props.up,
            BlockDirection::North => &mut props.north,
            BlockDirection::South => &mut props.south,
            BlockDirection::West => &mut props.west,
            BlockDirection::East => &mut props.east,
        };
        if *has_face {
            continue;
        }
        *has_face = true;
        world.set_block_state(&pos, props.to_state_id(resin), BlockFlags::NOTIFY_ALL);
        return Some(pos);
    }
    None
}
