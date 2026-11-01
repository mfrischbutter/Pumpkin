use pumpkin_data::effect::StatusEffect;
use pumpkin_data::sound::Sound;

use crate::entity::mob::skeleton::{
    INCREASED_ATTACK_INTERVAL, INCREASED_HARD_ATTACK_INTERVAL, SkeletonKind,
};

/// Vanilla `Parched`: slower shots whose arrows weaken for 30 seconds. Its immunity to
/// weakness is in `LivingEntity::can_be_affected`.
pub static PARCHED: SkeletonKind = SkeletonKind {
    step_sound: Sound::EntityParchedStep,
    ambient_sound: Sound::EntityParchedAmbient,
    attack_interval: INCREASED_ATTACK_INTERVAL,
    hard_attack_interval: INCREASED_HARD_ATTACK_INTERVAL,
    arrow_effect: Some((&StatusEffect::WEAKNESS, 600)),
    extra_hunts: &[],
};
