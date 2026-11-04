//! Vanilla `KineticWeapon.damageEntities` and `LivingEntity.stabAttack` for a mob holding a spear.
//! The rules a player's attack adds (attack cooldown, player versus player settings, exhaustion)
//! stay in [`super::spear`].

use std::sync::Arc;
use std::sync::atomic::Ordering;

use pumpkin_data::attributes::Attributes;
use pumpkin_data::damage::DamageType;
use pumpkin_data::data_component_impl::{AttackRangeImpl, EquipmentSlot, KineticWeaponImpl};
use pumpkin_data::entity::EntityStatus;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_protocol::bedrock::server::actor_event::ActorEventID;
use pumpkin_util::Hand;
use pumpkin_util::math::vector3::Vector3;

use super::spear::{SpearItem, StabEffects};
use crate::entity::EntityBase;
use crate::entity::living::LivingEntity;

/// Vanilla `KineticWeapon.damageEntities`: conditions weigh `0.2` of a player's speed for others.
const MOB_ACTION_FACTOR: f64 = 0.2;

/// Vanilla `LivingEntity.updatingUsingItem` for a mob's kinetic weapon: it stops when the weapon
/// left the hand, else it strikes on its own.
pub fn mob_use_tick(living: &LivingEntity, caller: &dyn EntityBase) {
    let Some(used) = living
        .item_in_use
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
    else {
        return;
    };
    let Some(weapon) = used.get_data_component::<KineticWeaponImpl>() else {
        return;
    };
    let hand = *living
        .active_hand
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let slot = if hand == Some(Hand::Left) {
        EquipmentSlot::OFF_HAND
    } else {
        EquipmentSlot::MAIN_HAND
    };
    let held = living
        .entity_equipment
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(&slot);
    if held.item.id != used.item.id {
        living.clear_active_hand();
        return;
    }
    damage_entities(living, caller, &held, weapon);
}

/// Vanilla `ProjectileUtil.getHitEntitiesAlong` for this mob, with `AttackRange.mobFactor`.
fn targets_along(
    living: &LivingEntity,
    stack: &ItemStack,
) -> (Vec<Arc<dyn EntityBase>>, Vector3<f64>) {
    let entity = &living.entity;
    let look =
        Vector3::rotation_vector(f64::from(entity.pitch.load()), f64::from(entity.yaw.load()));
    let Some(range) = stack.get_data_component::<AttackRangeImpl>() else {
        return (Vec::new(), look);
    };
    let head_look = Vector3::rotation_vector(
        f64::from(entity.pitch.load()),
        f64::from(entity.head_yaw.load()),
    );
    let targets = SpearItem::targets_along(
        entity,
        head_look,
        entity.get_eye_pos(),
        f64::from(range.min_reach * range.mob_factor),
        f64::from(range.max_reach * range.mob_factor),
        f64::from(range.hitbox_margin),
        |target| SpearItem::can_be_stabbed_by(entity, target),
    );
    (targets, look)
}

fn damage_entities(
    living: &LivingEntity,
    caller: &dyn EntityBase,
    stack: &ItemStack,
    weapon: &KineticWeaponImpl,
) {
    let ticks_used =
        SpearItem::USE_DURATION - living.item_use_time.load(Ordering::Relaxed) - weapon.delay_ticks;
    if ticks_used < 0 {
        return;
    }
    let entity = &living.entity;
    let (targets, look) = targets_along(living, stack);
    let attacker_speed = look.dot(&SpearItem::known_speed(entity));
    let base_damage = living.get_attribute_base(&Attributes::ATTACK_DAMAGE) as f32;
    let now = entity.tick_count.load(Ordering::Relaxed);
    let mut affected = false;

    for target in targets {
        let target_entity = target.get_entity();
        if living.was_recently_stabbed(target_entity.entity_id, now, weapon.contact_cooldown_ticks)
        {
            continue;
        }
        living.remember_stabbed_entity(target_entity.entity_id, now);

        let target_speed = look.dot(&SpearItem::known_speed(target_entity));
        let relative_speed = (attacker_speed - target_speed).max(0.0);
        let test = |condition: Option<&pumpkin_data::data_component_impl::KineticConditionImpl>| {
            condition.is_some_and(|condition| {
                condition.test(
                    ticks_used,
                    attacker_speed,
                    relative_speed,
                    MOB_ACTION_FACTOR,
                )
            })
        };
        let effects = StabEffects {
            damage: test(weapon.damage_conditions.as_ref()),
            knockback: test(weapon.knockback_conditions.as_ref()),
            dismount: test(weapon.dismount_conditions.as_ref()),
        };
        if !effects.damage && !effects.knockback && !effects.dismount {
            continue;
        }
        let damage =
            base_damage + (relative_speed * f64::from(weapon.damage_multiplier)).floor() as f32;
        affected |= stab_attack(living, caller, stack, &target, damage, effects);
    }

    if affected {
        entity.world.load().send_entity_status(
            entity,
            EntityStatus::KineticHit,
            ActorEventID::KineticDamageDealt,
        );
    }
}

/// Vanilla `LivingEntity.stabAttack`.
fn stab_attack(
    living: &LivingEntity,
    caller: &dyn EntityBase,
    stack: &ItemStack,
    target: &Arc<dyn EntityBase>,
    base_damage: f32,
    effects: StabEffects,
) -> bool {
    let target_entity = target.get_entity();
    let total_damage = if effects.damage {
        base_damage + SpearItem::enchantment_damage(stack, target_entity) as f32
    } else {
        0.0
    };
    let was_hurt = effects.damage
        && target.damage_with_context(
            target.as_ref(),
            total_damage,
            DamageType::SPEAR,
            None,
            Some(caller),
            Some(caller),
        );
    let dismounted = SpearItem::apply_knockback_and_dismount(
        &living.entity,
        stack,
        target,
        total_damage,
        effects.knockback,
        effects.dismount,
    );
    if was_hurt {
        SpearItem::apply_post_damage_effects(stack, target_entity);
    }
    if !was_hurt && !effects.knockback && !dismounted {
        return false;
    }
    living
        .last_attacking_id
        .store(target_entity.entity_id, Ordering::Relaxed);
    living.last_attack_time.store(
        living.entity.tick_count.load(Ordering::Relaxed),
        Ordering::Relaxed,
    );
    true
}
