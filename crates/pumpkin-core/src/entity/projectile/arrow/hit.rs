//! What vanilla `AbstractArrow` does when it hits something or a player touches it.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use pumpkin_data::attributes::Attributes;
use pumpkin_data::damage::DamageType;
use pumpkin_data::entity::EntityType;
use pumpkin_data::sound::Sound;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use rand::RngExt;

use super::ArrowPickup;
use super::abstract_arrow::AbstractArrow;
use crate::enchantment::EnchantmentHelper;
use crate::entity::item::ItemEntity;
use crate::entity::living::LivingEntity;
use crate::entity::player::Player;
use crate::entity::projectile_deflection::ProjectileDeflectionType;
use crate::entity::{Entity, EntityBase};

/// Java's `Math.signum`, which is 0 for 0 where Rust's is 1.
fn signum(value: f64) -> f64 {
    if value == 0.0 { 0.0 } else { value.signum() }
}

impl AbstractArrow {
    /// The pitch of `playSound(soundEvent, 1.0F, 1.2F / (random.nextFloat() * 0.2F + 0.9F))`.
    fn hit_sound_pitch() -> f32 {
        1.2 / rand::random::<f32>().mul_add(0.2, 0.9)
    }

    /// Vanilla `onHitEntity`. `do_post_hurt_effects` is vanilla's `doPostHurtEffects`.
    pub(crate) fn on_hit_entity(
        &self,
        projectile: &dyn EntityBase,
        target: &Arc<dyn EntityBase>,
        hit_pos: Vector3<f64>,
        do_post_hurt_effects: &dyn Fn(&LivingEntity),
    ) {
        let entity = &self.entity;
        let Some(mut damage) = self.note_pierced_and_compute_damage(target) else {
            entity.remove();
            return;
        };
        if self.is_critical.load(Ordering::Relaxed) {
            damage = damage.saturating_add(rand::rng().random_range(0..damage / 2 + 2));
        }

        let target_entity = target.get_entity();
        let remaining_fire_ticks = target_entity.fire_ticks.load(Ordering::Relaxed);
        if entity.is_on_fire() {
            target_entity.set_on_fire_for(5.0);
        }

        let owner = self.owner.get(&entity.world.load());
        let hurt = target.damage_with_context(
            target.as_ref(),
            damage as f32,
            DamageType::ARROW,
            Some(hit_pos),
            Some(projectile),
            // Like vanilla, an ownerless arrow is its own cause.
            Some(owner.as_deref().unwrap_or(projectile)),
        );

        if hurt {
            if let Some(living) = target.get_living_entity() {
                if self.get_pierce_level() == 0 {
                    living.add_stuck_arrow();
                }
                self.do_knockback(target_entity, living);
                do_post_hurt_effects(living);
                self.notify_owner_of_player_hit(&**target);
            }
            projectile.play_entity_sound(self.sound_event.load(), 1.0, Self::hit_sound_pitch());
            if self.get_pierce_level() == 0 {
                entity.remove();
            }
        } else if target.projectile_receives_side_effects_on_hit(false) {
            target_entity
                .fire_ticks
                .store(remaining_fire_ticks, Ordering::Relaxed);
            ProjectileDeflectionType::reverse(projectile, Vector3::new(0.2, 0.2, 0.2));
            if entity.velocity.load().length_squared() < 1.0e-7 {
                if self.pickup() == ArrowPickup::Allowed {
                    self.spawn_pickup_item();
                }
                entity.remove();
            }
        }
    }

    /// The start of vanilla `onHitEntity`: the damage by speed, the weapon's enchantments for
    /// this target, and the piercing count. `None` when the arrow has pierced all it may.
    fn note_pierced_and_compute_damage(&self, target: &Arc<dyn EntityBase>) -> Option<i32> {
        let target_entity = target.get_entity();
        let power = self.entity.velocity.load().length() as f32;
        let arrow_damage = self.get_weapon_item().map_or_else(
            || self.get_base_damage(),
            |weapon| {
                EnchantmentHelper::modify_projectile_damage(
                    &weapon,
                    target_entity.entity_type,
                    self.get_base_damage(),
                )
            },
        );
        let damage = (f64::from(power) * arrow_damage)
            .clamp(0.0, f64::from(i32::MAX))
            .ceil() as i32;

        let pierce = usize::from(self.get_pierce_level());
        if pierce > 0 {
            let mut pierced = self
                .pierced_entities
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if pierced.len() > pierce {
                return None;
            }
            pierced.push(target_entity.entity_id);
        }
        Some(damage)
    }

    /// Vanilla `doKnockback`: only the weapon's enchantments push, pure damage knocks back in
    /// `LivingEntity.hurtServer`.
    pub(crate) fn do_knockback(&self, target: &Entity, living: &LivingEntity) {
        let Some(weapon) = self.get_weapon_item() else {
            return;
        };
        let knockback = f64::from(EnchantmentHelper::modify_knockback(&weapon, 0.0));
        if knockback <= 0.0 {
            return;
        }
        let resistance =
            (1.0 - living.get_attribute_value(&Attributes::KNOCKBACK_RESISTANCE)).max(0.0);
        let velocity = self.entity.velocity.load();
        let scale = knockback * 0.6 * resistance;
        let push = Vector3::new(velocity.x, 0.0, velocity.z)
            .normalize()
            .multiply(scale, scale, scale);
        if push.length_squared() > 0.0 {
            target.add_velocity(Vector3::new(push.x, 0.1, push.z));
        }
    }

    /// Vanilla's `PLAY_ARROW_HIT_SOUND`: the ding for who shot a player.
    fn notify_owner_of_player_hit(&self, target: &dyn EntityBase) {
        if target.get_player().is_none() || self.entity.is_silent() {
            return;
        }
        if let Some(owner) = self.owner.get_player(&self.entity.world.load())
            && owner.entity_id() != target.get_entity().entity_id
        {
            owner.send_game_event(
                pumpkin_protocol::java::client::play::GameEvent::ArrowHitPlayer,
                0.0,
            );
        }
    }

    /// Vanilla `spawnAtLocation(level, getPickupItem(), 0.1F)`.
    pub(crate) fn spawn_pickup_item(&self) {
        let world = self.entity.world.load_full();
        let position = self.entity.pos.load().add_raw(0.0, f64::from(0.1f32), 0.0);
        let item = ItemEntity::new(
            Entity::new(world.clone(), position, &EntityType::ITEM),
            self.pickup_item(),
        );
        world.spawn_entity(Arc::new(item));
    }

    /// Vanilla `onHitBlock`. `enchantment_effects` is vanilla's `hitBlockEnchantmentEffects`.
    pub(crate) fn on_hit_block(
        &self,
        projectile: &dyn EntityBase,
        pos: BlockPos,
        hit_pos: Vector3<f64>,
        enchantment_effects: &dyn Fn(),
    ) {
        let entity = &self.entity;
        let world = entity.world.load();
        let block = world.get_block(&pos);
        let state = world.get_block_state(&pos);
        self.last_state.store(Some(state.id));
        if let Some(server) = world.server.upgrade() {
            world
                .block_registry
                .on_projectile_hit(block, &world, projectile, &pos, state, &hit_pos, &server);
        }
        if block == &pumpkin_data::Block::TARGET
            && let Some(player) = self.owner.get_player(&world)
        {
            player.trigger_advancement(
                crate::entity::player::advancement::trigger::AdvancementTrigger::Bullseye,
            );
        }
        enchantment_effects();

        let velocity = entity.velocity.load();
        let offset = Vector3::new(signum(velocity.x), signum(velocity.y), signum(velocity.z))
            .multiply(f64::from(0.05f32), f64::from(0.05f32), f64::from(0.05f32));
        entity.set_pos(entity.pos.load().sub(&offset));
        entity.velocity.store(Vector3::new(0.0, 0.0, 0.0));
        projectile.play_entity_sound(self.sound_event.load(), 1.0, Self::hit_sound_pitch());
        self.set_in_ground(true);
        self.shake_time.store(Self::SHAKE_TIME, Ordering::Relaxed);
        self.set_critical(false);
        self.set_pierce_level(0);
        self.set_sound_event(Sound::EntityArrowHit);
        self.pierced_entities
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
    }

    /// Vanilla `playerTouch` and `tryPickup`. `owner_may_take` is the trident's extra rule: one
    /// on its way back goes to its owner, whatever its pickup rule says.
    pub(crate) fn player_touch(&self, player: &Arc<Player>, owner_may_take: bool) {
        if !(self.is_in_ground() || self.is_no_physics())
            || self.shake_time.load(Ordering::Relaxed) > 0
            || player.living_entity.health.load() <= 0.0
        {
            return;
        }
        let add_to_inventory = match self.pickup() {
            ArrowPickup::Allowed => true,
            ArrowPickup::CreativeOnly if player.is_creative() => false,
            ArrowPickup::Disallowed | ArrowPickup::CreativeOnly if owner_may_take => true,
            ArrowPickup::Disallowed | ArrowPickup::CreativeOnly => return,
        };

        if !self.fire_pickup_event(player) {
            return;
        }
        let picked_up = self.pickup_item();
        if add_to_inventory {
            let mut stack = picked_up.clone();
            if !player.inventory.insert_stack_anywhere(&mut stack) && !player.is_creative() {
                return;
            }
        }
        player.living_entity.pickup(&self.entity, Some(&picked_up), 1);
        self.entity.remove();
    }

    /// Fires `PlayerPickupArrowEvent`; `false` when a plugin cancelled it.
    fn fire_pickup_event(&self, player: &Arc<Player>) -> bool {
        let Some(server) = player.world().server.upgrade() else {
            return true;
        };
        let mut event =
            crate::plugin::api::events::player::player_pickup_arrow::PlayerPickupArrowEvent::new(
                player.clone(),
                self.entity.entity_id,
            );
        server.plugin_manager.fire_blocking(&server, &mut event);
        !event.cancelled
    }
}
