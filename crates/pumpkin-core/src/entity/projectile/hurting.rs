//! Vanilla `AbstractHurtingProjectile`, the flight of fireballs, wither skulls and wind charges,
//! and vanilla `Projectile.deflect` with the rules for who may hit what.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};

use pumpkin_data::entity::EntityType;
use pumpkin_data::potion::Effect;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::tag::{self, Taggable};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::math::vector3::Vector3;

use super::fireball::{DEFLECTION_SCALE, INITIAL_ACCELERATION_POWER};
use super::owner::ProjectileOwner;
use super::{ProjectileHit, ThrownItemEntity, is_projectile};
use crate::entity::decoration::armor_stand::ArmorStandEntity;
use crate::entity::living::LivingEntity;
use crate::entity::player::Player;
use crate::entity::projectile_deflection::ProjectileDeflectionType;
use crate::entity::{Entity, EntityBase};
use crate::server::Server;

/// Vanilla `AbstractHurtingProjectile.tick`: how fast it turns to its heading.
const ROTATION_SPEED: f32 = 0.2;
/// Vanilla `AbstractHurtingProjectile.tick`: `igniteForSeconds(1.0F)`.
const BURN_TICKS: i32 = 20;
/// Vanilla `Projectile.lerpRotation`.
const LERP_ROTATION_SPEED: f32 = 0.2;
/// The kinds besides projectiles whose vanilla `Entity.isPickable` stays false.
const NOT_PICKABLE: [&EntityType; 11] = [
    &EntityType::ITEM,
    &EntityType::EXPERIENCE_ORB,
    &EntityType::EVOKER_FANGS,
    &EntityType::LIGHTNING_BOLT,
    &EntityType::OMINOUS_ITEM_SPAWNER,
    &EntityType::EYE_OF_ENDER,
    &EntityType::DRAGON_FIREBALL,
    &EntityType::MARKER,
    &EntityType::BLOCK_DISPLAY,
    &EntityType::ITEM_DISPLAY,
    &EntityType::TEXT_DISPLAY,
];

/// Vanilla `AbstractHurtingProjectile.accelerationPower`.
pub struct AccelerationPower(AtomicU64);

impl AccelerationPower {
    #[must_use]
    pub const fn new(power: f64) -> Self {
        Self(AtomicU64::new(power.to_bits()))
    }

    #[must_use]
    pub fn get(&self) -> f64 {
        f64::from_bits(self.0.load(Relaxed))
    }

    pub fn set(&self, power: f64) {
        self.0.store(power.to_bits(), Relaxed);
    }

    /// Vanilla `AbstractHurtingProjectile.onDeflection`.
    pub fn on_deflection(&self, by_attack: bool) {
        let power = if by_attack {
            INITIAL_ACCELERATION_POWER
        } else {
            self.get() * DEFLECTION_SCALE
        };
        self.set(power);
    }

    pub fn write_nbt(&self, nbt: &mut NbtCompound) {
        nbt.put_double("acceleration_power", self.get());
    }

    /// Updates only when the key is present, so a partial read keeps the rest.
    pub fn read_nbt(&self, nbt: &NbtCompound) {
        if let Some(power) = nbt.get_double("acceleration_power") {
            self.set(power);
        }
    }
}

/// The parts of vanilla `AbstractHurtingProjectile.tick` its subclasses override.
pub struct HurtingMotion {
    /// Vanilla `getInertia`.
    pub inertia: f32,
    /// Vanilla `getLiquidInertia`.
    pub liquid_inertia: f32,
    /// Vanilla `shouldBurn`.
    pub burns: bool,
}

impl HurtingMotion {
    /// Vanilla `AbstractHurtingProjectile`'s own values.
    pub const FIREBALL: Self = Self {
        inertia: 0.95,
        liquid_inertia: 0.8,
        burns: true,
    };
}

impl ThrownItemEntity {
    /// A projectile of `shooter`'s where `entity` stands, heading along `direction`.
    ///
    /// Vanilla `AbstractHurtingProjectile(type, mob, direction, level)` without its start at the
    /// mob's feet, since every caller moves it elsewhere right after.
    #[must_use]
    pub fn new_hurting(
        entity: Entity,
        shooter: &Entity,
        direction: Vector3<f64>,
        power: f64,
    ) -> Self {
        entity.velocity.store(direction.normalize() * power);
        entity.set_rotation(shooter.yaw.load(), shooter.pitch.load());
        Self {
            entity,
            owner: ProjectileOwner::new(Some(shooter)),
            collides_with_projectiles: false,
            has_hit: AtomicBool::new(false),
            gravity: 0.0,
        }
    }

    /// Vanilla `AbstractHurtingProjectile.tick`.
    pub(crate) fn tick_hurting(
        &self,
        caller: &dyn EntityBase,
        server: &Server,
        power: f64,
        motion: &HurtingMotion,
    ) {
        let entity = &self.entity;
        self.apply_inertia(power, motion);
        // Vanilla also checks `owner.isRemoved()`, which never holds: `getOwner` returns no
        // removed entity, so the shot of a dead mob flies on.
        if !entity.world.load().is_loaded(&entity.block_pos.load()) {
            entity.remove();
            return;
        }

        let start = entity.pos.load();
        let movement = entity.velocity.load();
        let hit = self.find_hit(caller, start, movement);
        let new_pos = hit
            .as_ref()
            .map_or_else(|| start + movement, ProjectileHit::hit_pos);
        rotate_towards_movement(entity, ROTATION_SPEED);
        entity.set_pos(new_pos);
        self.broadcast_velocity();
        entity.tick_block_collisions(caller);
        self.tick_projectile_state();
        entity.tick(caller, server);
        if motion.burns {
            entity.fire_ticks.fetch_max(BURN_TICKS, Relaxed);
        }
        if let Some(hit) = hit
            && !entity.is_removed()
        {
            self.hit_and_remove(caller, hit);
        }
    }

    /// Vanilla `AbstractHurtingProjectile.applyInertia`; the bubbles are the client's.
    fn apply_inertia(&self, power: f64, motion: &HurtingMotion) {
        let entity = &self.entity;
        let movement = entity.velocity.load();
        let inertia = if entity.touching_water.load(Relaxed) {
            motion.liquid_inertia
        } else {
            motion.inertia
        };
        entity
            .velocity
            .store((movement + movement.normalize() * power) * f64::from(inertia));
    }
}

/// Vanilla `ProjectileUtil.rotateTowardsMovement`; the rotation before the call is vanilla's
/// `yRotO` and `xRotO`.
pub(crate) fn rotate_towards_movement(entity: &Entity, speed: f32) {
    let movement = entity.velocity.load();
    if movement.length_squared() == 0.0 {
        return;
    }
    let horizontal = movement.horizontal_length();
    let yaw = movement.z.atan2(movement.x).to_degrees() as f32 + 90.0;
    let pitch = horizontal.atan2(movement.y).to_degrees() as f32 - 90.0;
    entity.set_rotation(
        lerp_rotation(speed, entity.yaw.load(), yaw),
        lerp_rotation(speed, entity.pitch.load(), pitch),
    );
}

/// Vanilla `Projectile.updateRotation`.
pub(crate) fn update_rotation(entity: &Entity) {
    let movement = entity.velocity.load();
    let horizontal = movement.horizontal_length();
    let pitch = movement.y.atan2(horizontal).to_degrees() as f32;
    let yaw = movement.x.atan2(movement.z).to_degrees() as f32;
    entity.set_rotation(
        lerp_rotation(LERP_ROTATION_SPEED, entity.yaw.load(), yaw),
        lerp_rotation(LERP_ROTATION_SPEED, entity.pitch.load(), pitch),
    );
}

/// Vanilla `Projectile.lerpRotation`: the short way around from `old` to `new`.
fn lerp_rotation(speed: f32, old: f32, new: f32) -> f32 {
    let delta = (new - old + 180.0).rem_euclid(360.0) - 180.0;
    speed.mul_add(delta, new - delta)
}

/// Vanilla `Projectile.deflect` with a `power` of 1. Returns whether it was deflected.
pub fn deflect(
    projectile: &dyn EntityBase,
    deflection: ProjectileDeflectionType,
    deflector: Option<&dyn EntityBase>,
    new_owner: Option<&Entity>,
    by_attack: bool,
) -> bool {
    if !projectile.can_be_deflected() {
        return false;
    }
    deflection.deflect(projectile, deflector);
    if let Some(owner) = projectile.get_projectile_owner() {
        owner.set(new_owner);
    }
    projectile.on_deflection(by_attack);
    true
}

pub(crate) fn is_redirectable(entity: &Entity) -> bool {
    entity
        .entity_type
        .has_tag(&tag::EntityType::MINECRAFT_REDIRECTABLE_PROJECTILE)
}

/// Vanilla `Player.deflectProjectile`: a hit sends a fireball or wind charge where the player
/// looks, and it is theirs from then on.
pub fn deflect_by_player(player: &Player, target: &dyn EntityBase) -> bool {
    let player_entity = player.get_entity();
    if !is_redirectable(target.get_entity())
        || !deflect(
            target,
            ProjectileDeflectionType::Redirected,
            Some(player),
            Some(player_entity),
            true,
        )
    {
        return false;
    }
    player_entity.world.load().play_sound(
        Sound::EntityPlayerAttackNodamage,
        SoundCategory::Players,
        &player_entity.pos.load(),
    );
    true
}

/// Vanilla `Projectile.onHit` and `onRedirectProjectile`: what hits a redirectable projectile
/// sends it off; a shulker bullet along its own flight, others where their owner looks.
pub(crate) fn redirect_hit_projectile(projectile: &dyn EntityBase, hit: &ProjectileHit) {
    let ProjectileHit::Entity { entity: target, .. } = hit else {
        return;
    };
    if !is_redirectable(target.get_entity()) {
        return;
    }
    let owner = projectile.get_owner_entity();
    let new_owner = owner.as_deref().map(EntityBase::get_entity);
    if projectile.get_entity().entity_type == &EntityType::SHULKER_BULLET {
        deflect(
            target.as_ref(),
            ProjectileDeflectionType::TransferVelocityDirection,
            Some(projectile),
            new_owner,
            true,
        );
    } else {
        deflect(
            target.as_ref(),
            ProjectileDeflectionType::Redirected,
            owner.as_deref(),
            new_owner,
            true,
        );
    }
}

/// Vanilla `Entity.isPickable`: what a projectile does not fly through.
#[must_use]
pub(crate) fn is_pickable(target: &dyn EntityBase) -> bool {
    let entity = target.get_entity();
    let kind = entity.entity_type;
    if is_projectile(kind) {
        return kind == &EntityType::SHULKER_BULLET || is_redirectable(entity);
    }
    !NOT_PICKABLE.contains(&kind)
        && target
            .cast_any()
            .downcast_ref::<ArmorStandEntity>()
            .is_none_or(|stand| !stand.is_marker())
}

/// Vanilla `AbstractHurtingProjectile.canHitEntity` and `ShulkerBullet.canHitEntity`: nothing
/// without physics, which a shulker bullet is too.
#[must_use]
pub(crate) fn can_hit_with_physics(target: &dyn EntityBase) -> bool {
    let entity = target.get_entity();
    !entity.no_physics.load(Relaxed) && entity.entity_type != &EntityType::SHULKER_BULLET
}

/// Vanilla `Entity.isAlive`.
#[must_use]
pub(crate) fn is_alive(target: &dyn EntityBase) -> bool {
    target
        .get_living_entity()
        .map_or_else(|| target.get_entity().is_alive(), LivingEntity::is_alive)
}

/// Vanilla `LivingEntity.addEffect`, with a player's advancement check.
pub(crate) fn add_effect(target: &dyn EntityBase, effect: Effect) {
    if let Some(player) = target.get_player() {
        player.add_effect(effect);
    } else if let Some(living) = target.get_living_entity() {
        living.add_effect(effect);
    }
}
