//! Vanilla `PatrolSpawner`: pillager patrols near a player away from villages, one of the
//! overworld's custom spawners.

use std::sync::Arc;

use pumpkin_data::entity::EntityType;
use pumpkin_data::environment_attribute::EnvironmentAttribute;
use pumpkin_util::math::position::BlockPos;
use pumpkin_world::chunk::ChunkHeightmapType::MotionBlockingNoLeaves;
use rand::RngExt;
use uuid::Uuid;

use crate::entity::EntityBase;
use crate::entity::mob::MobEntity;
use crate::entity::mob::equipment::RegionalDifficulty;
use crate::entity::mob::spawn::{EntitySpawnReason, finalize_spawn};
use crate::entity::r#type::from_type;
use crate::world::World;
use crate::world::natural_spawner::is_valid_empty_spawn_block;

/// Ticks between two rolls: vanilla waits `12000 + nextInt(1200)`.
const DELAY_TICKS: i32 = 12_000;
const DELAY_RANGE_TICKS: i32 = 1_200;
/// One roll in five goes on.
const ROLL_ODDS: i32 = 5;
/// The patrol spawns 24 to 47 blocks from the player on each axis.
const MIN_OFFSET: i32 = 24;
const OFFSET_RANGE: i32 = 24;
/// The chunks this far around the spot must be loaded.
const LOADED_RANGE: i32 = 10;
/// Vanilla `isCloseToVillage(pos, 2)`: no patrol this close to a village.
const VILLAGE_SECTION_DISTANCE: i32 = 2;
/// Vanilla `PatrollingMonster.checkPatrollingMonsterSpawnRules`: block light above this
/// stops a member.
const MAX_BLOCK_LIGHT: u8 = 8;
/// Each further member stands `nextInt(5) - nextInt(5)` blocks aside.
const MEMBER_SPREAD: i32 = 5;

#[derive(Default)]
pub struct PatrolSpawner {
    next_tick: i32,
}

impl PatrolSpawner {
    /// Vanilla `PatrolSpawner.tick`: every ten to eleven minutes by day, one roll in five
    /// puts a patrol near a random player who is not close to a village.
    pub fn tick(&mut self, world: &Arc<World>, spawn_enemies: bool) {
        if !spawn_enemies || !world.level_info.load().game_rules.spawn_patrols {
            return;
        }
        let mut rng = rand::rng();
        self.next_tick -= 1;
        if self.next_tick > 0 {
            return;
        }
        self.next_tick += DELAY_TICKS + rng.random_range(0..DELAY_RANGE_TICKS);
        if !world.is_bright_outside() || rng.random_range(0..ROLL_ODDS) != 0 {
            return;
        }
        let players = world.players.load();
        if players.is_empty() {
            return;
        }
        let player = &players[rng.random_range(0..players.len())];
        if player.is_spectator() {
            return;
        }
        let player_pos = player.get_entity().block_pos.load();
        if world.is_close_to_village(&player_pos, VILLAGE_SECTION_DISTANCE) {
            return;
        }
        let dx = (MIN_OFFSET + rng.random_range(0..OFFSET_RANGE)) * random_sign(&mut rng);
        let dz = (MIN_OFFSET + rng.random_range(0..OFFSET_RANGE)) * random_sign(&mut rng);
        let mut spawn_pos = player_pos.add(dx, 0, dz);
        if !world.has_chunks_at(
            spawn_pos.0.x - LOADED_RANGE,
            spawn_pos.0.z - LOADED_RANGE,
            spawn_pos.0.x + LOADED_RANGE,
            spawn_pos.0.z + LOADED_RANGE,
        ) || !world.environment_attributes().get_value_bool(
            EnvironmentAttribute::GameplayCanPillagerPatrolSpawn,
            &spawn_pos,
        ) {
            return;
        }
        let difficulty = RegionalDifficulty::at(world, spawn_pos.to_f64());
        let group_size = difficulty.effective_difficulty.ceil() as i32 + 1;
        for index in 0..group_size {
            // `get_heightmap_height` is the top block, vanilla's height the air above it.
            spawn_pos.0.y =
                world.get_heightmap_height(MotionBlockingNoLeaves, spawn_pos.0.x, spawn_pos.0.z)
                    + 1;
            if index == 0 {
                if !spawn_patrol_member(world, spawn_pos, true) {
                    break;
                }
            } else {
                spawn_patrol_member(world, spawn_pos, false);
            }
            spawn_pos.0.x +=
                rng.random_range(0..MEMBER_SPREAD) - rng.random_range(0..MEMBER_SPREAD);
            spawn_pos.0.z +=
                rng.random_range(0..MEMBER_SPREAD) - rng.random_range(0..MEMBER_SPREAD);
        }
    }
}

/// Vanilla `random.nextBoolean() ? -1 : 1`.
fn random_sign<R: rand::Rng + ?Sized>(rng: &mut R) -> i32 {
    if rng.random::<bool>() { -1 } else { 1 }
}

/// Vanilla `PatrolSpawner.spawnPatrolMember`: a pillager at `pos` when the block is free
/// and dim enough; the leader gets its patrol target.
fn spawn_patrol_member(world: &Arc<World>, pos: BlockPos, is_leader: bool) -> bool {
    if !is_valid_empty_spawn_block(world.get_block_state(&pos), &EntityType::PILLAGER) {
        return false;
    }
    if world.get_block_light_level(&pos).unwrap_or(0) > MAX_BLOCK_LIGHT
        || !MobEntity::check_any_light_monster_spawn_rules(
            &EntityType::PILLAGER,
            world,
            EntitySpawnReason::Patrol,
            &pos,
        )
    {
        return false;
    }
    let pillager = from_type(&EntityType::PILLAGER, pos.to_f64(), world, Uuid::new_v4());
    if is_leader
        && let Some(patroller) = pillager
            .get_mob()
            .and_then(|mob| mob.as_patrolling_monster())
    {
        patroller.set_patrol_leader(true);
        patroller.find_patrol_target();
    }
    finalize_spawn(&pillager, world, None);
    world.spawn_entity_non_save(pillager);
    true
}
