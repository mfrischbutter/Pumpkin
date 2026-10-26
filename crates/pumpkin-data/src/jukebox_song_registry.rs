//! Jukebox songs registered at runtime, next to the generated vanilla [`JukeboxSong`]s.
//!
//! The configuration sync sends the registered songs after the vanilla ones, sorted by id, so a
//! registered song's network id is the vanilla song count plus its index in that order. That
//! holds while these are the only custom songs: datapacks do not load jukebox songs.

use std::collections::BTreeMap;
use std::sync::{PoisonError, RwLock};

use crate::jukebox_song::JukeboxSong;

/// Vanilla `SharedConstants.TICKS_PER_SECOND`.
const TICKS_PER_SECOND: f32 = 20.0;

/// The number of vanilla songs, which take the first network ids.
const VANILLA_SONG_COUNT: u32 = {
    let mut count = 0;
    while JukeboxSong::from_id(count).is_some() {
        count += 1;
    }
    count
};

static DYNAMIC_SONGS: RwLock<BTreeMap<String, DynamicJukeboxSong>> = RwLock::new(BTreeMap::new());

/// A jukebox song that is not part of the vanilla registry.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DynamicJukeboxSong {
    pub length_in_seconds: f32,
    /// The signal (0 to 15) a comparator reads from a jukebox playing the song.
    pub comparator_output: u8,
}

/// What the server needs to play a vanilla or registered song.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct JukeboxSongEntry {
    /// The song's id in the synced `minecraft:jukebox_song` registry.
    pub network_id: u32,
    pub length_in_ticks: u64,
    pub comparator_output: u8,
}

/// Registers a song under its namespaced id, replacing an earlier song with the same id.
pub fn register_dynamic_jukebox_song(id: String, song: DynamicJukeboxSong) {
    DYNAMIC_SONGS
        .write()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(id, song);
}

/// Removes a registered song.
pub fn unregister_dynamic_jukebox_song(id: &str) {
    DYNAMIC_SONGS
        .write()
        .unwrap_or_else(PoisonError::into_inner)
        .remove(id);
}

/// Looks up a vanilla or registered song by its namespaced id.
#[must_use]
pub fn get_jukebox_song(id: &str) -> Option<JukeboxSongEntry> {
    if let Some(song) = id
        .strip_prefix("minecraft:")
        .and_then(JukeboxSong::from_name)
    {
        return Some(JukeboxSongEntry {
            network_id: song.get_id(),
            length_in_ticks: song.length_in_ticks(),
            comparator_output: song.comparator_output(),
        });
    }
    let songs = DYNAMIC_SONGS.read().unwrap_or_else(PoisonError::into_inner);
    let song = songs.get(id)?;
    let index = songs
        .range::<str, _>((std::ops::Bound::Unbounded, std::ops::Bound::Excluded(id)))
        .count() as u32;
    Some(JukeboxSongEntry {
        network_id: VANILLA_SONG_COUNT + index,
        // Vanilla: `JukeboxSong.lengthInTicks()` rounds up (from memory).
        length_in_ticks: (song.length_in_seconds * TICKS_PER_SECOND).ceil() as u64,
        comparator_output: song.comparator_output,
    })
}

/// Returns the namespaced id of the vanilla or registered song with this network id.
#[must_use]
pub fn jukebox_song_id(network_id: u32) -> Option<String> {
    if let Some(song) = JukeboxSong::from_id(network_id) {
        return Some(format!("minecraft:{}", song.to_name()));
    }
    let index = network_id.checked_sub(VANILLA_SONG_COUNT)?;
    DYNAMIC_SONGS
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .keys()
        .nth(index as usize)
        .cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registered_songs_follow_the_vanilla_ids_in_id_order() {
        let song = DynamicJukeboxSong {
            length_in_seconds: 2.5,
            comparator_output: 3,
        };
        register_dynamic_jukebox_song("test_b:song".to_string(), song);
        register_dynamic_jukebox_song("test_a:song".to_string(), song);

        let first = get_jukebox_song("test_a:song").unwrap();
        let second = get_jukebox_song("test_b:song").unwrap();
        assert_eq!(first.network_id, VANILLA_SONG_COUNT);
        assert_eq!(second.network_id, VANILLA_SONG_COUNT + 1);
        assert_eq!(first.length_in_ticks, 50);
        assert_eq!(
            jukebox_song_id(second.network_id).as_deref(),
            Some("test_b:song")
        );
        let pigstep = get_jukebox_song("minecraft:pigstep").unwrap();
        assert_eq!(pigstep.network_id, JukeboxSong::Pigstep.get_id());

        unregister_dynamic_jukebox_song("test_a:song");
        unregister_dynamic_jukebox_song("test_b:song");
        assert_eq!(get_jukebox_song("test_a:song"), None);
    }
}
