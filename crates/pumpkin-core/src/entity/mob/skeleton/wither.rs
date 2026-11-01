use pumpkin_data::attributes::Attributes;
use pumpkin_data::effect::StatusEffect;
use pumpkin_data::potion::Effect;
use pumpkin_data::sound::Sound;

use crate::entity::{
    EntityBase,
    ai::behavior::hunting::Hunt,
    ai::pathfinder::node::PathType,
    ai::target_match::TargetMatch,
    mob::MobEntity,
    mob::skeleton::{ATTACK_INTERVAL, HARD_ATTACK_INTERVAL, SkeletonEntityBase, SkeletonKind},
    projectile::arrow::ArrowEntity,
};

const WITHER_SKELETON_HUNTS: &[Hunt] = &[Hunt::new(3, TargetMatch::PIGLIN_LIKE, true)];

/// Vanilla `WitherSkeleton`. Its stone sword and the missing armor and enchantments are in
/// `EQUIPMENT_REGISTRY`, its immunity to wither in `LivingEntity::can_be_affected`.
pub static WITHER_SKELETON: SkeletonKind = SkeletonKind {
    step_sound: Sound::EntityWitherSkeletonStep,
    ambient_sound: Sound::EntityWitherSkeletonAmbient,
    attack_interval: ATTACK_INTERVAL,
    hard_attack_interval: HARD_ATTACK_INTERVAL,
    arrow_effect: None,
    extra_hunts: WITHER_SKELETON_HUNTS,
};

/// Vanilla `WitherSkeleton` constructor: lava costs 8 to path through.
pub(super) fn set_lava_malus(mob_entity: &mut MobEntity) {
    mob_entity
        .navigator
        .get_mut()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .set_pathfinding_malus(PathType::Lava, 8.0);
}

/// Vanilla `WitherSkeleton.finalizeSpawn`: a base attack damage of 4.
pub(super) fn finalize_spawn(skeleton: &SkeletonEntityBase) {
    skeleton
        .mob_entity
        .living_entity
        .set_attribute_base(&Attributes::ATTACK_DAMAGE, 4.0);
}

/// Vanilla `WitherSkeleton.doHurtTarget`: 10 seconds of wither.
pub(super) fn wither_target(target: &dyn EntityBase) {
    if let Some(living) = target.get_living_entity() {
        living.add_effect(Effect {
            effect_type: &StatusEffect::WITHER,
            duration: 200,
            amplifier: 0,
            ambient: false,
            show_particles: true,
            show_icon: true,
            blend: false,
        });
    }
}

/// Vanilla `WitherSkeleton.getArrow`: the arrow burns for 100 seconds.
pub(super) fn ignite_arrow(arrow: &ArrowEntity) {
    arrow.set_flame(true);
}
