//! Vanilla `VehicleEntity.hurtServer` and `destroy` with the minecart overrides.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use pumpkin_data::damage::DamageType;
use pumpkin_data::data_component_impl::CustomNameImpl;
use pumpkin_data::entity::EntityType;
use pumpkin_data::game_event::GameEvent;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::tag::{self, Taggable};
use pumpkin_inventory::Inventory;

use crate::entity::EntityBase;
use crate::entity::mob::piglin_anger::anger_nearby_piglins;
use crate::entity::player::Player;
use crate::plugin::api::events::vehicle::{
    vehicle_damage::VehicleDamageEvent, vehicle_destroy::VehicleDestroyEvent,
};
use crate::world::game_event::GameEventContext;

use super::{MinecartEntity, MinecartKind};

/// Vanilla `VehicleEntity.hurtServer`: the damage a survival player needs to exceed.
const MAX_DAMAGE: f32 = 40.0;
/// Vanilla `MinecartTNT.destroy`: at this speed a TNT cart primes instead of breaking.
const TNT_PRIME_SPEED_SQUARED: f32 = 0.01;

/// Vanilla `Entity.isOnFire` on the server.
fn is_on_fire(entity: &dyn EntityBase) -> bool {
    let base = entity.get_entity();
    !base.is_fire_immune() && base.fire_ticks.load(Ordering::Relaxed) > 0
}

/// Vanilla `source.getDirectEntity() instanceof AbstractArrow`.
fn is_abstract_arrow(entity: &dyn EntityBase) -> bool {
    [
        &EntityType::ARROW,
        &EntityType::SPECTRAL_ARROW,
        &EntityType::TRIDENT,
    ]
    .contains(&entity.get_entity().entity_type)
}

/// Vanilla `MinecartTNT.damageSourceIgnitesTnt`.
fn damage_source_ignites_tnt(damage_type: DamageType, source: Option<&dyn EntityBase>) -> bool {
    if let Some(projectile) = source.filter(|source| source.get_projectile_owner().is_some()) {
        return is_on_fire(projectile);
    }
    damage_type.has_tag(&tag::DamageType::MINECRAFT_IS_FIRE)
        || damage_type.has_tag(&tag::DamageType::MINECRAFT_IS_EXPLOSION)
}

impl MinecartEntity {
    /// Vanilla `hurtServer`. `source` is vanilla's direct entity, `cause` its attacker.
    pub(super) fn hurt_server(
        &self,
        amount: f32,
        damage_type: DamageType,
        source: Option<&dyn EntityBase>,
        cause: Option<&dyn EntityBase>,
    ) -> bool {
        let entity = &self.vehicle.entity;
        // Vanilla `MinecartTNT.hurtServer`, ahead of everything the base method checks.
        if let MinecartKind::Tnt(minecart) = &self.kind
            && let Some(arrow) = source.filter(|source| is_abstract_arrow(*source))
            && is_on_fire(arrow)
        {
            minecart.explode(entity, arrow.get_entity().velocity.load().length_squared());
        }
        if entity.is_removed() {
            return true;
        }
        if entity.is_invulnerable_to(&damage_type, cause) {
            return false;
        }

        let attacker_id = source.map(|source| source.get_entity().entity_id);
        let world = entity.world.load();
        let server = world.server.upgrade();
        let mut damage_event = VehicleDamageEvent::new(entity.entity_id, amount, attacker_id);
        if let Some(server) = &server {
            server
                .plugin_manager
                .fire_blocking(server, &mut damage_event);
        }
        if damage_event.cancelled {
            return false;
        }

        let damage = self.vehicle.apply_damage_wobble(amount);
        world.emit_game_event_with_context(
            GameEvent::EntityDamage.name(),
            entity.pos.load(),
            &GameEventContext {
                source_entity: cause,
            },
        );

        let creative = cause
            .and_then(EntityBase::get_player)
            .is_some_and(Player::is_creative);
        let destroys = (matches!(self.kind, MinecartKind::Tnt(_))
            && damage_source_ignites_tnt(damage_type, source))
            || (!creative && damage > MAX_DAMAGE);
        if !destroys && !creative {
            return true;
        }

        let mut destroy_event = VehicleDestroyEvent::new(entity.entity_id, attacker_id);
        if let Some(server) = &server {
            server
                .plugin_manager
                .fire_blocking(server, &mut destroy_event);
        }
        if destroy_event.cancelled {
            return false;
        }
        if destroys {
            self.destroy(damage_type, source);
        } else {
            self.discard();
        }
        true
    }

    /// Vanilla `destroy(level, source)` with the `MinecartTNT` and container overrides.
    fn destroy(&self, damage_type: DamageType, source: Option<&dyn EntityBase>) {
        let entity = &self.vehicle.entity;
        if let MinecartKind::Tnt(minecart) = &self.kind {
            let speed_squared = entity.velocity.load().horizontal_length_squared();
            if !damage_source_ignites_tnt(damage_type, source)
                && speed_squared < f64::from(TNT_PRIME_SPEED_SQUARED)
            {
                self.destroy_with_drop();
            } else if minecart.fuse() < 0 {
                minecart.prime_with_random_fuse(entity);
            }
            return;
        }
        self.destroy_with_drop();
        if self.container().is_some() {
            self.anger_piglins_at_breaker(source);
        }
    }

    /// The piglin part of vanilla `ContainerEntity.chestVehicleDestroyed`.
    fn anger_piglins_at_breaker(&self, source: Option<&dyn EntityBase>) {
        let world = self.vehicle.entity.world.load();
        if !world.level_info.load().game_rules.entity_drops {
            return;
        }
        let Some(player_id) = source
            .and_then(EntityBase::get_player)
            .map(Player::entity_id)
        else {
            return;
        };
        if let Some(player) = world.get_player_by_id(player_id) {
            anger_nearby_piglins(&world, &player, true);
        }
    }

    /// Vanilla `VehicleEntity.destroy(level, dropItem)`.
    fn destroy_with_drop(&self) {
        let entity = &self.vehicle.entity;
        self.kill_minecart();
        if !entity
            .world
            .load()
            .level_info
            .load()
            .game_rules
            .entity_drops
        {
            return;
        }
        let mut stack = ItemStack::new(1, self.drop_item());
        if let Some(name) = entity.custom_name.load().as_ref() {
            stack.set_data_component(CustomNameImpl { name: name.clone() });
        }
        entity.spawn_at_location(stack);
    }

    /// Vanilla `Entity.kill`, with `AbstractMinecartContainer.remove`'s content drop, which runs
    /// whatever the `entityDrops` game rule says.
    pub(super) fn kill_minecart(&self) {
        let entity = &self.vehicle.entity;
        if let Some(container) = self.container() {
            self.unpack_container_loot(container);
            let inventory: Arc<dyn Inventory> = container.clone();
            entity
                .world
                .load()
                .scatter_inventory(&entity.block_pos.load(), &inventory);
        }
        self.discard();
        entity
            .world
            .load()
            .emit_game_event(GameEvent::EntityDie.name(), entity.pos.load());
    }

    /// Vanilla `Entity.discard`: passengers get off, the cart goes.
    pub(super) fn discard(&self) {
        self.eject_passengers();
        self.vehicle.entity.remove();
    }

    /// Vanilla `VehicleEntity.ignoreExplosion`, on what vanilla's `getIndirectSourceEntity`
    /// resolves `source` to.
    #[must_use]
    pub fn ignores_explosion(&self, source: Option<&dyn EntityBase>) -> bool {
        let Some(source) = source else {
            return false;
        };
        let from_mob = source.get_mob().is_some()
            || source
                .get_owner_entity()
                .is_some_and(|owner| owner.get_mob().is_some());
        from_mob
            && !self
                .vehicle
                .entity
                .world
                .load()
                .level_info
                .load()
                .game_rules
                .mob_griefing
    }
}
