use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::entity::boss::ender_dragon::EnderDragonEntity;
use crate::entity::{Entity, EntityBase, living::LivingEntity};
use pumpkin_data::{
    damage::DamageType,
    entity::EntityType,
    tag::{self, Taggable},
};
use pumpkin_util::math::{position::BlockPos, vector3::Vector3};

pub struct EndCrystalEntity {
    entity: Entity,
    beam_target: Mutex<Option<BlockPos>>,
    show_bottom: AtomicBool,
    invulnerable: AtomicBool,
}

impl EndCrystalEntity {
    #[must_use]
    pub const fn new(mut entity: Entity) -> Self {
        entity.blocks_building = true;
        Self {
            entity,
            beam_target: Mutex::new(None),
            show_bottom: AtomicBool::new(false),
            invulnerable: AtomicBool::new(false),
        }
    }

    pub fn set_show_bottom(&self, show_bottom: bool) {
        self.show_bottom.store(show_bottom, Ordering::Relaxed);
        self.entity.set_synced_data(
            pumpkin_data::tracked_data::end_crystal::SHOW_BOTTOM,
            show_bottom,
        );
    }

    pub fn show_bottom(&self) -> bool {
        self.show_bottom.load(Ordering::Relaxed)
    }

    pub fn set_beam_target(&self, beam_target: Option<BlockPos>) {
        *self
            .beam_target
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = beam_target;
        self.entity.set_synced_data(
            pumpkin_data::tracked_data::end_crystal::BEAM_TARGET,
            beam_target,
        );
    }

    pub fn beam_target(&self) -> Option<BlockPos> {
        *self
            .beam_target
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub fn set_invulnerable(&self, invulnerable: bool) {
        self.invulnerable.store(invulnerable, Ordering::Relaxed);
    }

    pub fn is_invulnerable(&self) -> bool {
        self.invulnerable.load(Ordering::Relaxed)
    }
}

impl EntityBase for EndCrystalEntity {
    fn get_entity(&self) -> &Entity {
        &self.entity
    }

    fn get_living_entity(&self) -> Option<&LivingEntity> {
        None
    }

    fn damage_with_context(
        &self,
        _caller: &dyn EntityBase,
        _amount: f32,
        damage_type: DamageType,
        _position: Option<Vector3<f64>>,
        source: Option<&dyn EntityBase>,
        cause: Option<&dyn EntityBase>,
    ) -> bool {
        if self.is_invulnerable() {
            return false;
        }
        // Vanilla: the dragon cannot blow up its own crystals.
        if cause
            .or(source)
            .is_some_and(|by| by.get_entity().entity_type == &EntityType::ENDER_DRAGON)
        {
            return false;
        }

        self.entity.remove();
        let world = self.entity.world.load();
        if !damage_type.has_tag(&tag::DamageType::MINECRAFT_IS_EXPLOSION) {
            let pos = self.entity.pos.load();
            world.explode(
                Some(&self.entity),
                pos,
                6.0,
                crate::world::ExplosionInteraction::Block,
            );
        }

        let dragon = world.dragon_fight.as_ref().and_then(|fight_mutex| {
            let mut fight = fight_mutex
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let for_respawn = fight.is_respawning();
            fight.on_crystal_destroyed(&world, self.entity.entity_uuid);
            (!for_respawn).then_some(fight.dragon_uuid).flatten()
        });
        if let Some(dragon) = dragon.and_then(|uuid| {
            world
                .entities
                .load()
                .iter()
                .find(|entity| entity.get_entity().entity_uuid == uuid)
                .cloned()
        }) && let Some(dragon) = dragon.cast_any().downcast_ref::<EnderDragonEntity>()
        {
            dragon.on_crystal_destroyed(self, cause.or(source));
        }

        true
    }

    fn cast_any(&self) -> &dyn std::any::Any {
        self
    }
}
