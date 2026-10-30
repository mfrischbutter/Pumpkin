use crate::entity::mob::Mob;
use pumpkin_data::sound::{Sound, SoundCategory};

pub trait CustomSound: Mob {
    fn death_sound(&self) -> Option<Sound>;
    fn hurt_sound(&self) -> Option<Sound>;

    fn sound_volume(&self) -> f32 {
        1.0
    }

    /// Vanilla `getSoundSource`: the entity type's category unless overridden.
    fn sound_category(&self) -> SoundCategory {
        self.mob_sound_source()
    }
}
