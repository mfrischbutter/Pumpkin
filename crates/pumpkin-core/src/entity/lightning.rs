//! Vanilla `LightningBolt`.

use rustc_hash::FxHashSet;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicI32, AtomicI64, Ordering},
};

use pumpkin_data::game_event::GameEvent;
use pumpkin_data::tag::{self, Taggable};
use pumpkin_data::world::WorldEvent;
use pumpkin_data::{Block, BlockStateId};
use pumpkin_util::Difficulty;
use pumpkin_util::math::boundingbox::BoundingBox;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_world::world::BlockFlags;
use rand::RngExt;

use crate::block::blocks::fire::{FireBlockBase, fire::FireBlock};
use crate::block::blocks::weathering_copper::{
    get_first_state, get_previous_state, get_weather_state,
};
use crate::entity::player::Player;
use crate::entity::player::advancement::trigger::AdvancementTrigger;
use crate::entity::{Entity, EntityBase};
use crate::server::Server;
use crate::world::World;

/// Vanilla `START_LIFE`.
const START_LIFE: i32 = 2;
/// Vanilla `DAMAGE_RADIUS`.
const DAMAGE_RADIUS: f64 = 3.0;
/// Vanilla `DETECTION_RADIUS`.
const DETECTION_RADIUS: f64 = 15.0;
/// What vanilla adds to the top of its damage and detection boxes.
const BOX_EXTRA_HEIGHT: f64 = 6.0;
/// Vanilla `tick`: the distance within which players count the strike for the advancement.
const ADVANCEMENT_PLAYER_DISTANCE: f64 = 256.0;

pub struct LightningBoltEntity {
    entity: Entity,
    life: AtomicI32,
    seed: AtomicI64,
    flashes: AtomicI32,
    visual_only: AtomicBool,
    cause: Mutex<Option<Arc<Player>>>,
    hit_entities: Mutex<FxHashSet<i32>>,
    blocks_set_on_fire: AtomicI32,
}

impl LightningBoltEntity {
    pub fn new(entity: Entity) -> Self {
        let seed = rand::rng().random::<i64>();
        let flashes = rand::rng().random_range(1..=3);
        Self {
            entity,
            life: AtomicI32::new(START_LIFE),
            seed: AtomicI64::new(seed),
            flashes: AtomicI32::new(flashes),
            visual_only: AtomicBool::new(false),
            cause: Mutex::new(None),
            hit_entities: Mutex::new(FxHashSet::default()),
            blocks_set_on_fire: AtomicI32::new(0),
        }
    }

    pub fn set_visual_only(&self, visual_only: bool) {
        self.visual_only.store(visual_only, Ordering::Relaxed);
    }

    pub fn is_visual_only(&self) -> bool {
        self.visual_only.load(Ordering::Relaxed)
    }

    pub fn set_cause(&self, cause: Option<Arc<Player>>) {
        if let Ok(mut c) = self.cause.lock() {
            *c = cause;
        }
    }

    pub fn get_cause(&self) -> Option<Arc<Player>> {
        self.cause.lock().ok().and_then(|c| c.clone())
    }

    pub fn get_blocks_set_on_fire(&self) -> i32 {
        self.blocks_set_on_fire.load(Ordering::Relaxed)
    }

    pub fn get_seed(&self) -> i64 {
        self.seed.load(Ordering::Relaxed)
    }

    pub fn set_seed(&self, seed: i64) {
        self.seed.store(seed, Ordering::Relaxed);
    }

    fn get_strike_position(&self) -> BlockPos {
        let pos = self.entity.pos.load();
        Vector3::new(pos.x, pos.y - 1.0e-6, pos.z).to_block_pos()
    }

    /// Vanilla `powerLightningRod`, for every rod including the oxidized ones.
    fn power_lightning_rod(&self, world: &Arc<World>) {
        let strike_pos = self.get_strike_position();
        let block = world.get_block(&strike_pos);
        if block.has_tag(&tag::Block::MINECRAFT_LIGHTNING_RODS) {
            crate::block::blocks::redstone::lightning_rod::LightningRodBlock::trigger(
                world,
                &strike_pos,
            );
        }
    }

    /// Vanilla `BaseFireBlock.getState`, soul fire on its soil and else a fire facing what
    /// burns around it.
    fn fire_state_at(world: &Arc<World>, pos: &BlockPos) -> BlockStateId {
        let fire = FireBlockBase::get_fire_type(world, pos);
        if fire == Block::FIRE {
            FireBlock.get_state_for_position(world, &Block::FIRE, pos)
        } else {
            fire.default_state.id
        }
    }

    /// Vanilla `spawnFire`.
    fn spawn_fire(&self, world: &Arc<World>, additional_sources: i32) {
        if self.visual_only.load(Ordering::Relaxed) {
            return;
        }

        let pos = self.entity.block_pos.load();
        if !world.can_spread_fire_around(&pos) {
            return;
        }

        let try_place = |p: BlockPos| {
            if world.get_block_state(&p).is_air() && FireBlockBase::can_place_at(world, &p) {
                let fire_state = Self::fire_state_at(world, &p);
                world.set_block_state(&p, fire_state, BlockFlags::NOTIFY_ALL);
                self.blocks_set_on_fire.fetch_add(1, Ordering::Relaxed);
            }
        };

        try_place(pos);

        for _ in 0..additional_sources {
            let dx = rand::rng().random_range(-1..=1);
            let dy = rand::rng().random_range(-1..=1);
            let dz = rand::rng().random_range(-1..=1);
            let nearby_pos = pos.offset(Vector3::new(dx, dy, dz));
            try_place(nearby_pos);
        }
    }

    /// Vanilla `clearCopperOnLightningStrike`: the struck copper back to its first stage, and
    /// random walks that scrape one stage off what they cross. Waxed copper keeps its stage but
    /// still starts the walks.
    fn clear_copper_on_lightning_strike(world: &Arc<World>, struck_pos: &BlockPos) {
        let (struck_block, struck_state) = world.get_block_and_state_id(struck_pos);
        // Vanilla `HoneycombItem.WAX_OFF_BY_BLOCK` holds every waxed copper block.
        let is_waxed = struck_block.name.starts_with("waxed_");
        let is_weathering = get_weather_state(struck_block).is_some();
        if !is_weathering && !is_waxed {
            return;
        }
        if is_weathering {
            world.set_block_state(
                struck_pos,
                get_first_state(struck_block, struck_state),
                BlockFlags::NOTIFY_ALL,
            );
        }

        let strikes_count = rand::rng().random_range(3..=5);
        for _ in 0..strikes_count {
            let step_count = rand::rng().random_range(1..=8);
            Self::random_walk_cleaning_copper(world, struck_pos, step_count);
        }
    }

    fn random_walk_cleaning_copper(
        world: &Arc<World>,
        original_strike_pos: &BlockPos,
        step_count: i32,
    ) {
        let mut work_pos = *original_strike_pos;

        for _ in 0..step_count {
            if let Some(next_pos) = Self::random_step_cleaning_copper(world, &work_pos) {
                work_pos = next_pos;
            } else {
                break;
            }
        }
    }

    /// Vanilla `randomStepCleaningCopper`: unoxidized copper is a step as well, it only has
    /// nothing to lose.
    fn random_step_cleaning_copper(world: &Arc<World>, pos: &BlockPos) -> Option<BlockPos> {
        for candidate in random_in_cube(10, pos, 1) {
            let (block, state_id) = world.get_block_and_state_id(&candidate);
            if get_weather_state(block).is_some() {
                if let Some(previous) = get_previous_state(block, state_id) {
                    world.set_block_state(&candidate, previous, BlockFlags::NOTIFY_ALL);
                }
                world.sync_world_event(WorldEvent::ParticlesElectricSpark, candidate, -1);
                return Some(candidate);
            }
        }
        None
    }

    /// The first tick of vanilla `tick`. The thunder and impact sounds are not here: the client
    /// plays them itself when its copy of the bolt ticks, so sending them would double them.
    fn strike(&self) {
        let entity = &self.entity;
        let world = entity.world.load_full();
        let difficulty = world.level_info.load().difficulty;
        if difficulty == Difficulty::Normal || difficulty == Difficulty::Hard {
            self.spawn_fire(&world, 4);
        }

        self.power_lightning_rod(&world);
        Self::clear_copper_on_lightning_strike(&world, &self.get_strike_position());
        world.emit_game_event(GameEvent::LightningStrike.name(), entity.pos.load());
    }

    /// Vanilla `tick` on a live tick: every alive entity in the box is hit again, also those
    /// from earlier flashes, and the channeling player gets the advancement.
    fn hit_entities_in_box(&self) {
        let entity = &self.entity;
        let world = entity.world.load_full();
        let pos = entity.pos.load();
        let damage_box = BoundingBox::new(
            Vector3::new(
                pos.x - DAMAGE_RADIUS,
                pos.y - DAMAGE_RADIUS,
                pos.z - DAMAGE_RADIUS,
            ),
            Vector3::new(
                pos.x + DAMAGE_RADIUS,
                pos.y + BOX_EXTRA_HEIGHT + DAMAGE_RADIUS,
                pos.z + DAMAGE_RADIUS,
            ),
        );
        let cause = self.get_cause();

        for hit_entity in world.get_all_at_box(&damage_box) {
            if !is_alive(hit_entity.as_ref()) {
                continue;
            }
            let hit_id = hit_entity.get_entity().entity_id;
            if hit_id == entity.entity_id {
                continue;
            }
            hit_entity.on_lightning_strike(hit_entity.as_ref(), self);
            self.hit_entities
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .insert(hit_id);
            if let Some(player) = &cause {
                player.trigger_advancement(AdvancementTrigger::ChanneledLightning {
                    victim_type: entity_type_name(hit_entity.as_ref()),
                });
            }
        }
    }

    /// The end of vanilla `tick`: `LIGHTNING_STRIKE` for every player near enough, naming the
    /// bystanders the bolt did not hit.
    fn trigger_strike_advancement(&self) {
        let entity = &self.entity;
        let world = entity.world.load_full();
        let pos = entity.pos.load();
        let detection_box = BoundingBox::new(
            Vector3::new(
                pos.x - DETECTION_RADIUS,
                pos.y - DETECTION_RADIUS,
                pos.z - DETECTION_RADIUS,
            ),
            Vector3::new(
                pos.x + DETECTION_RADIUS,
                pos.y + BOX_EXTRA_HEIGHT + DETECTION_RADIUS,
                pos.z + DETECTION_RADIUS,
            ),
        );
        let bystanders: Vec<String> = {
            let hit_entities = self
                .hit_entities
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            world
                .get_all_at_box(&detection_box)
                .iter()
                .filter(|viewer| {
                    let id = viewer.get_entity().entity_id;
                    id != entity.entity_id
                        && is_alive(viewer.as_ref())
                        && !hit_entities.contains(&id)
                })
                .map(|viewer| entity_type_name(viewer.as_ref()))
                .collect()
        };
        let blocks_set_on_fire = self.blocks_set_on_fire.load(Ordering::Relaxed);
        for player in world.players.load().iter() {
            let distance = player.position().squared_distance_to_vec(&pos).sqrt();
            if distance >= ADVANCEMENT_PLAYER_DISTANCE {
                continue;
            }
            let trigger = |bystander_type| {
                player.trigger_advancement(AdvancementTrigger::LightningStrike {
                    bystander_type,
                    blocks_set_on_fire,
                    distance,
                });
            };
            trigger(None);
            for bystander in &bystanders {
                trigger(Some(bystander.clone()));
            }
        }
    }
}

impl EntityBase for LightningBoltEntity {
    fn tick(&self, caller: &dyn EntityBase, server: &Server) {
        let entity = &self.entity;
        entity.tick(caller, server);

        let life = self.life.load(Ordering::Relaxed);
        if life == START_LIFE {
            self.strike();
        }

        let new_life = life - 1;
        self.life.store(new_life, Ordering::Relaxed);

        if new_life < 0 {
            let flashes = self.flashes.load(Ordering::Relaxed);
            if flashes == 0 {
                self.trigger_strike_advancement();
                entity.remove();
                return;
            } else if new_life < -rand::rng().random_range(0..10) {
                self.flashes.store(flashes - 1, Ordering::Relaxed);
                self.life.store(1, Ordering::Relaxed);
                self.seed.store(rand::random::<i64>(), Ordering::Relaxed);
                self.spawn_fire(&entity.world.load_full(), 0);
            }
        }

        if self.life.load(Ordering::Relaxed) >= 0 && !self.visual_only.load(Ordering::Relaxed) {
            self.hit_entities_in_box();
        }
    }

    fn init_data_tracker(&self) {}

    fn get_entity(&self) -> &Entity {
        &self.entity
    }

    fn get_living_entity(&self) -> Option<&crate::entity::living::LivingEntity> {
        None
    }
    fn cast_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// Vanilla `Entity.isAlive`, which for living entities also asks for health left.
fn is_alive(entity: &dyn EntityBase) -> bool {
    entity.get_entity().is_alive()
        && entity
            .get_living_entity()
            .is_none_or(|living| !living.is_dead_or_dying())
}

fn entity_type_name(entity: &dyn EntityBase) -> String {
    format!(
        "minecraft:{}",
        entity.get_entity().entity_type.resource_name
    )
}

fn random_in_cube(count: usize, center: &BlockPos, radius: i32) -> Vec<BlockPos> {
    let mut list = Vec::with_capacity(count);
    for _ in 0..count {
        let dx = rand::rng().random_range(-radius..=radius);
        let dy = rand::rng().random_range(-radius..=radius);
        let dz = rand::rng().random_range(-radius..=radius);
        list.push(center.offset(Vector3::new(dx, dy, dz)));
    }
    list
}
