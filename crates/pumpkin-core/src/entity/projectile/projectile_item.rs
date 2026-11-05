//! Vanilla `ProjectileItem`: the items a dispenser or an ominous item spawner launches as a
//! projectile, with the launch settings of `ProjectileItem.DispenseConfig`.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use pumpkin_data::entity::EntityType;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::world::WorldEvent;
use pumpkin_util::math::vector3::Vector3;
use rand::{Rng, RngExt, rng};

use super::ThrownItemEntity;
use super::arrow::{ArrowEntity, ArrowPickup};
use super::egg::EggEntity;
use super::experience_bottle::ExperienceBottleEntity;
use super::firework_rocket::FireworkRocketEntity;
use super::lingering_potion::LingeringPotionEntity;
use super::owner::ProjectileOwner;
use super::small_fireball::SmallFireballEntity;
use super::snowball::SnowballEntity;
use super::splash_potion::SplashPotionEntity;
use super::trident::TridentEntity;
use super::wind_charge::{WIND_CHARGE_GRAVITY, WindChargeEntity};
use crate::entity::{Entity, EntityBase};
use crate::world::World;

/// Where a dispenser lets the projectile go: vanilla `DispenseConfig.positionFunction`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DispensePosition {
    /// `DispenserBlock.getDispensePosition(source, 0.7, (0, 0.1, 0))`: 0.7 blocks out of
    /// the centre along the facing and a tenth up.
    Default,
    /// `DispenserBlock.getDispensePosition(source, 1.0, ZERO)`: a full block out.
    FullBlock,
    /// `FireworkRocketItem.getEntityJustOutsideOfBlockPos`: just past the face.
    JustOutside,
}

/// Vanilla `ProjectileItem.DispenseConfig`: how a dispenser launches the item.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DispenseConfig {
    pub position: DispensePosition,
    pub uncertainty: f64,
    pub power: f64,
    /// Replaces the dispenser's `SoundDispenserProjectileLaunch`; an ominous item spawner
    /// plays it too.
    pub override_dispense_event: Option<WorldEvent>,
}

impl DispenseConfig {
    /// Vanilla `DispenseConfig.DEFAULT`.
    pub const DEFAULT: Self = Self {
        position: DispensePosition::Default,
        uncertainty: 6.0,
        power: 1.1,
        override_dispense_event: None,
    };
    /// `ThrowablePotionItem` and `ExperienceBottleItem`: harder and straighter.
    const POTION: Self = Self {
        uncertainty: Self::DEFAULT.uncertainty * 0.5,
        power: Self::DEFAULT.power * 1.25,
        ..Self::DEFAULT
    };
    /// `FireChargeItem` and `WindChargeItem`, each with its own launch sound.
    const CHARGE: Self = Self {
        position: DispensePosition::FullBlock,
        uncertainty: 6.666_666_5,
        power: 1.0,
        override_dispense_event: None,
    };
    /// `FireworkRocketItem`.
    const FIREWORK: Self = Self {
        position: DispensePosition::JustOutside,
        uncertainty: 1.0,
        power: 0.5,
        override_dispense_event: Some(WorldEvent::SoundFireworkShoot),
    };
}

const ARROWS: [&Item; 3] = [&Item::ARROW, &Item::TIPPED_ARROW, &Item::SPECTRAL_ARROW];
const EGGS: [&Item; 3] = [&Item::EGG, &Item::BLUE_EGG, &Item::BROWN_EGG];
const POTIONS: [&Item; 3] = [
    &Item::SPLASH_POTION,
    &Item::LINGERING_POTION,
    &Item::EXPERIENCE_BOTTLE,
];

fn is_one_of(item: &Item, items: &[&Item]) -> bool {
    items.iter().any(|candidate| candidate.id == item.id)
}

/// Vanilla `ProjectileItem.createDispenseConfig` for `item`, `None` when the item is no
/// projectile item.
#[must_use]
pub fn dispense_config(item: &Item) -> Option<DispenseConfig> {
    let config = if is_one_of(item, &ARROWS)
        || is_one_of(item, &EGGS)
        || item.id == Item::SNOWBALL.id
        || item.id == Item::TRIDENT.id
    {
        DispenseConfig::DEFAULT
    } else if is_one_of(item, &POTIONS) {
        DispenseConfig::POTION
    } else if item.id == Item::FIRE_CHARGE.id {
        DispenseConfig {
            override_dispense_event: Some(WorldEvent::SoundBlazeFireball),
            ..DispenseConfig::CHARGE
        }
    } else if item.id == Item::WIND_CHARGE.id {
        DispenseConfig {
            override_dispense_event: Some(WorldEvent::SoundWindChargeShoot),
            ..DispenseConfig::CHARGE
        }
    } else if item.id == Item::FIREWORK_ROCKET.id {
        DispenseConfig::FIREWORK
    } else {
        return None;
    };
    Some(config)
}

/// Vanilla `ProjectileItem.asProjectile` and `Projectile.spawnProjectileUsingShoot`.
///
/// The projectile of one of `stack` at `position`, facing `direction`, shot along `shoot`
/// with the config's power and uncertainty and owned by `owner`, not yet in the world.
/// `None` when the stack is no projectile item.
#[must_use]
pub fn shoot_projectile_item(
    world: &Arc<World>,
    position: Vector3<f64>,
    stack: &ItemStack,
    direction: Vector3<f64>,
    shoot: Vector3<f64>,
    config: &DispenseConfig,
    owner: Option<&Entity>,
) -> Option<Arc<dyn EntityBase>> {
    let item = stack.item;
    let projectile: Arc<dyn EntityBase> = if is_one_of(item, &ARROWS) {
        let entity = Entity::new(
            world.clone(),
            position,
            ArrowEntity::entity_type_for_item(item),
        );
        let arrow = ArrowEntity::new_with_item(entity, owner, stack, ArrowPickup::Allowed);
        arrow.apply_on_projectile_spawned(stack);
        arrow.set_velocity(shoot.x, shoot.y, shoot.z, config.power, config.uncertainty);
        Arc::new(arrow)
    } else if item.id == Item::TRIDENT.id {
        let entity = Entity::new(world.clone(), position, &EntityType::TRIDENT);
        let trident = TridentEntity::new_dispensed(entity, owner, stack.copy_with_count(1));
        trident.apply_on_projectile_spawned(stack);
        trident.set_velocity(shoot.x, shoot.y, shoot.z, config.power, config.uncertainty);
        Arc::new(trident)
    } else if item.id == Item::FIRE_CHARGE.id {
        // `FireChargeItem.asProjectile` aims the fireball a little off the direction; the
        // shot then sets its movement.
        let entity = Entity::new(world.clone(), position, &EntityType::SMALL_FIREBALL);
        let fireball = SmallFireballEntity::new(entity);
        fireball.set_item(stack);
        fireball.thrown.owner.set(owner);
        fireball
            .thrown
            .entity
            .set_velocity(jittered_direction(direction));
        fireball
            .thrown
            .set_velocity(shoot.x, shoot.y, shoot.z, config.power, config.uncertainty);
        Arc::new(fireball)
    } else if item.id == Item::WIND_CHARGE.id {
        let entity = Entity::new(world.clone(), position, &EntityType::WIND_CHARGE);
        let thrown = ThrownItemEntity {
            entity,
            owner: ProjectileOwner::new(owner),
            collides_with_projectiles: false,
            has_hit: AtomicBool::new(false),
            gravity: WIND_CHARGE_GRAVITY,
        };
        thrown.entity.set_velocity(jittered_direction(direction));
        thrown.set_velocity(shoot.x, shoot.y, shoot.z, config.power, config.uncertainty);
        Arc::new(WindChargeEntity::new_normal(thrown))
    } else if item.id == Item::FIREWORK_ROCKET.id {
        let entity = Entity::new(world.clone(), position, &EntityType::FIREWORK_ROCKET);
        let rocket = FireworkRocketEntity::new_with_stack(entity, stack.copy_with_count(1));
        rocket.set_shot_at_angle(true);
        rocket.entity.owner.set(owner);
        rocket
            .entity
            .set_velocity(shoot.x, shoot.y, shoot.z, config.power, config.uncertainty);
        Arc::new(rocket)
    } else {
        let thrown = Thrown::new(world, position, item)?;
        thrown.set_item_stack(stack.copy_with_count(1));
        thrown.thrown().owner.set(owner);
        thrown
            .thrown()
            .set_velocity(shoot.x, shoot.y, shoot.z, config.power, config.uncertainty);
        thrown.into_entity()
    };
    Some(projectile)
}

/// The thrown items that show their stack: one constructor shape for all of them.
enum Thrown {
    Egg(EggEntity),
    Snowball(SnowballEntity),
    SplashPotion(SplashPotionEntity),
    LingeringPotion(LingeringPotionEntity),
    ExperienceBottle(ExperienceBottleEntity),
}

impl Thrown {
    fn new(world: &Arc<World>, position: Vector3<f64>, item: &Item) -> Option<Self> {
        let entity = |entity_type| Entity::new(world.clone(), position, entity_type);
        let thrown = if is_one_of(item, &EGGS) {
            Self::Egg(EggEntity::new(entity(&EntityType::EGG)))
        } else if item.id == Item::SNOWBALL.id {
            Self::Snowball(SnowballEntity::new(entity(&EntityType::SNOWBALL)))
        } else if item.id == Item::SPLASH_POTION.id {
            Self::SplashPotion(SplashPotionEntity::new(entity(&EntityType::SPLASH_POTION)))
        } else if item.id == Item::LINGERING_POTION.id {
            Self::LingeringPotion(LingeringPotionEntity::new(entity(
                &EntityType::LINGERING_POTION,
            )))
        } else if item.id == Item::EXPERIENCE_BOTTLE.id {
            Self::ExperienceBottle(ExperienceBottleEntity::new(entity(
                &EntityType::EXPERIENCE_BOTTLE,
            )))
        } else {
            return None;
        };
        Some(thrown)
    }

    fn set_item_stack(&self, stack: ItemStack) {
        match self {
            Self::Egg(egg) => egg.set_item_stack(stack),
            Self::Snowball(snowball) => snowball.set_item_stack(stack),
            Self::SplashPotion(potion) => potion.set_item_stack(stack),
            Self::LingeringPotion(potion) => potion.set_item_stack(stack),
            Self::ExperienceBottle(bottle) => bottle.set_item_stack(stack),
        }
    }

    const fn thrown(&self) -> &ThrownItemEntity {
        match self {
            Self::Egg(egg) => &egg.thrown,
            Self::Snowball(snowball) => &snowball.thrown,
            Self::SplashPotion(potion) => &potion.thrown,
            Self::LingeringPotion(potion) => &potion.thrown,
            Self::ExperienceBottle(bottle) => &bottle.thrown,
        }
    }

    fn into_entity(self) -> Arc<dyn EntityBase> {
        match self {
            Self::Egg(egg) => Arc::new(egg),
            Self::Snowball(snowball) => Arc::new(snowball),
            Self::SplashPotion(potion) => Arc::new(potion),
            Self::LingeringPotion(potion) => Arc::new(potion),
            Self::ExperienceBottle(bottle) => Arc::new(bottle),
        }
    }
}

/// Vanilla `FireChargeItem.asProjectile` and `WindChargeItem.asProjectile`: the direction
/// with a triangular spread of 0.11485 on every axis.
fn jittered_direction(direction: Vector3<f64>) -> Vector3<f64> {
    const SPREAD: f64 = 0.114_850_000_000_000_01;
    let mut rng = rng();
    Vector3::new(
        triangle(&mut rng, direction.x, SPREAD),
        triangle(&mut rng, direction.y, SPREAD),
        triangle(&mut rng, direction.z, SPREAD),
    )
    .normalize()
}

/// Vanilla `RandomSource.triangle(mode, deviation)`.
pub fn triangle<R: Rng>(rng: &mut R, mode: f64, deviation: f64) -> f64 {
    (rng.random::<f64>() - rng.random::<f64>()).mul_add(deviation, mode)
}
