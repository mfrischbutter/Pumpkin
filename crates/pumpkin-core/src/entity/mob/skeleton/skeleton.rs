use pumpkin_data::entity::EntityType;
use pumpkin_data::sound::Sound;
use pumpkin_data::tracked_data;
use pumpkin_data::world::WorldEvent;

use crate::entity::mob::conversion::ConversionTracker;
use crate::entity::mob::skeleton::{ATTACK_INTERVAL, HARD_ATTACK_INTERVAL, SkeletonKind};

pub static SKELETON: SkeletonKind = SkeletonKind {
    step_sound: Sound::EntitySkeletonStep,
    ambient_sound: Sound::EntitySkeletonAmbient,
    attack_interval: ATTACK_INTERVAL,
    hard_attack_interval: HARD_ATTACK_INTERVAL,
    arrow_effect: None,
    extra_hunts: &[],
};

/// Vanilla `Skeleton.freezingTracker`: 140 ticks in powder snow start a 300 tick conversion
/// into a stray.
pub(super) fn freezing_tracker() -> ConversionTracker {
    ConversionTracker::new(
        &EntityType::STRAY,
        WorldEvent::SoundSkeletonToStray,
        tracked_data::skeleton::DATA_STRAY_CONVERSION_ID,
        |mob| mob.get_entity().is_in_powder_snow(),
        ("FreezingTime", 140),
        ("StrayConversionTime", 300),
    )
}
