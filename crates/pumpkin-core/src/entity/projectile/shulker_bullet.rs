use std::cmp::Ordering;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::AtomicI32;
use std::sync::atomic::Ordering::Relaxed;

use crossbeam::atomic::AtomicCell;
use pumpkin_data::BlockDirection;
use pumpkin_data::BlockState;
use pumpkin_data::block_properties::Axis;
use pumpkin_data::damage::DamageType;
use pumpkin_data::effect::StatusEffect;
use pumpkin_data::entity::EntityType;
use pumpkin_data::game_event::GameEvent;
use pumpkin_data::particle::Particle;
use pumpkin_data::potion::Effect;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::Difficulty;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use rand::RngExt;
use rand::rngs::ThreadRng;

use crate::entity::living::LivingEntity;
use crate::entity::projectile::owner::{EntityReference, ProjectileOwner};
use crate::entity::projectile::{ProjectileHit, ThrownItemEntity};
use crate::entity::{Entity, EntityBase};
use crate::server::Server;
use crate::world::World;
use crate::world::game_event::GameEventContext;

/// Vanilla `SPEED`: how fast the bullet steers along one leg of its path.
const SPEED: f64 = 0.15;
/// Vanilla `getDefaultGravity`, which pulls the bullet down once it has no live target.
const GRAVITY: f64 = 0.04;
/// Per tick the steering speeds up by this factor, up to one block per tick on each axis.
const STEERING_GROWTH: f64 = 1.025;
/// The share of the gap to the steering the motion closes per tick.
const STEERING_RESPONSE: f64 = 0.2;
/// Closer than this to the centre of its target block the bullet flies straight at it.
const DIRECT_FLIGHT_DISTANCE_SQ: f64 = 4.0;
/// Height in the target block to aim at without a target.
const DEFAULT_Y_OFFSET: f64 = 0.5;
/// Tries for a random free direction when no step leads towards the target.
const RANDOM_DIRECTION_ATTEMPTS: i32 = 5;
const HIT_DAMAGE: f32 = 4.0;
const LEVITATION_TICKS: i32 = 200;
const PARTICLE_SPREAD: f32 = 0.2;
const HIT_PARTICLES: i32 = 2;
const HURT_PARTICLES: i32 = 15;

/// Represents a Shulker Bullet, which homes in on its target in axis steps.
///
/// Wiki: <https://minecraft.wiki/w/Shulker_Bullet>
pub struct ShulkerBulletEntity {
    thrown: ThrownItemEntity,
    final_target: EntityReference,
    current_move_direction: AtomicCell<Option<BlockDirection>>,
    flight_steps: AtomicI32,
    target_delta: AtomicCell<Vector3<f64>>,
}

impl ShulkerBulletEntity {
    /// Vanilla `ShulkerBullet(level, owner, target, invalidStartAxis)`.
    ///
    /// The bullet starts in the middle of the shooter and its first leg avoids
    /// `invalid_start_axis`, the axis the shulker is attached along.
    #[must_use]
    pub fn new(owner: &Entity, target: &Entity, invalid_start_axis: Axis) -> Self {
        let bounds = owner.bounding_box.load();
        let center = Vector3::new(
            f64::midpoint(bounds.min.x, bounds.max.x),
            f64::midpoint(bounds.min.y, bounds.max.y),
            f64::midpoint(bounds.min.z, bounds.max.z),
        );
        let entity = Entity::new(owner.world.load_full(), center, &EntityType::SHULKER_BULLET);
        let bullet = Self {
            thrown: Self::projectile(entity, ProjectileOwner::new(Some(owner))),
            final_target: EntityReference::new(Some(target)),
            current_move_direction: AtomicCell::new(Some(BlockDirection::Up)),
            flight_steps: AtomicI32::new(0),
            target_delta: AtomicCell::new(Vector3::new(0.0, 0.0, 0.0)),
        };
        bullet.select_next_move_direction(Some(invalid_start_axis), Some(target));
        bullet
    }

    /// A bullet without shooter and target, as loaded from disk or summoned.
    #[must_use]
    pub const fn orphan(entity: Entity) -> Self {
        Self {
            thrown: Self::projectile(entity, ProjectileOwner::none()),
            final_target: EntityReference::none(),
            current_move_direction: AtomicCell::new(None),
            flight_steps: AtomicI32::new(0),
            target_delta: AtomicCell::new(Vector3::new(0.0, 0.0, 0.0)),
        }
    }

    const fn projectile(entity: Entity, owner: ProjectileOwner) -> ThrownItemEntity {
        ThrownItemEntity {
            entity,
            owner,
            collides_with_projectiles: false,
            has_hit: AtomicBool::new(false),
            gravity: 0.0,
        }
    }

    /// Vanilla `selectNextMoveDirection`: the next leg, one block along an axis towards the
    /// target, or straight at it once it is near.
    fn select_next_move_direction(&self, avoid_axis: Option<Axis>, target: Option<&Entity>) {
        let entity = &self.thrown.entity;
        let pos = entity.pos.load();
        let current = entity.block_pos.load();
        let (target_pos, y_offset) = aim(current, target);
        let mut rng = rand::rng();

        let near = target_pos.dist_to_center_sqr(pos.x, pos.y, pos.z) < DIRECT_FLIGHT_DISTANCE_SQ;
        let (wanted, selection) = if near {
            let wanted = Vector3::new(
                f64::from(target_pos.0.x) + 0.5,
                f64::from(target_pos.0.y) + y_offset,
                f64::from(target_pos.0.z) + 0.5,
            );
            (wanted, None)
        } else {
            let world = entity.world.load();
            let direction = pick_direction(&world, current, target_pos, avoid_axis, &mut rng);
            (pos + direction.to_offset().to_f64(), Some(direction))
        };
        self.current_move_direction.store(selection);

        let delta = wanted - pos;
        let distance = delta.length();
        let target_delta = if distance == 0.0 {
            Vector3::new(0.0, 0.0, 0.0)
        } else {
            delta * (SPEED / distance)
        };
        self.target_delta.store(target_delta);
        self.flight_steps
            .store(10 + rng.random_range(0..5) * 10, Relaxed);
    }

    /// Vanilla `tick`, the motion before the move: steer towards a live target, else fall.
    fn update_motion(&self, live_target: bool) {
        let entity = &self.thrown.entity;
        let movement = entity.velocity.load();
        if !live_target {
            if !entity.has_no_gravity() {
                entity
                    .velocity
                    .store(Vector3::new(movement.x, movement.y - GRAVITY, movement.z));
            }
            return;
        }
        let delta = self.target_delta.load();
        let steering = Vector3::new(
            (delta.x * STEERING_GROWTH).clamp(-1.0, 1.0),
            (delta.y * STEERING_GROWTH).clamp(-1.0, 1.0),
            (delta.z * STEERING_GROWTH).clamp(-1.0, 1.0),
        );
        self.target_delta.store(steering);
        entity
            .velocity
            .store(movement + (steering - movement) * STEERING_RESPONSE);
    }

    /// Vanilla `tick`, the path after the move: a new leg when this one is used up, blocked
    /// or level with the target.
    fn update_path(&self, world: &World, target: &Entity) {
        let steps = self.flight_steps.load(Relaxed);
        if steps > 0 {
            self.flight_steps.store(steps - 1, Relaxed);
            if steps == 1 {
                let axis = self.current_move_direction.load().map(|d| d.to_axis());
                self.select_next_move_direction(axis, Some(target));
            }
        }

        let Some(direction) = self.current_move_direction.load() else {
            return;
        };
        let current = self.thrown.entity.block_pos.load();
        let axis = direction.to_axis();
        let blocked = can_stand_on(world, current.offset(direction.to_offset()));
        let target_pos = target.block_pos.load();
        if blocked || axis_coordinate(current, axis) == axis_coordinate(target_pos, axis) {
            self.select_next_move_direction(Some(axis), Some(target));
        }
    }

    /// Vanilla `ShulkerBullet.onHitEntity`: 4 damage and levitation for 10 seconds.
    fn hit_entity(&self, world: &World, target: &dyn EntityBase) {
        let owner = self.thrown.owner.get(world);
        let living_owner = owner
            .as_deref()
            .filter(|owner| owner.get_living_entity().is_some());
        let hurt = target.damage_with_context(
            target,
            HIT_DAMAGE,
            DamageType::MOB_PROJECTILE,
            None,
            Some(self),
            living_owner,
        );
        if hurt && let Some(living) = target.get_living_entity() {
            living.add_effect(Effect {
                effect_type: &StatusEffect::LEVITATION,
                duration: LEVITATION_TICKS,
                amplifier: 0,
                ambient: false,
                show_particles: true,
                show_icon: true,
                blend: false,
            });
        }
    }

    /// The game event of vanilla `destroy`, for a bullet being removed.
    fn emit_destroyed(&self, world: &World) {
        world.emit_game_event_with_context(
            GameEvent::EntityDamage.name(),
            self.thrown.entity.pos.load(),
            &GameEventContext {
                source_entity: Some(self),
            },
        );
    }

    fn spawn_particles(&self, world: &World, particle: Particle, count: i32) {
        world.spawn_particle(
            self.thrown.entity.pos.load(),
            Vector3::new(PARTICLE_SPREAD, PARTICLE_SPREAD, PARTICLE_SPREAD),
            0.0,
            count,
            particle,
        );
    }
}

/// The block the bullet steers to and the height in it to aim at.
fn aim(current: BlockPos, target: Option<&Entity>) -> (BlockPos, f64) {
    target.map_or((current.down(), DEFAULT_Y_OFFSET), |target| {
        let y_offset = f64::from(target.height()) * 0.5;
        let pos = target.pos.load();
        (BlockPos::floored(pos.x, pos.y + y_offset, pos.z), y_offset)
    })
}

/// A free step towards `target` on an axis other than `avoid_axis`, else a random free step.
fn pick_direction(
    world: &World,
    current: BlockPos,
    target: BlockPos,
    avoid_axis: Option<Axis>,
    rng: &mut ThreadRng,
) -> BlockDirection {
    let mut options = Vec::with_capacity(3);
    for axis in [Axis::X, Axis::Y, Axis::Z] {
        if avoid_axis == Some(axis) {
            continue;
        }
        let towards = positive_direction(axis);
        let from = axis_coordinate(current, axis);
        let to = axis_coordinate(target, axis);
        let direction = match from.cmp(&to) {
            Ordering::Less => towards,
            Ordering::Greater => towards.opposite(),
            Ordering::Equal => continue,
        };
        if is_empty_block(world, current.offset(direction.to_offset())) {
            options.push(direction);
        }
    }

    // Vanilla always draws a random direction first, also when it then picks an option.
    let mut selection = random_direction(rng);
    if !options.is_empty() {
        return options[rng.random_range(0..options.len())];
    }
    let mut attempts = RANDOM_DIRECTION_ATTEMPTS;
    while attempts > 0 && !is_empty_block(world, current.offset(selection.to_offset())) {
        selection = random_direction(rng);
        attempts -= 1;
    }
    selection
}

/// Vanilla `Direction.getRandom`.
fn random_direction(rng: &mut ThreadRng) -> BlockDirection {
    BlockDirection::all()[rng.random_range(0..6)]
}

const fn positive_direction(axis: Axis) -> BlockDirection {
    match axis {
        Axis::X => BlockDirection::East,
        Axis::Y => BlockDirection::Up,
        Axis::Z => BlockDirection::South,
    }
}

const fn axis_coordinate(pos: BlockPos, axis: Axis) -> i32 {
    match axis {
        Axis::X => pos.0.x,
        Axis::Y => pos.0.y,
        Axis::Z => pos.0.z,
    }
}

/// Vanilla `isEmptyBlock`; unloaded blocks read as air.
fn is_empty_block(world: &World, pos: BlockPos) -> bool {
    world
        .get_block_state_if_loaded(&pos)
        .is_none_or(BlockState::is_air)
}

/// Vanilla `loadedAndEntityCanStandOn`: a loaded block with a full top face.
fn can_stand_on(world: &World, pos: BlockPos) -> bool {
    world
        .get_block_state_if_loaded(&pos)
        .is_some_and(|state| state.is_side_solid(BlockDirection::Up))
}

/// Vanilla's `isAlive` check of the target, which spectators fail as well.
fn is_live_target(target: &Arc<dyn EntityBase>) -> bool {
    let alive = target
        .get_living_entity()
        .map_or_else(|| target.get_entity().is_alive(), LivingEntity::is_alive);
    alive && !target.is_spectator()
}

impl EntityBase for ShulkerBulletEntity {
    fn get_projectile_owner(&self) -> Option<&ProjectileOwner> {
        Some(&self.thrown.owner)
    }

    fn get_entity(&self) -> &Entity {
        &self.thrown.entity
    }

    fn get_living_entity(&self) -> Option<&LivingEntity> {
        None
    }

    fn cast_any(&self) -> &dyn std::any::Any {
        self
    }

    /// Vanilla `getSoundSource`.
    fn sound_source(&self) -> SoundCategory {
        SoundCategory::Hostile
    }

    /// Vanilla `hurtServer`: any hit destroys the bullet.
    fn damage_with_context(
        &self,
        _caller: &dyn EntityBase,
        _amount: f32,
        _damage_type: DamageType,
        _position: Option<Vector3<f64>>,
        _source: Option<&dyn EntityBase>,
        _cause: Option<&dyn EntityBase>,
    ) -> bool {
        let entity = &self.thrown.entity;
        if entity.is_removed() {
            return false;
        }
        let world = entity.world.load();
        self.play_entity_sound(Sound::EntityShulkerBulletHurt, 1.0, 1.0);
        self.spawn_particles(&world, Particle::Crit, HURT_PARTICLES);
        entity.remove();
        self.emit_destroyed(&world);
        true
    }

    fn tick(&self, caller: &dyn EntityBase, _server: &Server) {
        let entity = &self.thrown.entity;
        let world = entity.world.load_full();
        // Vanilla `checkDespawn`.
        if world.level_info.load().difficulty == Difficulty::Peaceful {
            entity.remove();
            return;
        }

        let target = self.final_target.get(&world);
        if target.is_none() {
            self.final_target.clear();
        }
        self.update_motion(target.as_ref().is_some_and(is_live_target));
        self.thrown.move_and_collide(caller);
        if entity.is_removed() {
            return;
        }
        if let Some(target) = target {
            self.update_path(&world, target.get_entity());
        }
    }

    /// Vanilla `onHitEntity` and `onHitBlock`; the projectile step removes the bullet after.
    fn on_hit(&self, hit: ProjectileHit) {
        let world = self.thrown.entity.world.load_full();
        match hit {
            ProjectileHit::Entity { ref entity, .. } => self.hit_entity(&world, entity.as_ref()),
            ProjectileHit::Block { .. } => {
                self.spawn_particles(&world, Particle::Explosion, HIT_PARTICLES);
                self.play_entity_sound(Sound::EntityShulkerBulletHit, 1.0, 1.0);
            }
        }
        self.emit_destroyed(&world);
    }

    fn write_custom_nbt(&self, nbt: &mut NbtCompound) {
        self.final_target.write_nbt(nbt, "Target");
        if let Some(direction) = self.current_move_direction.load() {
            nbt.put_byte("Dir", direction.to_index() as i8);
        }
        nbt.put_int("Steps", self.flight_steps.load(Relaxed));
        let delta = self.target_delta.load();
        nbt.put_double("TXD", delta.x);
        nbt.put_double("TYD", delta.y);
        nbt.put_double("TZD", delta.z);
    }

    /// Updates only the keys present, so a partial read keeps the rest.
    fn read_custom_nbt(&self, nbt: &NbtCompound) {
        if let Some(steps) = nbt.get_int("Steps") {
            self.flight_steps.store(steps, Relaxed);
        }
        let delta = self.target_delta.load();
        let delta = Vector3::new(
            nbt.get_double("TXD").unwrap_or(delta.x),
            nbt.get_double("TYD").unwrap_or(delta.y),
            nbt.get_double("TZD").unwrap_or(delta.z),
        );
        self.target_delta.store(delta);
        if nbt.has("Dir") {
            let direction = nbt
                .get_byte("Dir")
                .and_then(|index| BlockDirection::from_index(index as u8));
            self.current_move_direction.store(direction);
        }
        self.final_target.read_nbt(nbt, "Target");
    }
}
