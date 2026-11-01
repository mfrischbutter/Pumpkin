use pumpkin_data::effect::StatusEffect;
use pumpkin_data::sound::Sound;

use crate::entity::mob::skeleton::{ATTACK_INTERVAL, HARD_ATTACK_INTERVAL, SkeletonKind};

/// Vanilla `Stray`: its arrows slow for 30 seconds.
pub static STRAY: SkeletonKind = SkeletonKind {
    step_sound: Sound::EntityStrayStep,
    ambient_sound: Sound::EntityStrayAmbient,
    attack_interval: ATTACK_INTERVAL,
    hard_attack_interval: HARD_ATTACK_INTERVAL,
    arrow_effect: Some((&StatusEffect::SLOWNESS, 600)),
    extra_hunts: &[],
};
