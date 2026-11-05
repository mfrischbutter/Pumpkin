use crate::entity::projectile::owner::ProjectileOwner;
use std::sync::RwLock;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};

use pumpkin_data::damage::DamageType;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_protocol::codec::item_stack_seralizer::ItemStackSerializer;
use pumpkin_util::math::vector3::Vector3;

use crate::{
    entity::{
        Entity, EntityBase,
        projectile::{
            ProjectileHit, ThrownItemEntity,
            hurting::{self, AccelerationPower, HurtingMotion},
        },
    },
    server::Server,
    world::{ExplosionInteraction, explosion::ExplosionSettings},
};

pub const MIN_CAMERA_DISTANCE_SQUARED: f32 = 12.25;
pub const INITIAL_ACCELERATION_POWER: f64 = 0.1;
pub const DEFLECTION_SCALE: f64 = 0.5;
pub const DEFAULT_EXPLOSION_POWER: i32 = 1;
/// Vanilla `LargeFireball.onHitEntity` damage.
const HIT_DAMAGE: f32 = 6.0;
pub const AIR_INERTIA: f64 = 0.95;
pub const WATER_INERTIA: f64 = 0.8;

/// Vanilla `Fireball.DATA_ITEM_STACK`, which large and small fireballs share, with its NBT.
pub struct FireballItem(RwLock<ItemStack>);

impl Default for FireballItem {
    /// Vanilla `Fireball.getDefaultItem`.
    fn default() -> Self {
        Self(RwLock::new(ItemStack::new(1, &Item::FIRE_CHARGE)))
    }
}

impl FireballItem {
    #[must_use]
    pub fn get(&self) -> ItemStack {
        self.0
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Vanilla `Fireball.setItem`.
    pub fn set(&self, entity: &Entity, source: &ItemStack) {
        let item = if source.is_empty() {
            ItemStack::new(1, &Item::FIRE_CHARGE)
        } else {
            source.copy_with_count(1)
        };
        *self
            .0
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = item.clone();
        entity.set_synced_data(
            pumpkin_data::tracked_data::fireball::ITEM_STACK,
            ItemStackSerializer::from(item),
        );
    }

    pub fn init_data_tracker(&self, entity: &Entity) {
        entity.set_synced_data(
            pumpkin_data::tracked_data::fireball::ITEM_STACK,
            ItemStackSerializer::from(self.get()),
        );
    }

    pub fn write_nbt(&self, nbt: &mut NbtCompound) {
        let mut item = NbtCompound::new();
        self.get().write_item_stack(&mut item);
        nbt.put_compound("Item", item);
    }

    /// Updates only when the key is present, so a partial read keeps the rest.
    pub fn read_nbt(&self, entity: &Entity, nbt: &NbtCompound) {
        if let Some(item) = nbt
            .get_compound("Item")
            .and_then(ItemStack::read_item_stack)
        {
            self.set(entity, &item);
        }
    }
}

/// Vanilla `DamageSources.fireball`: unattributed without an owner.
#[must_use]
pub(super) const fn fireball_damage(has_owner: bool) -> DamageType {
    if has_owner {
        DamageType::FIREBALL
    } else {
        DamageType::UNATTRIBUTED_FIREBALL
    }
}

pub struct FireballEntity {
    pub thrown: ThrownItemEntity,
    pub item: FireballItem,
    pub acceleration_power: AccelerationPower,
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
        Self::from_thrown(thrown)
    }

    /// Vanilla `LargeFireball(level, mob, direction, explosionPower)`, where `entity` stands.
    #[must_use]
    pub fn new_shot(entity: Entity, shooter: &Entity, direction: Vector3<f64>) -> Self {
        Self::from_thrown(ThrownItemEntity::new_hurting(
            entity,
            shooter,
            direction,
            INITIAL_ACCELERATION_POWER,
        ))
    }

    fn from_thrown(thrown: ThrownItemEntity) -> Self {
        Self {
            thrown,
            item: FireballItem::default(),
            acceleration_power: AccelerationPower::new(INITIAL_ACCELERATION_POWER),
            explosion_power: AtomicI32::new(DEFAULT_EXPLOSION_POWER),
        }
    }

    pub fn get_item(&self) -> ItemStack {
        self.item.get()
    }

    pub fn set_item(&self, source: &ItemStack) {
        self.item.set(self.get_entity(), source);
    }

    pub fn get_explosion_power(&self) -> i32 {
        self.explosion_power.load(Ordering::Relaxed)
    }

    pub fn set_explosion_power(&self, power: i32) {
        self.explosion_power.store(power, Ordering::Relaxed);
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
        self.acceleration_power.write_nbt(nbt);
        self.item.write_nbt(nbt);
        nbt.put_byte("ExplosionPower", self.get_explosion_power() as i8);
    }

    fn read_custom_nbt(&self, nbt: &NbtCompound) {
        self.acceleration_power.read_nbt(nbt);
        self.item.read_nbt(self.get_entity(), nbt);
        if let Some(power) = nbt.get_byte("ExplosionPower") {
            self.set_explosion_power(i32::from(power));
        }
    }

    fn init_data_tracker(&self) {
        self.item.init_data_tracker(self.get_entity());
    }

    fn tick(&self, caller: &dyn EntityBase, server: &Server) {
        self.thrown.tick_hurting(
            caller,
            server,
            self.acceleration_power.get(),
            &HurtingMotion::FIREBALL,
        );
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

    fn on_deflection(&self, by_attack: bool) {
        self.acceleration_power.on_deflection(by_attack);
    }

    /// Vanilla `AbstractHurtingProjectile.canHitEntity`.
    fn projectile_can_hit(&self, target: &dyn EntityBase) -> bool {
        hurting::can_hit_with_physics(target)
    }

    /// Vanilla `LargeFireball.onHitEntity` and `onHit`: fire where mobs may grief.
    fn on_hit(&self, hit: ProjectileHit) {
        let entity = self.get_entity();
        let world = entity.world.load_full();

        if let ProjectileHit::Entity { entity: target, .. } = &hit {
            let owner = self.thrown.owner.get(&world);
            let _ = target.damage_with_context(
                target.as_ref(),
                HIT_DAMAGE,
                fireball_damage(owner.is_some()),
                Some(hit.hit_pos()),
                Some(self),
                owner.as_deref(),
            );
        }

        let grief = world.level_info.load().game_rules.mob_griefing;
        world.explode_with_settings(
            Some(entity),
            entity.pos.load(),
            self.get_explosion_power() as f32,
            ExplosionInteraction::Mob,
            &ExplosionSettings {
                fire: grief,
                ..ExplosionSettings::default()
            },
        );
    }
}
