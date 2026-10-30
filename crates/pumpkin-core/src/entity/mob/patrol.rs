use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crossbeam::atomic::AtomicCell;
use pumpkin_data::data_component_impl::EquipmentSlot;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_world::chunk::ChunkHeightmapType;
use rand::RngExt;

use crate::entity::EntityBase;
use crate::entity::ai::goal::{Controls, Goal};
use crate::entity::mob::Mob;
use crate::entity::mob::raider::{Raider, create_ominous_banner};
use crate::world::World;

/// A patrolling monster farther than this from every player despawns.
const PATROL_DESPAWN_DISTANCE_SQ: f64 = 16_384.0;
/// Vanilla `LongDistancePatrolGoal.NAVIGATION_FAILED_COOLDOWN`.
const NAVIGATION_FAILED_COOLDOWN: i64 = 200;
const PATROL_COMPANION_RANGE: f64 = 16.0;
/// A leader this close to its patrol target picks a new one.
const PATROL_TARGET_REACHED_DISTANCE: f64 = 10.0;
const PATROL_STEP_LENGTH: f64 = 10.0;
/// Vanilla rotates by 90 as radians, not degrees.
const PATROL_STEP_ROTATION: f32 = 90.0;
const PATROL_STEP_SPREAD: f64 = 0.4;
const RANDOM_STEP_RANGE: i32 = 16;

pub struct PatrolData {
    pub patrol_target: AtomicCell<Option<BlockPos>>,
    pub patrol_leader: AtomicBool,
    pub patrolling: AtomicBool,
}

impl Default for PatrolData {
    fn default() -> Self {
        Self {
            patrol_target: AtomicCell::new(None),
            patrol_leader: AtomicBool::new(false),
            patrolling: AtomicBool::new(false),
        }
    }
}

pub trait PatrollingMonster: Mob {
    fn get_patrol_data(&self) -> &PatrolData;

    fn can_be_leader(&self) -> bool {
        true
    }

    fn is_patrol_leader(&self) -> bool {
        self.get_patrol_data().patrol_leader.load(Ordering::Relaxed)
    }

    fn set_patrol_leader(&self, is_leader: bool) {
        self.get_patrol_data()
            .patrol_leader
            .store(is_leader, Ordering::Relaxed);
        self.set_patrolling(true);
    }

    fn is_patrolling(&self) -> bool {
        self.get_patrol_data().patrolling.load(Ordering::Relaxed)
    }

    fn set_patrolling(&self, patrolling: bool) {
        self.get_patrol_data()
            .patrolling
            .store(patrolling, Ordering::Relaxed);
    }

    fn get_patrol_target(&self) -> Option<BlockPos> {
        self.get_patrol_data().patrol_target.load()
    }

    fn set_patrol_target(&self, target: BlockPos) {
        self.get_patrol_data().patrol_target.store(Some(target));
        self.set_patrolling(true);
    }

    fn has_patrol_target(&self) -> bool {
        self.get_patrol_data().patrol_target.load().is_some()
    }

    /// Vanilla `canJoinPatrol`; `Raider` overrides it to skip raiders of an active raid.
    fn can_join_patrol(&self) -> bool {
        self.as_raider()
            .is_none_or(|raider| !Raider::has_active_raid(raider))
    }

    /// Vanilla `PatrollingMonster.removeWhenFarAway`.
    fn patrol_remove_when_far_away(&self, distance_sq: f64) -> bool {
        !self.is_patrolling() || distance_sq > PATROL_DESPAWN_DISTANCE_SQ
    }

    fn find_patrol_target(&self) {
        let block_pos = self.get_mob_entity().living_entity.entity.block_pos.load();
        let dx: i32 = rand::random::<i32>().rem_euclid(1000) - 500;
        let dz: i32 = rand::random::<i32>().rem_euclid(1000) - 500;
        let target = BlockPos(Vector3::new(
            block_pos.0.x + dx,
            block_pos.0.y,
            block_pos.0.z + dz,
        ));
        self.set_patrol_target(target);
    }

    fn finalize_patrol_spawn(&self, is_patrol_spawn: bool) {
        if !is_patrol_spawn && self.can_be_leader() {
            let r: f32 = rand::random();
            if r < 0.06 {
                self.set_patrol_leader(true);
            }
        }

        if self.is_patrol_leader() {
            let banner = create_ominous_banner();
            let living = &self.get_mob_entity().living_entity;
            if let Ok(mut equipment) = living.entity_equipment.try_lock() {
                equipment.put(&EquipmentSlot::HEAD, banner.clone());
                drop(equipment);
                living.send_equipment_changes(&[(EquipmentSlot::HEAD, banner)]);
            }
        }

        if is_patrol_spawn {
            self.set_patrolling(true);
        }
    }

    fn write_patrol_nbt(&self, nbt: &mut NbtCompound) {
        let data = self.get_patrol_data();
        if let Some(target) = data.patrol_target.load() {
            nbt.put_int("PatrolTargetX", target.0.x);
            nbt.put_int("PatrolTargetY", target.0.y);
            nbt.put_int("PatrolTargetZ", target.0.z);
        }
        nbt.put_bool("PatrolLeader", data.patrol_leader.load(Ordering::Relaxed));
        nbt.put_bool("Patrolling", data.patrolling.load(Ordering::Relaxed));
    }

    fn read_patrol_nbt(&self, nbt: &NbtCompound) {
        let data = self.get_patrol_data();
        if let (Some(x), Some(y), Some(z)) = (
            nbt.get_int("PatrolTargetX"),
            nbt.get_int("PatrolTargetY"),
            nbt.get_int("PatrolTargetZ"),
        ) {
            data.patrol_target
                .store(Some(BlockPos(Vector3::new(x, y, z))));
        }
        if let Some(leader) = nbt.get_bool("PatrolLeader") {
            data.patrol_leader.store(leader, Ordering::Relaxed);
        }
        if let Some(patrolling) = nbt.get_bool("Patrolling") {
            data.patrolling.store(patrolling, Ordering::Relaxed);
        }
    }
}

/// Vanilla `PatrollingMonster.LongDistancePatrolGoal`.
///
/// The patrol walks toward its target in steps that swing out to the side, the leader leading
/// its companions; a patroller left without companions stops patrolling.
pub struct LongDistancePatrolGoal {
    speed_modifier: f64,
    leader_speed_modifier: f64,
    cooldown_until: i64,
}

impl LongDistancePatrolGoal {
    #[must_use]
    pub const fn new(speed_modifier: f64, leader_speed_modifier: f64) -> Self {
        Self {
            speed_modifier,
            leader_speed_modifier,
            cooldown_until: -1,
        }
    }

    /// Vanilla `findPatrolCompanions`.
    fn find_patrol_companions(mob: &dyn Mob) -> Vec<Arc<dyn EntityBase>> {
        let entity = mob.get_entity();
        let area = entity.bounding_box.load().expand(
            PATROL_COMPANION_RANGE,
            PATROL_COMPANION_RANGE,
            PATROL_COMPANION_RANGE,
        );
        entity
            .world
            .load()
            .get_entities_at_box(&area)
            .into_iter()
            .filter(|candidate| {
                candidate.get_entity().entity_id != entity.entity_id
                    && candidate
                        .get_mob()
                        .and_then(Mob::as_patrolling_monster)
                        .is_some_and(PatrollingMonster::can_join_patrol)
            })
            .collect()
    }

    /// Vanilla `getHeightmapPos(MOTION_BLOCKING_NO_LEAVES, ...)`.
    fn surface_pos(world: &World, x: i32, z: i32) -> BlockPos {
        let y = world.get_heightmap_height(ChunkHeightmapType::MotionBlockingNoLeaves, x, z) + 1;
        BlockPos::new(x, y, z)
    }

    /// The next step toward the patrol target, rotated to the side and cut to 10 blocks.
    fn next_step(world: &World, patrol_target: BlockPos, pos: Vector3<f64>) -> BlockPos {
        let target = Vector3::new(
            f64::from(patrol_target.0.x) + 0.5,
            f64::from(patrol_target.0.y),
            f64::from(patrol_target.0.z) + 0.5,
        );
        let offset = pos.sub(&target);
        let (sin, cos) = PATROL_STEP_ROTATION.sin_cos();
        let (sin, cos) = (f64::from(sin), f64::from(cos));
        let rotated = Vector3::new(
            offset.x.mul_add(cos, offset.z * sin),
            offset.y,
            offset.z.mul_add(cos, -(offset.x * sin)),
        );
        let long_distance_target = rotated
            .multiply(PATROL_STEP_SPREAD, PATROL_STEP_SPREAD, PATROL_STEP_SPREAD)
            .add(&target);
        let move_target = long_distance_target
            .sub(&pos)
            .normalize()
            .multiply(PATROL_STEP_LENGTH, PATROL_STEP_LENGTH, PATROL_STEP_LENGTH)
            .add(&pos);
        let step = BlockPos::floored_v(move_target);
        Self::surface_pos(world, step.0.x, step.0.z)
    }

    fn move_to(mob: &dyn Mob, pos: BlockPos, speed: f64) -> bool {
        let steered = mob.get_mob_entity().steered();
        let owner = &steered.mob_entity().living_entity;
        steered.navigator().move_to_coords(
            f64::from(pos.0.x),
            f64::from(pos.0.y),
            f64::from(pos.0.z),
            speed,
            owner,
        )
    }

    /// Vanilla `moveRandomly`.
    fn move_randomly(&self, mob: &dyn Mob, world: &World) {
        let mut random = mob.get_random();
        let block_pos = mob.get_entity().block_pos.load();
        let x = block_pos.0.x - RANDOM_STEP_RANGE / 2 + random.random_range(0..RANDOM_STEP_RANGE);
        let z = block_pos.0.z - RANDOM_STEP_RANGE / 2 + random.random_range(0..RANDOM_STEP_RANGE);
        Self::move_to(mob, Self::surface_pos(world, x, z), self.speed_modifier);
    }
}

impl Goal for LongDistancePatrolGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        let Some(patrol) = mob.as_patrolling_monster() else {
            return false;
        };
        let is_on_cooldown = mob.get_entity().world.load().get_world_age() < self.cooldown_until;
        patrol.is_patrolling()
            && mob.get_mob_entity().get_target().is_none()
            && mob.get_controlling_passenger().is_none()
            && patrol.has_patrol_target()
            && !is_on_cooldown
    }

    fn controls(&self) -> Controls {
        Controls::MOVE
    }

    fn tick(&mut self, mob: &dyn Mob) {
        let Some(patrol) = mob.as_patrolling_monster() else {
            return;
        };
        if !mob.get_mob_entity().steered().navigator().is_done() {
            return;
        }
        let Some(patrol_target) = patrol.get_patrol_target() else {
            return;
        };
        let is_leader = patrol.is_patrol_leader();
        let companions = Self::find_patrol_companions(mob);
        let pos = mob.get_entity().pos.load();
        let target_center = Vector3::new(
            f64::from(patrol_target.0.x) + 0.5,
            f64::from(patrol_target.0.y) + 0.5,
            f64::from(patrol_target.0.z) + 0.5,
        );
        if patrol.is_patrolling() && companions.is_empty() {
            patrol.set_patrolling(false);
        } else if is_leader
            && target_center.squared_distance_to_vec(&pos)
                < PATROL_TARGET_REACHED_DISTANCE * PATROL_TARGET_REACHED_DISTANCE
        {
            patrol.find_patrol_target();
        } else {
            let world = mob.get_entity().world.load();
            let path_target = Self::next_step(&world, patrol_target, pos);
            let speed = if is_leader {
                self.leader_speed_modifier
            } else {
                self.speed_modifier
            };
            if Self::move_to(mob, path_target, speed) {
                if is_leader {
                    for companion in &companions {
                        if let Some(companion) =
                            companion.get_mob().and_then(Mob::as_patrolling_monster)
                        {
                            companion.set_patrol_target(path_target);
                        }
                    }
                }
            } else {
                self.move_randomly(mob, &world);
                self.cooldown_until = world.get_world_age() + NAVIGATION_FAILED_COOLDOWN;
            }
        }
    }
}
