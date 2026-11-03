//! The leash part of vanilla `Entity.interact`, for a boat as vanilla's `Leashable`.

use std::sync::Arc;

use pumpkin_data::game_event::GameEvent;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_util::math::boundingbox::BoundingBox;
use pumpkin_util::math::vector3::Vector3;

use super::{BoatEntity, has_infinite_materials};
use crate::entity::player::Player;
use crate::entity::{Entity, EntityBase};

/// Vanilla `Leashable.leashableInArea`: the cube it searches.
const LEASHABLE_SEARCH_SIZE: f64 = 32.0;

/// Vanilla `Leashable.dropLeash`.
pub(super) fn drop_leash(entity: &Entity) {
    entity.unleash();
    if !entity.is_leashed() {
        entity.spawn_at_location(ItemStack::new(1, &Item::LEAD));
    }
}

fn is_leashed_to(entity: &Entity, holder_id: i32) -> bool {
    entity
        .get_leash_holder()
        .is_some_and(|holder| holder.get_entity().entity_id == holder_id)
}

impl BoatEntity {
    /// Vanilla `Leashable.leashableInArea` around this boat, for one leash holder.
    fn leashed_nearby(&self, holder_id: i32) -> Vec<Arc<dyn EntityBase>> {
        let entity = &self.vehicle.entity;
        let bounding_box = entity.bounding_box.load();
        let center = (bounding_box.min + bounding_box.max) * 0.5;
        let area = BoundingBox::of_size(
            center,
            LEASHABLE_SEARCH_SIZE,
            LEASHABLE_SEARCH_SIZE,
            LEASHABLE_SEARCH_SIZE,
        );
        entity
            .world
            .load()
            .get_entities_at_box(&area)
            .into_iter()
            .filter(|other| is_leashed_to(other.get_entity(), holder_id))
            .collect()
    }

    /// Vanilla `Leashable.canHaveALeashAttachedTo`, leashing `other` to this boat.
    fn can_attach_to_boat(&self, other: &dyn EntityBase) -> bool {
        let entity = &self.vehicle.entity;
        let other_entity = other.get_entity();
        other_entity.entity_id != entity.entity_id
            && other_entity.leash_distance_to(entity) <= Entity::LEASH_SNAP_DISTANCE
            && other
                .get_mob()
                .is_none_or(crate::entity::mob::Mob::can_be_leashed)
    }

    /// The sneaking branch of vanilla `Entity.interact`: what the player leads is tied to this.
    fn tie_players_leashes_here(&self, player: &Arc<Player>) -> bool {
        let entity = &self.vehicle.entity;
        let world = entity.world.load();
        let Some(boat) = world.get_entity_by_id(entity.entity_id) else {
            return false;
        };

        let mut any_leashed = false;
        for other in self.leashed_nearby(player.entity_id()) {
            if self.can_attach_to_boat(other.as_ref()) {
                other.get_entity().leash_to(boat.clone());
                any_leashed = true;
            }
        }
        if any_leashed {
            self.emit_game_event(GameEvent::EntityAction, Some(&**player));
            self.play_entity_sound(Sound::ItemLeadTied, 1.0, 1.0);
        }
        any_leashed
    }

    /// Vanilla `Entity.dropAllLeashConnections` and `shearOffAllLeashConnections`.
    fn shear_off_all_leash_connections(&self, player: &Arc<Player>) -> bool {
        let entity = &self.vehicle.entity;
        let attached = self.leashed_nearby(entity.entity_id);
        let mut dropped = !attached.is_empty();
        if entity.is_leashed() {
            drop_leash(entity);
            dropped = true;
        }
        for other in attached {
            drop_leash(other.get_entity());
        }
        if dropped {
            self.emit_game_event(GameEvent::Shear, Some(&**player));
            let block_center: Vector3<f64> = entity.block_pos.load().to_centered_f64();
            entity.world.load().play_sound(
                Sound::ItemShearsSnip,
                SoundCategory::Players,
                &block_center,
            );
        }
        dropped
    }

    /// The lead and shears checks of vanilla `Entity.interact`. Returns whether they acted.
    pub(super) fn interact_with_leash(
        &self,
        player: &Arc<Player>,
        item_stack: &mut ItemStack,
    ) -> bool {
        let entity = &self.vehicle.entity;
        if player.get_entity().is_sneaking() && self.tie_players_leashes_here(player) {
            return true;
        }

        if item_stack.item.id == Item::SHEARS.id && self.shear_off_all_leash_connections(player) {
            if !has_infinite_materials(player) {
                let _ = item_stack.damage_item(1);
            }
            return true;
        }

        if !entity.is_alive() {
            return false;
        }

        let holder = entity.get_leash_holder();
        if holder
            .as_ref()
            .is_some_and(|holder| holder.get_entity().entity_id == player.entity_id())
        {
            if has_infinite_materials(player) {
                entity.remove_leash();
            } else {
                drop_leash(entity);
            }
            self.emit_game_event(GameEvent::EntityInteract, Some(&**player));
            self.play_entity_sound(Sound::ItemLeadUntied, 1.0, 1.0);
            return true;
        }

        let held_by_player = holder
            .as_ref()
            .is_some_and(|holder| holder.get_player().is_some());
        if item_stack.item.id == Item::LEAD.id
            && !held_by_player
            && entity.leash_distance_to(player.get_entity()) <= Entity::LEASH_SNAP_DISTANCE
        {
            if entity.is_leashed() {
                drop_leash(entity);
            }
            entity.leash_to(player.clone() as Arc<dyn EntityBase>);
            self.play_entity_sound(Sound::ItemLeadTied, 1.0, 1.0);
            if !has_infinite_materials(player) {
                item_stack.decrement(1);
            }
            return true;
        }

        false
    }
}
