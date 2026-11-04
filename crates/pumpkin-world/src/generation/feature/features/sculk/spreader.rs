//! The spreader and its charge cursors (vanilla `SculkSpreader`).

use std::collections::HashMap;

use pumpkin_data::BlockDirection;
use pumpkin_data::tag::Block::MINECRAFT_SCULK_REPLACEABLE;
use pumpkin_data::tag::Block::MINECRAFT_SCULK_REPLACEABLE_WORLD_GEN;
use pumpkin_data::tag::Tag;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_util::random::RandomGenerator;

use super::SculkBehaviour;
use super::get_block_state;
use super::is_sculk_behaviour;
use super::shuffle;
use super::vein::VeinRules;
use crate::generation::proto_chunk::GenerationCache;

/// Vanilla `SculkSpreader.MAX_CHARGE`.
pub const MAX_CHARGE: i32 = 1000;
/// Vanilla `SculkSpreader.MAX_CURSORS`.
pub const MAX_CURSORS: usize = 32;
/// Vanilla `SculkSpreader.MAX_CURSOR_DISTANCE`.
const MAX_CURSOR_DISTANCE: i32 = 1024;
/// Vanilla `ChargeCursor.MAX_WORLDGEN_SPREAD`: how far from the origin, horizontally, a world
/// generation cursor may move.
const MAX_WORLDGEN_SPREAD: i32 = 12;
/// Vanilla `SculkBehaviour.getSculkSpreadDelay`, the same for every block.
const SCULK_SPREAD_DELAY: i32 = 1;

/// The neighbours sharing a face or an edge with a block, in the order of vanilla's
/// `BlockPos.betweenClosed` (x fastest, then y, then z), which the cursor shuffles.
pub const NON_CORNER_NEIGHBOURS: [Vector3<i32>; 18] = [
    Vector3::new(0, -1, -1),
    Vector3::new(-1, 0, -1),
    Vector3::new(0, 0, -1),
    Vector3::new(1, 0, -1),
    Vector3::new(0, 1, -1),
    Vector3::new(-1, -1, 0),
    Vector3::new(0, -1, 0),
    Vector3::new(1, -1, 0),
    Vector3::new(-1, 0, 0),
    Vector3::new(1, 0, 0),
    Vector3::new(-1, 1, 0),
    Vector3::new(0, 1, 0),
    Vector3::new(1, 1, 0),
    Vector3::new(0, -1, 1),
    Vector3::new(-1, 0, 1),
    Vector3::new(0, 0, 1),
    Vector3::new(1, 0, 1),
    Vector3::new(0, 1, 1),
];

/// Drives sculk spreading for a single patch.
pub struct SculkSpreader {
    is_world_generation: bool,
    replaceable_blocks: Tag,
    growth_spawn_cost: i32,
    no_growth_radius: i32,
    charge_decay_rate: i32,
    additional_decay_rate: i32,
    cursors: Vec<ChargeCursor>,
}

impl SculkSpreader {
    /// Vanilla `SculkSpreader.createLevelSpreader`, the spreader of a sculk catalyst.
    #[must_use]
    pub const fn new_level_spreader() -> Self {
        Self::new(false, MINECRAFT_SCULK_REPLACEABLE, 10, 4, 10, 5)
    }

    /// Vanilla `SculkSpreader.createWorldGenSpreader`.
    #[must_use]
    pub const fn new_world_gen_spreader() -> Self {
        Self::new(true, MINECRAFT_SCULK_REPLACEABLE_WORLD_GEN, 50, 1, 5, 10)
    }

    const fn new(
        is_world_generation: bool,
        replaceable_blocks: Tag,
        growth_spawn_cost: i32,
        no_growth_radius: i32,
        charge_decay_rate: i32,
        additional_decay_rate: i32,
    ) -> Self {
        Self {
            is_world_generation,
            replaceable_blocks,
            growth_spawn_cost,
            no_growth_radius,
            charge_decay_rate,
            additional_decay_rate,
            cursors: Vec::new(),
        }
    }

    #[must_use]
    pub const fn is_world_generation(&self) -> bool {
        self.is_world_generation
    }

    #[must_use]
    pub const fn replaceable_blocks(&self) -> Tag {
        self.replaceable_blocks
    }

    #[must_use]
    pub const fn growth_spawn_cost(&self) -> i32 {
        self.growth_spawn_cost
    }

    #[must_use]
    pub const fn no_growth_radius(&self) -> i32 {
        self.no_growth_radius
    }

    #[must_use]
    pub const fn charge_decay_rate(&self) -> i32 {
        self.charge_decay_rate
    }

    #[must_use]
    pub const fn additional_decay_rate(&self) -> i32 {
        self.additional_decay_rate
    }

    /// Adds charge at a position, split into cursors of at most [`MAX_CHARGE`].
    pub fn add_cursors(&mut self, start_pos: BlockPos, mut charge: i32) {
        while charge > 0 {
            let current_charge = charge.min(MAX_CHARGE);
            self.add_cursor(ChargeCursor::new(start_pos, current_charge));
            charge -= current_charge;
        }
    }

    fn add_cursor(&mut self, cursor: ChargeCursor) {
        if self.cursors.len() < MAX_CURSORS {
            self.cursors.push(cursor);
        }
    }

    /// Removes all cursors (between rounds).
    pub fn clear(&mut self) {
        self.cursors.clear();
    }

    #[cfg(test)]
    #[must_use]
    pub fn cursors(&self) -> &[ChargeCursor] {
        &self.cursors
    }

    /// Vanilla `SculkSpreader.updateCursors`: updates every cursor once, in order.
    pub fn update_cursors<T: GenerationCache>(
        &mut self,
        level: &mut T,
        origin_pos: BlockPos,
        random: &mut RandomGenerator,
        spread_veins: bool,
    ) {
        if self.cursors.is_empty() {
            return;
        }

        let cursors = std::mem::take(&mut self.cursors);
        let mut processed_cursors: Vec<ChargeCursor> = Vec::with_capacity(cursors.len());
        // Index into `processed_cursors` of the cursor others at the position merge into.
        let mut mergeable_cursors: HashMap<BlockPos, usize> = HashMap::new();

        for mut cursor in cursors {
            if cursor.is_pos_unreasonable(origin_pos) {
                continue;
            }
            cursor.update(level, origin_pos, random, self, spread_veins);
            if cursor.charge <= 0 {
                continue;
            }
            let pos = cursor.pos;
            match mergeable_cursors.get(&pos).copied() {
                None => {
                    mergeable_cursors.insert(pos, processed_cursors.len());
                    processed_cursors.push(cursor);
                }
                Some(existing)
                    if !self.is_world_generation
                        && cursor.charge + processed_cursors[existing].charge <= MAX_CHARGE =>
                {
                    processed_cursors[existing].merge_with(&cursor);
                }
                Some(existing) => {
                    if cursor.charge < processed_cursors[existing].charge {
                        mergeable_cursors.insert(pos, processed_cursors.len());
                    }
                    processed_cursors.push(cursor);
                }
            }
        }

        self.cursors = processed_cursors;
    }
}

/// A moving point of sculk charge.
pub struct ChargeCursor {
    pub pos: BlockPos,
    pub charge: i32,
    pub update_delay: i32,
    pub decay_delay: i32,
    /// Vanilla's `facings`: `None` until the cursor first stands on sculk, then the faces of that
    /// block, one bit per `BlockDirection::to_index`.
    pub facings: Option<u8>,
}

impl ChargeCursor {
    #[must_use]
    pub const fn new(pos: BlockPos, charge: i32) -> Self {
        Self {
            pos,
            charge,
            update_delay: 0,
            decay_delay: 1,
            facings: None,
        }
    }

    const fn is_pos_unreasonable(&self, origin_pos: BlockPos) -> bool {
        (self.pos.0.x - origin_pos.0.x).abs() > MAX_CURSOR_DISTANCE
            || (self.pos.0.y - origin_pos.0.y).abs() > MAX_CURSOR_DISTANCE
            || (self.pos.0.z - origin_pos.0.z).abs() > MAX_CURSOR_DISTANCE
    }

    /// Vanilla `ChargeCursor.update`.
    fn update<T: GenerationCache>(
        &mut self,
        level: &mut T,
        origin_pos: BlockPos,
        random: &mut RandomGenerator,
        spreader: &SculkSpreader,
        spread_veins: bool,
    ) {
        if self.charge <= 0 {
            return;
        }
        if self.update_delay > 0 {
            self.update_delay -= 1;
            return;
        }

        let mut current_state = get_block_state(level, self.pos);
        let mut behaviour = SculkBehaviour::of(current_state);
        if spread_veins
            && behaviour.attempt_spread_vein(
                level,
                self.pos,
                current_state,
                self.facings,
                spreader.is_world_generation,
            )
            && behaviour.can_change_block_state_on_spread()
        {
            current_state = get_block_state(level, self.pos);
            behaviour = SculkBehaviour::of(current_state);
        }

        // Discharging below gets the state from before the charge was used, as in vanilla: what
        // using it added to a vein at the cursor is dropped again.
        self.charge =
            behaviour.attempt_use_charge(self, level, origin_pos, random, spreader, spread_veins);
        if self.charge <= 0 {
            behaviour.on_discharged(level, current_state, self.pos);
            return;
        }

        if let Some(transfer_pos) =
            Self::valid_movement_pos(level, self.pos, random, origin_pos, spreader)
        {
            behaviour.on_discharged(level, current_state, self.pos);
            self.pos = transfer_pos;
            current_state = get_block_state(level, transfer_pos);
        } else if spreader.is_world_generation {
            behaviour.on_discharged(level, current_state, self.pos);
            self.charge = 0;
            return;
        }

        if is_sculk_behaviour(current_state.to_block_id()) {
            self.facings = Some(VeinRules::available_faces(current_state));
        }
        self.decay_delay = behaviour.update_decay_delay(self.decay_delay);
        self.update_delay = SCULK_SPREAD_DELAY;
    }

    fn merge_with(&mut self, other: &Self) {
        self.charge += other.charge;
        self.update_delay = self.update_delay.min(other.update_delay);
    }

    /// Vanilla `ChargeCursor.getValidMovementPos`: a random sculk neighbour, preferring a vein on
    /// a block sculk can replace; without one the last one found in the shuffled order.
    fn valid_movement_pos<T: GenerationCache>(
        level: &T,
        pos: BlockPos,
        random: &mut RandomGenerator,
        origin_pos: BlockPos,
        spreader: &SculkSpreader,
    ) -> Option<BlockPos> {
        let mut offsets = NON_CORNER_NEIGHBOURS;
        shuffle(&mut offsets, random);

        let mut sculk_position = None;
        for offset in offsets {
            let neighbour = pos.offset(offset);
            if !Self::can_move_to_pos(origin_pos, neighbour, spreader) {
                continue;
            }
            let transferee = get_block_state(level, neighbour);
            if is_sculk_behaviour(transferee.to_block_id())
                && Self::is_movement_unobstructed(level, pos, neighbour)
            {
                sculk_position = Some(neighbour);
                if VeinRules::has_substrate_access(level, transferee, neighbour) {
                    break;
                }
            }
        }
        sculk_position
    }

    const fn can_move_to_pos(origin: BlockPos, target: BlockPos, spreader: &SculkSpreader) -> bool {
        if !spreader.is_world_generation {
            return true;
        }
        let dx = origin.0.x - target.0.x;
        let dz = origin.0.z - target.0.z;
        dx * dx + dz * dz <= MAX_WORLDGEN_SPREAD * MAX_WORLDGEN_SPREAD
    }

    fn is_movement_unobstructed<T: GenerationCache>(
        level: &T,
        from: BlockPos,
        to: BlockPos,
    ) -> bool {
        if from.manhattan_distance(to) == 1 {
            return true;
        }
        let delta = Vector3::new(to.0.x - from.0.x, to.0.y - from.0.y, to.0.z - from.0.z);
        let direction_x = if delta.x < 0 {
            BlockDirection::West
        } else {
            BlockDirection::East
        };
        let direction_y = if delta.y < 0 {
            BlockDirection::Down
        } else {
            BlockDirection::Up
        };
        let direction_z = if delta.z < 0 {
            BlockDirection::North
        } else {
            BlockDirection::South
        };
        if delta.x == 0 {
            Self::is_unobstructed(level, from, direction_y)
                || Self::is_unobstructed(level, from, direction_z)
        } else if delta.y == 0 {
            Self::is_unobstructed(level, from, direction_x)
                || Self::is_unobstructed(level, from, direction_z)
        } else {
            Self::is_unobstructed(level, from, direction_x)
                || Self::is_unobstructed(level, from, direction_y)
        }
    }

    fn is_unobstructed<T: GenerationCache>(
        level: &T,
        from: BlockPos,
        direction: BlockDirection,
    ) -> bool {
        let test_pos = from.offset(direction.to_offset());
        !get_block_state(level, test_pos)
            .to_state()
            .is_side_solid(direction.opposite())
    }
}

#[cfg(test)]
mod tests {
    use pumpkin_data::Block;
    use pumpkin_util::random::legacy_rand::LegacyRand;

    use super::super::test_utils::{empty_chunk, set};
    use super::*;

    #[test]
    fn non_corner_neighbours_are_in_block_pos_order() {
        let mut expected = Vec::new();
        for z in -1..=1 {
            for y in -1..=1 {
                for x in -1..=1 {
                    let is_corner = x != 0 && y != 0 && z != 0;
                    if !is_corner && (x, y, z) != (0, 0, 0) {
                        expected.push(Vector3::new(x, y, z));
                    }
                }
            }
        }
        assert_eq!(NON_CORNER_NEIGHBOURS.to_vec(), expected);
    }

    #[test]
    fn charge_saturation() {
        let mut s = SculkSpreader::new_world_gen_spreader();
        let origin = BlockPos::new(0, 60, 0);
        s.add_cursors(origin, 2500);
        // 2500 = 1000 + 1000 + 500 → 3 cursors.
        let charges: Vec<i32> = s.cursors().iter().map(|cursor| cursor.charge).collect();
        assert_eq!(charges, [1000, 1000, 500]);
    }

    #[test]
    fn max_cursors_limit() {
        let mut s = SculkSpreader::new_world_gen_spreader();
        let origin = BlockPos::new(0, 60, 0);
        for _ in 0..50 {
            s.add_cursors(origin, 1000);
        }
        assert_eq!(s.cursors().len(), MAX_CURSORS);
    }

    #[test]
    fn cursor_does_not_move_onto_sensor() {
        let mut level = empty_chunk();
        let pos = BlockPos::new(1, 60, 1);
        set(&mut level, pos, Block::SCULK.default_state);
        set(&mut level, pos.up(), Block::SCULK_SENSOR.default_state);
        let mut random = RandomGenerator::Legacy(LegacyRand::from_seed(1));
        let spreader = SculkSpreader::new_world_gen_spreader();
        assert_eq!(
            ChargeCursor::valid_movement_pos(&level, pos, &mut random, pos, &spreader),
            None,
        );
    }

    #[test]
    fn cursor_moves_to_last_valid_neighbour_without_substrate() {
        // Without a vein on a replaceable block the last sculk neighbour in shuffled order wins.
        let mut level = empty_chunk();
        let pos = BlockPos::new(1, 60, 1);
        set(&mut level, pos, Block::SCULK.default_state);
        set(&mut level, pos.up(), Block::SCULK.default_state);
        set(&mut level, pos.east(), Block::SCULK.default_state);
        let mut order = NON_CORNER_NEIGHBOURS;
        shuffle(
            &mut order,
            &mut RandomGenerator::Legacy(LegacyRand::from_seed(1)),
        );
        let last = order
            .iter()
            .rev()
            .map(|&offset| pos.offset(offset))
            .find(|&neighbour| neighbour == pos.up() || neighbour == pos.east());

        let mut random = RandomGenerator::Legacy(LegacyRand::from_seed(1));
        let spreader = SculkSpreader::new_world_gen_spreader();
        assert_eq!(
            ChargeCursor::valid_movement_pos(&level, pos, &mut random, pos, &spreader),
            last,
        );
    }

    #[test]
    fn world_gen_cursor_stays_within_twelve_blocks_of_the_origin() {
        let mut level = empty_chunk();
        let origin = BlockPos::new(0, 60, 0);
        let pos = BlockPos::new(12, 60, 0);
        set(&mut level, pos, Block::SCULK.default_state);
        set(&mut level, pos.east(), Block::SCULK.default_state);
        let spreader = SculkSpreader::new_world_gen_spreader();
        let mut random = RandomGenerator::Legacy(LegacyRand::from_seed(1));
        assert_eq!(
            ChargeCursor::valid_movement_pos(&level, pos, &mut random, origin, &spreader),
            None,
        );
        set(&mut level, pos.down(), Block::SCULK.default_state);
        assert_eq!(
            ChargeCursor::valid_movement_pos(&level, pos, &mut random, origin, &spreader),
            Some(pos.down()),
        );
    }

    #[test]
    fn world_gen_cursor_without_a_move_discharges() {
        // Sculk with nothing to move to: the cursor loses its charge in its first update.
        let mut level = empty_chunk();
        let pos = BlockPos::new(1, 60, 1);
        set(&mut level, pos, Block::SCULK.default_state);
        let mut spreader = SculkSpreader::new_world_gen_spreader();
        spreader.add_cursors(pos, 20);
        let mut random = RandomGenerator::Legacy(LegacyRand::from_seed(1));
        spreader.update_cursors(&mut level, pos, &mut random, false);
        assert!(spreader.cursors().is_empty());
    }

    #[test]
    fn discharged_vein_loses_the_faces_placing_sculk_added() {
        // A vein on a floor with walls north and above. Spreading adds its north face; when the
        // cursor then turns the wall into sculk, the sculk's spreading adds the up face, but the
        // vein is discharged with the state from before, so only the floor face stays.
        let seed = (0..64)
            .find(|&seed| {
                let mut supports = BlockDirection::all();
                shuffle(
                    &mut supports,
                    &mut RandomGenerator::Legacy(LegacyRand::from_seed(seed)),
                );
                let position =
                    |direction| supports.iter().position(|&support| support == direction);
                position(BlockDirection::North) < position(BlockDirection::Down)
            })
            .unwrap_or_default();

        let mut level = empty_chunk();
        let pos = BlockPos::new(8, 60, 8);
        set(&mut level, pos.down(), Block::DEEPSLATE.default_state);
        set(&mut level, pos.north(), Block::DEEPSLATE.default_state);
        set(&mut level, pos.up(), Block::DEEPSLATE.default_state);
        let floor_vein = VeinRules::with_face(
            Block::SCULK_VEIN.default_state.id,
            BlockDirection::Down,
            true,
        );
        set(&mut level, pos, floor_vein.to_state());

        let spreader = SculkSpreader::new_world_gen_spreader();
        let mut cursor = ChargeCursor::new(pos, 10);
        let mut random = RandomGenerator::Legacy(LegacyRand::from_seed(seed));
        cursor.update(&mut level, pos, &mut random, &spreader, true);

        assert_eq!(
            get_block_state(&level, pos.north()),
            Block::SCULK.default_state.id
        );
        assert_eq!(get_block_state(&level, pos), floor_vein);
    }
}
