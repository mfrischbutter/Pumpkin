//! Vanilla `ThrownTrident`, on [`AbstractArrow`].

use std::ops::Deref;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU8, Ordering};

use pumpkin_data::damage::DamageType;
use pumpkin_data::data_component_impl::EnchantmentGlintOverrideImpl;
use pumpkin_data::enchantment::Enchantment;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::Sound;
use pumpkin_data::tag::{self, Taggable};
use pumpkin_data::tracked_data::{abstract_arrow as arrow_data, trident as trident_data};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;

use super::ProjectileHit;
use super::arrow::{AbstractArrow, ArrowHooks, ArrowPickup};
use super::hit_result::{self, EntityHit};
use super::owner::ProjectileOwner;
use crate::enchantment::EnchantmentHelper;
use crate::enchantment::effects::EnchantmentEntityEffectExt;
use crate::entity::living::LivingEntity;
use crate::entity::player::Player;
use crate::entity::projectile_deflection::ProjectileDeflectionType;
use crate::entity::{Entity, EntityBase};
use crate::server::Server;

/// Vanilla `TridentItem.BASE_DAMAGE`.
const BASE_DAMAGE: f64 = 8.0;
/// Vanilla `ThrownTrident.WATER_INERTIA`: water hardly slows a trident.
const WATER_INERTIA: f64 = 0.99;
/// What `BlockPos.clampLocationWithin` leaves to the block's faces.
const BLOCK_FACE_MARGIN: f64 = 1.0e-5;

pub struct TridentEntity {
    pub arrow: AbstractArrow,
    /// Vanilla `dealtDamage`: it hit, or lay in the ground for a while.
    dealt_damage: AtomicBool,
    /// Vanilla `ID_LOYALTY`.
    loyalty: AtomicU8,
    /// Vanilla `clientSideReturnTridentTickCount`, which also counts on the server: the return
    /// sound plays once.
    return_ticks: AtomicI32,
}

impl Deref for TridentEntity {
    type Target = AbstractArrow;

    fn deref(&self) -> &AbstractArrow {
        &self.arrow
    }
}

impl TridentEntity {
    fn from_item(
        entity: Entity,
        owner: Option<&Entity>,
        item_stack: ItemStack,
        pickup: ArrowPickup,
    ) -> Self {
        let trident = Self {
            arrow: AbstractArrow::new(entity, owner, item_stack, pickup),
            dealt_damage: AtomicBool::new(false),
            loyalty: AtomicU8::new(0),
            return_ticks: AtomicI32::new(0),
        };
        trident.update_item_data();
        trident
    }

    pub fn new(entity: Entity, owner: Option<&Entity>) -> Self {
        Self::from_item(
            entity,
            owner,
            ItemStack::new(1, &Item::TRIDENT),
            ArrowPickup::Disallowed,
        )
    }

    /// Vanilla's `ThrownTrident(level, owner, tridentItem)`.
    pub fn new_shot(
        entity: Entity,
        shooter: &Entity,
        item_stack: ItemStack,
        pickup: ArrowPickup,
    ) -> Self {
        AbstractArrow::move_to_shooter(&entity, shooter);
        Self::from_item(entity, Some(shooter), item_stack, pickup)
    }

    /// Vanilla `TridentItem.asProjectile`: the item is also the weapon.
    pub fn new_dispensed(entity: Entity, owner: Option<&Entity>, item_stack: ItemStack) -> Self {
        let weapon = item_stack.clone();
        let trident = Self::from_item(entity, owner, item_stack, ArrowPickup::Allowed);
        trident.set_fired_from_weapon(&weapon);
        trident
    }

    /// Applies projectile-spawned enchantment effects matching vanilla `Projectile::applyOnProjectileSpawned`.
    pub fn apply_on_projectile_spawned(&self, pickup_item_stack: &ItemStack) {
        super::apply_on_projectile_spawned(self.get_entity(), pickup_item_stack, None, None);
    }

    /// Vanilla `getLoyaltyFromItem` and `hasFoil`, from the item the trident carries.
    fn update_item_data(&self) {
        let item_stack = self.pickup_item();
        let loyalty = EnchantmentHelper::modify_trident_return_acceleration(&item_stack, 0.0)
            .clamp(0.0, 127.0) as u8;
        let foil = item_stack
            .get_data_component::<EnchantmentGlintOverrideImpl>()
            .map_or_else(|| item_stack.has_enchantments(), |glint| glint.glint);
        self.loyalty.store(loyalty, Ordering::Relaxed);
        self.entity
            .set_synced_data(trident_data::ID_LOYALTY, loyalty);
        self.entity.set_synced_data(trident_data::ID_FOIL, foil);
    }

    /// The owner check of vanilla's return flight: alive and no spectator.
    fn is_acceptable_return_owner(owner: &dyn EntityBase) -> bool {
        let alive = owner
            .get_living_entity()
            .map_or_else(|| owner.get_entity().is_alive(), LivingEntity::is_alive);
        alive && !owner.is_spectator()
    }

    /// The start of vanilla `ThrownTrident.tick`. Returns `false` when the trident is gone.
    fn tick_return_to_owner(&self, caller: &dyn EntityBase) -> bool {
        let entity = &self.entity;
        let loyalty = self.loyalty.load(Ordering::Relaxed);
        if loyalty == 0 || !(self.dealt_damage.load(Ordering::Relaxed) || self.is_no_physics()) {
            return true;
        }
        let Some(owner) = self.owner.get(&entity.world.load()) else {
            return true;
        };
        if !Self::is_acceptable_return_owner(&*owner) {
            if self.pickup() == ArrowPickup::Allowed {
                self.spawn_pickup_item();
            }
            entity.remove();
            return false;
        }

        let position = entity.pos.load();
        let owner_entity = owner.get_entity();
        let to_owner = owner_entity.get_eye_pos().sub(&position);
        if owner.get_player().is_none() && to_owner.length() < f64::from(owner_entity.width()) + 1.0
        {
            entity.remove();
            return false;
        }

        self.set_no_physics(true);
        let level = f64::from(loyalty);
        entity.set_pos(Vector3::new(
            position.x,
            to_owner.y.mul_add(0.015 * level, position.y),
            position.z,
        ));
        let acceleration = 0.05 * level;
        entity.velocity.store(
            entity
                .velocity
                .load()
                .multiply(0.95, 0.95, 0.95)
                .add(
                    &to_owner
                        .normalize()
                        .multiply(acceleration, acceleration, acceleration),
                ),
        );
        if self.return_ticks.fetch_add(1, Ordering::Relaxed) == 0 {
            caller.play_entity_sound(Sound::ItemTridentReturn, 10.0, 1.0);
        }
        true
    }

    /// Vanilla `ThrownTrident.onHitEntity`.
    fn on_hit_target(&self, target: &Arc<dyn EntityBase>, hit_pos: Vector3<f64>) {
        let weapon = self.pickup_item();
        let target_entity = target.get_entity();
        let damage = EnchantmentHelper::modify_projectile_damage(
            &weapon,
            target_entity.entity_type,
            BASE_DAMAGE,
        );
        let owner = self.owner.get(&self.entity.world.load());
        self.dealt_damage.store(true, Ordering::Relaxed);
        let hurt = target.damage_with_context(
            target.as_ref(),
            damage as f32,
            DamageType::TRIDENT,
            Some(hit_pos),
            Some(self),
            Some(owner.as_deref().unwrap_or(self)),
        );
        if hurt {
            self.channeling_after_attack(target_entity, &weapon);
            if let Some(living) = target.get_living_entity() {
                self.do_knockback(target_entity, living);
            }
        }
        if target.projectile_receives_side_effects_on_hit(hurt) {
            self.play_entity_sound(Sound::ItemTridentHit, 1.0, 1.0);
            ProjectileDeflectionType::reverse(self, Vector3::new(0.02, 0.2, 0.02));
        }
    }

    /// `minecraft:post_attack` of `channeling.json`. The extracted effects leave out their
    /// requirements, so they are spelled out here: thunder and a victim under the open sky.
    fn channeling_after_attack(&self, target: &Entity, weapon: &ItemStack) {
        let level = weapon.get_enchantment_level(&Enchantment::CHANNELING);
        let world = self.entity.world.load_full();
        let position = target.pos.load();
        if level <= 0
            || !world.is_thundering()
            || !world.can_see_sky(&BlockPos::floored_v(position))
        {
            return;
        }
        let owner = self.owner.get_player(&world);
        for effect in Enchantment::CHANNELING.effects.post_attack {
            effect
                .effect
                .apply(&world, level, owner.as_ref(), Some(target), position);
        }
    }

    /// `minecraft:hit_block` of `channeling.json` from vanilla `hitBlockEnchantmentEffects`:
    /// thunder and a lightning rod under the open sky.
    fn channeling_on_hit_block(&self, pos: BlockPos, hit_pos: Vector3<f64>) {
        let level = self
            .pickup_item()
            .get_enchantment_level(&Enchantment::CHANNELING);
        let world = self.entity.world.load_full();
        if level <= 0
            || !world.is_thundering()
            || !world
                .get_block(&pos)
                .has_tag(&tag::Block::MINECRAFT_LIGHTNING_RODS)
            || !world.can_see_sky(&pos)
        {
            return;
        }
        let corner = pos.0.to_f64();
        let clamp = |value: f64, start: f64| {
            value.clamp(start + BLOCK_FACE_MARGIN, start + 1.0 - BLOCK_FACE_MARGIN)
        };
        let position = Vector3::new(
            clamp(hit_pos.x, corner.x),
            clamp(hit_pos.y, corner.y),
            clamp(hit_pos.z, corner.z),
        );
        let owner = self.owner.get_player(&world);
        for effect in Enchantment::CHANNELING.effects.hit_block {
            effect
                .effect
                .apply(&world, level, owner.as_ref(), Some(&self.entity), position);
        }
    }
}

impl ArrowHooks for TridentEntity {
    fn water_inertia(&self) -> f64 {
        WATER_INERTIA
    }

    /// Vanilla `ThrownTrident.findHitEntities`: the nearest one, once.
    fn find_hit_entities(&self, from: Vector3<f64>, to: Vector3<f64>) -> Vec<EntityHit> {
        if self.dealt_damage.load(Ordering::Relaxed) {
            return Vec::new();
        }
        hit_result::nearest_entity_hit(
            &self.entity.world.load(),
            &self.entity,
            (from, to),
            &hit_result::search_area(&self.entity),
            hit_result::compute_margin(&self.entity),
            &|target| self.can_hit_entity(target),
        )
        .into_iter()
        .collect()
    }

    /// Vanilla `ThrownTrident.tickDespawn`: a loyal one that can be picked up stays.
    fn tick_despawn(&self) {
        if self.pickup() != ArrowPickup::Allowed || self.loyalty.load(Ordering::Relaxed) == 0 {
            self.arrow.tick_despawn();
        }
    }
}

impl EntityBase for TridentEntity {
    fn get_projectile_owner(&self) -> Option<&ProjectileOwner> {
        Some(&self.owner)
    }

    fn get_projectile_item(&self) -> Option<ItemStack> {
        Some(self.pickup_item())
    }

    fn init_data_tracker(&self) {
        let entity = self.get_entity();
        entity.set_synced_data(arrow_data::ID_FLAGS, self.flags());
        entity.set_synced_data(arrow_data::PIERCE_LEVEL, self.get_pierce_level());
        entity.set_synced_data(arrow_data::IN_GROUND, self.is_in_ground());
        self.update_item_data();
    }

    fn write_custom_nbt(&self, nbt: &mut NbtCompound) {
        self.write_arrow_nbt(nbt);
        nbt.put_bool("DealtDamage", self.dealt_damage.load(Ordering::Relaxed));
    }

    fn read_custom_nbt(&self, nbt: &NbtCompound) {
        self.read_arrow_nbt(nbt);
        if let Some(dealt_damage) = nbt.get_bool("DealtDamage") {
            self.dealt_damage.store(dealt_damage, Ordering::Relaxed);
        }
        self.update_item_data();
    }

    fn tick(&self, caller: &dyn EntityBase, server: &Server) {
        if self.in_ground_ticks() > 4 {
            self.dealt_damage.store(true, Ordering::Relaxed);
        }
        if self.tick_return_to_owner(caller) {
            self.arrow.tick(self, caller, server);
        }
    }

    fn on_hit(&self, hit: ProjectileHit) {
        match hit {
            ProjectileHit::Block { pos, hit_pos, .. } => {
                self.arrow.on_hit_block(self, pos, hit_pos, &|| {
                    self.channeling_on_hit_block(pos, hit_pos);
                });
            }
            ProjectileHit::Entity {
                entity: target,
                hit_pos,
                ..
            } => self.on_hit_target(&target, hit_pos),
        }
    }

    fn get_entity(&self) -> &Entity {
        &self.arrow.entity
    }

    fn get_living_entity(&self) -> Option<&LivingEntity> {
        None
    }

    fn cast_any(&self) -> &dyn std::any::Any {
        self
    }

    /// Vanilla `ThrownTrident.playerTouch` and `tryPickup`.
    fn on_player_collision(&self, player: &Arc<Player>) {
        let owned = self.owner.matches(player.get_entity());
        if owned || self.owner.get(&self.entity.world.load()).is_none() {
            self.player_touch(player, self.is_no_physics() && owned);
        }
    }
}
