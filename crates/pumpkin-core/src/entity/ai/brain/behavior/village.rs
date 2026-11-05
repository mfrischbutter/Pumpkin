//! The brain behaviours of mobs that live in villages: following the day's schedule, sleeping,
//! strolling, talking and running from danger.

use std::sync::Arc;

use pumpkin_data::block_properties::WhiteBedLikeProperties as BedProperties;
use pumpkin_data::entity::{EntityPose, EntityType};
use pumpkin_data::environment_attribute::Activity;
use pumpkin_data::tag;
use pumpkin_data::tag::Taggable;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_world::poi::section_of;
use rand::RngExt;
use rand::seq::SliceRandom;

use super::super::BrainTick;
use super::super::memory::position_tracker::EntityTracker;
use super::super::memory::walk_target::WalkTarget;
use super::super::memory::{MemoryModuleId, MemoryModuleType, MemoryStatus, types};
use super::interact_with_door;
use super::poi::{bottom_center, closer_to_center_than};
use super::{Behavior, OneShot};
use crate::block::blocks::bed::BedBlock;
use crate::entity::EntityBase;
use crate::entity::ai::util::{default_random_pos, land_random_pos};
use crate::entity::mob::Mob;

/// Vanilla `UpdateActivityFromSchedule`.
#[must_use]
pub fn update_activity_from_schedule() -> OneShot {
    OneShot::new("UpdateActivityFromSchedule", Vec::new(), |tick| {
        tick.state
            .update_activity_from_schedule(tick.time, tick.world.get_time_of_day());
        true
    })
}

/// Vanilla `LivingEntity.startSleeping`: the mob lies down in the bed at `bed`.
pub fn start_sleeping(mob: &dyn Mob, bed: BlockPos) -> bool {
    let entity = mob.get_entity();
    let world = entity.world.load_full();
    let (block, state) = world.get_block_and_state(&bed);
    if !block.has_tag(&tag::Block::MINECRAFT_BEDS) {
        return false;
    }
    entity.set_pos(Vector3::new(
        f64::from(bed.0.x) + 0.5,
        f64::from(bed.0.y) + 0.6875,
        f64::from(bed.0.z) + 0.5,
    ));
    BedBlock::set_occupied(true, &world, block, &bed, state.id);
    entity.set_pose(EntityPose::Sleeping);
    mob.get_mob_entity()
        .living_entity
        .set_sleeping_pos(Some(bed));
    entity.velocity.store(Vector3::new(0.0, 0.0, 0.0));
    true
}

/// Vanilla `LivingEntity.stopSleeping`: gets up next to the bed, or on top of it.
pub fn stop_sleeping(mob: &dyn Mob) {
    let entity = mob.get_entity();
    let living = &mob.get_mob_entity().living_entity;
    if let Some(bed) = living.sleeping_pos.load() {
        let world = entity.world.load_full();
        let (block, state) = world.get_block_and_state(&bed);
        if block.has_tag(&tag::Block::MINECRAFT_BEDS) {
            BedBlock::set_occupied(false, &world, block, &bed, state.id);
            let props = BedProperties::from_state_id(state.id);
            let stand_up =
                crate::entity::player::Player::find_bed_spawn_position(&world, &bed, props.facing)
                    .unwrap_or_else(|| {
                        Vector3::new(
                            f64::from(bed.0.x) + 0.5,
                            f64::from(bed.0.y) + 1.1,
                            f64::from(bed.0.z) + 0.5,
                        )
                    });
            let look = (bottom_center(bed) - stand_up).normalize();
            entity.set_pos(stand_up);
            entity.yaw.store(pumpkin_util::math::wrap_degrees(
                look.z.atan2(look.x).to_degrees() as f32 - 90.0,
            ));
            entity.pitch.store(0.0);
        }
    }
    entity.set_pose(EntityPose::Standing);
    living.set_sleeping_pos(None);
}

/// Vanilla `SleepInBed`.
#[derive(Default)]
pub struct SleepInBed {
    next_ok_start_time: i64,
}

const SLEEP_IN_BED_CONDITIONS: [(MemoryModuleId, MemoryStatus); 5] = [
    (types::HOME.id(), MemoryStatus::ValuePresent),
    (types::LAST_WOKEN.id(), MemoryStatus::Registered),
    (types::LAST_SLEPT.id(), MemoryStatus::Registered),
    (types::WALK_TARGET.id(), MemoryStatus::Registered),
    (
        types::CANT_REACH_WALK_TARGET_SINCE.id(),
        MemoryStatus::Registered,
    ),
];

impl Behavior for SleepInBed {
    fn entry_conditions(&self) -> &[(MemoryModuleId, MemoryStatus)] {
        &SLEEP_IN_BED_CONDITIONS
    }

    // Vanilla `SleepInBed.timedOut`: never.
    fn min_duration(&self) -> i32 {
        i32::MAX
    }

    fn max_duration(&self) -> i32 {
        i32::MAX
    }

    fn check_extra_start_conditions(&mut self, tick: &mut BrainTick<'_>) -> bool {
        let entity = tick.mob.get_entity();
        if entity.has_vehicle() {
            return false;
        }
        let Some(target) = tick.state.get(types::HOME).copied() else {
            return false;
        };
        if !super::poi::is_in_dimension(tick.world, &target) {
            return false;
        }
        if let Some(last_woken) = tick.state.get(types::LAST_WOKEN) {
            let since = tick.time - last_woken;
            if since > 0 && since < 100 {
                return false;
            }
        }
        let (block, state) = tick.world.get_block_and_state(&target.pos);
        closer_to_center_than(target.pos, entity.pos.load(), 2.0)
            && block.has_tag(&tag::Block::MINECRAFT_VILLAGERS_CAN_SLEEP_ON_BED)
            && !BedProperties::from_state_id(state.id).occupied
    }

    fn can_still_use(&mut self, tick: &mut BrainTick<'_>) -> bool {
        let Some(home) = tick.state.get(types::HOME).copied() else {
            return false;
        };
        let position = tick.mob.get_entity().pos.load();
        tick.state.is_active(Activity::Rest)
            && position.y > f64::from(home.pos.0.y) + 0.4
            && closer_to_center_than(home.pos, position, 1.14)
    }

    fn start(&mut self, tick: &mut BrainTick<'_>) {
        if tick.time <= self.next_ok_start_time {
            return;
        }
        interact_with_door::close_doors_when_going_to_bed(tick);
        if let Some(home) = tick.state.get(types::HOME).copied()
            && start_sleeping(tick.mob, home.pos)
        {
            tick.state.set(types::LAST_SLEPT, tick.time);
        }
        tick.state.erase(types::WALK_TARGET.id());
        tick.state.erase(types::CANT_REACH_WALK_TARGET_SINCE.id());
    }

    fn stop(&mut self, tick: &mut BrainTick<'_>) {
        if tick.mob.get_mob_entity().living_entity.is_sleeping() {
            stop_sleeping(tick.mob);
            tick.state.set(types::LAST_WOKEN, tick.time);
            self.next_ok_start_time = tick.time + 40;
        }
    }

    fn debug_name(&self) -> &'static str {
        "SleepInBed"
    }
}

/// Vanilla `WakeUp`.
#[must_use]
pub fn wake_up() -> OneShot {
    OneShot::new("WakeUp", Vec::new(), |tick| {
        if tick.state.is_active(Activity::Rest)
            || !tick.mob.get_mob_entity().living_entity.is_sleeping()
        {
            return false;
        }
        stop_sleeping(tick.mob);
        tick.state.set(types::LAST_WOKEN, tick.time);
        true
    })
}

/// Vanilla `InsideBrownianWalk`.
#[must_use]
pub fn inside_brownian_walk(speed_modifier: f32) -> OneShot {
    OneShot::new(
        "InsideBrownianWalk",
        vec![(types::WALK_TARGET.id(), MemoryStatus::ValueAbsent)],
        move |tick| {
            let body_pos = tick.mob.get_entity().block_pos.load();
            if tick.world.can_see_sky(&body_pos) {
                return false;
            }
            let mut poses: Vec<BlockPos> = BlockPos::iterate(
                BlockPos::new(body_pos.0.x - 1, body_pos.0.y - 1, body_pos.0.z - 1),
                BlockPos::new(body_pos.0.x + 1, body_pos.0.y + 1, body_pos.0.z + 1),
            )
            .collect();
            poses.shuffle(&mut tick.mob.get_random());
            let target = poses.into_iter().find(|pos| {
                !tick.world.can_see_sky(pos) && tick.world.get_block_state(&pos.down()).is_solid()
            });
            if let Some(target) = target {
                tick.state.set(
                    types::WALK_TARGET,
                    WalkTarget::from_block_pos(target, speed_modifier, 0),
                );
            }
            true
        },
    )
}

/// Vanilla `GoToClosestVillage`.
#[must_use]
pub fn go_to_closest_village(speed_modifier: f32, close_enough_distance: i32) -> OneShot {
    OneShot::new(
        "GoToClosestVillage",
        vec![(types::WALK_TARGET.id(), MemoryStatus::ValueAbsent)],
        move |tick| {
            let body_pos = tick.mob.get_entity().block_pos.load();
            if tick.world.is_village(&body_pos) {
                return false;
            }
            let here = tick.world.sections_to_village(section_of(&body_pos));
            let mut target = None;
            for _ in 0..5 {
                let Some(pos) = land_random_pos::get_pos(tick.mob, 15, 7) else {
                    continue;
                };
                let there = tick
                    .world
                    .sections_to_village(section_of(&pos.to_block_pos()));
                if there < here {
                    target = Some(pos);
                    break;
                }
                if there == here {
                    target = Some(pos);
                }
            }
            if let Some(target) = target {
                tick.state.set(
                    types::WALK_TARGET,
                    WalkTarget::from_vec(target, speed_modifier, close_enough_distance),
                );
            }
            true
        },
    )
}

/// Vanilla `BehaviorUtils.findSectionClosestToVillage`, as a target position.
fn section_closest_to_village(
    tick: &BrainTick<'_>,
    body_pos: BlockPos,
    radius: i32,
) -> Option<Vector3<f64>> {
    let center = section_of(&body_pos);
    let mut best = tick.world.sections_to_village(center);
    let mut best_section = None;
    for x in -radius..=radius {
        for y in -radius..=radius {
            for z in -radius..=radius {
                let section = (center.0 + x, center.1 + y, center.2 + z);
                let distance = tick.world.sections_to_village(section);
                if distance < best {
                    best = distance;
                    best_section = Some(section);
                }
            }
        }
    }
    best_section.map(|(x, y, z)| {
        Vector3::new(
            f64::from(x * 16 + 8),
            f64::from(y * 16 + 8),
            f64::from(z * 16 + 8),
        )
    })
}

/// Vanilla `VillageBoundRandomStroll`.
#[must_use]
pub fn village_bound_random_stroll(speed_modifier: f32, max_xz: i32, max_y: i32) -> OneShot {
    OneShot::new(
        "VillageBoundRandomStroll",
        vec![(types::WALK_TARGET.id(), MemoryStatus::ValueAbsent)],
        move |tick| {
            let body_pos = tick.mob.get_entity().block_pos.load();
            let target = if tick.world.is_village(&body_pos) {
                land_random_pos::get_pos(tick.mob, max_xz, max_y)
            } else if let Some(towards) = section_closest_to_village(tick, body_pos, 2) {
                default_random_pos::get_pos_towards(
                    tick.mob,
                    max_xz,
                    max_y,
                    towards,
                    std::f64::consts::FRAC_PI_2,
                )
            } else {
                land_random_pos::get_pos(tick.mob, max_xz, max_y)
            };
            tick.state.set_optional(
                types::WALK_TARGET,
                target.map(|pos| WalkTarget::from_vec(pos, speed_modifier, 0)),
            );
            true
        },
    )
}

/// Which mob [`interact_with`] walks up to.
pub type TargetFilter = fn(&Arc<dyn EntityBase>) -> bool;

/// Vanilla `InteractWith`.
#[must_use]
pub fn interact_with(
    kind: &'static EntityType,
    range: i32,
    self_filter: fn(&dyn Mob) -> bool,
    target_filter: TargetFilter,
    memory: MemoryModuleType<Arc<dyn EntityBase>>,
    speed_modifier: f32,
    stop_distance: i32,
) -> OneShot {
    let range_sqr = f64::from(range * range);
    OneShot::new(
        "InteractWith",
        vec![
            (memory.id(), MemoryStatus::Registered),
            (types::LOOK_TARGET.id(), MemoryStatus::Registered),
            (types::WALK_TARGET.id(), MemoryStatus::ValueAbsent),
            (
                types::NEAREST_VISIBLE_LIVING_ENTITIES.id(),
                MemoryStatus::ValuePresent,
            ),
        ],
        move |tick| {
            if !self_filter(tick.mob) {
                return false;
            }
            let position = tick.mob.get_entity().pos.load();
            let valid = |mob: &Arc<dyn EntityBase>| {
                mob.get_entity().entity_type == kind && target_filter(mob)
            };
            let (any, closest) = {
                let Some(visible) = tick.state.get(types::NEAREST_VISIBLE_LIVING_ENTITIES) else {
                    return false;
                };
                let ctx = tick.visibility();
                (
                    visible.contains_any(&ctx, valid),
                    visible.find_closest(&ctx, |mob| {
                        mob.get_entity()
                            .pos
                            .load()
                            .squared_distance_to_vec(&position)
                            <= range_sqr
                            && valid(mob)
                    }),
                )
            };
            if !any {
                return false;
            }
            if let Some(mob) = closest {
                look_and_walk_to(tick, memory, mob, speed_modifier, stop_distance);
            }
            true
        },
    )
}

fn look_and_walk_to(
    tick: &mut BrainTick<'_>,
    memory: MemoryModuleType<Arc<dyn EntityBase>>,
    mob: Arc<dyn EntityBase>,
    speed_modifier: f32,
    stop_distance: i32,
) {
    tick.state.set(memory, mob.clone());
    tick.state.set(
        types::LOOK_TARGET,
        Arc::new(EntityTracker::new(mob.clone(), true)),
    );
    tick.state.set(
        types::WALK_TARGET,
        WalkTarget::from_entity(mob, speed_modifier, stop_distance),
    );
}

/// Vanilla `SetLookAndInteract`.
#[must_use]
pub fn set_look_and_interact(kind: &'static EntityType, range: i32) -> OneShot {
    let range_sqr = f64::from(range * range);
    OneShot::new(
        "SetLookAndInteract",
        vec![
            (types::LOOK_TARGET.id(), MemoryStatus::Registered),
            (types::INTERACTION_TARGET.id(), MemoryStatus::ValueAbsent),
            (
                types::NEAREST_VISIBLE_LIVING_ENTITIES.id(),
                MemoryStatus::ValuePresent,
            ),
        ],
        move |tick| {
            let position = tick.mob.get_entity().pos.load();
            let closest = {
                let Some(visible) = tick.state.get(types::NEAREST_VISIBLE_LIVING_ENTITIES) else {
                    return false;
                };
                visible.find_closest(&tick.visibility(), |mob| {
                    mob.get_entity().entity_type == kind
                        && mob
                            .get_entity()
                            .pos
                            .load()
                            .squared_distance_to_vec(&position)
                            <= range_sqr
                })
            };
            let Some(mob) = closest else {
                return false;
            };
            tick.state.set(types::INTERACTION_TARGET, mob.clone());
            tick.state
                .set(types::LOOK_TARGET, Arc::new(EntityTracker::new(mob, true)));
            true
        },
    )
}

/// Vanilla `SetEntityLookTarget.create`: looks at the closest visible mob the filter accepts.
#[must_use]
pub fn set_entity_look_target(filter: TargetFilter, max_dist: f32) -> OneShot {
    let max_dist_sqr = f64::from(max_dist * max_dist);
    OneShot::new(
        "SetEntityLookTarget",
        vec![
            (types::LOOK_TARGET.id(), MemoryStatus::ValueAbsent),
            (
                types::NEAREST_VISIBLE_LIVING_ENTITIES.id(),
                MemoryStatus::ValuePresent,
            ),
        ],
        move |tick| {
            let position = tick.mob.get_entity().pos.load();
            let closest = {
                let Some(visible) = tick.state.get(types::NEAREST_VISIBLE_LIVING_ENTITIES) else {
                    return false;
                };
                visible.find_closest(&tick.visibility(), |mob| {
                    filter(mob)
                        && mob
                            .get_entity()
                            .pos
                            .load()
                            .squared_distance_to_vec(&position)
                            <= max_dist_sqr
                })
            };
            let Some(mob) = closest else {
                return false;
            };
            tick.state
                .set(types::LOOK_TARGET, Arc::new(EntityTracker::new(mob, true)));
            true
        },
    )
}

/// Vanilla `SocializeAtBell`.
#[must_use]
pub fn socialize_at_bell() -> OneShot {
    OneShot::new(
        "SocializeAtBell",
        vec![
            (types::WALK_TARGET.id(), MemoryStatus::Registered),
            (types::LOOK_TARGET.id(), MemoryStatus::Registered),
            (types::MEETING_POINT.id(), MemoryStatus::ValuePresent),
            (
                types::NEAREST_VISIBLE_LIVING_ENTITIES.id(),
                MemoryStatus::ValuePresent,
            ),
            (types::INTERACTION_TARGET.id(), MemoryStatus::ValueAbsent),
        ],
        |tick| {
            let Some(memory) = tick.state.get(types::MEETING_POINT).copied() else {
                return false;
            };
            let position = tick.mob.get_entity().pos.load();
            if tick.mob.get_random().random_range(0..100) != 0
                || !super::poi::is_in_dimension(tick.world, &memory)
                || !closer_to_center_than(memory.pos, position, 4.0)
            {
                return false;
            }
            let is_villager =
                |mob: &Arc<dyn EntityBase>| mob.get_entity().entity_type == &EntityType::VILLAGER;
            let (any, closest) = {
                let Some(visible) = tick.state.get(types::NEAREST_VISIBLE_LIVING_ENTITIES) else {
                    return false;
                };
                let ctx = tick.visibility();
                (
                    visible.contains_any(&ctx, is_villager),
                    visible.find_closest(&ctx, |mob| {
                        is_villager(mob)
                            && mob
                                .get_entity()
                                .pos
                                .load()
                                .squared_distance_to_vec(&position)
                                <= 32.0
                    }),
                )
            };
            if !any {
                return false;
            }
            if let Some(mob) = closest {
                look_and_walk_to(tick, types::INTERACTION_TARGET, mob, 0.3, 1);
            }
            true
        },
    )
}

/// Vanilla `SetWalkTargetAwayFrom.entity`, not interrupting a current walk.
#[must_use]
pub fn set_walk_target_away_from_entity(
    memory: MemoryModuleType<Arc<dyn EntityBase>>,
    speed_modifier: f32,
    desired_distance: i32,
) -> OneShot {
    OneShot::new(
        "SetWalkTargetAwayFrom",
        vec![
            (types::WALK_TARGET.id(), MemoryStatus::Registered),
            (memory.id(), MemoryStatus::ValuePresent),
        ],
        move |tick| {
            if tick.state.has_memory_value(types::WALK_TARGET.id()) {
                return false;
            }
            let Some(avoid) = tick
                .state
                .get(memory)
                .map(|mob| mob.get_entity().pos.load())
            else {
                return false;
            };
            let position = tick.mob.get_entity().pos.load();
            if position.squared_distance_to_vec(&avoid) >= f64::from(desired_distance.pow(2)) {
                return false;
            }
            let flee_to =
                (0..10).find_map(|_| land_random_pos::get_pos_away(tick.mob, 16, 7, avoid));
            if let Some(flee_to) = flee_to {
                tick.state.set(
                    types::WALK_TARGET,
                    WalkTarget::from_vec(flee_to, speed_modifier, 0),
                );
            }
            true
        },
    )
}

/// Vanilla `SetWalkTargetAwayFrom.pos`.
#[must_use]
pub fn set_walk_target_away_from_pos(
    memory: MemoryModuleType<BlockPos>,
    speed_modifier: f32,
    desired_distance: i32,
) -> OneShot {
    OneShot::new(
        "SetWalkTargetAwayFrom",
        vec![
            (types::WALK_TARGET.id(), MemoryStatus::Registered),
            (memory.id(), MemoryStatus::ValuePresent),
        ],
        move |tick| {
            let Some(avoid) = tick.state.get(memory).map(|pos| bottom_center(*pos)) else {
                return false;
            };
            let position = tick.mob.get_entity().pos.load();
            if position.squared_distance_to_vec(&avoid) >= f64::from(desired_distance.pow(2)) {
                return false;
            }
            if let Some(walk) = tick.state.get(types::WALK_TARGET)
                && (walk.speed_modifier() - speed_modifier).abs() < f32::EPSILON
            {
                let current = walk.target().current_position() - position;
                if current.dot(&(avoid - position)) < 0.0 {
                    return false;
                }
            }
            let flee_to =
                (0..10).find_map(|_| land_random_pos::get_pos_away(tick.mob, 16, 7, avoid));
            if let Some(flee_to) = flee_to {
                tick.state.set(
                    types::WALK_TARGET,
                    WalkTarget::from_vec(flee_to, speed_modifier, 0),
                );
            }
            true
        },
    )
}
