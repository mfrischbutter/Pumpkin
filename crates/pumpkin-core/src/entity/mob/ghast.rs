use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};

use pumpkin_data::damage::DamageType;
use pumpkin_data::entity::EntityType;
use pumpkin_data::sound::Sound;
use pumpkin_data::tag::{self, Taggable};
use pumpkin_data::tracked_data;
use pumpkin_data::world::WorldEvent;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_world::chunk::ChunkHeightmapType;
use rand::RngExt;
use rand::rngs::ThreadRng;

use crate::entity::ai::behavior::hunting::{self, Hunt};
use crate::entity::ai::control::ghast_move_control::GhastMoveControl;
use crate::entity::ai::target_match::TargetMatch;
use crate::entity::custom_sound::CustomSound;
use crate::entity::projectile::fireball::{DEFAULT_EXPLOSION_POWER, FireballEntity};
use crate::entity::{
    Entity, EntityBase,
    ai::goal::{Controls, Goal},
    mob::{Mob, MobEntity},
};

const GHAST_HUNTS: &[Hunt] = &[Hunt::new(1, TargetMatch::Type(&EntityType::PLAYER), true)
    .matching(hunting::within_four_blocks_height)];

const XP_REWARD: u32 = 5;
/// Vanilla `getSoundVolume`.
const SOUND_VOLUME: f32 = 5.0;
/// Vanilla `travel`: `travelFlying(input, 0.02F)`.
const AIR_SPEED: f32 = 0.02;
/// What a fireball a player sent back deals, vanilla `hurtServer`.
const REFLECTED_FIREBALL_DAMAGE: f32 = 1000.0;
/// Vanilla `maxDist * maxDist`: beyond 64 blocks the ghast neither faces nor charges.
const MAX_DISTANCE_SQ: f64 = 4096.0;
/// Charge ticks until the warning sound; above it the ghast shows as charging.
const WARN_CHARGE_TIME: i32 = 10;
const SHOOT_CHARGE_TIME: i32 = 20;
/// Charge time after a shot, so the next one takes three seconds.
const CHARGE_TIME_AFTER_SHOT: i32 = -40;
/// How far in front of the ghast the fireball starts.
const FIREBALL_OFFSET: f64 = 4.0;
/// Vanilla `RandomFloatAroundGoal.MAX_ATTEMPTS`.
const MAX_ATTEMPTS: usize = 64;
/// Blocks per axis a random float target lies from the ghast.
const FLOAT_RANGE: f32 = 16.0;
const FLOAT_SPEED: f64 = 1.0;
/// A wanted position nearer than this is reached, a farther one is given up.
const MIN_WANTED_DISTANCE_SQ: f64 = 1.0;
const MAX_WANTED_DISTANCE_SQ: f64 = 3600.0;

/// Represents a Ghast, the big floating Nether mob that shoots exploding fireballs.
///
/// Wiki: <https://minecraft.wiki/w/Ghast>
pub struct GhastEntity {
    pub mob_entity: MobEntity,
    charging: AtomicBool,
    explosion_power: AtomicI32,
}

impl GhastEntity {
    #[must_use]
    pub fn new(entity: Entity) -> Arc<Self> {
        let mut mob_entity = MobEntity::new(entity);
        *mob_entity
            .move_control
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            Box::new(GhastMoveControl::default());
        let ghast = Arc::new(Self {
            mob_entity,
            charging: AtomicBool::new(false),
            explosion_power: AtomicI32::new(DEFAULT_EXPLOSION_POWER),
        });

        let goal_selector = &ghast.mob_entity.goals_selector;
        goal_selector.add_goal(5, Box::new(RandomFloatAroundGoal));
        goal_selector.add_goal(7, Box::new(GhastLookGoal));
        goal_selector.add_goal(7, Box::new(GhastShootFireballGoal::default()));

        let target_selector = &ghast.mob_entity.target_selector;
        hunting::apply(target_selector, &ghast.mob_entity, GHAST_HUNTS);

        ghast
    }

    #[must_use]
    pub fn is_charging(&self) -> bool {
        self.charging.load(Ordering::Relaxed)
    }

    pub fn set_charging(&self, charging: bool) {
        self.charging.store(charging, Ordering::Relaxed);
        self.get_entity()
            .set_synced_data(tracked_data::ghast::DATA_IS_CHARGING, charging);
    }

    /// The power of the fireballs this ghast shoots, NBT `ExplosionPower`.
    #[must_use]
    pub fn explosion_power(&self) -> i32 {
        self.explosion_power.load(Ordering::Relaxed)
    }
}

/// Vanilla `Ghast.isReflectedFireball`: a large fireball a player sent back.
fn is_reflected_fireball(source: Option<&dyn EntityBase>, cause: Option<&dyn EntityBase>) -> bool {
    source.is_some_and(|source| source.get_entity().entity_type == &EntityType::FIREBALL)
        && cause.is_some_and(|cause| cause.get_player().is_some())
}

impl Mob for GhastEntity {
    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    fn get_ambient_sound(&self) -> Option<Sound> {
        Some(Sound::EntityGhastAmbient)
    }

    fn as_custom_sound(&self) -> Option<&dyn CustomSound> {
        Some(self)
    }

    fn travel(&self, caller: &dyn EntityBase) -> bool {
        self.mob_entity
            .living_entity
            .travel_flying_at(caller, AIR_SPEED);
        true
    }

    /// Vanilla `checkFallDamage` does nothing for a ghast.
    fn should_check_fall_damage(&self) -> bool {
        false
    }

    fn mob_init_data_tracker(&self) {
        self.get_entity()
            .set_synced_data(tracked_data::ghast::DATA_IS_CHARGING, self.is_charging());
    }

    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        nbt.put_byte("ExplosionPower", self.explosion_power() as i8);
    }

    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        if let Some(power) = nbt.get_byte("ExplosionPower") {
            self.explosion_power
                .store(i32::from(power), Ordering::Relaxed);
        }
    }

    fn modify_incoming_damage_with_context(
        &self,
        amount: f32,
        _damage_type: DamageType,
        source: Option<&dyn EntityBase>,
        cause: Option<&dyn EntityBase>,
    ) -> f32 {
        if is_reflected_fireball(source, cause) {
            REFLECTED_FIREBALL_DAMAGE
        } else {
            amount
        }
    }

    /// Vanilla `Ghast.isInvulnerableTo`: a reflected fireball gets past the fire immunity.
    fn mob_is_invulnerable_to(
        &self,
        damage_type: &DamageType,
        source: Option<&dyn EntityBase>,
        cause: Option<&dyn EntityBase>,
    ) -> bool {
        let entity = self.get_entity();
        let invulnerable = entity.invulnerable.load(Ordering::Relaxed)
            && !damage_type.has_tag(&tag::DamageType::MINECRAFT_BYPASSES_INVULNERABILITY);
        let reflected = is_reflected_fireball(source, cause);
        invulnerable || (!reflected && entity.is_invulnerable_to(damage_type, cause))
    }

    fn get_base_experience_reward(&self) -> u32 {
        XP_REWARD
    }
}

impl CustomSound for GhastEntity {
    fn death_sound(&self) -> Option<Sound> {
        None
    }

    fn hurt_sound(&self) -> Option<Sound> {
        None
    }

    fn sound_volume(&self) -> f32 {
        SOUND_VOLUME
    }
}

fn as_ghast(mob: &dyn Mob) -> Option<&GhastEntity> {
    mob.cast_any().downcast_ref::<GhastEntity>()
}

/// Vanilla `Ghast.faceMovementDirection`: towards the target within 64 blocks, else where
/// it flies.
fn face_movement_direction(mob: &dyn Mob) {
    let entity = mob.get_entity();
    let Some(target) = mob.get_mob_entity().get_target() else {
        let movement = entity.velocity.load();
        face(entity, movement.x, movement.z);
        return;
    };
    let pos = entity.pos.load();
    let target_pos = target.get_entity().pos.load();
    if target_pos.squared_distance_to_vec(&pos) < MAX_DISTANCE_SQ {
        face(entity, target_pos.x - pos.x, target_pos.z - pos.z);
    }
}

/// Turns the rotation and the body towards the horizontal direction `(x, z)`.
fn face(entity: &Entity, x: f64, z: f64) {
    let yaw = -(x.atan2(z) as f32).to_degrees();
    entity.yaw.store(yaw);
    entity.body_yaw.store(yaw);
}

/// Vanilla `Ghast.GhastLookGoal`.
struct GhastLookGoal;

impl Goal for GhastLookGoal {
    fn can_start(&mut self, _mob: &dyn Mob) -> bool {
        true
    }

    fn should_run_every_tick(&self) -> bool {
        true
    }

    fn tick(&mut self, mob: &dyn Mob) {
        face_movement_direction(mob);
    }

    fn controls(&self) -> Controls {
        Controls::LOOK
    }
}

/// Vanilla `Ghast.GhastShootFireballGoal`: charges while it sees its target, then shoots.
#[derive(Default)]
struct GhastShootFireballGoal {
    charge_time: i32,
}

impl Goal for GhastShootFireballGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        mob.get_mob_entity().get_target().is_some()
    }

    fn start(&mut self, _mob: &dyn Mob) {
        self.charge_time = 0;
    }

    fn stop(&mut self, mob: &dyn Mob) {
        if let Some(ghast) = as_ghast(mob) {
            ghast.set_charging(false);
        }
    }

    fn should_run_every_tick(&self) -> bool {
        true
    }

    fn tick(&mut self, mob: &dyn Mob) {
        let Some(target) = mob.get_mob_entity().get_target() else {
            return;
        };
        let entity = mob.get_entity();
        let target_entity = target.get_entity();
        let distance_sq = target_entity
            .pos
            .load()
            .squared_distance_to_vec(&entity.pos.load());
        if distance_sq < MAX_DISTANCE_SQ && mob.has_line_of_sight(target_entity) {
            self.charge_time += 1;
            if self.charge_time == WARN_CHARGE_TIME && !entity.is_silent() {
                let world = entity.world.load();
                world.sync_world_event(WorldEvent::SoundGhastWarning, entity.block_pos.load(), 0);
            }
            if self.charge_time == SHOOT_CHARGE_TIME {
                shoot_fireball(mob, target_entity);
                self.charge_time = CHARGE_TIME_AFTER_SHOT;
            }
        } else if self.charge_time > 0 {
            self.charge_time -= 1;
        }

        if let Some(ghast) = as_ghast(mob) {
            ghast.set_charging(self.charge_time > WARN_CHARGE_TIME);
        }
    }

    fn controls(&self) -> Controls {
        Controls::empty()
    }
}

/// The shot of vanilla `GhastShootFireballGoal.tick`: a large fireball from 4 blocks in front
/// of the ghast towards the middle of the target.
fn shoot_fireball(mob: &dyn Mob, target: &Entity) {
    let entity = mob.get_entity();
    let world = entity.world.load_full();
    let pos = entity.pos.load();
    let view = entity.rotation().to_f64();
    let target_pos = target.pos.load();
    let spawn_pos = Vector3::new(
        pos.x + view.x * FIREBALL_OFFSET,
        pos.y + f64::from(entity.height()) * 0.5 + 0.5,
        pos.z + view.z * FIREBALL_OFFSET,
    );
    let direction = Vector3::new(
        target_pos.x - spawn_pos.x,
        target_pos.y + f64::from(target.height()) * 0.5 - spawn_pos.y,
        target_pos.z - spawn_pos.z,
    );

    if !entity.is_silent() {
        world.sync_world_event(WorldEvent::SoundGhastFireball, entity.block_pos.load(), 0);
    }

    let base_entity = Entity::new(world.clone(), spawn_pos, &EntityType::FIREBALL);
    let fireball = FireballEntity::new_shot(base_entity, entity, direction);
    // `ThrownItemEntity::new` puts the projectile at the shooter's eye, re-anchor it.
    fireball.thrown.entity.pos.store(spawn_pos);
    let power = as_ghast(mob).map_or(DEFAULT_EXPLOSION_POWER, GhastEntity::explosion_power);
    fireball.set_explosion_power(power);
    world.spawn_entity(Arc::new(fireball));
}

/// Vanilla `Ghast.RandomFloatAroundGoal` as the ghast uses it, with no distance to blocks.
struct RandomFloatAroundGoal;

impl Goal for RandomFloatAroundGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        let mob_entity = mob.get_mob_entity();
        let steered = mob_entity.steered();
        let move_control = steered.move_control();
        if !move_control.has_wanted() {
            return true;
        }
        let pos = mob_entity.living_entity.entity.pos.load();
        let distance_sq = move_control
            .get_wanted_position()
            .squared_distance_to_vec(&pos);
        !(MIN_WANTED_DISTANCE_SQ..=MAX_WANTED_DISTANCE_SQ).contains(&distance_sq)
    }

    fn should_continue(&mut self, _mob: &dyn Mob) -> bool {
        false
    }

    fn start(&mut self, mob: &dyn Mob) {
        let target = suitable_fly_to_position(mob);
        mob.get_mob_entity()
            .steered()
            .move_control()
            .set_wanted_position(target.x, target.y, target.z, FLOAT_SPEED);
    }

    fn controls(&self) -> Controls {
        Controls::MOVE
    }
}

/// Vanilla `RandomFloatAroundGoal.getSuitableFlyToPosition` with no distance to blocks.
///
/// Any random position in the home is good. Only when none was, a random one is taken,
/// mirrored downwards when it lies above the ground.
fn suitable_fly_to_position(mob: &dyn Mob) -> Vector3<f64> {
    let mob_entity = mob.get_mob_entity();
    let entity = &mob_entity.living_entity.entity;
    let center = entity.pos.load();
    let mut rng = mob.get_random();
    for _ in 0..MAX_ATTEMPTS {
        let target = random_position(center, &mut rng);
        if mob_entity.is_in_position_target_range_pos(&BlockPos::floored_v(target)) {
            return target;
        }
    }

    let target = random_position(center, &mut rng);
    let pos = BlockPos::floored_v(target);
    let world = entity.world.load();
    let top = world.get_heightmap_height(ChunkHeightmapType::MotionBlocking, pos.0.x, pos.0.z);
    // Vanilla `getHeight`: the first free block above the top one.
    let height = top + 1;
    if height < pos.0.y && height > world.min_y {
        return Vector3::new(target.x, center.y - (center.y - target.y).abs(), target.z);
    }
    target
}

/// Vanilla `RandomFloatAroundGoal.chooseRandomPosition`.
fn random_position(center: Vector3<f64>, rng: &mut ThreadRng) -> Vector3<f64> {
    let mut offset = || f64::from((rng.random::<f32>() * 2.0 - 1.0) * FLOAT_RANGE);
    Vector3::new(
        center.x + offset(),
        center.y + offset(),
        center.z + offset(),
    )
}
