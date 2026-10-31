//! Vanilla `PhantomSpawner`: phantoms above players who have not slept for three days,
//! one of the overworld's custom spawners.

use std::sync::Arc;

use pumpkin_data::entity::EntityType;
use pumpkin_data::statistic::CustomStatistic;
use rand::RngExt;
use uuid::Uuid;

use crate::entity::EntityBase;
use crate::entity::mob::equipment::RegionalDifficulty;
use crate::entity::mob::spawn::finalize_spawn;
use crate::entity::r#type::from_type;
use crate::world::World;
use crate::world::natural_spawner::is_valid_empty_spawn_block;

/// Seconds between two attempts: vanilla rolls `(60 + nextInt(60)) * 20` ticks.
const MIN_DELAY_SECONDS: i32 = 60;
const DELAY_RANGE_SECONDS: i32 = 60;
/// Phantoms only come once the sky has darkened this far.
const MIN_SKY_DARKEN: i32 = 5;
/// The local difficulty must beat `nextFloat() * 3`.
const DIFFICULTY_ROLL: f32 = 3.0;
/// Three days without rest (`TIME_SINCE_REST`) before the roll can pass.
const TIME_SINCE_REST_THRESHOLD: i32 = 72_000;
/// The group spawns 20 to 34 blocks above the player and up to 10 blocks aside.
const MIN_HEIGHT_ABOVE: i32 = 20;
const HEIGHT_RANGE: i32 = 15;
const SIDE_RANGE: i32 = 10;

#[derive(Default)]
pub struct PhantomSpawner {
    next_tick: i32,
}

impl PhantomSpawner {
    /// Vanilla `PhantomSpawner.tick`: every one to two minutes at night, each player who
    /// has not slept for three days may get one to four phantoms above them.
    pub fn tick(&mut self, world: &Arc<World>, spawn_enemies: bool) {
        if !spawn_enemies || !world.level_info.load().game_rules.spawn_phantoms {
            return;
        }
        let mut rng = rand::rng();
        self.next_tick -= 1;
        if self.next_tick > 0 {
            return;
        }
        self.next_tick += (MIN_DELAY_SECONDS + rng.random_range(0..DELAY_RANGE_SECONDS)) * 20;
        let has_skylight = world.dimension.has_skylight;
        if world.get_sky_darken() < MIN_SKY_DARKEN && has_skylight {
            return;
        }
        for player in world.players.load().iter() {
            if player.is_spectator() {
                continue;
            }
            let player_pos = player.get_entity().block_pos.load();
            if has_skylight && (player_pos.0.y < world.sea_level || !world.can_see_sky(&player_pos))
            {
                continue;
            }
            let difficulty = RegionalDifficulty::at(world, player_pos.to_f64());
            if difficulty.effective_difficulty <= rng.random::<f32>() * DIFFICULTY_ROLL {
                continue;
            }
            let time_since_rest = player
                .get_custom_stat(CustomStatistic::TimeSinceRest)
                .max(1);
            if rng.random_range(0..time_since_rest) < TIME_SINCE_REST_THRESHOLD {
                continue;
            }
            let dy = MIN_HEIGHT_ABOVE + rng.random_range(0..HEIGHT_RANGE);
            let dx = rng.random_range(-SIDE_RANGE..=SIDE_RANGE);
            let dz = rng.random_range(-SIDE_RANGE..=SIDE_RANGE);
            let spawn_pos = player_pos.add(dx, dy, dz);
            if !is_valid_empty_spawn_block(world.get_block_state(&spawn_pos), &EntityType::PHANTOM)
            {
                continue;
            }
            let group_size = 1 + rng.random_range(0..=difficulty.base_difficulty as i32);
            let mut group_data = None;
            for _ in 0..group_size {
                let phantom = from_type(
                    &EntityType::PHANTOM,
                    spawn_pos.to_f64(),
                    world,
                    Uuid::new_v4(),
                );
                group_data = finalize_spawn(&phantom, world, group_data);
                world.spawn_entity_non_save(phantom);
            }
        }
    }
}
