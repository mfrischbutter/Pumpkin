//! Vanilla `Arrow` and `SpectralArrow`, one entity with a type check, on [`AbstractArrow`].

mod abstract_arrow;
mod hit;
mod nbt;
mod tick;

pub use abstract_arrow::AbstractArrow;
pub(crate) use abstract_arrow::ArrowHooks;

use std::borrow::Cow;
use std::ops::Deref;
use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};

use crate::entity::living::{EffectSource, LivingEntity};
use crate::entity::player::Player;
use crate::entity::projectile::ProjectileHit;
use crate::entity::projectile::hit_result::EntityHit;
use crate::entity::projectile::owner::ProjectileOwner;
use crate::entity::{Entity, EntityBase};
use crate::server::Server;
use pumpkin_data::data_component_impl::{
    PotionContentsImpl, PotionDurationScaleImpl, StatusEffectInstance,
};
use pumpkin_data::effect::StatusEffect;
use pumpkin_data::entity::EntityType;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::tracked_data::{abstract_arrow as arrow_data, arrow as tipped_data};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_protocol::java::client::play::CEntityStatus;
use pumpkin_util::math::vector3::Vector3;

/// Vanilla `Arrow.EVENT_POTION_PUFF`, missing from the extracted entity statuses.
const EVENT_POTION_PUFF: i8 = 0;
/// Vanilla `Arrow.EXPOSED_POTION_DECAY_TIME`.
const EXPOSED_POTION_DECAY_TIME: u32 = 600;
/// Vanilla `SpectralArrow.DEFAULT_DURATION`.
const DEFAULT_DURATION: i32 = 200;

/// Represents the pickup rules for arrows
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArrowPickup {
    Disallowed,
    Allowed,
    CreativeOnly,
}

impl ArrowPickup {
    #[must_use]
    pub const fn from_byte(byte: u8) -> Self {
        match byte {
            1 => Self::Allowed,
            2 => Self::CreativeOnly,
            _ => Self::Disallowed,
        }
    }

    #[must_use]
    pub const fn to_byte(self) -> u8 {
        match self {
            Self::Disallowed => 0,
            Self::Allowed => 1,
            Self::CreativeOnly => 2,
        }
    }

    /// Vanilla `AbstractArrow.setOwner`: a player's arrows can be picked up, an ominous item
    /// spawner's never.
    #[must_use]
    pub fn for_owner(self, owner: Option<&Entity>) -> Self {
        match owner.map(|owner| owner.entity_type) {
            Some(kind) if kind == &EntityType::PLAYER && self == Self::Disallowed => Self::Allowed,
            Some(kind) if kind == &EntityType::OMINOUS_ITEM_SPAWNER => Self::Disallowed,
            _ => self,
        }
    }
}

pub struct ArrowEntity {
    pub arrow: AbstractArrow,
    /// Vanilla `SpectralArrow.duration`.
    duration: AtomicI32,
}

impl Deref for ArrowEntity {
    type Target = AbstractArrow;

    fn deref(&self) -> &AbstractArrow {
        &self.arrow
    }
}

impl ArrowEntity {
    pub fn new(entity: Entity, owner: Option<&Entity>) -> Self {
        let item_stack = ItemStack::new(1, Self::default_item(entity.entity_type));
        Self::new_with_item(entity, owner, &item_stack, ArrowPickup::Disallowed)
    }

    pub fn new_with_item(
        entity: Entity,
        owner: Option<&Entity>,
        item_stack: &ItemStack,
        pickup: ArrowPickup,
    ) -> Self {
        Self {
            arrow: AbstractArrow::new(entity, owner, item_stack.copy_with_count(1), pickup),
            duration: AtomicI32::new(DEFAULT_DURATION),
        }
    }

    pub fn new_shot(
        entity: Entity,
        shooter: &Entity,
        item_stack: &ItemStack,
        pickup: ArrowPickup,
    ) -> Self {
        AbstractArrow::move_to_shooter(&entity, shooter);
        Self::new_with_item(entity, Some(shooter), item_stack, pickup)
    }

    pub fn new_shot_with_weapon(
        entity: Entity,
        shooter: &Entity,
        item_stack: &ItemStack,
        weapon: &ItemStack,
        pickup: ArrowPickup,
    ) -> Self {
        let arrow = Self::new_shot(entity, shooter, item_stack, pickup);
        arrow.set_fired_from_weapon(weapon);
        arrow
    }

    /// Applies projectile-spawned enchantment effects matching vanilla `Projectile::applyOnProjectileSpawned`.
    pub fn apply_on_projectile_spawned(&self, pickup_item_stack: &ItemStack) {
        let weapon = self.get_weapon_item();
        super::apply_on_projectile_spawned(
            self.get_entity(),
            pickup_item_stack,
            weapon.as_ref(),
            Some(self),
        );
    }

    #[must_use]
    pub const fn entity_type_for_item(item: &'static Item) -> &'static EntityType {
        if item.id == Item::SPECTRAL_ARROW.id {
            &EntityType::SPECTRAL_ARROW
        } else {
            &EntityType::ARROW
        }
    }

    #[must_use]
    pub const fn default_item(entity_type: &'static EntityType) -> &'static Item {
        if entity_type.id == EntityType::SPECTRAL_ARROW.id {
            &Item::SPECTRAL_ARROW
        } else {
            &Item::ARROW
        }
    }

    fn is_spectral(&self) -> bool {
        self.entity.entity_type == &EntityType::SPECTRAL_ARROW
    }

    const fn spectral_glowing_effect(duration: i32) -> pumpkin_data::potion::Effect {
        pumpkin_data::potion::Effect {
            effect_type: &StatusEffect::GLOWING,
            duration,
            amplifier: 0,
            ambient: false,
            show_particles: true,
            show_icon: true,
            blend: false,
        }
    }

    #[must_use]
    pub fn get_effect_color(item_stack: &ItemStack) -> i32 {
        if let Some(pc) = item_stack.get_data_component::<PotionContentsImpl>() {
            if let Some(color) = pc.custom_color {
                return color;
            }
            let effects = crate::item::potion::PotionContents::read_potion_effects(item_stack);
            if effects.is_empty() {
                return -1;
            }
            let mut r = 0;
            let mut g = 0;
            let mut b = 0;
            let mut total = 0;
            for (effect, _, amplifier, _, _, _) in effects {
                let color = effect.color;
                let weight = (amplifier as i32) + 1;
                r += weight * ((color >> 16) & 0xFF);
                g += weight * ((color >> 8) & 0xFF);
                b += weight * (color & 0xFF);
                total += weight;
            }
            if total == 0 {
                -1
            } else {
                ((r / total) << 16) | ((g / total) << 8) | (b / total)
            }
        } else {
            -1
        }
    }

    /// Vanilla `Arrow.updateColor`.
    fn update_color(&self) {
        if self.is_spectral() {
            return;
        }
        let color = Self::get_effect_color(&self.pickup_item());
        self.entity
            .set_synced_data(tipped_data::ID_EFFECT_COLOR, color);
    }

    /// Vanilla `Arrow.addEffect` with `new MobEffectInstance(effect, duration)`: the effect
    /// joins the custom effects of the arrow's item and colors the arrow.
    #[allow(clippy::semicolon_outside_block)]
    pub fn add_effect(&self, effect: &'static StatusEffect, duration: i32) {
        {
            let mut item_stack = self
                .item_stack
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let mut contents = item_stack
                .get_data_component::<PotionContentsImpl>()
                .cloned()
                .unwrap_or(PotionContentsImpl {
                    potion_id: None,
                    custom_color: None,
                    custom_effects: Vec::new(),
                    custom_name: None,
                });
            contents.custom_effects.push(StatusEffectInstance {
                effect_id: Cow::Borrowed(effect.minecraft_name),
                amplifier: 0,
                duration,
                ambient: false,
                show_particles: true,
                show_icon: true,
            });
            item_stack.set_data_component(contents);
        }
        self.update_color();
    }

    /// `potionContents.equals(PotionContents.EMPTY)` of vanilla `Arrow`, negated.
    fn has_potion_contents(&self) -> bool {
        self.item_stack
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get_data_component::<PotionContentsImpl>()
            .is_some_and(|contents| {
                contents.potion_id.is_some()
                    || contents.custom_color.is_some()
                    || !contents.custom_effects.is_empty()
            })
    }

    /// The server half of vanilla `Arrow.tick`: a tipped arrow left in the ground for 30
    /// seconds puffs out its potion and turns into a plain arrow.
    fn tick_potion_decay(&self) {
        if self.is_spectral()
            || !self.is_in_ground()
            || self.in_ground_time.load(Ordering::Relaxed) < EXPOSED_POTION_DECAY_TIME
            || !self.has_potion_contents()
        {
            return;
        }
        let entity = &self.entity;
        entity.world.load().send_to_tracking_players(
            entity,
            &CEntityStatus::new(entity.entity_id, EVENT_POTION_PUFF),
        );
        *self
            .item_stack
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = ItemStack::new(1, &Item::ARROW);
        self.update_color();
    }

    /// Vanilla `Arrow.doPostHurtEffects` and `SpectralArrow.doPostHurtEffects`.
    fn do_post_hurt_effects(&self, living: &LivingEntity) {
        let owner = self.owner.get(&self.entity.world.load());
        let effect_source = EffectSource {
            direct: self,
            owner: owner.as_deref(),
        };
        if self.is_spectral() {
            living.add_effect(Self::spectral_glowing_effect(
                self.duration.load(Ordering::Relaxed),
            ));
            return;
        }
        let item_stack = self.pickup_item();
        let scale = item_stack
            .get_data_component::<PotionDurationScaleImpl>()
            .map_or(1.0, |component| component.scale);
        crate::item::potion::PotionContents::apply_effects_to(
            living,
            crate::item::potion::PotionContents::read_potion_effects(&item_stack),
            scale,
            crate::item::potion::PotionApplicationSource::Arrow,
            Some(effect_source),
        );
    }
}

impl ArrowHooks for ArrowEntity {
    fn find_hit_entities(&self, from: Vector3<f64>, to: Vector3<f64>) -> Vec<EntityHit> {
        self.arrow.find_hit_entities(from, to)
    }

    fn tick_despawn(&self) {
        self.arrow.tick_despawn();
    }
}

impl EntityBase for ArrowEntity {
    fn get_projectile_owner(&self) -> Option<&ProjectileOwner> {
        Some(&self.owner)
    }

    fn get_projectile_item(&self) -> Option<ItemStack> {
        Some(self.pickup_item())
    }

    fn write_custom_nbt(&self, nbt: &mut NbtCompound) {
        self.write_arrow_nbt(nbt);
        if self.is_spectral() {
            nbt.put_int("Duration", self.duration.load(Ordering::Relaxed));
        }
    }

    fn read_custom_nbt(&self, nbt: &NbtCompound) {
        self.read_arrow_nbt(nbt);
        self.update_color();
        if let Some(duration) = nbt.get_int("Duration") {
            self.duration.store(duration, Ordering::Relaxed);
        }
    }

    fn init_data_tracker(&self) {
        let entity = self.get_entity();
        entity.set_synced_data(arrow_data::ID_FLAGS, self.flags());
        entity.set_synced_data(arrow_data::PIERCE_LEVEL, self.get_pierce_level());
        entity.set_synced_data(arrow_data::IN_GROUND, self.is_in_ground());
        self.update_color();

        if self.is_on_fire() {
            entity.set_on_fire(true);
        }
    }

    fn tick(&self, caller: &dyn EntityBase, server: &Server) {
        self.arrow.tick(self, caller, server);
        self.tick_potion_decay();
    }

    fn on_hit(&self, hit: ProjectileHit) {
        match hit {
            ProjectileHit::Block { pos, hit_pos, .. } => {
                self.arrow.on_hit_block(self, pos, hit_pos, &|| {});
            }
            ProjectileHit::Entity {
                entity: target,
                hit_pos,
                ..
            } => {
                self.arrow.on_hit_entity(self, &target, hit_pos, &|living| {
                    self.do_post_hurt_effects(living);
                });
            }
        }
    }

    fn get_entity(&self) -> &Entity {
        &self.arrow.entity
    }

    fn get_living_entity(&self) -> Option<&LivingEntity> {
        None
    }

    fn on_player_collision(&self, player: &Arc<Player>) {
        self.player_touch(player, false);
    }

    fn cast_any(&self) -> &dyn std::any::Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::{ArrowEntity, ArrowPickup};
    use crate::entity::ai::test_support::world;
    use crate::entity::decoration::armor_stand::ArmorStandEntity;
    use crate::entity::interaction::InteractionEntity;
    use crate::entity::{Entity, EntityBase};
    use pumpkin_util::math::vector3::Vector3;
    use std::sync::Arc;

    type TestResult = Result<(), Box<dyn std::error::Error>>;
    use pumpkin_data::data_component::DataComponent;
    use pumpkin_data::data_component_impl::{
        DataComponentImpl, PotionContentsImpl, PotionDurationScaleImpl,
    };
    use pumpkin_data::entity::EntityType;
    use pumpkin_data::item::Item;
    use pumpkin_data::item_stack::ItemStack;

    fn tipped_payload(count: u8) -> ItemStack {
        let mut tipped = ItemStack::new(32, &Item::TIPPED_ARROW);
        tipped.patch.push((
            DataComponent::PotionContents,
            Some(
                PotionContentsImpl {
                    potion_id: Some(5),
                    custom_color: Some(0x123456),
                    custom_effects: Vec::new(),
                    custom_name: Some("payload".to_string()),
                }
                .to_dyn(),
            ),
        ));
        tipped.patch.push((
            DataComponent::PotionDurationScale,
            Some(PotionDurationScaleImpl { scale: 0.5 }.to_dyn()),
        ));
        tipped.copy_with_count(count)
    }

    #[test]
    fn projectile_payload_keeps_components_at_one_count() {
        let tipped = tipped_payload(32);

        let payload = tipped.copy_with_count(1);

        assert_eq!(payload.item_count, 1);
        assert!(payload.are_items_and_components_equal(&tipped));
        assert_eq!(
            ArrowEntity::entity_type_for_item(payload.item),
            &EntityType::ARROW
        );
        assert_eq!(
            ArrowEntity::entity_type_for_item(&Item::SPECTRAL_ARROW),
            &EntityType::SPECTRAL_ARROW
        );
    }

    #[test]
    fn spectral_arrow_applies_vanilla_glowing_effect() {
        let effect = ArrowEntity::spectral_glowing_effect(super::DEFAULT_DURATION);

        assert_eq!(
            effect.effect_type,
            &pumpkin_data::effect::StatusEffect::GLOWING
        );
        assert_eq!(effect.duration, 200);
        assert_eq!(effect.amplifier, 0);
        assert!(effect.show_particles);
        assert!(effect.show_icon);
    }

    #[test]
    fn tipped_arrow_effect_color_uses_custom_color_or_effects() {
        let tipped = tipped_payload(1);
        let color = ArrowEntity::get_effect_color(&tipped);
        assert_eq!(color, 0x123456);

        let normal = ItemStack::new(1, &Item::ARROW);
        let normal_color = ArrowEntity::get_effect_color(&normal);
        assert_eq!(normal_color, -1);
    }

    #[tokio::test]
    async fn arrow_nbt_round_trips_pickup_item_and_state() -> TestResult {
        let folder = tempfile::tempdir()?;
        let world = world(&folder);
        let pos = Vector3::new(0.0, 64.0, 0.0);
        let payload = tipped_payload(1);
        let arrow = ArrowEntity::new_with_item(
            Entity::new(world.clone(), pos, &EntityType::ARROW),
            None,
            &payload,
            ArrowPickup::CreativeOnly,
        );
        arrow.set_pierce_level(3);
        arrow.set_base_damage(4.5);
        let mut nbt = pumpkin_nbt::compound::NbtCompound::new();
        arrow.write_custom_nbt(&mut nbt);

        let restored = ArrowEntity::new(Entity::new(world.clone(), pos, &EntityType::ARROW), None);
        restored.read_custom_nbt(&nbt);
        let restored_item = restored.pickup_item();
        world.level.shutdown().await;

        assert_eq!(restored.pickup(), ArrowPickup::CreativeOnly);
        assert_eq!(restored.get_pierce_level(), 3);
        assert!((restored.get_base_damage() - 4.5).abs() < f64::EPSILON);
        assert!(restored_item.are_equal(&payload));
        Ok(())
    }

    #[tokio::test]
    async fn arrows_fly_through_interaction_entities() -> TestResult {
        let folder = tempfile::tempdir()?;
        let world = world(&folder);
        let pos = Vector3::new(0.0, 64.0, 0.0);
        let arrow_entity = Entity::new(world.clone(), pos, &EntityType::ARROW);
        let arrow = ArrowEntity::new(arrow_entity, None);
        let interaction: Arc<dyn EntityBase> =
            InteractionEntity::new(Entity::new(world.clone(), pos, &EntityType::INTERACTION));
        let stand: Arc<dyn EntityBase> = Arc::new(ArmorStandEntity::new(Entity::new(
            world.clone(),
            pos,
            &EntityType::ARMOR_STAND,
        )));

        let hits_interaction = arrow.can_hit_entity(&*interaction);
        let hits_stand = arrow.can_hit_entity(&*stand);
        world.level.shutdown().await;

        assert!(!hits_interaction);
        assert!(hits_stand);
        Ok(())
    }
}
