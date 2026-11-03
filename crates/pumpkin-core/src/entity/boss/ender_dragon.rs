use pumpkin_data::damage::DamageType;
use pumpkin_data::entity::EntityType;
use pumpkin_data::tag::{self, Taggable};
use pumpkin_data::tracked_data;
use pumpkin_data::world::WorldEvent;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_util::math::wrap_degrees;
use pumpkin_world::chunk::ChunkHeightmapType;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::{Arc, Mutex};

use crate::entity::ai::target_predicate::TargetPredicate;
use crate::entity::experience_orb::ExperienceOrbEntity;
use crate::entity::projectile::dragon_fireball::DragonFireballEntity;
use crate::entity::{
    Entity, EntityBase,
    mob::{Mob, MobEntity},
    player::Player,
};
use crate::world::World;
use crate::world::dragon_fight::DragonFight;

pub mod body;
pub mod flight_history;
pub mod part;
pub mod path;
pub mod phase;

use flight_history::DragonFlightHistory;
use part::{BODY, EnderDragonPart, HEAD, NECK, PART_SIZES};
use path::{DragonPath, INNER_NODES_START, Nodes};
use phase::{EnderDragonPhase, PhaseManager};

pub const DEATH_TIMER_MAX: i32 = 200;
const CRYSTAL_RANGE: f64 = 32.0;
/// Vanilla's `invulnerableTime` runs from 20 and its `hurtTime` from 10, so this is `hurtTime > 0`.
const HURT_RECENTLY_COOLDOWN: i32 = 10;

fn normalized(vector: Vector3<f64>) -> Vector3<f64> {
    if vector.length_squared() < 1.0E-10 {
        Vector3::new(0.0, 0.0, 0.0)
    } else {
        vector.normalize()
    }
}

/// The angle in degrees between where the dragon faces and `target`, with vanilla's `+ 0.5`.
#[must_use]
pub fn horizontal_aim_angle(dragon: &EnderDragonEntity, target: Vector3<f64>) -> f32 {
    let own = dragon.position();
    let aim = normalized(Vector3::new(target.x - own.x, 0.0, target.z - own.z));
    let yaw = dragon
        .mob_entity
        .living_entity
        .entity
        .yaw
        .load()
        .to_radians();
    let dir = Vector3::new(f64::from(yaw.sin()), 0.0, -f64::from(yaw.cos()));
    (dir.dot(&aim) as f32).acos().to_degrees() + 0.5
}

/// The dragon's parts for a projectile, which never hits the dragon's own box; anything else is
/// its own target.
#[must_use]
pub fn projectile_targets(entity: &Arc<dyn EntityBase>) -> Vec<Arc<dyn EntityBase>> {
    entity
        .cast_any()
        .downcast_ref::<EnderDragonEntity>()
        .map_or_else(
            || vec![entity.clone()],
            |dragon| {
                dragon
                    .parts
                    .iter()
                    .map(|part| part.clone() as Arc<dyn EntityBase>)
                    .collect()
            },
        )
}

pub struct EnderDragonEntity {
    pub mob_entity: MobEntity,
    pub parts: Vec<Arc<EnderDragonPart>>,
    /// The next hit came through the head or the neck.
    hit_in_full: AtomicBool,
    /// The health before the hit being taken, for what sitting endures.
    health_before_hit: Mutex<f32>,
    flight_history: Mutex<DragonFlightHistory>,
    in_wall: AtomicBool,
    death_time: AtomicI32,
    sitting_damage_received: Mutex<f32>,
    nearest_crystal: Mutex<Option<Arc<dyn EntityBase>>>,
    fight_origin: Mutex<BlockPos>,
    pub phase_manager: PhaseManager,
    phase: Mutex<EnderDragonPhase>,
    yaw_rot_accel: Mutex<f32>,
    nodes: Mutex<Option<Nodes>>,
}

impl EnderDragonEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        entity.no_physics.store(true, Ordering::Relaxed);
        let base_id = entity.entity_id;
        let dragon_uuid = entity.entity_uuid;
        let world = entity.world.load();

        // Clients number the parts after the dragon's id.
        let _ = Entity::reserve_ids(8);
        let parts = PART_SIZES
            .iter()
            .enumerate()
            .map(|(index, &(width, height))| {
                let part_entity = Entity::from_uuid_with_id(
                    base_id + 1 + index as i32,
                    uuid::Uuid::new_v4(),
                    world.clone(),
                    entity.pos.load(),
                    &EntityType::ENDER_DRAGON,
                );
                part_entity.set_dimensions(width, height);
                let full = index == HEAD || index == NECK;
                Arc::new(EnderDragonPart::new(part_entity, dragon_uuid, full))
            })
            .collect();

        Arc::new(Self {
            mob_entity: MobEntity::new(entity),
            parts,
            hit_in_full: AtomicBool::new(false),
            health_before_hit: Mutex::new(0.0),
            flight_history: Mutex::new(DragonFlightHistory::default()),
            in_wall: AtomicBool::new(false),
            death_time: AtomicI32::new(0),
            sitting_damage_received: Mutex::new(0.0),
            nearest_crystal: Mutex::new(None),
            fight_origin: Mutex::new(BlockPos::new(0, 0, 0)),
            phase_manager: PhaseManager::default(),
            phase: Mutex::new(EnderDragonPhase::Hovering),
            yaw_rot_accel: Mutex::new(0.0),
            nodes: Mutex::new(None),
        })
    }

    pub fn set_fight_origin(&self, pos: BlockPos) {
        *self
            .fight_origin
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = pos;
    }

    #[must_use]
    pub fn position(&self) -> Vector3<f64> {
        self.mob_entity.living_entity.entity.pos.load()
    }

    #[must_use]
    pub fn horizontal_speed(&self) -> f32 {
        let velocity = self.mob_entity.living_entity.entity.velocity.load();
        velocity.x.hypot(velocity.z) as f32
    }

    #[must_use]
    pub fn phase_type(&self) -> EnderDragonPhase {
        *self
            .phase
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Vanilla `EnderDragonPhaseManager.setPhase`, which clients follow.
    pub fn set_phase(&self, target: EnderDragonPhase) {
        let old = {
            let mut phase = self
                .phase
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if *phase == target {
                return;
            }
            std::mem::replace(&mut *phase, target)
        };
        self.phase_manager.get(old).end();
        self.mob_entity.living_entity.entity.set_synced_data(
            tracked_data::ender_dragon::PHASE,
            VarInt(target.network_id()),
        );
        self.phase_manager.get(target).begin();
    }

    /// The part with this id, vanilla `Level.getEntity` by way of `getEnderDragonParts`.
    #[must_use]
    pub fn part_by_id(&self, id: i32) -> Option<Arc<dyn EntityBase>> {
        self.parts
            .iter()
            .find(|part| part.entity.entity_id == id)
            .map(|part| part.clone() as Arc<dyn EntityBase>)
    }

    /// Vanilla `strafePlayer` of the holding pattern.
    pub fn strafe(&self, player: &Arc<Player>) {
        self.set_phase(EnderDragonPhase::StrafePlayer);
        self.phase_manager.strafe_player().set_target(self, player);
    }

    /// Runs `action` on the fight, if this dragon is the fight's dragon: a summoned one has none.
    fn with_fight<R>(&self, action: impl FnOnce(&mut DragonFight, &Arc<World>) -> R) -> Option<R> {
        let entity = &self.mob_entity.living_entity.entity;
        let world = entity.world.load_full();
        let mut fight = world
            .dragon_fight
            .as_ref()?
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        (fight.dragon_uuid == Some(entity.entity_uuid)).then(|| action(&mut fight, &world))
    }

    /// The crystals of this dragon's fight; `None` for a dragon without a fight.
    #[must_use]
    pub fn alive_crystals(&self) -> Option<i32> {
        self.with_fight(|fight, _| fight.alive_crystals())
    }

    fn first_allowed_node(&self) -> usize {
        if self.alive_crystals().unwrap_or(0) == 0 {
            INNER_NODES_START
        } else {
            0
        }
    }

    fn nodes(&self) -> Nodes {
        *self
            .nodes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get_or_insert_with(|| {
                let world = self.mob_entity.living_entity.entity.world.load();
                path::build_nodes(|x, z| {
                    world.get_heightmap_height(ChunkHeightmapType::MotionBlockingNoLeaves, x, z) + 1
                })
            })
    }

    #[must_use]
    pub fn find_closest_node(&self) -> usize {
        self.find_closest_node_to(self.position())
    }

    #[must_use]
    pub fn find_closest_node_to(&self, pos: Vector3<f64>) -> usize {
        path::closest_node(&self.nodes(), pos, self.first_allowed_node())
    }

    #[must_use]
    pub fn find_path(
        &self,
        start: usize,
        end: usize,
        final_node: Option<Vector3<i32>>,
    ) -> Option<DragonPath> {
        path::find_path(
            &self.nodes(),
            start,
            end,
            final_node,
            self.first_allowed_node(),
        )
    }

    /// The bottom center of the top of the podium, by the given heightmap.
    #[must_use]
    pub fn podium(&self, heightmap: ChunkHeightmapType) -> Vector3<f64> {
        let origin = *self
            .fight_origin
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let world = self.mob_entity.living_entity.entity.world.load();
        // `get_heightmap_height` is the top block, vanilla's `getHeightmapPos` the one above it.
        let y = world.get_heightmap_height(heightmap, origin.0.x, origin.0.z) + 1;
        Vector3::new(
            f64::from(origin.0.x) + 0.5,
            f64::from(y),
            f64::from(origin.0.z) + 0.5,
        )
    }

    /// The player `targeting` accepts nearest to `at`, of those `filter` lets through.
    #[must_use]
    pub fn nearest_player(
        &self,
        at: Vector3<f64>,
        targeting: &TargetPredicate,
        filter: impl Fn(&Player) -> bool,
    ) -> Option<Arc<Player>> {
        let world = self.mob_entity.living_entity.entity.world.load();
        world
            .players
            .load()
            .iter()
            .filter(|player| filter(player) && targeting.test(&world, Some(self), player.as_ref()))
            .min_by(|a, b| {
                let dist = |p: &Arc<Player>| at.distance_squared(p.living_entity.entity.pos.load());
                dist(a).total_cmp(&dist(b))
            })
            .cloned()
    }

    #[must_use]
    pub fn can_attack_player(&self, player: &Arc<Player>) -> bool {
        self.can_attack(player.as_ref())
    }

    #[must_use]
    pub fn sees(&self, entity: &Entity) -> bool {
        self.has_line_of_sight(entity)
    }

    /// Vanilla's `yRotA` step: damps the turn so far and adds to it.
    pub fn add_yaw_acceleration(&self, delta: f32) -> f32 {
        let mut accel = self
            .yaw_rot_accel
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *accel = *accel * 0.8 + delta;
        *accel
    }

    fn view_vector(x_rot: f32, y_rot: f32) -> Vector3<f64> {
        let (pitch, yaw) = (x_rot.to_radians(), y_rot.to_radians());
        Vector3::new(
            f64::from(-yaw.sin() * pitch.cos()),
            f64::from(-pitch.sin()),
            f64::from(yaw.cos() * pitch.cos()),
        )
    }

    /// Vanilla `getHeadLookVector`.
    #[must_use]
    pub fn head_look_vector(&self) -> Vector3<f64> {
        let entity = &self.mob_entity.living_entity.entity;
        let yaw = entity.yaw.load();
        let phase = self.phase_type();
        if phase == EnderDragonPhase::Landing || phase == EnderDragonPhase::Takeoff {
            let egg = self.podium(ChunkHeightmapType::MotionBlockingNoLeaves);
            let center = egg + Vector3::new(0.0, 0.5, 0.0);
            let dist = (center.distance_squared(self.position()).sqrt() as f32 / 4.0).max(1.0);
            Self::view_vector(-(6.0 / dist) * 1.5 * 5.0, yaw)
        } else if phase.is_sitting() {
            Self::view_vector(-45.0, yaw)
        } else {
            Self::view_vector(entity.pitch.load(), yaw)
        }
    }

    /// The fireball of vanilla's strafing phase, from just ahead of the head.
    pub fn shoot_fireball(&self, player: &Arc<Player>) {
        let entity = &self.mob_entity.living_entity.entity;
        let world = entity.world.load_full();
        let view = Self::view_vector(entity.pitch.load(), entity.yaw.load());
        let head = self.parts[HEAD].entity.pos.load();
        let start = Vector3::new(head.x - view.x, head.y + 0.5 + 0.5, head.z - view.z);
        let target = player.living_entity.entity.pos.load()
            + Vector3::new(
                0.0,
                f64::from(player.living_entity.entity.height()) * 0.5,
                0.0,
            );
        if !entity.silent.load(Ordering::Relaxed) {
            world.sync_world_event(WorldEvent::SoundDragonFireball, entity.block_pos.load(), 0);
        }
        let fireball = DragonFireballEntity::new_shot(
            Entity::new(world.clone(), start, &EntityType::DRAGON_FIREBALL),
            entity,
            start,
            normalized(target - start),
        );
        world.spawn_entity(Arc::new(fireball));
    }

    /// Vanilla `onCrystalDestroyed`: its own crystal blowing up hurts it, by the head.
    pub fn on_crystal_destroyed(&self, crystal: &dyn EntityBase, cause: Option<&dyn EntityBase>) {
        let world = self.mob_entity.living_entity.entity.world.load_full();
        let crystal_entity = crystal.get_entity();
        let by_player = cause
            .and_then(|cause| cause.get_player())
            .and_then(|player| {
                world
                    .players
                    .load()
                    .iter()
                    .find(|other| other.gameprofile.id == player.gameprofile.id)
                    .cloned()
            });
        let player = by_player.or_else(|| {
            world.get_nearest_player(crystal_entity.pos.load(), 64.0, |player| {
                !player.is_creative() && !player.is_spectator()
            })
        });
        let was_nearest = self
            .nearest_crystal
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .is_some_and(|nearest| nearest.get_entity().entity_id == crystal_entity.entity_id);
        if was_nearest {
            self.hit_in_full.store(true, Ordering::Relaxed);
            self.damage_with_context(
                self,
                10.0,
                DamageType::EXPLOSION,
                None,
                Some(crystal),
                player
                    .as_ref()
                    .map(|player| player.as_ref() as &dyn EntityBase),
            );
        }
        self.phase_manager
            .get(self.phase_type())
            .on_crystal_destroyed(self, player.as_ref());
    }

    /// Vanilla `checkCrystals`: a crystal it has found heals 1 every 10 ticks.
    fn check_crystals(&self) {
        let living = &self.mob_entity.living_entity;
        let entity = &living.entity;
        let mut nearest = self
            .nearest_crystal
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if nearest
            .as_ref()
            .is_some_and(|crystal| crystal.get_entity().is_removed())
        {
            *nearest = None;
        } else if nearest.is_some()
            && entity.tick_count.load(Ordering::Relaxed) % 10 == 0
            && living.health.load() < living.get_max_health()
        {
            living.heal(1.0);
        }
        if rand::random_range(0..10) == 0 {
            let area =
                entity
                    .bounding_box
                    .load()
                    .expand(CRYSTAL_RANGE, CRYSTAL_RANGE, CRYSTAL_RANGE);
            let own = entity.pos.load();
            *nearest = entity
                .world
                .load()
                .get_all_at_box(&area)
                .into_iter()
                .filter(|other| other.get_entity().entity_type == &EntityType::END_CRYSTAL)
                .min_by(|a, b| {
                    let dist =
                        |e: &Arc<dyn EntityBase>| own.distance_squared(e.get_entity().pos.load());
                    dist(a).total_cmp(&dist(b))
                });
        }
    }

    fn update_fight(&self) {
        let living = &self.mob_entity.living_entity;
        let custom_name = living.entity.custom_name.load();
        self.with_fight(|fight, world| {
            fight.update_dragon(
                world,
                living.entity.entity_uuid,
                living.health.load(),
                living.get_max_health(),
                custom_name.as_ref().as_ref(),
            );
        });
    }

    /// Vanilla `EnderDragon.aiStep`, which takes the place of `LivingEntity.aiStep`.
    fn ai_step(&self) {
        let living = &self.mob_entity.living_entity;
        let entity = &living.entity;
        if entity.is_removed() || living.is_dead_or_dying() {
            return;
        }
        entity.update_last_pos();
        self.check_crystals();
        entity.yaw.store(wrap_degrees(entity.yaw.load()));
        if self.mob_entity.is_no_ai() {
            return;
        }
        self.mob_entity
            .sensing
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .tick();
        self.flight_history
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .record(entity.pos.load().y, entity.yaw.load());

        let first = self.phase_type();
        self.phase_manager.get(first).tick(self);
        let current = self.phase_type();
        if current != first {
            self.phase_manager.get(current).tick(self);
        }
        let phase = self.phase_manager.get(current);
        if let Some(target) = phase.fly_target() {
            self.fly_toward(target, phase);
        }

        let yaw = entity.yaw.load();
        entity.body_yaw.store(yaw);
        entity.head_yaw.store(yaw);
        self.tick_parts();
        let in_wall = self.check_walls(&self.part_box(HEAD))
            | self.check_walls(&self.part_box(NECK))
            | self.check_walls(&self.part_box(BODY));
        self.in_wall.store(in_wall, Ordering::Relaxed);
        self.update_fight();
    }

    /// Vanilla `tickDeath`: rises for 10 seconds and gives its experience on the way.
    fn tick_death(&self) {
        self.update_fight();
        let entity = &self.mob_entity.living_entity.entity;
        let world = entity.world.load_full();
        let time = self.death_time.fetch_add(1, Ordering::Relaxed) + 1;
        let first_kill = self
            .with_fight(|fight, _| !fight.has_previously_killed_dragon())
            .unwrap_or(false);
        let xp_count = if first_kill { 12000.0 } else { 500.0 };
        let drops = world.level_info.load().game_rules.mob_drops;
        if time > 150 && time % 5 == 0 && drops {
            ExperienceOrbEntity::spawn(&world, entity.pos.load(), (xp_count * 0.08f32) as u32);
        }
        if time == 1 && !entity.silent.load(Ordering::Relaxed) {
            world.sync_world_event(WorldEvent::SoundDragonDeath, entity.block_pos.load(), 0);
        }
        entity.update_last_pos();
        let rise = Vector3::new(0.0, 0.1, 0.0);
        entity.set_pos(entity.pos.load() + rise);
        for part in &self.parts {
            part.entity.set_pos(part.entity.pos.load() + rise);
        }
        if time >= DEATH_TIMER_MAX {
            if drops {
                ExperienceOrbEntity::spawn(&world, entity.pos.load(), (xp_count * 0.2f32) as u32);
            }
            self.end_fight();
            entity.remove();
        }
    }

    fn end_fight(&self) {
        let uuid = self.mob_entity.living_entity.entity.entity_uuid;
        self.with_fight(|fight, world| fight.set_dragon_killed(world, uuid));
    }
}

impl Mob for EnderDragonEntity {
    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    fn mob_init_data_tracker(&self) {
        self.mob_entity.living_entity.entity.set_synced_data(
            tracked_data::ender_dragon::PHASE,
            VarInt(self.phase_type().network_id()),
        );
    }

    fn mob_tick(&self, _caller: &dyn EntityBase) {
        self.ai_step();
    }

    fn has_own_ai_step(&self) -> bool {
        true
    }

    fn mob_tick_death(&self) -> bool {
        self.tick_death();
        true
    }

    /// Vanilla `handleKillingBlow` and `kill`: only `/kill` ends it on the spot. A flying dragon
    /// flies to the podium at 1 health, and a sitting one dies where it sits.
    fn mob_handle_killing_blow(&self, damage_type: DamageType) -> bool {
        if damage_type == DamageType::GENERIC_KILL {
            self.update_fight();
            self.end_fight();
            self.mob_entity.living_entity.entity.remove();
        } else if !self.phase_type().is_sitting() {
            self.mob_entity.living_entity.set_health(1.0);
            self.set_phase(EnderDragonPhase::Dying);
        }
        true
    }

    /// The checks of vanilla `EnderDragon.hurt`: only players and what always hurts it, not
    /// while dying, and a sitting dragon burns the arrows it catches.
    fn pre_damage_with_context(
        &self,
        damage_type: DamageType,
        source: Option<&dyn EntityBase>,
        cause: Option<&dyn EntityBase>,
    ) -> bool {
        if damage_type == DamageType::GENERIC_KILL {
            return true;
        }
        let phase = self.phase_type();
        if phase == EnderDragonPhase::Dying {
            return false;
        }
        if phase.stops_projectiles()
            && let Some(direct) = source
        {
            let kind = direct.get_entity().entity_type;
            if kind == &EntityType::ARROW
                || kind == &EntityType::SPECTRAL_ARROW
                || kind == &EntityType::TRIDENT
                || kind == &EntityType::WIND_CHARGE
                || kind == &EntityType::BREEZE_WIND_CHARGE
            {
                direct.get_entity().set_on_fire_for(1.0);
                return false;
            }
        }
        let by_player = cause.or(source).is_some_and(|by| by.get_player().is_some());
        if !by_player
            && !damage_type.has_tag(&tag::DamageType::MINECRAFT_ALWAYS_HURTS_ENDER_DRAGONS)
        {
            return false;
        }
        *self
            .health_before_hit
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            self.mob_entity.living_entity.health.load();
        true
    }

    /// Vanilla `EnderDragon.hurt`: a part other than the head or neck takes a quarter.
    fn modify_incoming_damage_with_context(
        &self,
        amount: f32,
        _damage_type: DamageType,
        _source: Option<&dyn EntityBase>,
        _cause: Option<&dyn EntityBase>,
    ) -> f32 {
        if self.hit_in_full.swap(false, Ordering::Relaxed) {
            amount
        } else {
            amount / 4.0 + amount.min(1.0)
        }
    }

    /// Vanilla `EnderDragon.hurt`: a sitting dragon that lost a quarter of its health flies up.
    fn on_damage(&self, _damage_type: DamageType, _source: Option<&dyn EntityBase>) {
        if !self.phase_type().is_sitting() {
            return;
        }
        let living = &self.mob_entity.living_entity;
        let lost = *self
            .health_before_hit
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            - living.health.load();
        let mut received = self
            .sitting_damage_received
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *received += lost;
        if *received > 0.25 * living.get_max_health() {
            *received = 0.0;
            drop(received);
            self.set_phase(EnderDragonPhase::Takeoff);
        }
    }

    fn get_mob_gravity(&self) -> f64 {
        0.0
    }

    fn mob_is_pushable(&self) -> bool {
        false
    }

    /// Vanilla `checkDespawn` does nothing for a dragon.
    fn remove_when_far_away(&self, _distance_sq: f64) -> bool {
        false
    }

    fn get_ambient_sound(&self) -> Option<pumpkin_data::sound::Sound> {
        Some(pumpkin_data::sound::Sound::EntityEnderDragonAmbient)
    }

    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        nbt.put_int("DragonPhase", self.phase_type().network_id());
        nbt.put_int("DragonDeathTime", self.death_time.load(Ordering::Relaxed));
        nbt.put_float(
            "sitting_damage_received",
            *self
                .sitting_damage_received
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        );
    }

    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        if let Some(phase) = nbt.get_int("DragonPhase") {
            self.set_phase(EnderDragonPhase::from_id(phase));
        }
        if let Some(time) = nbt.get_int("DragonDeathTime") {
            self.death_time.store(time, Ordering::Relaxed);
        }
        if let Some(received) = nbt.get_float("sitting_damage_received") {
            *self
                .sitting_damage_received
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = received;
        }
    }
}

pub trait Vector3Ext {
    fn distance_squared(self, other: Self) -> f64;
}

impl Vector3Ext for Vector3<f64> {
    fn distance_squared(self, other: Self) -> f64 {
        let (dx, dy, dz) = (self.x - other.x, self.y - other.y, self.z - other.z);
        dx * dx + dy * dy + dz * dz
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_dragon_looks_along_its_yaw() {
        let south = EnderDragonEntity::view_vector(0.0, 0.0);
        assert!((south.z - 1.0).abs() < 1.0E-6 && south.x.abs() < 1.0E-6);
        let west = EnderDragonEntity::view_vector(0.0, 90.0);
        assert!((west.x + 1.0).abs() < 1.0E-6);
        let up = EnderDragonEntity::view_vector(-45.0, 0.0);
        assert!(up.y > 0.7);
    }
}
