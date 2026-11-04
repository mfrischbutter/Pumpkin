//! Vanilla `AbstractArrow`: what arrows, spectral arrows and tridents share.

use std::sync::RwLock;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, Ordering};

use crossbeam::atomic::AtomicCell;
use pumpkin_data::BlockStateId;
use pumpkin_data::entity::EntityType;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::Sound;
use pumpkin_data::tracked_data::abstract_arrow;
use pumpkin_util::math::vector3::Vector3;

use super::ArrowPickup;
use crate::enchantment::EnchantmentHelper;
use crate::entity::projectile::hit_result::{self, EntityHit};
use crate::entity::projectile::owner::ProjectileOwner;
use crate::entity::projectile::projectile_item::triangle;
use crate::entity::{Entity, EntityBase};

/// Vanilla `AbstractArrow.FLAG_CRIT`.
const FLAG_CRIT: u8 = 1;
/// Vanilla `AbstractArrow.FLAG_NOPHYSICS`.
const FLAG_NOPHYSICS: u8 = 2;
/// The degrees in a radian, as vanilla's `180.0F / (float)Math.PI`.
const DEGREES_PER_RADIAN: f32 = 57.295_776;

/// The methods vanilla's arrow classes override in the tick of [`AbstractArrow`].
pub trait ArrowHooks {
    /// Vanilla `getWaterInertia`.
    fn water_inertia(&self) -> f64 {
        AbstractArrow::WATER_INERTIA
    }

    /// Vanilla `findHitEntities`.
    fn find_hit_entities(&self, from: Vector3<f64>, to: Vector3<f64>) -> Vec<EntityHit>;

    /// Vanilla `tickDespawn`.
    fn tick_despawn(&self);
}

/// The state of vanilla `AbstractArrow`, inside [`super::ArrowEntity`] and
/// [`crate::entity::projectile::trident::TridentEntity`].
pub struct AbstractArrow {
    pub entity: Entity,
    pub owner: ProjectileOwner,
    /// Vanilla `pickupItemStack`.
    pub item_stack: RwLock<ItemStack>,
    pub(super) base_damage: AtomicU64,
    pub(super) pickup: AtomicCell<ArrowPickup>,
    pub(super) is_critical: AtomicBool,
    pub(super) pierce_level: AtomicU8,
    pub(super) in_ground: AtomicBool,
    pub(super) in_ground_time: AtomicU32,
    pub(super) life: AtomicU32,
    pub(super) shake_time: AtomicU8,
    /// Vanilla `lastState`: the block the arrow hit.
    pub(super) last_state: AtomicCell<Option<BlockStateId>>,
    /// Vanilla `piercingIgnoreEntityIds`.
    pub(super) pierced_entities: RwLock<Vec<i32>>,
    /// Vanilla `firedFromWeapon`.
    pub(super) weapon: RwLock<Option<ItemStack>>,
    /// Vanilla `soundEvent`.
    pub(super) sound_event: AtomicCell<Sound>,
    pub(super) has_been_shot: AtomicBool,
}

impl AbstractArrow {
    pub const ARROW_BASE_DAMAGE: f64 = 2.0;
    pub(super) const WATER_INERTIA: f64 = 0.6;
    pub(super) const INERTIA: f64 = 0.99;
    pub(super) const GRAVITY: f64 = 0.05;
    pub(super) const SHAKE_TIME: u8 = 7;
    pub(super) const DESPAWN_TIME: u32 = 1200;

    /// The arrow without a weapon; the pickup rule follows vanilla `setOwner`.
    #[must_use]
    pub fn new(
        entity: Entity,
        owner: Option<&Entity>,
        item_stack: ItemStack,
        pickup: ArrowPickup,
    ) -> Self {
        let sound_event = Self::default_hit_ground_sound(entity.entity_type);
        Self {
            entity,
            owner: ProjectileOwner::new(owner),
            item_stack: RwLock::new(item_stack),
            base_damage: AtomicU64::new(Self::ARROW_BASE_DAMAGE.to_bits()),
            pickup: AtomicCell::new(pickup.for_owner(owner)),
            is_critical: AtomicBool::new(false),
            pierce_level: AtomicU8::new(0),
            in_ground: AtomicBool::new(false),
            in_ground_time: AtomicU32::new(0),
            life: AtomicU32::new(0),
            shake_time: AtomicU8::new(0),
            last_state: AtomicCell::new(None),
            pierced_entities: RwLock::new(Vec::new()),
            weapon: RwLock::new(None),
            sound_event: AtomicCell::new(sound_event),
            has_been_shot: AtomicBool::new(false),
        }
    }

    /// Vanilla `getDefaultHitGroundSoundEvent`.
    fn default_hit_ground_sound(entity_type: &EntityType) -> Sound {
        if entity_type == &EntityType::TRIDENT {
            Sound::ItemTridentHitGround
        } else {
            Sound::EntityArrowHit
        }
    }

    /// Where vanilla's shooter constructor puts it: `shooter.getEyeY() - 0.1F`.
    pub(crate) fn move_to_shooter(entity: &Entity, shooter: &Entity) {
        let mut position = shooter.pos.load();
        position.y = shooter.get_eye_y() - f64::from(0.1f32);
        entity.set_pos(position);
    }

    /// The weapon part of vanilla's constructor: the arrow pierces as the weapon says.
    pub(crate) fn set_fired_from_weapon(&self, weapon: &ItemStack) {
        if weapon.is_empty() {
            return;
        }
        *self
            .weapon
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(weapon.copy_with_count(1));
        let pierce = EnchantmentHelper::process_projectile_piercing(weapon, 0);
        if pierce > 0 {
            self.set_pierce_level(pierce);
        }
    }

    /// Vanilla `getPickupItem`.
    #[must_use]
    pub fn pickup_item(&self) -> ItemStack {
        self.item_stack
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Vanilla `getWeaponItem` of the base class.
    #[must_use]
    pub fn get_weapon_item(&self) -> Option<ItemStack> {
        self.weapon
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    #[must_use]
    pub fn pickup(&self) -> ArrowPickup {
        self.pickup.load()
    }

    pub fn set_pickup(&self, pickup: ArrowPickup) {
        self.pickup.store(pickup);
    }

    /// Vanilla `setSoundEvent`.
    pub fn set_sound_event(&self, sound: Sound) {
        self.sound_event.store(sound);
    }

    /// Vanilla `Projectile.shootFromRotation` without the shooter's movement, see
    /// [`Self::add_shooter_movement`].
    pub fn set_velocity_from_rotation(
        &self,
        pitch: f32,
        yaw: f32,
        roll: f32,
        speed: f32,
        divergence: f32,
    ) {
        let yaw_rad = yaw.to_radians();
        let pitch_rad = pitch.to_radians();
        let roll_rad = (pitch + roll).to_radians();

        let x = -yaw_rad.sin() * pitch_rad.cos();
        let y = -roll_rad.sin();
        let z = yaw_rad.cos() * pitch_rad.cos();

        self.set_velocity(
            f64::from(x),
            f64::from(y),
            f64::from(z),
            f64::from(speed),
            f64::from(divergence),
        );
    }

    /// The end of vanilla `Projectile.shootFromRotation`: the shooter's movement carries over,
    /// upwards and downwards only in the air.
    pub fn add_shooter_movement(&self, shooter: &Entity) {
        let movement = shooter.movement.load();
        let y = if shooter.on_ground.load(Ordering::Relaxed) {
            0.0
        } else {
            movement.y
        };
        self.entity.velocity.store(
            self.entity
                .velocity
                .load()
                .add_raw(movement.x, y, movement.z),
        );
    }

    /// Vanilla `AbstractArrow.shoot`.
    pub fn set_velocity(&self, x: f64, y: f64, z: f64, power: f64, uncertainty: f64) {
        let deviation = 0.017_227_5 * uncertainty;
        let mut rng = rand::rng();
        let velocity = Vector3::new(x, y, z)
            .normalize()
            .add_raw(
                triangle(&mut rng, 0.0, deviation),
                triangle(&mut rng, 0.0, deviation),
                triangle(&mut rng, 0.0, deviation),
            )
            .multiply(power, power, power);

        self.entity.velocity.store(velocity);
        self.entity.velocity_dirty.store(true, Ordering::Relaxed);
        self.entity.set_rotation(
            velocity.x.atan2(velocity.z) as f32 * DEGREES_PER_RADIAN,
            velocity.y.atan2(velocity.horizontal_length()) as f32 * DEGREES_PER_RADIAN,
        );
        self.life.store(0, Ordering::Relaxed);
    }

    pub(crate) fn flags(&self) -> u8 {
        let mut flags = 0u8;
        if self.is_critical.load(Ordering::Relaxed) {
            flags |= FLAG_CRIT;
        }
        if self.is_no_physics() {
            flags |= FLAG_NOPHYSICS;
        }
        flags
    }

    /// Vanilla `setCritArrow`.
    pub fn set_critical(&self, critical: bool) {
        self.is_critical.store(critical, Ordering::Relaxed);
        self.entity
            .set_synced_data(abstract_arrow::ID_FLAGS, self.flags());
    }

    /// Vanilla `setNoPhysics`.
    pub fn set_no_physics(&self, no_physics: bool) {
        self.entity.no_physics.store(no_physics, Ordering::Relaxed);
        self.entity
            .set_synced_data(abstract_arrow::ID_FLAGS, self.flags());
    }

    #[must_use]
    pub fn is_no_physics(&self) -> bool {
        self.entity.no_physics.load(Ordering::Relaxed)
    }

    /// Vanilla `setBaseDamageFromMob`.
    pub fn set_base_damage_from_mob(&self, power: f64, difficulty_id: i32) {
        let spread = triangle(&mut rand::rng(), f64::from(difficulty_id) * 0.11, 0.57425);
        self.set_base_damage(power * 2.0 + spread);
    }

    /// Vanilla `setPierceLevel`.
    pub fn set_pierce_level(&self, level: u8) {
        self.pierce_level.store(level, Ordering::Relaxed);
        self.entity
            .set_synced_data(abstract_arrow::PIERCE_LEVEL, level);
    }

    #[must_use]
    pub fn get_pierce_level(&self) -> u8 {
        self.pierce_level.load(Ordering::Relaxed)
    }

    #[must_use]
    pub fn get_base_damage(&self) -> f64 {
        f64::from_bits(self.base_damage.load(Ordering::Relaxed))
    }

    pub fn set_base_damage(&self, damage: f64) {
        self.base_damage.store(damage.to_bits(), Ordering::Relaxed);
    }

    /// Vanilla `igniteForSeconds(100)` of the Flame enchantment and `WitherSkeleton.getArrow`.
    pub fn set_flame(&self, flame: bool) {
        if flame {
            self.entity.set_on_fire_for(100.0);
            self.entity.set_on_fire(true);
        } else {
            self.entity.extinguish();
        }
    }

    #[must_use]
    pub fn is_on_fire(&self) -> bool {
        self.entity.is_on_fire()
    }

    /// Vanilla `inGroundTime`.
    #[must_use]
    pub fn in_ground_ticks(&self) -> u32 {
        self.in_ground_time.load(Ordering::Relaxed)
    }

    /// Vanilla `isInGround`.
    #[must_use]
    pub fn is_in_ground(&self) -> bool {
        self.in_ground.load(Ordering::Relaxed)
    }

    /// Vanilla `setInGround`.
    pub fn set_in_ground(&self, in_ground: bool) {
        self.in_ground.store(in_ground, Ordering::Relaxed);
        self.entity
            .set_synced_data(abstract_arrow::IN_GROUND, in_ground);
    }

    /// Vanilla `canHitEntity`: `Projectile`'s rules, the owner's team and what it already pierced.
    pub(crate) fn can_hit_entity(&self, target: &dyn EntityBase) -> bool {
        let world = self.entity.world.load();
        if target.get_player().is_some()
            && let Some(owner) = self.owner.get(&world)
            && owner.get_player().is_some()
            && !owner.do_teams_allow_damage(target)
        {
            return false;
        }
        hit_result::can_be_hit(target)
            && self.owner.can_hit(&world, target.get_entity())
            && !self
                .pierced_entities
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .contains(&target.get_entity().entity_id)
    }

    /// Vanilla `findHitEntities` of the base class.
    pub(crate) fn find_hit_entities(&self, from: Vector3<f64>, to: Vector3<f64>) -> Vec<EntityHit> {
        hit_result::many_entity_hits(
            &self.entity.world.load(),
            &self.entity,
            (from, to),
            &hit_result::search_area(&self.entity),
            hit_result::compute_margin(&self.entity),
            &|target| self.can_hit_entity(target),
        )
    }

    /// Vanilla `tickDespawn` of the base class.
    pub(crate) fn tick_despawn(&self) {
        let life = self.life.fetch_add(1, Ordering::Relaxed) + 1;
        if life >= Self::DESPAWN_TIME {
            self.entity.remove();
        }
    }
}
