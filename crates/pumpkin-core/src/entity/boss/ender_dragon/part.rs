//! The parts of a dragon: what clients hit and what projectiles hit, never in the world's list.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use pumpkin_data::damage::DamageType;
use pumpkin_util::math::vector3::Vector3;

use super::EnderDragonEntity;
use crate::entity::{Entity, EntityBase, living::LivingEntity};

/// The index of each part, in the order clients number them after the dragon's id.
pub const HEAD: usize = 0;
pub const NECK: usize = 1;
pub const BODY: usize = 2;
pub const TAIL: usize = 3;
pub const WING_1: usize = 6;
pub const WING_2: usize = 7;

/// Width and height of each part, vanilla's `EnderDragon` constructor.
pub const PART_SIZES: [(f32, f32); 8] = [
    (1.0, 1.0),
    (3.0, 3.0),
    (5.0, 3.0),
    (2.0, 2.0),
    (2.0, 2.0),
    (2.0, 2.0),
    (4.0, 2.0),
    (4.0, 2.0),
];

pub struct EnderDragonPart {
    pub entity: Entity,
    pub dragon_uuid: uuid::Uuid,
    /// Vanilla: the head and the neck take a hit in full, the rest a quarter.
    takes_full_damage: bool,
}

impl EnderDragonPart {
    #[must_use]
    pub const fn new(entity: Entity, dragon_uuid: uuid::Uuid, takes_full_damage: bool) -> Self {
        Self {
            entity,
            dragon_uuid,
            takes_full_damage,
        }
    }

    fn dragon(&self) -> Option<Arc<dyn EntityBase>> {
        self.entity
            .world
            .load()
            .entities
            .load()
            .iter()
            .find(|entity| entity.get_entity().entity_uuid == self.dragon_uuid)
            .cloned()
    }
}

impl EntityBase for EnderDragonPart {
    fn get_entity(&self) -> &Entity {
        &self.entity
    }

    /// Vanilla `EnderDragonPart.hurtServer`: the dragon takes it, by this part.
    fn damage_with_context(
        &self,
        _caller: &dyn EntityBase,
        amount: f32,
        damage_type: DamageType,
        position: Option<Vector3<f64>>,
        source: Option<&dyn EntityBase>,
        cause: Option<&dyn EntityBase>,
    ) -> bool {
        let Some(dragon) = self.dragon() else {
            return false;
        };
        if self.takes_full_damage
            && let Some(dragon) = dragon.cast_any().downcast_ref::<EnderDragonEntity>()
        {
            dragon.hit_in_full.store(true, Ordering::Relaxed);
        }
        dragon.damage_with_context(
            dragon.as_ref(),
            amount,
            damage_type,
            position,
            source,
            cause,
        )
    }

    fn cast_any(&self) -> &dyn std::any::Any {
        self
    }

    fn get_living_entity(&self) -> Option<&LivingEntity> {
        None
    }

    fn can_hit(&self) -> bool {
        true
    }
}
