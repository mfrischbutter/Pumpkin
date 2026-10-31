use crate::entity::projectile::owner::ProjectileOwner;
use std::sync::RwLock;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU64, Ordering};

use pumpkin_data::damage::DamageType;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_protocol::codec::item_stack_seralizer::ItemStackSerializer;
use pumpkin_util::math::vector3::Vector3;

use crate::{
    entity::{
        Entity, EntityBase,
        projectile::{ProjectileHit, ThrownItemEntity},
    },
    server::Server,
};

pub const MIN_CAMERA_DISTANCE_SQUARED: f32 = 12.25;
pub const INITIAL_ACCELERATION_POWER: f64 = 0.1;
pub const DEFLECTION_SCALE: f64 = 0.5;
pub const DEFAULT_EXPLOSION_POWER: i32 = 1;
/// Vanilla `LargeFireball.onHitEntity` damage.
const HIT_DAMAGE: f32 = 6.0;
pub const AIR_INERTIA: f64 = 0.95;
pub const WATER_INERTIA: f64 = 0.8;

pub struct FireballEntity {
    pub thrown: ThrownItemEntity,
    pub item_stack: RwLock<ItemStack>,
    pub acceleration_power: AtomicU64,
    pub explosion_power: AtomicI32,
}

impl FireballEntity {
    #[must_use]
    pub fn new(entity: Entity) -> Self {
        let thrown = ThrownItemEntity {
            entity,
            owner: ProjectileOwner::none(),
            collides_with_projectiles: false,
            has_hit: AtomicBool::new(false),
            gravity: 0.0,
        };

        Self {
            thrown,
            item_stack: RwLock::new(Self::get_default_item()),
            acceleration_power: AtomicU64::new(INITIAL_ACCELERATION_POWER.to_bits()),
            explosion_power: AtomicI32::new(DEFAULT_EXPLOSION_POWER),
        }
    }

    #[must_use]
    pub fn new_shot(entity: Entity, shooter: &Entity, direction: Vector3<f64>) -> Self {
        let thrown = ThrownItemEntity::new(entity, shooter, 0.0);
        let accel = INITIAL_ACCELERATION_POWER;
        let vel = direction.normalize().multiply(accel, accel, accel);
        thrown.entity.velocity.store(vel);

        Self {
            thrown,
            item_stack: RwLock::new(Self::get_default_item()),
            acceleration_power: AtomicU64::new(accel.to_bits()),
            explosion_power: AtomicI32::new(DEFAULT_EXPLOSION_POWER),
        }
    }

    #[must_use]
    pub fn new_directional(
        entity: Entity,
        direction: Vector3<f64>,
        acceleration_power: f64,
    ) -> Self {
        let thrown = ThrownItemEntity {
            entity,
            owner: ProjectileOwner::none(),
            collides_with_projectiles: false,
            has_hit: AtomicBool::new(false),
            gravity: 0.0,
        };
        let vel = direction.normalize().multiply(
            acceleration_power,
            acceleration_power,
            acceleration_power,
        );
        thrown.entity.velocity.store(vel);

        Self {
            thrown,
            item_stack: RwLock::new(Self::get_default_item()),
            acceleration_power: AtomicU64::new(acceleration_power.to_bits()),
            explosion_power: AtomicI32::new(DEFAULT_EXPLOSION_POWER),
        }
    }

    #[must_use]
    pub fn get_default_item() -> ItemStack {
        ItemStack::new(1, &Item::FIRE_CHARGE)
    }

    pub fn get_item(&self) -> ItemStack {
        self.item_stack
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    pub fn set_item(&self, source: ItemStack) {
        let new_item = if source.item_count == 0 {
            Self::get_default_item()
        } else {
            let mut item = source;
            item.item_count = 1;
            item
        };
        *self
            .item_stack
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = new_item.clone();

        self.get_entity().set_synced_data(
            pumpkin_data::tracked_data::fireball::ITEM_STACK,
            ItemStackSerializer::from(new_item),
        );
    }

    pub fn get_acceleration_power(&self) -> f64 {
        f64::from_bits(self.acceleration_power.load(Ordering::Relaxed))
    }

    pub fn set_acceleration_power(&self, power: f64) {
        self.acceleration_power
            .store(power.to_bits(), Ordering::Relaxed);
    }

    pub fn get_explosion_power(&self) -> i32 {
        self.explosion_power.load(Ordering::Relaxed)
    }

    pub fn set_explosion_power(&self, power: i32) {
        self.explosion_power.store(power, Ordering::Relaxed);
    }

    pub fn on_deflection(&self, by_attack: bool) {
        if by_attack {
            self.set_acceleration_power(INITIAL_ACCELERATION_POWER);
        } else {
            let current = self.get_acceleration_power();
            self.set_acceleration_power(current * DEFLECTION_SCALE);
        }
    }

    pub fn should_render_at_sqr_distance(&self, distance_sqr: f64) -> bool {
        if self.get_entity().tick_count.load(Ordering::Relaxed) < 2
            && distance_sqr < f64::from(MIN_CAMERA_DISTANCE_SQUARED)
        {
            false
        } else {
            let bb_size = self
                .get_entity()
                .bounding_box
                .load()
                .get_average_side_length()
                * 4.0;
            let size = if bb_size.is_nan() { 4.0 } else { bb_size } * 64.0;
            distance_sqr < size * size
        }
    }
}

impl EntityBase for FireballEntity {
    fn get_projectile_owner(&self) -> Option<&ProjectileOwner> {
        Some(&self.thrown.owner)
    }

    fn get_projectile_item(&self) -> Option<ItemStack> {
        Some(self.get_item())
    }

    fn write_custom_nbt(&self, nbt: &mut NbtCompound) {
        nbt.put_double("acceleration_power", self.get_acceleration_power());
        nbt.put_byte("ExplosionPower", self.get_explosion_power() as i8);
    }

    fn read_custom_nbt(&self, nbt: &NbtCompound) {
        if let Some(accel) = nbt
            .get_double("acceleration_power")
            .or_else(|| nbt.get_double("power"))
        {
            self.set_acceleration_power(accel);
        }
        if let Some(power) = nbt.get_byte("ExplosionPower") {
            self.set_explosion_power(i32::from(power));
        }
    }

    fn init_data_tracker(&self) {
        let entity = self.get_entity();
        let stack = self
            .item_stack
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        entity.set_synced_data(
            pumpkin_data::tracked_data::fireball::ITEM_STACK,
            ItemStackSerializer::from(stack.clone()),
        );
    }

    fn tick(&self, caller: &dyn EntityBase, _server: &Server) {
        let power = self.get_acceleration_power();
        self.thrown.process_hurting_tick(caller, power);
    }

    fn get_entity(&self) -> &Entity {
        self.thrown.get_entity()
    }

    fn get_living_entity(&self) -> Option<&crate::entity::living::LivingEntity> {
        None
    }

    fn cast_any(&self) -> &dyn std::any::Any {
        self
    }

    /// Vanilla `LargeFireball.onHitEntity` and `onHit`.
    fn on_hit(&self, hit: ProjectileHit) {
        let world = self.get_entity().world.load_full();

        if let ProjectileHit::Entity { ref entity, .. } = hit {
            let owner = self.thrown.owner.get(&world);
            let _ = entity.damage_with_context(
                entity.as_ref(),
                HIT_DAMAGE,
                DamageType::FIREBALL,
                Some(hit.hit_pos()),
                Some(self),
                owner.as_deref(),
            );
        }

        world.explode(
            Some(self.get_entity()),
            hit.hit_pos(),
            self.get_explosion_power() as f32,
            crate::world::ExplosionInteraction::Mob,
        );
    }
}
