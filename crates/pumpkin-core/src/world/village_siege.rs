//! Vanilla `VillageSiege`: on one night in ten, twenty zombies on the village a player is
//! in, one of the overworld's custom spawners.

use std::sync::Arc;

use pumpkin_data::entity::EntityType;
use pumpkin_data::tag::Taggable;
use pumpkin_data::tag::WorldgenBiome::MINECRAFT_WITHOUT_ZOMBIE_SIEGES;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_world::chunk::ChunkHeightmapType::WorldSurface;
use rand::RngExt;
use uuid::Uuid;

use crate::entity::EntityBase;
use crate::entity::mob::MobEntity;
use crate::entity::mob::spawn::{EntitySpawnReason, finalize_spawn};
use crate::entity::r#type::from_type;
use crate::world::World;

/// One night in ten gets a siege.
const SIEGE_ODDS: i32 = 10;
/// Vanilla `ClockTimeMarkers.ROLL_VILLAGE_SIEGE` on the day clock: tick 18000 of a day.
const ROLL_TIME_OF_DAY: i64 = 18_000;
const DAY_TICKS: i64 = 24_000;
const ZOMBIES_PER_SIEGE: i32 = 20;
/// Ticks between two zombies.
const SPAWN_INTERVAL: i32 = 2;
/// Tries to place the siege 32 blocks from the player.
const SETUP_ATTEMPTS: i32 = 10;
const SIEGE_DISTANCE: f32 = 32.0;
/// Tries to find a spot within 8 blocks of the siege position.
const SPAWN_ATTEMPTS: i32 = 10;
const SPAWN_SPREAD: i32 = 16;

#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    SiegeTonight,
    SiegeDone,
}

pub struct VillageSiege {
    has_setup_siege: bool,
    state: State,
    zombies_to_spawn: i32,
    next_spawn_time: i32,
    spawn_pos: BlockPos,
}

impl Default for VillageSiege {
    fn default() -> Self {
        Self {
            has_setup_siege: false,
            state: State::SiegeDone,
            zombies_to_spawn: 0,
            next_spawn_time: 0,
            spawn_pos: BlockPos::ZERO,
        }
    }
}

impl VillageSiege {
    /// Vanilla `VillageSiege.tick`: the roll at tick 18000 of a night, then a zombie every
    /// other tick until twenty came; daylight or no enemies end it.
    pub fn tick(&mut self, world: &Arc<World>, spawn_enemies: bool) {
        if world.is_bright_outside() || !spawn_enemies {
            self.state = State::SiegeDone;
            self.has_setup_siege = false;
            return;
        }
        if world.get_time_of_day() % DAY_TICKS == ROLL_TIME_OF_DAY {
            self.state = if rand::rng().random_range(0..SIEGE_ODDS) == 0 {
                State::SiegeTonight
            } else {
                State::SiegeDone
            };
        }
        if self.state == State::SiegeDone {
            return;
        }
        if !self.has_setup_siege {
            if !self.try_to_setup_siege(world) {
                return;
            }
            self.has_setup_siege = true;
        }
        if self.next_spawn_time > 0 {
            self.next_spawn_time -= 1;
            return;
        }
        self.next_spawn_time = SPAWN_INTERVAL;
        if self.zombies_to_spawn > 0 {
            self.try_spawn(world);
            self.zombies_to_spawn -= 1;
        } else {
            self.state = State::SiegeDone;
        }
    }

    /// Vanilla `VillageSiege.tryToSetupSiege`: the first non-spectator player in a village
    /// whose biome allows sieges gets one; the siege position is 32 blocks away in a random
    /// direction that has a spawn spot.
    fn try_to_setup_siege(&mut self, world: &Arc<World>) -> bool {
        let mut rng = rand::rng();
        for player in world.players.load().iter() {
            if player.is_spectator() {
                continue;
            }
            let center = player.get_entity().block_pos.load();
            if !world.is_village(&center)
                || world
                    .get_biome(&center)
                    .has_tag(&MINECRAFT_WITHOUT_ZOMBIE_SIEGES)
            {
                continue;
            }
            for _ in 0..SETUP_ATTEMPTS {
                let angle = rng.random::<f32>() * std::f32::consts::TAU;
                self.spawn_pos = BlockPos::new(
                    center.0.x + (angle.cos() * SIEGE_DISTANCE).floor() as i32,
                    center.0.y,
                    center.0.z + (angle.sin() * SIEGE_DISTANCE).floor() as i32,
                );
                if find_random_spawn_pos(world, self.spawn_pos).is_some() {
                    self.next_spawn_time = 0;
                    self.zombies_to_spawn = ZOMBIES_PER_SIEGE;
                    break;
                }
            }
            return true;
        }
        false
    }

    /// Vanilla `VillageSiege.trySpawn`: a zombie finalized where vanilla creates it, at the
    /// origin, then moved to the spot.
    fn try_spawn(&self, world: &Arc<World>) {
        let Some(spawn_pos) = find_random_spawn_pos(world, self.spawn_pos) else {
            return;
        };
        let zombie = from_type(
            &EntityType::ZOMBIE,
            Vector3::default(),
            world,
            Uuid::new_v4(),
        );
        finalize_spawn(&zombie, world, None);
        zombie.get_entity().set_pos(spawn_pos);
        zombie
            .get_entity()
            .set_rotation(rand::rng().random::<f32>() * 360.0, 0.0);
        world.spawn_entity_non_save(zombie);
    }
}

/// Vanilla `VillageSiege.findRandomSpawnPos`: a surface spot within 8 blocks of `pos` that
/// is in the village and passes the monster spawn rules.
fn find_random_spawn_pos(world: &Arc<World>, pos: BlockPos) -> Option<Vector3<f64>> {
    let mut rng = rand::rng();
    for _ in 0..SPAWN_ATTEMPTS {
        let x = pos.0.x + rng.random_range(0..SPAWN_SPREAD) - SPAWN_SPREAD / 2;
        let z = pos.0.z + rng.random_range(0..SPAWN_SPREAD) - SPAWN_SPREAD / 2;
        // `get_heightmap_height` is the top block, vanilla's height the air above it.
        let y = world.get_heightmap_height(WorldSurface, x, z) + 1;
        let spot = BlockPos::new(x, y, z);
        if world.is_village(&spot)
            && MobEntity::check_monster_spawn_rules(
                &EntityType::ZOMBIE,
                world,
                EntitySpawnReason::Event,
                &spot,
                world.is_thundering(),
            )
        {
            return Some(Vector3::new(
                f64::from(x) + 0.5,
                f64::from(y),
                f64::from(z) + 0.5,
            ));
        }
    }
    None
}
