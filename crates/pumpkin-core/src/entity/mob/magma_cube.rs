use pumpkin_data::sound::Sound;

use crate::entity::mob::slime::{CubeSpecies, LavaJump, SizedSound};

/// Vanilla `MagmaCube`: a Nether [`SlimeEntity`] with these overrides.
///
/// It waits four times as long between jumps, jumps higher the bigger it is, swims up lava
/// fast, hurts what it touches at every size and wears armor.
///
/// Wiki: <https://minecraft.wiki/w/Magma_Cube>
///
/// [`SlimeEntity`]: crate::entity::mob::slime::SlimeEntity
pub static MAGMA_CUBE: CubeSpecies = CubeSpecies {
    hurt_sound: SizedSound {
        tiny: Sound::EntityMagmaCubeHurtSmall,
        big: Sound::EntityMagmaCubeHurt,
    },
    death_sound: SizedSound {
        tiny: Sound::EntityMagmaCubeDeathSmall,
        big: Sound::EntityMagmaCubeDeath,
    },
    squish_sound: SizedSound {
        tiny: Sound::EntityMagmaCubeSquishSmall,
        big: Sound::EntityMagmaCubeSquish,
    },
    jump_sound: SizedSound {
        tiny: Sound::EntityMagmaCubeJump,
        big: Sound::EntityMagmaCubeJump,
    },
    jump_delay_factor: 4,
    squish_decay: 0.9,
    tiny_deals_damage: true,
    attack_damage_bonus: 2.0,
    armor_per_size: Some(3),
    jump_power_per_size: 0.1,
    lava_jump: Some(LavaJump {
        base: 0.22,
        per_size: 0.05,
    }),
};
