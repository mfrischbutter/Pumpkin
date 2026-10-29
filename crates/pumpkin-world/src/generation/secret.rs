//! The generation secret: a random value stored with the world that keys the
//! parts of world generation a client could recover the seed from.
//!
//! Vanilla derives structure placement and layouts, caves, bedrock, slime
//! chunks and stronghold rings from the world seed with Java's 48-bit random,
//! and sends a hash of the seed to clients for biome zooming. All of that is
//! visible in game, and tools such as `SeedCrackerX` recover the seed from it
//! within minutes; with the seed, every ore, structure and slime chunk of the
//! world is known. With a secret, each of those parts is keyed by a value
//! derived from the secret and the seed instead, so what a client sees reveals
//! nothing about the seed or about any other part. Terrain and biome noise
//! keep the seed: a client has to see them anyway, and nothing recovers a
//! 64-bit seed from them.
//!
//! The secret is drawn when the world is created (`secure_generation` in the
//! configuration) and stored next to the seed in `world_gen_settings.dat`. It
//! never changes afterwards: chunks generated with another secret would not
//! fit together.

use sha2::{Digest, Sha256};

/// The parts of generation keyed by the secret, each with a seed of its own.
pub mod part {
    /// Structure starts and layouts, and the frequency reduction of structure sets.
    pub const STRUCTURES: &str = "structures";
    /// The stronghold rings.
    pub const STRONGHOLDS: &str = "strongholds";
    /// The population seed of features: ores, dungeons, trees, vegetation.
    pub const FEATURES: &str = "features";
    /// Cave and canyon carvers.
    pub const CARVERS: &str = "carvers";
    /// The positional randoms of surface rules: bedrock floor and roof, deepslate.
    pub const SURFACE: &str = "surface";
    /// Which chunks are slime chunks.
    pub const SLIME_CHUNKS: &str = "slime_chunks";
    /// The biome zoom seed: hashed, it is what clients get instead of the seed's hash.
    pub const BIOME_ZOOM: &str = "biome_zoom";
}

/// A world's generation secret, or none for vanilla generation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GenerationSecret(Option<u64>);

impl GenerationSecret {
    /// No secret: every part is keyed by the seed, as in vanilla.
    pub const NONE: Self = Self(None);

    /// The secret as stored with the world's generation settings.
    #[must_use]
    pub const fn from_stored(stored: Option<i64>) -> Self {
        match stored {
            Some(secret) => Self(Some(secret as u64)),
            None => Self(None),
        }
    }

    /// Draws a new secret from the operating system's randomness.
    #[must_use]
    pub fn random() -> Self {
        Self(Some(rand::random()))
    }

    /// The secret in the form it is stored with the world's generation settings.
    #[must_use]
    pub const fn stored(&self) -> Option<i64> {
        match self.0 {
            Some(secret) => Some(secret as i64),
            None => None,
        }
    }

    #[must_use]
    pub const fn is_set(&self) -> bool {
        self.0.is_some()
    }

    /// The seed of one part of generation: the world seed itself without a secret.
    #[must_use]
    pub fn seed_for(&self, part: &str, world_seed: u64) -> u64 {
        self.0
            .map_or(world_seed, |secret| world_seed ^ derive(secret, part))
    }

    /// The placement salt of a structure set: vanilla's without a secret.
    #[must_use]
    pub fn structure_salt(&self, set_name: &str, salt: u32) -> u32 {
        self.0.map_or(salt, |secret| {
            salt ^ derive(secret, &format!("structure_set:{set_name}")) as u32
        })
    }
}

/// A 64-bit value only this secret and label produce; nothing about the secret
/// follows from it.
fn derive(secret: u64, label: &str) -> u64 {
    let mut hasher = Sha256::new();
    hasher.update(secret.to_le_bytes());
    hasher.update(label.as_bytes());
    let digest = hasher.finalize();
    let mut bytes = [0u8; 8];
    bytes.copy_from_slice(&digest[..8]);
    u64::from_le_bytes(bytes)
}

#[cfg(test)]
mod test {
    use super::{GenerationSecret, part};

    const SEED: u64 = 0x5eed_c0de_0000_1234;
    /// Vanilla's salts of the desert pyramid and igloo structure sets.
    const PYRAMIDS_SALT: u32 = 14_357_617;
    const IGLOOS_SALT: u32 = 14_357_618;

    #[test]
    fn no_secret_keeps_vanilla_values() {
        let none = GenerationSecret::NONE;
        assert_eq!(none.seed_for(part::STRUCTURES, SEED), SEED);
        assert_eq!(
            none.structure_salt("desert_pyramids", PYRAMIDS_SALT),
            PYRAMIDS_SALT
        );
    }

    #[test]
    fn secret_keys_every_part_differently() {
        let secret = GenerationSecret::from_stored(Some(-7));
        let structures = secret.seed_for(part::STRUCTURES, SEED);
        let carvers = secret.seed_for(part::CARVERS, SEED);
        assert_ne!(structures, SEED);
        assert_ne!(structures, carvers);
        assert_eq!(structures, secret.seed_for(part::STRUCTURES, SEED));

        let pyramids = secret.structure_salt("desert_pyramids", PYRAMIDS_SALT);
        let igloos = secret.structure_salt("igloos", IGLOOS_SALT);
        assert_ne!(pyramids, PYRAMIDS_SALT);
        assert_ne!(pyramids ^ PYRAMIDS_SALT, igloos ^ IGLOOS_SALT);

        assert_eq!(GenerationSecret::from_stored(secret.stored()), secret);
    }
}
