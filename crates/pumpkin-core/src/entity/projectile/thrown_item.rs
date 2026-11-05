//! Vanilla `ThrowableProjectile` and `ThrowableItemProjectile`: what snowballs, eggs, pearls,
//! potions and experience bottles share on top of [`ThrownItemEntity`].

use std::sync::atomic::Ordering;
use std::sync::{Arc, RwLock};

use pumpkin_data::damage::DamageType;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_nbt::compound::NbtCompound;

use super::{ThrownItemEntity, apply_on_projectile_spawned};
use crate::entity::EntityBase;
use crate::server::Server;

/// Vanilla `hurt(damageSources().thrown(this, getOwner()), amount)`.
pub fn hurt_thrown(projectile: &dyn EntityBase, target: &Arc<dyn EntityBase>, amount: f32) {
    let owner = projectile.get_owner_entity();
    target.damage_with_context(
        &**target,
        amount,
        DamageType::THROWN,
        None,
        Some(projectile),
        owner.as_deref(),
    );
}

/// Vanilla `ThrowableItemProjectile.addAdditionalSaveData`.
pub fn write_thrown_item(stack: &RwLock<ItemStack>, nbt: &mut NbtCompound) {
    let mut item = NbtCompound::new();
    stack
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .write_item_stack(&mut item);
    nbt.put_compound("Item", item);
}

/// Vanilla `ThrowableItemProjectile.readAdditionalSaveData`, keeping the item when the key is
/// missing.
pub fn read_thrown_item(stack: &RwLock<ItemStack>, nbt: &NbtCompound) {
    if let Some(item) = nbt.get_compound("Item")
        && let Some(item) = ItemStack::read_item_stack(item)
    {
        *stack
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = item.copy_with_count(1);
    }
}

/// Vanilla `Projectile.lerpRotation`.
fn lerp_rotation(mut previous: f32, rotation: f32) -> f32 {
    while rotation - previous < -180.0 {
        previous -= 360.0;
    }
    while rotation - previous >= 180.0 {
        previous += 360.0;
    }
    previous + 0.2 * (rotation - previous)
}

impl ThrownItemEntity {
    /// Vanilla `Projectile.spawnProjectileFromRotation` up to adding it to the world: the shot
    /// of `shootFromRotation`, which takes along the shooter's movement, and
    /// `applyOnProjectileSpawned`.
    #[allow(clippy::too_many_arguments)]
    pub fn shoot_item_from_rotation(
        &self,
        source: &dyn EntityBase,
        stack: &ItemStack,
        pitch: f32,
        yaw: f32,
        roll: f32,
        speed: f32,
        divergence: f32,
    ) {
        self.set_velocity_from(pitch, yaw, roll, speed, divergence);
        let movement = crate::entity::known_movement(source);
        let on_ground = source.get_entity().on_ground.load(Ordering::Relaxed);
        self.entity
            .velocity
            .store(self.entity.velocity.load().add_raw(
                movement.x,
                if on_ground { 0.0 } else { movement.y },
                movement.z,
            ));
        apply_on_projectile_spawned(&self.entity, stack, None, None);
    }

    /// Vanilla `ThrowableProjectile.tick`. The base `Entity.tick` comes first: vanilla runs it
    /// at the end of the previous tick at this same position, so water drag sees the same
    /// state.
    pub fn process_throwable_tick(&self, caller: &dyn EntityBase, server: &Server) {
        self.entity.tick(caller, server);
        if self.entity.is_removed() {
            return;
        }
        self.update_rotation();
        self.process_tick(caller);
    }

    /// Vanilla `Projectile.updateRotation`; only the yaw matters here, a hatched chick takes it,
    /// and gravity and drag do not change it.
    fn update_rotation(&self) {
        let velocity = self.entity.velocity.load();
        let pitch = velocity.y.atan2(velocity.horizontal_length()) as f32;
        let yaw = velocity.x.atan2(velocity.z) as f32;
        self.entity.set_rotation(
            lerp_rotation(self.entity.yaw.load(), yaw.to_degrees()),
            lerp_rotation(self.entity.pitch.load(), pitch.to_degrees()),
        );
    }
}
