//! What villagers do when the bell rings or a raid comes, and how their children play.

use std::sync::Arc;
use std::sync::atomic::Ordering::Relaxed;

use pumpkin_data::Block;
use pumpkin_data::data_component_impl::{
    FireworkExplosionImpl, FireworkExplosionShape, FireworksImpl,
};
use pumpkin_data::dye_color::DyeColor;
use pumpkin_data::entity::EntityType;
use pumpkin_data::environment_attribute::Activity;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::poi_type::PoiType;
use pumpkin_data::sound::Sound;
use pumpkin_data::tag;
use pumpkin_data::tag::Taggable;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_world::chunk::ChunkHeightmapType;
use pumpkin_world::poi::Occupancy;
use rand::RngExt;
use rand::seq::{IndexedRandom, IteratorRandom};

use super::VillagerEntity;
use crate::block::blocks::redstone::bell::ring_bell;
use crate::entity::ai::brain::BrainTick;
use crate::entity::ai::brain::behavior::poi::{bottom_center, closer_to_center_than, global_pos};
use crate::entity::ai::brain::behavior::{Behavior, OneShot};
use crate::entity::ai::brain::memory::position_tracker::EntityTracker;
use crate::entity::ai::brain::memory::walk_target::WalkTarget;
use crate::entity::ai::brain::memory::{MemoryModuleId, MemoryStatus, types};
use crate::entity::ai::util::land_random_pos;
use crate::entity::mob::Mob;
use crate::entity::projectile::firework_rocket::FireworkRocketEntity;
use crate::entity::{Entity, EntityBase};
use crate::world::World;

/// What the behaviours ask of vanilla `ServerLevel.getRaidAt`.
#[derive(Clone, Copy)]
struct RaidView {
    first_wave_spawned: bool,
    between_waves: bool,
    stopped: bool,
    victory: bool,
    loss: bool,
    active: bool,
}

fn raid_at(world: &World, pos: &BlockPos) -> Option<RaidView> {
    let raids = world
        .raids
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    raids.get_raid_at(pos).map(|raid| {
        let first_wave_spawned = raid.get_groups_spawned() > 0;
        RaidView {
            first_wave_spawned,
            between_waves: first_wave_spawned && raid.get_total_raiders_alive() == 0,
            stopped: raid.is_stopped(),
            victory: raid.is_victory(),
            loss: raid.is_loss(),
            active: raid.is_active(),
        }
    })
}

fn body_raid(tick: &BrainTick<'_>) -> Option<RaidView> {
    raid_at(tick.world, &tick.mob.get_entity().block_pos.load())
}

/// Vanilla `ReactToBell`.
pub(super) fn react_to_bell() -> OneShot {
    OneShot::new(
        "ReactToBell",
        vec![(types::HEARD_BELL_TIME.id(), MemoryStatus::ValuePresent)],
        |tick| {
            if body_raid(tick).is_none() {
                tick.state.set_active_activity_if_possible(Activity::Hide);
            }
            true
        },
    )
}

/// Vanilla `SetRaidStatus`.
pub(super) fn set_raid_status() -> OneShot {
    OneShot::new("SetRaidStatus", Vec::new(), |tick| {
        if tick.mob.get_random().random_range(0..20) != 0 {
            return false;
        }
        if let Some(raid) = body_raid(tick) {
            let activity = if raid.first_wave_spawned && !raid.between_waves {
                Activity::Raid
            } else {
                Activity::PreRaid
            };
            tick.state.set_default_activity(activity);
            tick.state.set_active_activity_if_possible(activity);
        }
        true
    })
}

/// Vanilla `ResetRaidStatus`.
pub(super) fn reset_raid_status() -> OneShot {
    OneShot::new("ResetRaidStatus", Vec::new(), |tick| {
        if tick.mob.get_random().random_range(0..20) != 0 {
            return false;
        }
        if body_raid(tick).is_none_or(|raid| raid.stopped || raid.loss) {
            tick.state.set_default_activity(Activity::Idle);
            tick.state
                .update_activity_from_schedule(tick.time, tick.world.get_time_of_day());
        }
        true
    })
}

/// Vanilla `RingBell`.
pub(super) fn ring_bell_behavior() -> OneShot {
    OneShot::new(
        "RingBell",
        vec![(types::MEETING_POINT.id(), MemoryStatus::ValuePresent)],
        |tick| {
            if tick.mob.get_random().random::<f32>() <= 0.95 {
                return false;
            }
            let Some(point) = tick.state.get(types::MEETING_POINT).copied() else {
                return false;
            };
            let block_pos = tick.mob.get_entity().block_pos.load();
            let delta = point.pos.0 - block_pos.0;
            let near =
                i64::from(delta.x).pow(2) + i64::from(delta.y).pow(2) + i64::from(delta.z).pow(2)
                    < 9;
            if near && tick.world.get_block(&point.pos) == &Block::BELL {
                ring_bell(point.pos, tick.world, None, None);
            }
            true
        },
    )
}

/// Vanilla `MoveToSkySeeingSpot.hasNoBlocksAbove`.
fn has_no_blocks_above(tick: &BrainTick<'_>, target: &BlockPos) -> bool {
    tick.world.can_see_sky(target)
        && f64::from(tick.world.get_heightmap_height(
            ChunkHeightmapType::MotionBlocking,
            target.0.x,
            target.0.z,
        )) <= tick.mob.get_entity().pos.load().y
}

/// Vanilla `MoveToSkySeeingSpot`.
pub(super) fn move_to_sky_seeing_spot(speed_modifier: f32) -> OneShot {
    OneShot::new(
        "MoveToSkySeeingSpot",
        vec![(types::WALK_TARGET.id(), MemoryStatus::ValueAbsent)],
        move |tick| {
            let pos = tick.mob.get_entity().block_pos.load();
            if tick.world.can_see_sky(&pos) {
                return false;
            }
            let mut rng = tick.mob.get_random();
            for _ in 0..10 {
                let candidate = BlockPos::new(
                    pos.0.x + rng.random_range(-10..10),
                    pos.0.y + rng.random_range(-3..3),
                    pos.0.z + rng.random_range(-10..10),
                );
                if has_no_blocks_above(tick, &candidate) {
                    tick.state.set(
                        types::WALK_TARGET,
                        WalkTarget::from_vec(bottom_center(candidate), speed_modifier, 0),
                    );
                    break;
                }
            }
            true
        },
    )
}

/// Vanilla `LocateHidingPlace`.
pub(super) fn locate_hiding_place(
    radius: i32,
    speed_modifier: f32,
    close_enough_dist: i32,
) -> OneShot {
    OneShot::new(
        "LocateHidingPlace",
        vec![
            (types::WALK_TARGET.id(), MemoryStatus::ValueAbsent),
            (types::HOME.id(), MemoryStatus::Registered),
            (types::HIDING_PLACE.id(), MemoryStatus::Registered),
            (types::PATH.id(), MemoryStatus::Registered),
            (types::LOOK_TARGET.id(), MemoryStatus::Registered),
            (types::BREED_TARGET.id(), MemoryStatus::Registered),
            (types::INTERACTION_TARGET.id(), MemoryStatus::Registered),
        ],
        move |tick| {
            let position = tick.mob.get_entity().pos.load();
            let origin = tick.mob.get_entity().block_pos.load();
            let is_home = |poi: &'static PoiType| poi == &PoiType::HOME;
            let near = tick
                .world
                .closest_poi(origin, close_enough_dist + 1, is_home, Occupancy::Any)
                .filter(|pos| closer_to_center_than(*pos, position, f64::from(close_enough_dist)));
            let place = near
                .or_else(|| {
                    tick.world
                        .pois_in_range(origin, radius, is_home, Occupancy::Any)
                        .choose(&mut tick.mob.get_random())
                        .map(|(pos, _)| *pos)
                })
                .or_else(|| tick.state.get(types::HOME).map(|home| home.pos));
            if let Some(place) = place {
                tick.state.erase(types::PATH.id());
                tick.state.erase(types::LOOK_TARGET.id());
                tick.state.erase(types::BREED_TARGET.id());
                tick.state.erase(types::INTERACTION_TARGET.id());
                tick.state
                    .set(types::HIDING_PLACE, global_pos(tick.world, place));
                if !closer_to_center_than(place, position, f64::from(close_enough_dist)) {
                    tick.state.set(
                        types::WALK_TARGET,
                        WalkTarget::from_block_pos(place, speed_modifier, close_enough_dist),
                    );
                }
            }
            true
        },
    )
}

/// Vanilla `SetHiddenState`.
pub(super) fn set_hidden_state(seconds: i32, close_enough_dist: i32) -> OneShot {
    const HIDE_TIMEOUT: i64 = 300;
    let stay_hidden_ticks = seconds * 20;
    let mut ticks_hidden = 0;
    OneShot::new(
        "SetHiddenState",
        vec![
            (types::HIDING_PLACE.id(), MemoryStatus::ValuePresent),
            (types::HEARD_BELL_TIME.id(), MemoryStatus::ValuePresent),
        ],
        move |tick| {
            let timed_out = tick
                .state
                .get(types::HEARD_BELL_TIME)
                .is_some_and(|heard| heard + HIDE_TIMEOUT <= tick.time);
            if ticks_hidden <= stay_hidden_ticks && !timed_out {
                if let Some(place) = tick.state.get(types::HIDING_PLACE) {
                    let delta = place.pos.0 - tick.mob.get_entity().block_pos.load().0;
                    let distance_sqr = i64::from(delta.x).pow(2)
                        + i64::from(delta.y).pow(2)
                        + i64::from(delta.z).pow(2);
                    if distance_sqr < i64::from(close_enough_dist).pow(2) {
                        ticks_hidden += 1;
                    }
                }
            } else {
                tick.state.erase(types::HEARD_BELL_TIME.id());
                tick.state.erase(types::HIDING_PLACE.id());
                tick.state
                    .update_activity_from_schedule(tick.time, tick.world.get_time_of_day());
                ticks_hidden = 0;
            }
            true
        },
    )
}

/// The raid package's first step: the raid is won (`raidExistsAndNotVictory`, sic).
pub(super) fn raid_is_won(tick: &mut BrainTick<'_>) -> bool {
    body_raid(tick).is_some_and(|raid| raid.victory)
}

/// The raid package's hiding condition (`raidExistsAndActive`).
pub(super) fn raid_is_running(tick: &mut BrainTick<'_>) -> bool {
    body_raid(tick).is_some_and(|raid| raid.active && !raid.victory && !raid.loss)
}

/// Vanilla `CelebrateVillagersSurvivedRaid`.
#[derive(Default)]
pub(super) struct CelebrateVillagersSurvivedRaid;

impl CelebrateVillagersSurvivedRaid {
    fn firework(color: DyeColor, flight_duration: i32) -> ItemStack {
        let mut rocket = ItemStack::new(1, &Item::FIREWORK_ROCKET);
        rocket.set_data_component(FireworksImpl::new(
            flight_duration,
            vec![FireworkExplosionImpl::new(
                FireworkExplosionShape::Burst,
                vec![color.firework_color() as i32],
                Vec::new(),
                false,
                false,
            )],
        ));
        rocket
    }
}

const NO_CONDITIONS: [(MemoryModuleId, MemoryStatus); 0] = [];

impl Behavior for CelebrateVillagersSurvivedRaid {
    fn entry_conditions(&self) -> &[(MemoryModuleId, MemoryStatus)] {
        &NO_CONDITIONS
    }

    fn min_duration(&self) -> i32 {
        600
    }

    fn max_duration(&self) -> i32 {
        600
    }

    fn check_extra_start_conditions(&mut self, tick: &mut BrainTick<'_>) -> bool {
        let pos = tick.mob.get_entity().block_pos.load();
        raid_at(tick.world, &pos).is_some_and(|raid| raid.victory)
            && has_no_blocks_above(tick, &pos)
    }

    fn can_still_use(&mut self, tick: &mut BrainTick<'_>) -> bool {
        body_raid(tick).is_some_and(|raid| !raid.stopped)
    }

    fn stop(&mut self, tick: &mut BrainTick<'_>) {
        tick.state
            .update_activity_from_schedule(tick.time, tick.world.get_time_of_day());
    }

    fn tick(&mut self, tick: &mut BrainTick<'_>) {
        let mut rng = tick.mob.get_random();
        let entity = tick.mob.get_entity();
        if rng.random_range(0..100) == 0 {
            entity.play_sound(Sound::EntityVillagerCelebrate);
        }
        let pos = entity.block_pos.load();
        if rng.random_range(0..200) == 0 && has_no_blocks_above(tick, &pos) {
            let color = DyeColor::ALL
                .choose(&mut rng)
                .copied()
                .unwrap_or(DyeColor::White);
            let flight_duration = rng.random_range(0..3);
            let position = entity.pos.load();
            let rocket = FireworkRocketEntity::new_shot_at_angle(
                Entity::new(
                    tick.world.clone(),
                    Vector3::new(position.x, entity.get_eye_y(), position.z),
                    &EntityType::FIREWORK_ROCKET,
                ),
                entity,
                Self::firework(color, flight_duration),
            );
            tick.world.spawn_entity(Arc::new(rocket));
        }
    }

    fn debug_name(&self) -> &'static str {
        "CelebrateVillagersSurvivedRaid"
    }
}

/// Vanilla `JumpOnBed`.
pub(super) struct JumpOnBed {
    speed_modifier: f32,
    target_bed: Option<BlockPos>,
    remaining_time_to_reach_bed: i32,
    remaining_jumps: i32,
    remaining_cooldown_until_next_jump: i32,
}

const JUMP_ON_BED_CONDITIONS: [(MemoryModuleId, MemoryStatus); 2] = [
    (types::NEAREST_BED.id(), MemoryStatus::ValuePresent),
    (types::WALK_TARGET.id(), MemoryStatus::ValueAbsent),
];

fn is_jumpable(world: &World, pos: &BlockPos) -> bool {
    world
        .get_block(pos)
        .has_tag(&tag::Block::MINECRAFT_VILLAGER_BABIES_CAN_JUMP_ON_BED)
}

fn is_baby(mob: &dyn Mob) -> bool {
    mob.get_entity().age.load(Relaxed) < 0
}

impl JumpOnBed {
    pub(super) const fn new(speed_modifier: f32) -> Self {
        Self {
            speed_modifier,
            target_bed: None,
            remaining_time_to_reach_bed: 0,
            remaining_jumps: 0,
            remaining_cooldown_until_next_jump: 0,
        }
    }

    fn on_or_over_bed(tick: &BrainTick<'_>) -> bool {
        let pos = tick.mob.get_entity().block_pos.load();
        is_jumpable(tick.world, &pos) || is_jumpable(tick.world, &pos.down())
    }
}

impl Behavior for JumpOnBed {
    fn entry_conditions(&self) -> &[(MemoryModuleId, MemoryStatus)] {
        &JUMP_ON_BED_CONDITIONS
    }

    // Vanilla `timedOut`: never.
    fn min_duration(&self) -> i32 {
        i32::MAX
    }

    fn max_duration(&self) -> i32 {
        i32::MAX
    }

    fn check_extra_start_conditions(&mut self, tick: &mut BrainTick<'_>) -> bool {
        is_baby(tick.mob)
            && (Self::on_or_over_bed(tick) || tick.state.has_memory_value(types::NEAREST_BED.id()))
    }

    fn start(&mut self, tick: &mut BrainTick<'_>) {
        if let Some(bed) = tick.state.get(types::NEAREST_BED).copied() {
            self.target_bed = Some(bed);
            self.remaining_time_to_reach_bed = 100;
            self.remaining_jumps = 3 + tick.mob.get_random().random_range(0..4);
            self.remaining_cooldown_until_next_jump = 0;
            tick.state.set(
                types::WALK_TARGET,
                WalkTarget::from_block_pos(bed, self.speed_modifier, 0),
            );
        }
    }

    fn stop(&mut self, _tick: &mut BrainTick<'_>) {
        self.target_bed = None;
        self.remaining_time_to_reach_bed = 0;
        self.remaining_jumps = 0;
        self.remaining_cooldown_until_next_jump = 0;
    }

    fn can_still_use(&mut self, tick: &mut BrainTick<'_>) -> bool {
        let on_bed = Self::on_or_over_bed(tick);
        is_baby(tick.mob)
            && self
                .target_bed
                .is_some_and(|bed| is_jumpable(tick.world, &bed))
            && (on_bed || self.remaining_time_to_reach_bed > 0)
            && (!on_bed || self.remaining_jumps > 0)
    }

    fn tick(&mut self, tick: &mut BrainTick<'_>) {
        if !Self::on_or_over_bed(tick) {
            self.remaining_time_to_reach_bed -= 1;
        } else if self.remaining_cooldown_until_next_jump > 0 {
            self.remaining_cooldown_until_next_jump -= 1;
        } else if is_jumpable(tick.world, &tick.mob.get_entity().block_pos.load()) {
            tick.mob
                .get_mob_entity()
                .jump_control
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .jump();
            self.remaining_jumps -= 1;
            self.remaining_cooldown_until_next_jump = 5;
        }
    }

    fn debug_name(&self) -> &'static str {
        "JumpOnBed"
    }
}

/// Who a friend is chasing, read from its brain.
fn chases(friend: &Arc<dyn EntityBase>) -> Option<i32> {
    friend
        .cast_any()
        .downcast_ref::<VillagerEntity>()?
        .with_other_brain(|brain| {
            brain
                .get(types::INTERACTION_TARGET)
                .map(|target| target.get_entity().entity_id)
        })?
}

fn chase_kid(tick: &mut BrainTick<'_>, kid: Arc<dyn EntityBase>) {
    tick.state.set(types::INTERACTION_TARGET, kid.clone());
    tick.state.set(
        types::LOOK_TARGET,
        Arc::new(EntityTracker::new(kid.clone(), true)),
    );
    tick.state.set(
        types::WALK_TARGET,
        WalkTarget::from_entity(kid, FLEE_AND_CHASE_SPEED, 1),
    );
}

/// Vanilla `PlayTagWithOtherKids.FLEE_SPEED_MODIFIER` and `CHASE_SPEED_MODIFIER`.
const FLEE_AND_CHASE_SPEED: f32 = 0.6;
/// Vanilla `PlayTagWithOtherKids.MAX_CHASERS_PER_TARGET`.
const MAX_CHASERS_PER_TARGET: usize = 5;

/// Vanilla `PlayTagWithOtherKids`.
pub(super) fn play_tag_with_other_kids() -> OneShot {
    OneShot::new(
        "PlayTagWithOtherKids",
        vec![
            (
                types::VISIBLE_VILLAGER_BABIES.id(),
                MemoryStatus::ValuePresent,
            ),
            (types::WALK_TARGET.id(), MemoryStatus::ValueAbsent),
            (types::LOOK_TARGET.id(), MemoryStatus::Registered),
            (types::INTERACTION_TARGET.id(), MemoryStatus::Registered),
        ],
        |tick| {
            if tick.mob.get_random().random_range(0..10) != 0 {
                return false;
            }
            let friends = tick
                .state
                .get(types::VISIBLE_VILLAGER_BABIES)
                .cloned()
                .unwrap_or_default();
            let me = tick.mob.get_entity().entity_id;
            let targets: Vec<(Arc<dyn EntityBase>, Option<i32>)> = friends
                .iter()
                .map(|friend| (friend.clone(), chases(friend)))
                .collect();
            if targets.iter().any(|(_, target)| *target == Some(me)) {
                for _ in 0..10 {
                    if let Some(pos) = land_random_pos::get_pos(tick.mob, 20, 8)
                        && tick.world.is_village(&pos.to_block_pos())
                    {
                        tick.state.set(
                            types::WALK_TARGET,
                            WalkTarget::from_vec(pos, FLEE_AND_CHASE_SPEED, 0),
                        );
                        break;
                    }
                }
                return true;
            }
            // The kid being chased by the fewest, as `findSomeoneBeingChased`.
            let being_chased = friends
                .iter()
                .filter_map(|friend| {
                    let id = friend.get_entity().entity_id;
                    let chasers = targets
                        .iter()
                        .filter(|(_, target)| *target == Some(id))
                        .count();
                    (chasers > 0 && chasers <= MAX_CHASERS_PER_TARGET)
                        .then_some((chasers, friend.clone()))
                })
                .min_by_key(|(chasers, _)| *chasers)
                .map(|(_, friend)| friend);
            let kid =
                being_chased.or_else(|| friends.iter().choose(&mut tick.mob.get_random()).cloned());
            if let Some(kid) = kid {
                chase_kid(tick, kid);
            }
            true
        },
    )
}
