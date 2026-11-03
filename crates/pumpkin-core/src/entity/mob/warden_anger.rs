//! Vanilla `AngerLevel` and `AngerManagement`: whom a warden is angry at, and how much.

use std::cmp::Ordering;
use std::sync::Arc;

use pumpkin_data::sound::Sound;
use pumpkin_nbt::{compound::NbtCompound, tag::NbtTag};
use uuid::Uuid;

use crate::entity::{EntityBase, RemovalReason};
use crate::world::World;

const CONVERSION_DELAY: i32 = 2;
const MAX_ANGER: i32 = 150;
const AGITATED_ANGER: i32 = 40;
/// Vanilla `AngerLevel.ANGRY.getMinimumAnger`.
pub const ANGRY_ANGER: i32 = 80;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AngerLevel {
    Calm,
    Agitated,
    Angry,
}

impl AngerLevel {
    #[must_use]
    pub const fn by_anger(anger: i32) -> Self {
        if anger >= ANGRY_ANGER {
            Self::Angry
        } else if anger >= AGITATED_ANGER {
            Self::Agitated
        } else {
            Self::Calm
        }
    }

    #[must_use]
    pub fn is_angry(self) -> bool {
        self == Self::Angry
    }

    #[must_use]
    pub const fn ambient_sound(self) -> Sound {
        match self {
            Self::Calm => Sound::EntityWardenAmbient,
            Self::Agitated => Sound::EntityWardenAgitated,
            Self::Angry => Sound::EntityWardenAngry,
        }
    }

    #[must_use]
    pub const fn listening_sound(self) -> Sound {
        match self {
            Self::Calm => Sound::EntityWardenListening,
            Self::Agitated | Self::Angry => Sound::EntityWardenListeningAngry,
        }
    }
}

struct Suspect {
    entity: Arc<dyn EntityBase>,
    anger: i32,
}

impl Suspect {
    fn is_player(&self) -> bool {
        self.entity.get_player().is_some()
    }
}

/// Vanilla `AngerManagement.Sorter`: those it is angry at first, of those players, then the angriest.
fn suspect_order(anger1: i32, player1: bool, anger2: i32, player2: bool) -> Ordering {
    let angry1 = AngerLevel::by_anger(anger1).is_angry();
    let angry2 = AngerLevel::by_anger(anger2).is_angry();
    angry2
        .cmp(&angry1)
        .then(player2.cmp(&player1))
        .then(anger2.cmp(&anger1))
}

pub struct AngerManagement {
    conversion_delay: i32,
    highest_anger: i32,
    suspects: Vec<Suspect>,
    /// Those not in the world at the moment, by vanilla's `angerByUuid`.
    anger_by_uuid: Vec<(Uuid, i32)>,
}

impl Default for AngerManagement {
    fn default() -> Self {
        Self {
            conversion_delay: rand::random_range(0..=CONVERSION_DELAY),
            highest_anger: 0,
            suspects: Vec::new(),
            anger_by_uuid: Vec::new(),
        }
    }
}

impl AngerManagement {
    /// Vanilla's tick, every 20 ticks of the warden: everyone's anger fades by 1.
    pub fn tick(&mut self, world: &World, valid: impl Fn(&dyn EntityBase) -> bool) {
        self.conversion_delay -= 1;
        if self.conversion_delay <= 0 {
            self.convert_from_uuids(world);
            self.conversion_delay = CONVERSION_DELAY;
        }
        self.anger_by_uuid.retain_mut(|(_, anger)| {
            *anger -= 1;
            *anger > 0
        });

        let mut kept = Vec::with_capacity(self.suspects.len());
        for mut suspect in std::mem::take(&mut self.suspects) {
            let removal = suspect.entity.get_entity().removal_reason.load();
            if suspect.anger > 1 && removal.is_none() && valid(suspect.entity.as_ref()) {
                suspect.anger -= 1;
                kept.push(suspect);
            } else if suspect.anger > 1
                && matches!(
                    removal,
                    Some(
                        RemovalReason::ChangedDimension
                            | RemovalReason::UnloadedToChunk
                            | RemovalReason::UnloadedWithPlayer
                    )
                )
            {
                self.anger_by_uuid
                    .push((suspect.entity.get_entity().entity_uuid, suspect.anger - 1));
            }
        }
        self.suspects = kept;
        self.sort_and_update_highest_anger();
    }

    fn convert_from_uuids(&mut self, world: &World) {
        let mut waiting = Vec::with_capacity(self.anger_by_uuid.len());
        for (uuid, anger) in std::mem::take(&mut self.anger_by_uuid) {
            if let Some(entity) = world.get_entity_by_uuid(uuid) {
                self.suspects.push(Suspect { entity, anger });
            } else {
                waiting.push((uuid, anger));
            }
        }
        self.anger_by_uuid = waiting;
    }

    fn sort_and_update_highest_anger(&mut self) {
        self.suspects
            .sort_by(|a, b| suspect_order(a.anger, a.is_player(), b.anger, b.is_player()));
        self.highest_anger = self.suspects.iter().map(|s| s.anger).max().unwrap_or(0);
    }

    /// Vanilla `increaseAnger`; the new anger at `entity`.
    pub fn increase_anger(&mut self, entity: &Arc<dyn EntityBase>, increment: i32) -> i32 {
        let id = entity.get_entity().entity_id;
        let anger = if let Some(suspect) = self
            .suspects
            .iter_mut()
            .find(|suspect| suspect.entity.get_entity().entity_id == id)
        {
            suspect.anger = (suspect.anger + increment).min(MAX_ANGER);
            suspect.anger
        } else {
            let uuid = entity.get_entity().entity_uuid;
            let saved = self
                .anger_by_uuid
                .iter()
                .position(|(other, _)| *other == uuid)
                .map_or(0, |index| self.anger_by_uuid.remove(index).1);
            let anger = increment.min(MAX_ANGER) + saved;
            self.suspects.push(Suspect {
                entity: entity.clone(),
                anger,
            });
            anger
        };
        self.sort_and_update_highest_anger();
        anger
    }

    pub fn clear_anger(&mut self, entity: &dyn EntityBase) {
        let id = entity.get_entity().entity_id;
        self.suspects
            .retain(|suspect| suspect.entity.get_entity().entity_id != id);
        self.sort_and_update_highest_anger();
    }

    /// Vanilla `getActiveAnger`: at the target, or the highest of all without one.
    #[must_use]
    pub fn active_anger(&self, target: Option<&dyn EntityBase>) -> i32 {
        target.map_or(self.highest_anger, |target| {
            let id = target.get_entity().entity_id;
            self.suspects
                .iter()
                .find(|suspect| suspect.entity.get_entity().entity_id == id)
                .map_or(0, |suspect| suspect.anger)
        })
    }

    /// Vanilla `getActiveEntity`: the first suspect `valid` lets through.
    #[must_use]
    pub fn active_entity(
        &self,
        valid: impl Fn(&dyn EntityBase) -> bool,
    ) -> Option<Arc<dyn EntityBase>> {
        self.suspects
            .iter()
            .find(|suspect| valid(suspect.entity.as_ref()))
            .map(|suspect| suspect.entity.clone())
    }

    #[must_use]
    pub fn write_nbt(&self) -> NbtCompound {
        let pair = |uuid: Uuid, anger: i32| {
            let mut entry = NbtCompound::new();
            entry.put_uuid("uuid", uuid);
            entry.put_int("anger", anger);
            NbtTag::Compound(entry)
        };
        let suspects = self
            .suspects
            .iter()
            .map(|suspect| pair(suspect.entity.get_entity().entity_uuid, suspect.anger))
            .chain(
                self.anger_by_uuid
                    .iter()
                    .map(|&(uuid, anger)| pair(uuid, anger)),
            )
            .collect();
        let mut nbt = NbtCompound::new();
        nbt.put_list("suspects", suspects);
        nbt
    }

    #[must_use]
    pub fn read_nbt(nbt: &NbtCompound) -> Self {
        let anger_by_uuid = nbt
            .get_list("suspects")
            .unwrap_or_default()
            .iter()
            .filter_map(NbtTag::extract_compound)
            .filter_map(|entry| Some((entry.get_uuid("uuid")?, entry.get_int("anger")?)))
            .filter(|(_, anger)| *anger >= 0)
            .collect();
        Self {
            anger_by_uuid,
            ..Self::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_are_vanillas() {
        assert_eq!(AngerLevel::by_anger(0), AngerLevel::Calm);
        assert_eq!(AngerLevel::by_anger(39), AngerLevel::Calm);
        assert_eq!(AngerLevel::by_anger(40), AngerLevel::Agitated);
        assert_eq!(AngerLevel::by_anger(79), AngerLevel::Agitated);
        assert_eq!(AngerLevel::by_anger(80), AngerLevel::Angry);
        assert!(AngerLevel::by_anger(150).is_angry());
    }

    #[test]
    fn those_it_is_angry_at_come_first_then_players_then_the_angriest() {
        // Angry at a mob beats calm toward a player.
        assert_eq!(suspect_order(90, false, 50, true), Ordering::Less);
        // Among the angry, a player beats a mob with more anger.
        assert_eq!(suspect_order(80, true, 150, false), Ordering::Less);
        // Among the same kind, the higher anger first.
        assert_eq!(suspect_order(100, true, 90, true), Ordering::Less);
        assert_eq!(suspect_order(30, false, 35, false), Ordering::Greater);
    }

    #[test]
    fn saved_anger_survives_nbt_and_fades() {
        let mut anger = AngerManagement::default();
        let uuid = Uuid::from_u128(7);
        anger.anger_by_uuid.push((uuid, 60));
        let mut read = AngerManagement::read_nbt(&anger.write_nbt());
        assert_eq!(read.anger_by_uuid, vec![(uuid, 60)]);
        read.anger_by_uuid.retain_mut(|(_, anger)| {
            *anger -= 1;
            *anger > 0
        });
        assert_eq!(read.anger_by_uuid, vec![(uuid, 59)]);
    }
}
