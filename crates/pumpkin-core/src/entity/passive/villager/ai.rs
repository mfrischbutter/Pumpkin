//! Vanilla `VillagerGoalPackages` and the villager-only behaviours it uses.

use std::sync::Arc;
use std::sync::atomic::Ordering::Relaxed;

use pumpkin_data::block_properties::WhiteBedLikeProperties as BedProperties;
use pumpkin_data::entity::{EntityType, MobCategory};
use pumpkin_data::environment_attribute::Activity;
use pumpkin_data::poi_type::PoiType;
use pumpkin_data::tag::PointOfInterestType::MINECRAFT_ACQUIRABLE_JOB_SITE;
use pumpkin_data::tag::{self, Taggable};
use pumpkin_data::villager::VillagerProfession;

use super::VillagerEntity;
use super::raid::{
    CelebrateVillagersSurvivedRaid, JumpOnBed, locate_hiding_place, move_to_sky_seeing_spot,
    play_tag_with_other_kids, raid_is_running, raid_is_won, react_to_bell, reset_raid_status,
    ring_bell_behavior, set_hidden_state, set_raid_status,
};
use super::social::{TradeWithVillager, VillagerMakeLove};
use super::work::{
    GiveGiftToHero, HarvestFarmland, ShowTradesToPlayer, UseBonemeal, use_composter,
};
use crate::entity::EntityBase;
use crate::entity::ai::brain::activity::ActivityData;
use crate::entity::ai::brain::behavior::interact_with_door;
use crate::entity::ai::brain::behavior::one_shot::{sequence, trigger_one_shuffled};
use crate::entity::ai::brain::behavior::poi::{
    AcquirePoi, BlockMemoryWalk, acquire_poi, closer_to_center_than, is_in_dimension,
    set_closest_home_as_walk_target, set_walk_target_from_block_memory, stroll_around_poi,
    stroll_to_poi, stroll_to_poi_list, validate_nearby_poi,
};
use crate::entity::ai::brain::behavior::village::{
    SleepInBed, go_to_closest_village, inside_brownian_walk, interact_with, set_entity_look_target,
    set_look_and_interact, set_walk_target_away_from_entity, socialize_at_bell,
    update_activity_from_schedule, village_bound_random_stroll, wake_up,
};
use crate::entity::ai::brain::behavior::{
    Behavior, BehaviorControl, DoNothing, GateBehavior, LookAtTargetSink, MoveToTargetSink,
    OneShot, OrderPolicy, RunningPolicy, Swim, Timed, go_to_wanted_item, run_one,
    run_one_with_conditions, set_walk_target_from_look_target, utils,
};
use crate::entity::ai::brain::memory::PackedMemories;
use crate::entity::ai::brain::memory::position_tracker::{BlockPosTracker, EntityTracker};
use crate::entity::ai::brain::memory::walk_target::WalkTarget;
use crate::entity::ai::brain::memory::{MemoryModuleId, MemoryStatus, types};
use crate::entity::ai::brain::sensing::SensorType;
use crate::entity::ai::brain::{Brain, BrainProvider, BrainTick, Schedule};
use crate::entity::mob::Mob;
use crate::world::World;
use crate::world::villager_poi::poi_type_for_profession;

/// Vanilla `Villager.SPEED_MODIFIER`.
const SPEED_MODIFIER: f32 = 0.5;
/// Vanilla `VillagerGoalPackages.STROLL_SPEED_MODIFIER`.
const STROLL_SPEED_MODIFIER: f32 = 0.4;

type Packaged = Box<dyn BehaviorControl>;

const MEMORIES: [MemoryModuleId; 5] = [
    types::HEARD_BELL_TIME.id(),
    types::LAST_WORKED_AT_POI.id(),
    types::GOLEM_DETECTED_RECENTLY.id(),
    types::INTERACTION_TARGET.id(),
    types::PATH.id(),
];

fn as_villager(mob: &dyn Mob) -> Option<&VillagerEntity> {
    mob.cast_any().downcast_ref::<VillagerEntity>()
}

fn profession_of(mob: &dyn Mob) -> VillagerProfession {
    as_villager(mob).map_or(VillagerProfession::None, VillagerEntity::profession)
}

fn is_baby(mob: &dyn Mob) -> bool {
    mob.get_entity().age.load(Relaxed) < 0
}

/// Vanilla `Villager.BRAIN_PROVIDER` and `registerBrainGoals`.
pub(super) fn make_brain(villager: &VillagerEntity, packed: &PackedMemories) -> Brain {
    let mut brain = BrainProvider {
        memory_types: &MEMORIES,
        sensor_types: &[
            SensorType::NearestLivingEntities,
            SensorType::NearestPlayers,
            SensorType::NearestItems,
            SensorType::NearestBed,
            SensorType::HurtBy,
            SensorType::VillagerHostiles,
            SensorType::VillagerBabies,
            SensorType::SecondaryPois,
            SensorType::GolemDetected,
        ],
        activities,
    }
    .make_brain(villager, packed);
    brain.set_schedule(Some(if is_baby(villager) {
        Schedule::BabyVillager
    } else {
        Schedule::Villager
    }));
    let world = villager.get_entity().world.load();
    let pos = villager.get_entity().block_pos.load();
    brain.update_activity_from_schedule(&world, world.get_world_age(), &pos);
    brain
}

fn activities(mob: &dyn Mob) -> Vec<ActivityData> {
    let mut activities = vec![
        core_package(),
        meet_package(),
        rest_package(),
        idle_package(),
        panic_package(),
        pre_raid_package(),
        raid_package(),
        hide_package(),
    ];
    if is_baby(mob) {
        activities.push(play_package());
    } else {
        activities.push(work_package(
            profession_of(mob) == VillagerProfession::Farmer,
        ));
    }
    activities
}

/// Vanilla `VillagerProfession.heldJobSite`.
fn held_job_site(mob: &dyn Mob, poi: &'static PoiType) -> bool {
    poi_type_for_profession(profession_of(mob)).is_some_and(|held| held == poi)
}

/// Vanilla `VillagerProfession.acquirableJobSite`.
fn acquirable_job_site(mob: &dyn Mob, poi: &'static PoiType) -> bool {
    match profession_of(mob) {
        VillagerProfession::None => poi.has_tag(&MINECRAFT_ACQUIRABLE_JOB_SITE),
        _ => held_job_site(mob, poi),
    }
}

fn is_home(_mob: &dyn Mob, poi: &'static PoiType) -> bool {
    poi == &PoiType::HOME
}

fn is_meeting(_mob: &dyn Mob, poi: &'static PoiType) -> bool {
    poi == &PoiType::MEETING
}

/// Vanilla `VillagerGoalPackages.validateBedPoi`.
fn valid_bed(world: &World, pos: &pumpkin_util::math::position::BlockPos) -> bool {
    let (block, state) = world.get_block_and_state(pos);
    block.has_tag(&tag::Block::MINECRAFT_VILLAGERS_CAN_SLEEP_ON_BED)
        && !BedProperties::from_state_id(state.id).occupied
}

const fn any_poi(_world: &World, _pos: &pumpkin_util::math::position::BlockPos) -> bool {
    true
}

/// Vanilla's entity event 14: green sparkles.
fn notify_happy(mob: &dyn Mob) {
    if let Some(villager) = as_villager(mob) {
        villager.show_happy();
    }
}

fn release_memory_poi(
    _mob: &dyn Mob,
    world: &World,
    pos: &crate::entity::ai::brain::memory::global_pos::GlobalPos,
) {
    world.release_poi(pos.pos);
}

/// Vanilla `VillagerGoalPackages.getCorePackage`.
fn core_package() -> ActivityData {
    let packaged: Vec<(i32, Packaged)> = vec![
        (0, Box::new(Timed::new(Swim::new(0.8)))),
        (0, Box::new(interact_with_door::create())),
        (0, Box::new(Timed::new(LookAtTargetSink::new(45, 90)))),
        (0, Box::new(Timed::new(VillagerPanicTrigger))),
        (0, Box::new(wake_up())),
        (0, Box::new(react_to_bell())),
        (0, Box::new(set_raid_status())),
        (
            0,
            Box::new(validate_nearby_poi(held_job_site, types::JOB_SITE)),
        ),
        (
            0,
            Box::new(validate_nearby_poi(
                acquirable_job_site,
                types::POTENTIAL_JOB_SITE,
            )),
        ),
        (1, Box::new(Timed::new(MoveToTargetSink::default()))),
        (2, Box::new(poi_competitor_scan())),
        (3, Box::new(Timed::new(LookAndFollowTradingPlayerSink))),
        (
            5,
            Box::new(go_to_wanted_item::create(SPEED_MODIFIER, false, 4)),
        ),
        (
            6,
            Box::new(acquire_poi(AcquirePoi {
                kind: acquirable_job_site,
                memory_to_validate: types::JOB_SITE,
                memory_to_acquire: types::POTENTIAL_JOB_SITE,
                only_if_adult: true,
                on_acquired: None,
                valid: any_poi,
            })),
        ),
        (7, Box::new(Timed::new(GoToPotentialJobSite))),
        (8, Box::new(yield_job_site())),
        (
            10,
            Box::new(acquire_poi(AcquirePoi {
                kind: is_home,
                memory_to_validate: types::HOME,
                memory_to_acquire: types::HOME,
                only_if_adult: false,
                on_acquired: Some(notify_happy),
                valid: valid_bed,
            })),
        ),
        (
            10,
            Box::new(acquire_poi(AcquirePoi {
                kind: is_meeting,
                memory_to_validate: types::MEETING_POINT,
                memory_to_acquire: types::MEETING_POINT,
                only_if_adult: true,
                on_acquired: Some(notify_happy),
                valid: any_poi,
            })),
        ),
        (10, Box::new(assign_profession_from_job_site())),
        (10, Box::new(reset_profession())),
    ];
    ActivityData::with_pairs(Activity::Core, packaged)
}

/// Vanilla `VillagerGoalPackages.getWorkPackage`, until the farming behaviours are there.
fn work_package(farmer: bool) -> ActivityData {
    let farm_weights = if farmer { (2, 4) } else { (5, 7) };
    let packaged: Vec<(i32, Packaged)> = vec![
        minimal_look_behavior(),
        (
            5,
            Box::new(run_one(vec![
                (
                    Box::new(Timed::new(WorkAtPoi {
                        last_check: 0,
                        composter: farmer,
                    })),
                    7,
                ),
                (
                    Box::new(stroll_around_poi(types::JOB_SITE, STROLL_SPEED_MODIFIER, 4)),
                    2,
                ),
                (
                    Box::new(stroll_to_poi(types::JOB_SITE, STROLL_SPEED_MODIFIER, 1, 10)),
                    5,
                ),
                (
                    Box::new(stroll_to_poi_list(
                        types::SECONDARY_JOB_SITE,
                        SPEED_MODIFIER,
                        1,
                        6,
                        types::JOB_SITE,
                    )),
                    5,
                ),
                (
                    Box::new(Timed::new(HarvestFarmland::default())),
                    farm_weights.0,
                ),
                (Box::new(Timed::new(UseBonemeal::default())), farm_weights.1),
            ])),
        ),
        (10, Box::new(Timed::new(ShowTradesToPlayer::new(400, 1600)))),
        (10, Box::new(set_look_and_interact(&EntityType::PLAYER, 4))),
        (
            2,
            Box::new(set_walk_target_from_block_memory(
                types::JOB_SITE,
                BlockMemoryWalk {
                    speed_modifier: SPEED_MODIFIER,
                    close_enough_dist: 9,
                    too_far_distance: 100,
                    too_long_unreachable_duration: 1200,
                },
                release_memory_poi,
            )),
        ),
        (3, Box::new(Timed::new(GiveGiftToHero::default()))),
        (99, Box::new(update_activity_from_schedule())),
    ];
    ActivityData::new(
        Activity::Work,
        packaged,
        vec![(types::JOB_SITE.id(), MemoryStatus::ValuePresent)],
        Vec::new(),
    )
}

/// Vanilla `VillagerGoalPackages.getPlayPackage`, until the games are there.
fn play_package() -> ActivityData {
    let packaged: Vec<(i32, Packaged)> = vec![
        (0, Box::new(Timed::new(MoveToTargetSink::new(80, 120)))),
        full_look_behavior(),
        (5, Box::new(play_tag_with_other_kids())),
        (
            5,
            Box::new(run_one_with_conditions(
                vec![(
                    types::VISIBLE_VILLAGER_BABIES.id(),
                    MemoryStatus::ValueAbsent,
                )],
                vec![
                    (Box::new(interact_with_kind(&EntityType::VILLAGER)), 2),
                    (Box::new(interact_with_kind(&EntityType::CAT)), 1),
                    (
                        Box::new(village_bound_random_stroll(SPEED_MODIFIER, 10, 7)),
                        1,
                    ),
                    (
                        Box::new(set_walk_target_from_look_target::create(SPEED_MODIFIER, 2)),
                        1,
                    ),
                    (Box::new(Timed::new(JumpOnBed::new(SPEED_MODIFIER))), 2),
                    (Box::new(DoNothing::new(20, 40)), 2),
                ],
            )),
        ),
        (99, Box::new(update_activity_from_schedule())),
    ];
    ActivityData::with_pairs(Activity::Play, packaged)
}

/// Vanilla `VillagerGoalPackages.getRestPackage`.
fn rest_package() -> ActivityData {
    let packaged: Vec<(i32, Packaged)> = vec![
        (
            2,
            Box::new(set_walk_target_from_block_memory(
                types::HOME,
                BlockMemoryWalk {
                    speed_modifier: SPEED_MODIFIER,
                    close_enough_dist: 1,
                    too_far_distance: 150,
                    too_long_unreachable_duration: 1200,
                },
                release_memory_poi,
            )),
        ),
        (3, Box::new(validate_nearby_poi(is_home, types::HOME))),
        (3, Box::new(Timed::new(SleepInBed::default()))),
        (
            5,
            Box::new(run_one_with_conditions(
                vec![(types::HOME.id(), MemoryStatus::ValueAbsent)],
                vec![
                    (Box::new(set_closest_home_as_walk_target(SPEED_MODIFIER)), 1),
                    (Box::new(inside_brownian_walk(SPEED_MODIFIER)), 4),
                    (Box::new(go_to_closest_village(SPEED_MODIFIER, 4)), 2),
                    (Box::new(DoNothing::new(20, 40)), 2),
                ],
            )),
        ),
        minimal_look_behavior(),
        (99, Box::new(update_activity_from_schedule())),
    ];
    ActivityData::with_pairs(Activity::Rest, packaged)
}

/// Vanilla `VillagerGoalPackages.getMeetPackage`, until villagers trade with each other.
fn meet_package() -> ActivityData {
    let packaged: Vec<(i32, Packaged)> = vec![
        (
            2,
            Box::new(trigger_one_shuffled(
                "MeetAtBell",
                vec![
                    (
                        stroll_around_poi(types::MEETING_POINT, STROLL_SPEED_MODIFIER, 40)
                            .into_trigger(),
                        2,
                    ),
                    (socialize_at_bell().into_trigger(), 2),
                ],
            )),
        ),
        (10, Box::new(Timed::new(ShowTradesToPlayer::new(400, 1600)))),
        (10, Box::new(set_look_and_interact(&EntityType::PLAYER, 4))),
        (
            2,
            Box::new(set_walk_target_from_block_memory(
                types::MEETING_POINT,
                BlockMemoryWalk {
                    speed_modifier: SPEED_MODIFIER,
                    close_enough_dist: 6,
                    too_far_distance: 100,
                    too_long_unreachable_duration: 200,
                },
                release_memory_poi,
            )),
        ),
        (3, Box::new(Timed::new(GiveGiftToHero::default()))),
        (
            3,
            Box::new(validate_nearby_poi(is_meeting, types::MEETING_POINT)),
        ),
        (3, trade_gate()),
        full_look_behavior(),
        (99, Box::new(update_activity_from_schedule())),
    ];
    ActivityData::new(
        Activity::Meet,
        packaged,
        vec![(types::MEETING_POINT.id(), MemoryStatus::ValuePresent)],
        Vec::new(),
    )
}

/// The `GateBehavior` around `TradeWithVillager` in vanilla's meet and idle packages.
fn trade_gate() -> Packaged {
    Box::new(GateBehavior::new(
        "TradeGate",
        Vec::new(),
        vec![types::INTERACTION_TARGET.id()],
        OrderPolicy::Ordered,
        RunningPolicy::RunOne,
        vec![(Box::new(Timed::new(TradeWithVillager::default())), 1)],
    ))
}

fn can_breed(mob: &dyn Mob) -> bool {
    as_villager(mob).is_some_and(VillagerEntity::can_breed)
}

fn target_can_breed(mob: &Arc<dyn EntityBase>) -> bool {
    mob.cast_any()
        .downcast_ref::<VillagerEntity>()
        .is_some_and(VillagerEntity::can_breed)
}

fn interact_with_kind(kind: &'static EntityType) -> OneShot {
    interact_with(
        kind,
        8,
        |_| true,
        |_| true,
        types::INTERACTION_TARGET,
        SPEED_MODIFIER,
        2,
    )
}

/// Vanilla `VillagerGoalPackages.getIdlePackage`, until villagers breed and trade.
fn idle_package() -> ActivityData {
    let packaged: Vec<(i32, Packaged)> = vec![
        (
            2,
            Box::new(run_one(vec![
                (Box::new(interact_with_kind(&EntityType::VILLAGER)), 2),
                (
                    Box::new(interact_with(
                        &EntityType::VILLAGER,
                        8,
                        can_breed,
                        target_can_breed,
                        types::BREED_TARGET,
                        SPEED_MODIFIER,
                        2,
                    )),
                    1,
                ),
                (Box::new(interact_with_kind(&EntityType::CAT)), 1),
                (
                    Box::new(village_bound_random_stroll(SPEED_MODIFIER, 10, 7)),
                    1,
                ),
                (
                    Box::new(set_walk_target_from_look_target::create(SPEED_MODIFIER, 2)),
                    1,
                ),
                (Box::new(Timed::new(JumpOnBed::new(SPEED_MODIFIER))), 1),
                (Box::new(DoNothing::new(30, 60)), 1),
            ])),
        ),
        (3, Box::new(Timed::new(GiveGiftToHero::default()))),
        (3, Box::new(set_look_and_interact(&EntityType::PLAYER, 4))),
        (3, Box::new(Timed::new(ShowTradesToPlayer::new(400, 1600)))),
        (3, trade_gate()),
        (
            3,
            Box::new(GateBehavior::new(
                "BreedGate",
                Vec::new(),
                vec![types::BREED_TARGET.id()],
                OrderPolicy::Ordered,
                RunningPolicy::RunOne,
                vec![(Box::new(Timed::new(VillagerMakeLove::default())), 1)],
            )),
        ),
        full_look_behavior(),
        (99, Box::new(update_activity_from_schedule())),
    ];
    ActivityData::with_pairs(Activity::Idle, packaged)
}

/// Vanilla `VillagerGoalPackages.getPanicPackage`.
fn panic_package() -> ActivityData {
    let runaway_speed = SPEED_MODIFIER * 1.5;
    let packaged: Vec<(i32, Packaged)> = vec![
        (0, Box::new(villager_calm_down())),
        (
            1,
            Box::new(set_walk_target_away_from_entity(
                types::NEAREST_HOSTILE,
                runaway_speed,
                6,
            )),
        ),
        (
            1,
            Box::new(set_walk_target_away_from_entity(
                types::HURT_BY_ENTITY,
                runaway_speed,
                6,
            )),
        ),
        (
            3,
            Box::new(village_bound_random_stroll(runaway_speed, 2, 2)),
        ),
        minimal_look_behavior(),
    ];
    ActivityData::with_pairs(Activity::Panic, packaged)
}

/// Vanilla `VillagerGoalPackages.getPreRaidPackage`.
fn pre_raid_package() -> ActivityData {
    let running = SPEED_MODIFIER * 1.5;
    let packaged: Vec<(i32, Packaged)> = vec![
        (0, Box::new(ring_bell_behavior())),
        (
            0,
            Box::new(trigger_one_shuffled(
                "PreRaidRun",
                vec![
                    (
                        set_walk_target_from_block_memory(
                            types::MEETING_POINT,
                            BlockMemoryWalk {
                                speed_modifier: running,
                                close_enough_dist: 2,
                                too_far_distance: 150,
                                too_long_unreachable_duration: 200,
                            },
                            release_memory_poi,
                        )
                        .into_trigger(),
                        6,
                    ),
                    (
                        village_bound_random_stroll(running, 10, 7).into_trigger(),
                        2,
                    ),
                ],
            )),
        ),
        minimal_look_behavior(),
        (99, Box::new(reset_raid_status())),
    ];
    ActivityData::with_pairs(Activity::PreRaid, packaged)
}

/// Vanilla `VillagerGoalPackages.getRaidPackage`.
fn raid_package() -> ActivityData {
    let packaged: Vec<(i32, Packaged)> = vec![
        (
            0,
            Box::new(sequence(
                "RaidWon",
                raid_is_won,
                trigger_one_shuffled(
                    "RaidWonRun",
                    vec![
                        (move_to_sky_seeing_spot(SPEED_MODIFIER).into_trigger(), 5),
                        (
                            village_bound_random_stroll(SPEED_MODIFIER * 1.1, 10, 7).into_trigger(),
                            2,
                        ),
                    ],
                ),
            )),
        ),
        (0, Box::new(Timed::new(CelebrateVillagersSurvivedRaid))),
        (
            2,
            Box::new(sequence(
                "RaidRunning",
                raid_is_running,
                locate_hiding_place(24, SPEED_MODIFIER * 1.4, 1),
            )),
        ),
        minimal_look_behavior(),
        (99, Box::new(reset_raid_status())),
    ];
    ActivityData::with_pairs(Activity::Raid, packaged)
}

/// Vanilla `VillagerGoalPackages.getHidePackage`.
fn hide_package() -> ActivityData {
    let packaged: Vec<(i32, Packaged)> = vec![
        (0, Box::new(set_hidden_state(15, 3))),
        (
            1,
            Box::new(locate_hiding_place(32, SPEED_MODIFIER * 1.25, 2)),
        ),
        minimal_look_behavior(),
    ];
    ActivityData::with_pairs(Activity::Hide, packaged)
}

fn is_kind<const VILLAGER: bool>(mob: &Arc<dyn EntityBase>) -> bool {
    let kind = if VILLAGER {
        &EntityType::VILLAGER
    } else {
        &EntityType::PLAYER
    };
    mob.get_entity().entity_type == kind
}

fn is_cat(mob: &Arc<dyn EntityBase>) -> bool {
    mob.get_entity().entity_type == &EntityType::CAT
}

fn is_category<const CATEGORY: u8>(mob: &Arc<dyn EntityBase>) -> bool {
    let category = mob.get_entity().entity_type.category;
    match CATEGORY {
        0 => category == &MobCategory::CREATURE,
        1 => category == &MobCategory::WATER_CREATURE,
        2 => category == &MobCategory::AXOLOTLS,
        3 => category == &MobCategory::UNDERGROUND_WATER_CREATURE,
        4 => category == &MobCategory::WATER_AMBIENT,
        _ => category == &MobCategory::MONSTER,
    }
}

/// Vanilla `VillagerGoalPackages.getFullLookBehavior`.
fn full_look_behavior() -> (i32, Packaged) {
    (
        5,
        Box::new(run_one(vec![
            (Box::new(set_entity_look_target(is_cat, 8.0)), 8),
            (Box::new(set_entity_look_target(is_kind::<true>, 8.0)), 2),
            (Box::new(set_entity_look_target(is_kind::<false>, 8.0)), 2),
            (Box::new(set_entity_look_target(is_category::<0>, 8.0)), 1),
            (Box::new(set_entity_look_target(is_category::<1>, 8.0)), 1),
            (Box::new(set_entity_look_target(is_category::<2>, 8.0)), 1),
            (Box::new(set_entity_look_target(is_category::<3>, 8.0)), 1),
            (Box::new(set_entity_look_target(is_category::<4>, 8.0)), 1),
            (Box::new(set_entity_look_target(is_category::<5>, 8.0)), 1),
            (Box::new(DoNothing::new(30, 60)), 2),
        ])),
    )
}

/// Vanilla `VillagerGoalPackages.getMinimalLookBehavior`.
fn minimal_look_behavior() -> (i32, Packaged) {
    (
        5,
        Box::new(run_one(vec![
            (Box::new(set_entity_look_target(is_kind::<true>, 8.0)), 2),
            (Box::new(set_entity_look_target(is_kind::<false>, 8.0)), 2),
            (Box::new(DoNothing::new(30, 60)), 8),
        ])),
    )
}

/// Vanilla `VillagerPanicTrigger`.
struct VillagerPanicTrigger;

fn is_scared(tick: &BrainTick<'_>) -> bool {
    tick.state.has_memory_value(types::HURT_BY.id())
        || tick.state.has_memory_value(types::NEAREST_HOSTILE.id())
}

impl Behavior for VillagerPanicTrigger {
    fn entry_conditions(&self) -> &[(MemoryModuleId, MemoryStatus)] {
        &[]
    }

    fn can_still_use(&mut self, tick: &mut BrainTick<'_>) -> bool {
        is_scared(tick)
    }

    fn start(&mut self, tick: &mut BrainTick<'_>) {
        if !is_scared(tick) {
            return;
        }
        if !tick.state.is_active(Activity::Panic) {
            tick.state.erase(types::PATH.id());
            tick.state.erase(types::WALK_TARGET.id());
            tick.state.erase(types::LOOK_TARGET.id());
            tick.state.erase(types::BREED_TARGET.id());
            tick.state.erase(types::INTERACTION_TARGET.id());
        }
        tick.state.set_active_activity_if_possible(Activity::Panic);
    }

    fn tick(&mut self, tick: &mut BrainTick<'_>) {
        if tick.time % 100 == 0
            && let Some(villager) = as_villager(tick.mob)
        {
            villager.spawn_golem_if_needed(tick.state, tick.world, tick.time, 3);
        }
    }

    fn debug_name(&self) -> &'static str {
        "VillagerPanicTrigger"
    }
}

/// Vanilla `VillagerCalmDown`.
fn villager_calm_down() -> OneShot {
    OneShot::new(
        "VillagerCalmDown",
        vec![
            (types::HURT_BY.id(), MemoryStatus::Registered),
            (types::HURT_BY_ENTITY.id(), MemoryStatus::Registered),
            (types::NEAREST_HOSTILE.id(), MemoryStatus::Registered),
        ],
        |tick| {
            let position = tick.mob.get_entity().pos.load();
            let attacker_close = tick.state.get(types::HURT_BY_ENTITY).is_some_and(|mob| {
                mob.get_entity()
                    .pos
                    .load()
                    .squared_distance_to_vec(&position)
                    <= 36.0
            });
            if !(is_scared(tick) || attacker_close) {
                tick.state.erase(types::HURT_BY.id());
                tick.state.erase(types::HURT_BY_ENTITY.id());
                let pos = tick.mob.get_entity().block_pos.load();
                tick.state
                    .update_activity_from_schedule(tick.world, tick.time, &pos);
            }
            true
        },
    )
}

/// The living villagers in sight other than this one.
fn nearby_villagers(tick: &BrainTick<'_>) -> Vec<Arc<dyn EntityBase>> {
    let own = tick.mob.get_entity().entity_id;
    tick.state
        .get(types::NEAREST_LIVING_ENTITIES)
        .map_or_else(Vec::new, |mobs| {
            mobs.iter()
                .filter(|mob| {
                    let entity = mob.get_entity();
                    entity.entity_id != own
                        && entity.is_alive()
                        && entity.entity_type == &EntityType::VILLAGER
                })
                .cloned()
                .collect()
        })
}

/// Vanilla `PoiCompetitorScan`: of the villagers holding one job site, the one with more
/// experience keeps it.
fn poi_competitor_scan() -> OneShot {
    OneShot::new(
        "PoiCompetitorScan",
        vec![
            (types::JOB_SITE.id(), MemoryStatus::ValuePresent),
            (
                types::NEAREST_LIVING_ENTITIES.id(),
                MemoryStatus::ValuePresent,
            ),
        ],
        |tick| {
            let Some(site) = tick.state.get(types::JOB_SITE).copied() else {
                return false;
            };
            let Some(kind) = tick.world.poi_type_at(site.pos) else {
                return true;
            };
            let Some(me) = as_villager(tick.mob) else {
                return true;
            };
            let villagers = nearby_villagers(tick);
            // The winner so far, `None` for this villager.
            let mut winner: Option<&VillagerEntity> = None;
            let mut winner_xp = me.xp.load(Relaxed);
            for other in &villagers {
                let Some(other) = other.cast_any().downcast_ref::<VillagerEntity>() else {
                    continue;
                };
                if poi_type_for_profession(other.profession()) != Some(kind) {
                    continue;
                }
                let holds_it = other.with_other_brain(|brain| {
                    brain
                        .get(types::JOB_SITE)
                        .is_some_and(|their| their.pos == site.pos)
                });
                if holds_it != Some(true) {
                    continue;
                }
                // Vanilla `selectWinner`: the winner so far only stays with more experience;
                // the loser forgets the site.
                let other_xp = other.xp.load(Relaxed);
                if winner_xp > other_xp {
                    other.with_other_brain(|brain| brain.erase(types::JOB_SITE.id()));
                } else {
                    match winner {
                        None => tick.state.erase(types::JOB_SITE.id()),
                        Some(previous) => {
                            previous.with_other_brain(|brain| brain.erase(types::JOB_SITE.id()));
                        }
                    }
                    winner = Some(other);
                    winner_xp = other_xp;
                }
            }
            true
        },
    )
}

/// Vanilla `GoToPotentialJobSite`.
struct GoToPotentialJobSite;

const GO_TO_POTENTIAL_JOB_SITE_CONDITIONS: [(MemoryModuleId, MemoryStatus); 1] =
    [(types::POTENTIAL_JOB_SITE.id(), MemoryStatus::ValuePresent)];

impl Behavior for GoToPotentialJobSite {
    fn entry_conditions(&self) -> &[(MemoryModuleId, MemoryStatus)] {
        &GO_TO_POTENTIAL_JOB_SITE_CONDITIONS
    }

    fn min_duration(&self) -> i32 {
        1200
    }

    fn max_duration(&self) -> i32 {
        1200
    }

    fn check_extra_start_conditions(&mut self, tick: &mut BrainTick<'_>) -> bool {
        tick.state
            .get_active_non_core_activity()
            .is_none_or(|activity| {
                matches!(activity, Activity::Idle | Activity::Work | Activity::Play)
            })
    }

    fn can_still_use(&mut self, tick: &mut BrainTick<'_>) -> bool {
        tick.state.has_memory_value(types::POTENTIAL_JOB_SITE.id())
    }

    fn tick(&mut self, tick: &mut BrainTick<'_>) {
        if let Some(site) = tick.state.get(types::POTENTIAL_JOB_SITE).copied() {
            utils::set_walk_and_look_target_memories_to_block(
                tick.state,
                site.pos,
                SPEED_MODIFIER,
                1,
            );
        }
    }

    fn stop(&mut self, tick: &mut BrainTick<'_>) {
        if let Some(site) = tick.state.get(types::POTENTIAL_JOB_SITE).copied()
            && tick.world.poi_type_at(site.pos).is_some()
        {
            tick.world.release_poi(site.pos);
        }
        tick.state.erase(types::POTENTIAL_JOB_SITE.id());
    }

    fn debug_name(&self) -> &'static str {
        "GoToPotentialJobSite"
    }
}

/// Vanilla `YieldJobSite`: a jobless villager leaves a site to a villager of its profession.
fn yield_job_site() -> OneShot {
    OneShot::new(
        "YieldJobSite",
        vec![
            (types::POTENTIAL_JOB_SITE.id(), MemoryStatus::ValuePresent),
            (types::JOB_SITE.id(), MemoryStatus::ValueAbsent),
            (
                types::NEAREST_LIVING_ENTITIES.id(),
                MemoryStatus::ValuePresent,
            ),
            (types::WALK_TARGET.id(), MemoryStatus::Registered),
            (types::LOOK_TARGET.id(), MemoryStatus::Registered),
        ],
        |tick| {
            if is_baby(tick.mob) || profession_of(tick.mob) != VillagerProfession::None {
                return false;
            }
            let Some(site) = tick.state.get(types::POTENTIAL_JOB_SITE).copied() else {
                return false;
            };
            let Some(kind) = tick.world.poi_type_at(site.pos) else {
                return true;
            };
            let heir = nearby_villagers(tick).into_iter().find(|other| {
                other
                    .cast_any()
                    .downcast_ref::<VillagerEntity>()
                    .is_some_and(|other| other.wants_job_site(kind, site.pos))
            });
            let Some(heir) = heir else {
                return true;
            };
            tick.state.erase(types::WALK_TARGET.id());
            tick.state.erase(types::LOOK_TARGET.id());
            tick.state.erase(types::POTENTIAL_JOB_SITE.id());
            if let Some(heir) = heir.cast_any().downcast_ref::<VillagerEntity>() {
                heir.with_other_brain(|brain| {
                    if !brain.has_memory_value(types::JOB_SITE.id()) {
                        utils::set_walk_and_look_target_memories_to_block(
                            brain,
                            site.pos,
                            SPEED_MODIFIER,
                            1,
                        );
                        brain.set(types::POTENTIAL_JOB_SITE, site);
                    }
                });
            }
            true
        },
    )
}

impl VillagerEntity {
    /// Vanilla `YieldJobSite.nearbyWantsJobsite`.
    fn wants_job_site(
        &self,
        kind: &'static PoiType,
        pos: pumpkin_util::math::position::BlockPos,
    ) -> bool {
        if poi_type_for_profession(self.profession()) != Some(kind) {
            return false;
        }
        let memory = self.with_other_brain(|brain| {
            (
                brain.has_memory_value(types::POTENTIAL_JOB_SITE.id()),
                brain.get(types::JOB_SITE).copied(),
            )
        });
        let Some((has_potential, job_site)) = memory else {
            return false;
        };
        if has_potential {
            return false;
        }
        job_site.map_or_else(
            || {
                let steered = self.mob_entity.steered();
                let owner = &steered.mob_entity().living_entity;
                steered
                    .navigator()
                    .create_path(owner, pos.0.to_f64(), kind.valid_range as i32)
                    .is_some_and(|path| path.can_reach())
            },
            |site| site.pos == pos,
        )
    }
}

/// Vanilla `AssignProfessionFromJobSite`.
fn assign_profession_from_job_site() -> OneShot {
    OneShot::new(
        "AssignProfessionFromJobSite",
        vec![
            (types::POTENTIAL_JOB_SITE.id(), MemoryStatus::ValuePresent),
            (types::JOB_SITE.id(), MemoryStatus::Registered),
        ],
        |tick| {
            let Some(site) = tick.state.get(types::POTENTIAL_JOB_SITE).copied() else {
                return false;
            };
            if !closer_to_center_than(site.pos, tick.mob.get_entity().pos.load(), 2.0) {
                return false;
            }
            tick.state.erase(types::POTENTIAL_JOB_SITE.id());
            tick.state.set(types::JOB_SITE, site);
            let Some(villager) = as_villager(tick.mob) else {
                return true;
            };
            villager.show_happy();
            if villager.profession() == VillagerProfession::None
                && is_in_dimension(tick.world, &site)
                && let Some(kind) = tick.world.poi_type_at(site.pos)
                && let Some(profession) = super::profession_for_poi_type(kind)
            {
                villager.set_profession(profession);
            }
            true
        },
    )
}

/// Vanilla `ResetProfession`.
fn reset_profession() -> OneShot {
    OneShot::new(
        "ResetProfession",
        vec![(types::JOB_SITE.id(), MemoryStatus::ValueAbsent)],
        |tick| {
            let Some(villager) = as_villager(tick.mob) else {
                return false;
            };
            let profession = villager.profession();
            let can_be_fired =
                profession != VillagerProfession::None && profession != VillagerProfession::Nitwit;
            if can_be_fired && villager.xp.load(Relaxed) == 0 && villager.level() <= 1 {
                villager.set_profession(VillagerProfession::None);
                true
            } else {
                false
            }
        },
    )
}

/// Vanilla `WorkAtPoi`.
struct WorkAtPoi {
    last_check: i64,
    /// Vanilla `WorkAtComposter`, the farmer's kind.
    composter: bool,
}

const WORK_AT_POI_CONDITIONS: [(MemoryModuleId, MemoryStatus); 2] = [
    (types::JOB_SITE.id(), MemoryStatus::ValuePresent),
    (types::LOOK_TARGET.id(), MemoryStatus::Registered),
];

/// Vanilla `WorkAtPoi.DISTANCE`.
const WORK_DISTANCE: f64 = 1.73;

fn at_job_site(tick: &BrainTick<'_>) -> bool {
    tick.state.get(types::JOB_SITE).is_some_and(|site| {
        is_in_dimension(tick.world, site)
            && closer_to_center_than(site.pos, tick.mob.get_entity().pos.load(), WORK_DISTANCE)
    })
}

impl Behavior for WorkAtPoi {
    fn entry_conditions(&self) -> &[(MemoryModuleId, MemoryStatus)] {
        &WORK_AT_POI_CONDITIONS
    }

    fn check_extra_start_conditions(&mut self, tick: &mut BrainTick<'_>) -> bool {
        use rand::RngExt;
        if tick.time - self.last_check < 300 || tick.mob.get_random().random_range(0..2) != 0 {
            return false;
        }
        self.last_check = tick.time;
        at_job_site(tick)
    }

    fn can_still_use(&mut self, tick: &mut BrainTick<'_>) -> bool {
        at_job_site(tick)
    }

    fn start(&mut self, tick: &mut BrainTick<'_>) {
        tick.state.set(types::LAST_WORKED_AT_POI, tick.time);
        if let Some(site) = tick.state.get(types::JOB_SITE).copied() {
            tick.state
                .set(types::LOOK_TARGET, Arc::new(BlockPosTracker::new(site.pos)));
        }
        if let Some(villager) = as_villager(tick.mob) {
            villager.play_work_sound();
            if self.composter
                && let Some(site) = tick.state.get(types::JOB_SITE)
            {
                use_composter(tick.world, site.pos, villager);
            }
            villager.restock_if_needed(tick.world);
        }
    }

    fn debug_name(&self) -> &'static str {
        "WorkAtPoi"
    }
}

/// Vanilla `LookAndFollowTradingPlayerSink`.
struct LookAndFollowTradingPlayerSink;

const TRADING_SINK_CONDITIONS: [(MemoryModuleId, MemoryStatus); 2] = [
    (types::WALK_TARGET.id(), MemoryStatus::Registered),
    (types::LOOK_TARGET.id(), MemoryStatus::Registered),
];

fn trading_player_near(tick: &BrainTick<'_>) -> Option<Arc<crate::entity::player::Player>> {
    let entity = tick.mob.get_entity();
    let living = &tick.mob.get_mob_entity().living_entity;
    tick.mob.get_trading_player().filter(|player| {
        entity.is_alive()
            && !entity.is_in_water()
            && living.hurt_cooldown.load(Relaxed) <= 10
            && player
                .get_entity()
                .pos
                .load()
                .squared_distance_to_vec(&entity.pos.load())
                <= 16.0
    })
}

fn follow_player(tick: &mut BrainTick<'_>, player: Arc<crate::entity::player::Player>) {
    let player: Arc<dyn EntityBase> = player;
    tick.state.set(
        types::WALK_TARGET,
        WalkTarget::from_entity(player.clone(), SPEED_MODIFIER, 2),
    );
    tick.state.set(
        types::LOOK_TARGET,
        Arc::new(EntityTracker::new(player, true)),
    );
}

impl Behavior for LookAndFollowTradingPlayerSink {
    fn entry_conditions(&self) -> &[(MemoryModuleId, MemoryStatus)] {
        &TRADING_SINK_CONDITIONS
    }

    // Vanilla `timedOut`: never.
    fn min_duration(&self) -> i32 {
        i32::MAX
    }

    fn max_duration(&self) -> i32 {
        i32::MAX
    }

    fn check_extra_start_conditions(&mut self, tick: &mut BrainTick<'_>) -> bool {
        trading_player_near(tick).is_some()
    }

    fn can_still_use(&mut self, tick: &mut BrainTick<'_>) -> bool {
        trading_player_near(tick).is_some()
    }

    fn start(&mut self, tick: &mut BrainTick<'_>) {
        if let Some(player) = trading_player_near(tick) {
            follow_player(tick, player);
        }
    }

    fn tick(&mut self, tick: &mut BrainTick<'_>) {
        if let Some(player) = trading_player_near(tick) {
            follow_player(tick, player);
        }
    }

    fn stop(&mut self, tick: &mut BrainTick<'_>) {
        tick.state.erase(types::WALK_TARGET.id());
        tick.state.erase(types::LOOK_TARGET.id());
    }

    fn debug_name(&self) -> &'static str {
        "LookAndFollowTradingPlayerSink"
    }
}
