//! The brain behaviours around points of interest: claiming one, checking it is still there and
//! strolling around it.

use std::sync::atomic::Ordering::Relaxed;

use pumpkin_data::dimension::Dimension;
use pumpkin_data::poi_type::PoiType;
use pumpkin_data::tag;
use pumpkin_data::tag::Taggable;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_world::poi::Occupancy;
use rand::RngExt;
use rustc_hash::FxHashMap;

use super::super::memory::global_pos::GlobalPos;
use super::super::memory::walk_target::WalkTarget;
use super::super::memory::{MemoryModuleType, MemoryStatus, types};
use super::OneShot;
use crate::entity::ai::pathfinder::path::Path;
use crate::entity::ai::util::{default_random_pos, land_random_pos};
use crate::entity::mob::Mob;
use crate::world::World;
use pumpkin_data::block_properties::WhiteBedLikeProperties as BedProperties;

/// Vanilla `AcquirePoi.SCAN_RANGE`.
pub const SCAN_RANGE: i32 = 48;
/// Vanilla `AcquirePoi`'s `batchSize`.
const BATCH_SIZE: usize = 5;
/// Vanilla `AcquirePoi`'s `rate`.
const RATE: i64 = 20;
/// Vanilla `ValidateNearbyPoi.MAX_DISTANCE`.
const VALIDATE_DISTANCE: f64 = 16.0;

/// Which points of interest a behaviour cares about, asked of the mob.
pub type PoiPredicate = fn(&dyn Mob, &'static PoiType) -> bool;

/// `pos` in the world's dimension.
#[must_use]
pub fn global_pos(world: &World, pos: BlockPos) -> GlobalPos {
    GlobalPos::new(
        Dimension::from_name(world.dimension.minecraft_name).unwrap_or(&Dimension::OVERWORLD),
        pos,
    )
}

#[must_use]
pub const fn is_in_dimension(world: &World, pos: &GlobalPos) -> bool {
    pos.dimension.id == world.dimension.id
}

/// Vanilla `BlockPos.closerToCenterThan`.
#[must_use]
pub fn closer_to_center_than(pos: BlockPos, position: Vector3<f64>, distance: f64) -> bool {
    pos.to_centered_f64().squared_distance_to_vec(&position) < distance * distance
}

/// Vanilla `Vec3.atBottomCenterOf`.
#[must_use]
pub fn bottom_center(pos: BlockPos) -> Vector3<f64> {
    Vector3::new(
        f64::from(pos.0.x) + 0.5,
        f64::from(pos.0.y),
        f64::from(pos.0.z) + 0.5,
    )
}

/// Vanilla `AcquirePoi.JitteredLinearRetry`.
struct JitteredLinearRetry {
    previous_attempt: i64,
    next_scheduled_attempt: i64,
    current_delay: i64,
}

impl JitteredLinearRetry {
    const MIN_INTERVAL_INCREASE: i64 = 40;
    const MAX_INTERVAL_INCREASE: i64 = 80;
    const MAX_RETRY_PATHFINDING_INTERVAL: i64 = 400;

    fn new(mob: &dyn Mob, time: i64) -> Self {
        let mut retry = Self {
            previous_attempt: 0,
            next_scheduled_attempt: 0,
            current_delay: 0,
        };
        retry.mark_attempt(mob, time);
        retry
    }

    fn mark_attempt(&mut self, mob: &dyn Mob, time: i64) {
        self.previous_attempt = time;
        let suggested = self.current_delay
            + mob
                .get_random()
                .random_range(Self::MIN_INTERVAL_INCREASE..Self::MAX_INTERVAL_INCREASE);
        self.current_delay = suggested.min(Self::MAX_RETRY_PATHFINDING_INTERVAL);
        self.next_scheduled_attempt = time + self.current_delay;
    }

    const fn is_still_valid(&self, time: i64) -> bool {
        time - self.previous_attempt < Self::MAX_RETRY_PATHFINDING_INTERVAL
    }

    const fn should_retry(&self, time: i64) -> bool {
        time >= self.next_scheduled_attempt
    }
}

/// Vanilla `AcquirePoi.findPathToPois`.
#[must_use]
pub fn find_path_to_pois(mob: &dyn Mob, pois: &[(BlockPos, &'static PoiType)]) -> Option<Path> {
    if pois.is_empty() {
        return None;
    }
    let max_range = pois
        .iter()
        .fold(1, |range, (_, kind)| range.max(kind.valid_range as i32));
    let targets: Vec<BlockPos> = pois.iter().map(|(pos, _)| *pos).collect();
    let steered = mob.get_mob_entity().steered();
    let owner = &steered.mob_entity().living_entity;
    steered
        .navigator()
        .create_path_to_any(owner, &targets, max_range)
}

/// What [`acquire_poi`] claims.
#[derive(Clone, Copy)]
pub struct AcquirePoi {
    pub kind: PoiPredicate,
    /// The behaviour only runs while this memory is empty as well.
    pub memory_to_validate: MemoryModuleType<GlobalPos>,
    pub memory_to_acquire: MemoryModuleType<GlobalPos>,
    pub only_if_adult: bool,
    /// Tells the mob's watchers it got one, as vanilla's `onPoiAcquisitionEvent`.
    pub on_acquired: Option<fn(&dyn Mob)>,
    pub valid: fn(&World, &BlockPos) -> bool,
}

/// Vanilla `AcquirePoi`: every second or so, walks the five closest free points of interest it
/// has not given up on, and takes the ticket of one it can reach.
#[must_use]
pub fn acquire_poi(config: AcquirePoi) -> OneShot {
    let mut next_scheduled_start = 0;
    let mut batch_cache: FxHashMap<BlockPos, JitteredLinearRetry> = FxHashMap::default();
    let mut conditions = vec![(config.memory_to_acquire.id(), MemoryStatus::ValueAbsent)];
    if config.memory_to_validate.id() != config.memory_to_acquire.id() {
        conditions.push((config.memory_to_validate.id(), MemoryStatus::ValueAbsent));
    }
    OneShot::new("AcquirePoi", conditions, move |tick| {
        if config.only_if_adult && tick.mob.get_entity().age.load(Relaxed) < 0 {
            return false;
        }
        if next_scheduled_start == 0 {
            next_scheduled_start = tick.time + tick.mob.get_random().random_range(0..RATE);
            return false;
        }
        if tick.time < next_scheduled_start {
            return false;
        }
        next_scheduled_start = tick.time + RATE + tick.mob.get_random().random_range(0..RATE);
        batch_cache.retain(|_, retry| retry.is_still_valid(tick.time));

        let mob = tick.mob;
        let world = tick.world;
        let origin = mob.get_entity().block_pos.load();
        let mut pois = world.pois_in_range(
            origin,
            SCAN_RANGE,
            |poi| (config.kind)(mob, poi),
            Occupancy::HasSpace,
        );
        pois.sort_by_key(|(pos, _)| squared_distance(*pos, origin));
        let mut chosen = Vec::with_capacity(BATCH_SIZE);
        for poi in pois {
            let retry_now = batch_cache.get_mut(&poi.0).is_none_or(|retry| {
                let due = retry.should_retry(tick.time);
                if due {
                    retry.mark_attempt(mob, tick.time);
                }
                due
            });
            if retry_now {
                chosen.push(poi);
                if chosen.len() == BATCH_SIZE {
                    break;
                }
            }
        }
        chosen.retain(|(pos, _)| (config.valid)(world, pos));

        let path = find_path_to_pois(mob, &chosen).filter(Path::can_reach);
        if let Some(path) = path {
            let target = path.get_target();
            if world.poi_type_at(target).is_some() {
                world.take_poi(target);
                tick.state
                    .set(config.memory_to_acquire, global_pos(world, target));
                if let Some(on_acquired) = config.on_acquired {
                    on_acquired(mob);
                }
                batch_cache.clear();
            }
        } else {
            for (pos, _) in chosen {
                batch_cache
                    .entry(pos)
                    .or_insert_with(|| JitteredLinearRetry::new(mob, tick.time));
            }
        }
        true
    })
}

fn squared_distance(a: BlockPos, b: BlockPos) -> i64 {
    let delta = a.0 - b.0;
    i64::from(delta.x).pow(2) + i64::from(delta.y).pow(2) + i64::from(delta.z).pow(2)
}

/// The bed at `pos` is taken, by someone else than this sleeper.
fn bed_is_occupied(world: &World, pos: &BlockPos, mob: &dyn Mob) -> bool {
    let (block, state) = world.get_block_and_state(pos);
    block.has_tag(&tag::Block::MINECRAFT_VILLAGERS_CAN_SLEEP_ON_BED)
        && BedProperties::from_state_id(state.id).occupied
        && !mob.get_mob_entity().living_entity.is_sleeping()
}

/// Vanilla `ValidateNearbyPoi.bedIsOccupiedByVillager`.
fn bed_is_occupied_by_villager(world: &World, pos: &BlockPos) -> bool {
    let bed = pumpkin_util::math::boundingbox::BoundingBox::new(
        pos.0.to_f64(),
        pos.0.to_f64().add_raw(1.0, 1.0, 1.0),
    );
    world.get_all_at_box(&bed).iter().any(|entity| {
        entity.get_entity().entity_type == &pumpkin_data::entity::EntityType::VILLAGER
            && entity
                .get_living_entity()
                .is_some_and(crate::entity::living::LivingEntity::is_sleeping)
    })
}

/// Vanilla `ValidateNearbyPoi`: forgets a point of interest within 16 blocks that is gone, and a
/// bed somebody else lies in.
#[must_use]
pub fn validate_nearby_poi(kind: PoiPredicate, memory: MemoryModuleType<GlobalPos>) -> OneShot {
    OneShot::new(
        "ValidateNearbyPoi",
        vec![(memory.id(), MemoryStatus::ValuePresent)],
        move |tick| {
            let Some(remembered) = tick.state.get(memory).copied() else {
                return false;
            };
            let pos = remembered.pos;
            let position = tick.mob.get_entity().pos.load();
            if !is_in_dimension(tick.world, &remembered)
                || !closer_to_center_than(pos, position, VALIDATE_DISTANCE)
            {
                return false;
            }
            let exists = tick
                .world
                .poi_type_at(pos)
                .is_some_and(|poi| kind(tick.mob, poi));
            if !exists {
                tick.state.erase(memory.id());
            } else if bed_is_occupied(tick.world, &pos, tick.mob) {
                tick.state.erase(memory.id());
                if !bed_is_occupied_by_villager(tick.world, &pos) {
                    tick.world.release_poi(pos);
                }
            }
            true
        },
    )
}

/// Vanilla `StrollAroundPoi`.
#[must_use]
pub fn stroll_around_poi(
    memory: MemoryModuleType<GlobalPos>,
    speed_modifier: f32,
    max_distance_from_poi: i32,
) -> OneShot {
    const MIN_TIME_BETWEEN_STROLLS: i64 = 180;
    const STROLL_MAX_XZ_DIST: i32 = 8;
    const STROLL_MAX_Y_DIST: i32 = 6;
    let mut next_ok_start_time = 0;
    OneShot::new(
        "StrollAroundPoi",
        vec![
            (types::WALK_TARGET.id(), MemoryStatus::Registered),
            (memory.id(), MemoryStatus::ValuePresent),
        ],
        move |tick| {
            let Some(pos) = tick.state.get(memory).copied() else {
                return false;
            };
            let position = tick.mob.get_entity().pos.load();
            if !is_in_dimension(tick.world, &pos)
                || !closer_to_center_than(pos.pos, position, f64::from(max_distance_from_poi))
            {
                return false;
            }
            if tick.time <= next_ok_start_time {
                return true;
            }
            let target = land_random_pos::get_pos(tick.mob, STROLL_MAX_XZ_DIST, STROLL_MAX_Y_DIST);
            tick.state.set_optional(
                types::WALK_TARGET,
                target.map(|target| WalkTarget::from_vec(target, speed_modifier, 1)),
            );
            next_ok_start_time = tick.time + MIN_TIME_BETWEEN_STROLLS;
            true
        },
    )
}

/// Vanilla `StrollToPoi`.
#[must_use]
pub fn stroll_to_poi(
    memory: MemoryModuleType<GlobalPos>,
    speed_modifier: f32,
    close_enough_dist: i32,
    max_distance_from_poi: i32,
) -> OneShot {
    let mut next_ok_start_time = 0;
    OneShot::new(
        "StrollToPoi",
        vec![
            (types::WALK_TARGET.id(), MemoryStatus::Registered),
            (memory.id(), MemoryStatus::ValuePresent),
        ],
        move |tick| {
            let Some(pos) = tick.state.get(memory).copied() else {
                return false;
            };
            let position = tick.mob.get_entity().pos.load();
            if !is_in_dimension(tick.world, &pos)
                || !closer_to_center_than(pos.pos, position, f64::from(max_distance_from_poi))
            {
                return false;
            }
            if tick.time <= next_ok_start_time {
                return true;
            }
            tick.state.set(
                types::WALK_TARGET,
                WalkTarget::from_block_pos(pos.pos, speed_modifier, close_enough_dist),
            );
            next_ok_start_time = tick.time + 80;
            true
        },
    )
}

/// Vanilla `StrollToPoiList`.
#[must_use]
pub fn stroll_to_poi_list(
    stroll_to: MemoryModuleType<Vec<GlobalPos>>,
    speed_modifier: f32,
    close_enough_dist: i32,
    max_distance_from_poi: i32,
    must_be_close_to: MemoryModuleType<GlobalPos>,
) -> OneShot {
    let mut next_ok_start_time = 0;
    OneShot::new(
        "StrollToPoiList",
        vec![
            (types::WALK_TARGET.id(), MemoryStatus::Registered),
            (stroll_to.id(), MemoryStatus::ValuePresent),
            (must_be_close_to.id(), MemoryStatus::ValuePresent),
        ],
        move |tick| {
            let Some(stay_close_to) = tick.state.get(must_be_close_to).copied() else {
                return false;
            };
            let target = tick.state.get(stroll_to).and_then(|sites| {
                (!sites.is_empty())
                    .then(|| sites[tick.mob.get_random().random_range(0..sites.len())])
            });
            let Some(target) = target else {
                return false;
            };
            let position = tick.mob.get_entity().pos.load();
            if !is_in_dimension(tick.world, &target)
                || !closer_to_center_than(
                    stay_close_to.pos,
                    position,
                    f64::from(max_distance_from_poi),
                )
            {
                return false;
            }
            if tick.time > next_ok_start_time {
                tick.state.set(
                    types::WALK_TARGET,
                    WalkTarget::from_block_pos(target.pos, speed_modifier, close_enough_dist),
                );
                next_ok_start_time = tick.time + 100;
            }
            true
        },
    )
}

/// How far [`set_walk_target_from_block_memory`] goes and when it gives up.
#[derive(Clone, Copy)]
pub struct BlockMemoryWalk {
    pub speed_modifier: f32,
    pub close_enough_dist: i32,
    pub too_far_distance: i32,
    pub too_long_unreachable_duration: i64,
}

const fn manhattan(a: BlockPos, b: BlockPos) -> i32 {
    (a.0.x - b.0.x).abs() + (a.0.y - b.0.y).abs() + (a.0.z - b.0.z).abs()
}

/// Vanilla `SetWalkTargetFromBlockMemory`: walks to the remembered place; a place out of reach
/// for too long is given up, with `release` telling the mob.
#[must_use]
pub fn set_walk_target_from_block_memory(
    memory: MemoryModuleType<GlobalPos>,
    walk: BlockMemoryWalk,
    release: fn(&dyn Mob, &World, &GlobalPos),
) -> OneShot {
    const MAX_TRIES: i32 = 1000;
    OneShot::new(
        "SetWalkTargetFromBlockMemory",
        vec![
            (
                types::CANT_REACH_WALK_TARGET_SINCE.id(),
                MemoryStatus::Registered,
            ),
            (types::WALK_TARGET.id(), MemoryStatus::ValueAbsent),
            (memory.id(), MemoryStatus::ValuePresent),
        ],
        move |tick| {
            let Some(target) = tick.state.get(memory).copied() else {
                return false;
            };
            let unreachable_too_long = tick
                .state
                .get(types::CANT_REACH_WALK_TARGET_SINCE)
                .is_some_and(|since| tick.time - since > walk.too_long_unreachable_duration);
            let give_up = |tick: &mut super::super::BrainTick<'_>| {
                release(tick.mob, tick.world, &target);
                tick.state.erase(memory.id());
                tick.state
                    .set(types::CANT_REACH_WALK_TARGET_SINCE, tick.time);
            };
            if !is_in_dimension(tick.world, &target) || unreachable_too_long {
                give_up(tick);
                return true;
            }
            let block_pos = tick.mob.get_entity().block_pos.load();
            if manhattan(target.pos, block_pos) > walk.too_far_distance {
                let towards = (0..MAX_TRIES).find_map(|_| {
                    default_random_pos::get_pos_towards(
                        tick.mob,
                        15,
                        7,
                        bottom_center(target.pos),
                        std::f64::consts::FRAC_PI_2,
                    )
                    .filter(|pos| manhattan(pos.to_block_pos(), block_pos) <= walk.too_far_distance)
                });
                let Some(towards) = towards else {
                    give_up(tick);
                    return true;
                };
                tick.state.set(
                    types::WALK_TARGET,
                    WalkTarget::from_vec(towards, walk.speed_modifier, walk.close_enough_dist),
                );
            } else if manhattan(target.pos, block_pos) > walk.close_enough_dist {
                tick.state.set(
                    types::WALK_TARGET,
                    WalkTarget::from_block_pos(
                        target.pos,
                        walk.speed_modifier,
                        walk.close_enough_dist,
                    ),
                );
            }
            true
        },
    )
}

/// Vanilla `SetClosestHomeAsWalkTarget`'s and `NearestBedSensor`'s batch of beds: up to four
/// not tried in the last two seconds.
pub struct HomeBatch {
    cache: FxHashMap<BlockPos, i64>,
    last_update: i64,
}

impl HomeBatch {
    const CACHE_TIMEOUT: i64 = 40;
    const BATCH_SIZE: usize = 4;
    const RATE: i64 = 20;

    #[must_use]
    pub fn new() -> Self {
        Self {
            cache: FxHashMap::default(),
            last_update: 0,
        }
    }

    /// The target of a reachable bed; `None` while none is known or waiting out the rate.
    pub fn find_reachable_home(
        &mut self,
        mob: &dyn Mob,
        world: &World,
        time: i64,
    ) -> Option<BlockPos> {
        self.last_update = time + mob.get_random().random_range(0..Self::RATE);
        let origin = mob.get_entity().block_pos.load();
        let last_update = self.last_update;
        let mut pois = world.pois_in_range(
            origin,
            SCAN_RANGE,
            |poi| poi == &PoiType::HOME,
            Occupancy::Any,
        );
        pois.retain(|(pos, _)| !self.cache.contains_key(pos));
        let exhausted = pois.len() >= Self::BATCH_SIZE;
        pois.truncate(Self::BATCH_SIZE);
        for (pos, _) in &pois {
            self.cache.insert(*pos, last_update + Self::CACHE_TIMEOUT);
        }
        let path = find_path_to_pois(mob, &pois).filter(Path::can_reach);
        if path.is_none() && !exhausted {
            self.cache.retain(|_, expires| *expires >= last_update);
        }
        path.map(|path| path.get_target())
    }

    /// Whether the rate lets a search run at `time`.
    #[must_use]
    pub const fn may_update(&self, time: i64) -> bool {
        time - self.last_update >= Self::RATE
    }
}

impl Default for HomeBatch {
    fn default() -> Self {
        Self::new()
    }
}

/// Vanilla `SetClosestHomeAsWalkTarget`.
#[must_use]
pub fn set_closest_home_as_walk_target(speed_modifier: f32) -> OneShot {
    let mut batch = HomeBatch::new();
    OneShot::new(
        "SetClosestHomeAsWalkTarget",
        vec![
            (types::WALK_TARGET.id(), MemoryStatus::ValueAbsent),
            (types::HOME.id(), MemoryStatus::ValueAbsent),
        ],
        move |tick| {
            if !batch.may_update(tick.time) {
                return false;
            }
            let origin = tick.mob.get_entity().block_pos.load();
            let closest = tick.world.closest_poi(
                origin,
                SCAN_RANGE,
                |poi| poi == &PoiType::HOME,
                Occupancy::Any,
            );
            if closest.is_none_or(|bed| squared_distance(bed, origin) <= 4) {
                return false;
            }
            if let Some(target) = batch.find_reachable_home(tick.mob, tick.world, tick.time) {
                tick.state.set(
                    types::WALK_TARGET,
                    WalkTarget::from_block_pos(target, speed_modifier, 1),
                );
            }
            true
        },
    )
}
