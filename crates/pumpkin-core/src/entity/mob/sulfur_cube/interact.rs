//! What a sulfur cube does with players and hits: right clicks, pushes, touch damage, damage.

use std::sync::Arc;
use std::sync::atomic::Ordering::Relaxed;

use pumpkin_data::attributes::Attributes;
use pumpkin_data::damage::DamageType;
use pumpkin_data::entity::EntityType;
use pumpkin_data::game_event::GameEvent;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::Sound;
use pumpkin_data::statistic::StatisticCategory;
use pumpkin_data::tag::{self, Taggable};
use pumpkin_data::translation;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_util::text::TextComponent;

use super::SulfurCubeEntity;
use super::body::is_swallowable_item;
use crate::entity::EntityBase;
use crate::entity::ageable::AgeableMob;
use crate::entity::bucketable::bucket_mob_pickup;
use crate::entity::mob::Mob;
use crate::entity::player::Player;
use crate::entity::player::advancement::trigger::AdvancementTrigger;

/// Vanilla `SulfurCube.PUSH_DISTANCE_THRESHOLD`.
const PUSH_DISTANCE_THRESHOLD: f32 = 1.3;
/// Vanilla `SulfurCube.MAX_PLAYER_PUSH_SPEED`.
const MAX_PLAYER_PUSH_SPEED: f64 = 0.5;
/// Vanilla `SulfurCube.PLAYER_PUSH_SPEED_SCALE_MULTIPLIER`.
const PLAYER_PUSH_SPEED_SCALE_MULTIPLIER: f32 = 0.3;
/// Vanilla `SulfurCube.VEHICLE_PUSH_SPEED_SCALE_MULTIPLIER`.
const VEHICLE_PUSH_SPEED_SCALE_MULTIPLIER: f32 = 0.16;
/// Vanilla `SulfurCube.VERTICAL_PUSH_MULTIPLIER`.
const VERTICAL_PUSH_MULTIPLIER: f32 = 0.3;
/// Vanilla `SulfurCube.playerPush`: the player's speed counts double.
const PLAYER_SPEED_FACTOR: f64 = 2.0;
/// Vanilla `SulfurCube.playerPush`: ticks per second of the sound cooldown.
const TICKS_PER_SECOND: f32 = 20.0;

impl SulfurCubeEntity {
    /// Vanilla `Mob.interact`: the lead first, then `SulfurCube.mobInteract`.
    pub(super) fn interact_with_item(&self, player: &Arc<Player>, stack: &mut ItemStack) -> bool {
        if !self.mob_entity.living_entity.is_alive() {
            return false;
        }
        if self
            .mob_entity
            .mob_interact(player, stack, self.can_be_leashed())
        {
            return true;
        }
        if self.is_baby() {
            return self.interact_as_baby(player, stack);
        }
        if self.is_primed() {
            return false;
        }
        let is_igniter = stack.item == &Item::FLINT_AND_STEEL || stack.item == &Item::FIRE_CHARGE;
        if self.can_explode() && is_igniter {
            return self.ignite(player, stack);
        }
        if stack.item == &Item::SHEARS && self.ready_for_shearing() {
            return self.shear_by_player(player, stack);
        }
        if is_swallowable_item(stack) {
            let swallowed = self.equip_item(stack);
            if swallowed {
                stack.decrement_unless_creative(player.gamemode.load(), 1);
                let entity = self.get_entity();
                entity
                    .world
                    .load()
                    .emit_game_event(GameEvent::EntityInteract.name(), entity.pos.load());
            }
            return swallowed;
        }
        bucket_mob_pickup(player, stack, self)
            .unwrap_or_else(|| self.ageable_interact(player, stack))
    }

    /// Vanilla `SulfurCube.mobInteract` of a baby: food makes it grow.
    fn interact_as_baby(&self, player: &Arc<Player>, stack: &mut ItemStack) -> bool {
        if stack.item.has_tag(&tag::Item::MINECRAFT_SULFUR_CUBE_FOOD) && self.can_age_up() {
            let age = self.get_age();
            stack.decrement_unless_creative(player.gamemode.load(), 1);
            self.age_up(Self::get_speed_up_seconds_when_feeding(-age), true);
            self.make_sound(Sound::EntitySmallSulfurCubeEat);
            return true;
        }
        self.ageable_interact(player, stack)
    }

    /// Vanilla `SulfurCube.mobInteract` with flint and steel or a fire charge.
    fn ignite(&self, player: &Arc<Player>, stack: &mut ItemStack) -> bool {
        let world = self.get_entity().world.load();
        if !world.level_info.load().game_rules.tnt_explodes {
            player.send_system_message_raw(
                &TextComponent::translate(translation::java::BLOCK_MINECRAFT_TNT_DISABLED, []),
                true,
            );
            return false;
        }
        self.prime_time(false);
        let item_id = stack.item.id;
        if stack.item == &Item::FLINT_AND_STEEL {
            let _ = player.damage_item_stack(stack, 1);
        } else {
            stack.decrement_unless_creative(player.gamemode.load(), 1);
        }
        player.increment_stat(StatisticCategory::Used, i32::from(item_id), 1);
        true
    }

    /// Vanilla `SulfurCube.mobInteract` with shears, with the event of Pumpkin's sheep.
    fn shear_by_player(&self, player: &Arc<Player>, stack: &mut ItemStack) -> bool {
        let entity = self.get_entity();
        let world = entity.world.load();
        if let Some(server) = world.server.upgrade() {
            let mut event =
                crate::plugin::api::events::player::player_shear_entity::PlayerShearEntityEvent {
                    player: player.clone(),
                    entity_id: entity.entity_id,
                    hand: 0,
                    cancelled: false,
                };
            server.plugin_manager.fire_blocking(&server, &mut event);
            if event.cancelled {
                return false;
            }
        }
        let body = self.body_item();
        self.shear();
        world.emit_game_event(GameEvent::Shear.name(), entity.pos.load());
        let _ = player.damage_item_stack(stack, 1);
        player.trigger_advancement(AdvancementTrigger::PlayerShearedEquipment {
            entity_type: format!("minecraft:{}", EntityType::SULFUR_CUBE.resource_name),
            item_id: format!("minecraft:{}", body.item.registry_key),
        });
        true
    }

    /// Vanilla `SulfurCube.applyContactDamage`.
    pub(super) fn apply_contact_damage(&self, target: &dyn EntityBase) {
        let contact_damages = self.effects().contact_damages.clone();
        for damage in contact_damages {
            let this: &dyn EntityBase = self;
            let source =
                (damage.attribute_to_source || target.get_player().is_some()).then_some(this);
            target.damage_with_context(
                target,
                damage.amount.sample(),
                damage.damage_type,
                None,
                source,
                source,
            );
        }
    }

    /// Vanilla `SulfurCube.playerPush`: walking into a cube with a block shoves it.
    pub(super) fn player_push(&self, player: &Arc<Player>) {
        if !self.has_body_item() {
            return;
        }
        let player_entity = player.get_entity();
        let root_vehicle = player_entity.get_root_vehicle();
        let pusher = root_vehicle
            .as_ref()
            .map_or(player_entity, |vehicle| vehicle.get_entity());
        let living = &self.mob_entity.living_entity;
        let entity = &living.entity;
        let own_pos = entity.pos.load();
        let pusher_pos = pusher.pos.load();
        let cube_to_pusher = own_pos - pusher_pos;
        let cube_top = own_pos.y + f64::from(entity.height());
        let pusher_top = pusher_pos.y + f64::from(pusher.height());
        if cube_to_pusher.horizontal_length() >= f64::from(PUSH_DISTANCE_THRESHOLD)
            || pusher_pos.y > cube_top
            || pusher_top <= own_pos.y
        {
            return;
        }

        let knockback =
            (1.0 - living.get_attribute_value(&Attributes::KNOCKBACK_RESISTANCE)).max(0.0);
        let direction = Vector3::new(cube_to_pusher.x, 0.0, cube_to_pusher.z).normalize()
            * knockback;
        let scale = if root_vehicle.is_some() {
            VEHICLE_PUSH_SPEED_SCALE_MULTIPLIER
        } else {
            PLAYER_PUSH_SPEED_SCALE_MULTIPLIER
        };
        let player_speed = (player_entity.movement.load().length()
            * PLAYER_SPEED_FACTOR
            * f64::from(scale))
        .clamp(0.0, MAX_PLAYER_PUSH_SPEED);
        let vertical = if entity.on_ground.load(Relaxed) {
            knockback * f64::from(VERTICAL_PUSH_MULTIPLIER)
        } else {
            0.0
        };
        let push = Vector3::new(direction.x, vertical, direction.z) * player_speed;

        entity.velocity_dirty.store(true, Relaxed);
        self.play_push_sound(push);
        entity.velocity.store(entity.velocity.load() + push);
        self.apply_contact_damage(player.as_ref());
    }

    fn play_push_sound(&self, push: Vector3<f64>) {
        let settings = self.effects().sound_settings;
        let threshold = f64::from(settings.push_sound_impulse_threshold);
        if push.length_squared() > threshold * threshold
            && self.push_sound_cooldown.load(Relaxed) <= 0
        {
            self.push_sound_cooldown.store(
                (settings.push_sound_cooldown * TICKS_PER_SECOND) as i32,
                Relaxed,
            );
            self.play_entity_sound(settings.push_sound, 1.0, 1.0);
        }
    }

    /// Vanilla `SulfurCube.hurtServer`: `Some` when its block took the hit, `None` runs the usual
    /// hurt.
    pub(super) fn intercept_hurt(
        &self,
        amount: f32,
        damage_type: DamageType,
        position: Option<Vector3<f64>>,
        source: Option<&dyn EntityBase>,
        cause: Option<&dyn EntityBase>,
    ) -> Option<bool> {
        if !self.has_body_item() {
            return None;
        }
        if self.can_explode() && !self.is_primed() {
            if damage_type.has_tag(&tag::DamageType::MINECRAFT_IS_FIRE)
                || source.is_some_and(is_burning_arrow)
            {
                self.prime_time(false);
            } else if damage_type.has_tag(&tag::DamageType::MINECRAFT_IS_EXPLOSION) {
                self.prime_time(true);
            }
        }
        if !damage_type.has_tag(&tag::DamageType::MINECRAFT_SULFUR_CUBE_WITH_BLOCK_IMMUNE_TO) {
            return None;
        }
        if !damage_type.has_tag(&tag::DamageType::MINECRAFT_NO_KNOCKBACK) {
            self.deal_default_knockback(amount, position, source, cause);
        }
        Some(true)
    }
}

/// Vanilla `sourceEntity instanceof AbstractArrow projectile && projectile.isOnFire()`.
fn is_burning_arrow(source: &dyn EntityBase) -> bool {
    let entity = source.get_entity();
    (entity.entity_type == &EntityType::TRIDENT
        || entity
            .entity_type
            .has_tag(&tag::EntityType::MINECRAFT_ARROWS))
        && entity.is_on_fire()
}
