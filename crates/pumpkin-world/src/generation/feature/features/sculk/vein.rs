//! Sculk veins (vanilla `SculkVeinBlock`) and their spreading over block faces
//! (`MultifaceSpreader`, `MultifaceBlock`).

use pumpkin_data::Block;
use pumpkin_data::BlockDirection;
use pumpkin_data::BlockId;
use pumpkin_data::BlockState;
use pumpkin_data::BlockStateId;
use pumpkin_data::block_properties::GlowLichenLikeProperties;
use pumpkin_data::tag::Block::MINECRAFT_FIRE;
use pumpkin_data::tag::Block::MINECRAFT_SCULK_REPLACEABLE;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::random::RandomGenerator;
use pumpkin_util::random::RandomImpl;

use super::get_block_state;
use super::holds_no_fluid;
use super::holds_water_source;
use super::shuffle;
use super::spreader::ChargeCursor;
use super::spreader::SculkSpreader;
use crate::generation::proto_chunk::GenerationCache;

/// Vanilla `MultifaceSpreader.SpreadType`: where a face spreads to.
#[derive(Debug, Clone, Copy)]
pub enum SpreadType {
    /// Place at the same position, facing `spread_direction`.
    SamePosition,
    /// Place at the neighbour in `spread_direction`, facing `from_face`.
    SamePlane,
    /// Wrap around: place at neighbour + `from_face`, facing opposite.
    WrapAround,
}

impl SpreadType {
    /// Computes the target position and the face the new vein should have.
    #[must_use]
    pub fn spread_pos(
        self,
        pos: BlockPos,
        spread_direction: BlockDirection,
        from_face: BlockDirection,
    ) -> (BlockPos, BlockDirection) {
        match self {
            Self::SamePosition => (pos, spread_direction),
            Self::SamePlane => (pos.offset(spread_direction.to_offset()), from_face),
            Self::WrapAround => (
                pos.offset(spread_direction.to_offset())
                    .offset(from_face.to_offset()),
                spread_direction.opposite(),
            ),
        }
    }
}

/// Vanilla `MultifaceSpreader.DEFAULT_SPREAD_ORDER`, the spread types of a vein's spreader.
pub const DEFAULT_SPREAD_ORDER: [SpreadType; 3] = [
    SpreadType::SamePosition,
    SpreadType::SamePlane,
    SpreadType::WrapAround,
];

/// The spread types of vanilla's `SculkVeinBlock.getSameSpaceSpreader`.
pub const SAME_SPACE_SPREAD_ORDER: [SpreadType; 1] = [SpreadType::SamePosition];

/// Rules of sculk veins and their spreading.
pub struct VeinRules;

impl VeinRules {
    /// Vanilla `SculkVeinBlock.attemptUseCharge`: turns a block the vein grows on into sculk, or
    /// lets the charge decay.
    pub fn attempt_use_charge<T: GenerationCache>(
        cursor: &ChargeCursor,
        level: &mut T,
        random: &mut RandomGenerator,
        spreader: &SculkSpreader,
        spread_veins: bool,
    ) -> i32 {
        if spread_veins && Self::attempt_place_sculk(spreader, level, cursor.pos, random) {
            cursor.charge - 1
        } else if random.next_bounded_i32(spreader.charge_decay_rate()) == 0 {
            // `Mth.floor(charge * 0.5F)`
            cursor.charge / 2
        } else {
            cursor.charge
        }
    }

    /// Vanilla `SculkVeinBlock.attemptPlaceSculk`: replaces one block a face of the vein at `pos`
    /// grows on, in random order, with sculk.
    fn attempt_place_sculk<T: GenerationCache>(
        spreader: &SculkSpreader,
        level: &mut T,
        pos: BlockPos,
        random: &mut RandomGenerator,
    ) -> bool {
        let state = get_block_state(level, pos);
        let mut supports = BlockDirection::all();
        shuffle(&mut supports, random);

        for support in supports {
            if !Self::has_face(state, support) {
                continue;
            }
            let support_pos = pos.offset(support.to_offset());
            let support_state = get_block_state(level, support_pos);
            if !support_state
                .to_block_id()
                .has_tag(spreader.replaceable_blocks())
            {
                continue;
            }
            let default_sculk = Block::SCULK.default_state;
            level.set_block_state(&support_pos.0, default_sculk);
            Self::spread_all(
                level,
                default_sculk.id,
                support_pos,
                spreader.is_world_generation(),
                &DEFAULT_SPREAD_ORDER,
            );
            let skip = support.opposite();
            for vein_direction in BlockDirection::all() {
                if vein_direction == skip {
                    continue;
                }
                let vein_pos = support_pos.offset(vein_direction.to_offset());
                let possible_vein = get_block_state(level, vein_pos);
                if possible_vein.to_block_id() == BlockId::SCULK_VEIN {
                    Self::on_discharged(level, possible_vein, vein_pos);
                }
            }
            return true;
        }
        false
    }

    /// Vanilla `SculkVeinBlock.regrow`: a fresh vein with the given faces that can attach at `pos`.
    pub fn regrow<T: GenerationCache>(level: &mut T, pos: BlockPos, faces: u8) -> bool {
        let mut new_state = Block::SCULK_VEIN.default_state.id;
        let mut has_at_least_one_face = false;
        for face in BlockDirection::all() {
            if faces & (1 << face.to_index()) != 0 && Self::can_attach_to(level, pos, face) {
                new_state = Self::with_face(new_state, face, true);
                has_at_least_one_face = true;
            }
        }
        if !has_at_least_one_face {
            return false;
        }
        if !holds_no_fluid(level, pos) {
            new_state = Self::with_waterlogged(new_state);
        }
        level.set_block_state(&pos.0, new_state.to_state());
        true
    }

    /// Vanilla `SculkVeinBlock.onDischarged`: drops the faces of `state`, the vein's state as the
    /// caller saw it, that touch sculk, and the vein with its last face.
    pub fn on_discharged<T: GenerationCache>(
        level: &mut T,
        mut state: BlockStateId,
        pos: BlockPos,
    ) {
        if state.to_block_id() != BlockId::SCULK_VEIN {
            return;
        }
        for direction in BlockDirection::all() {
            if Self::has_face(state, direction)
                && get_block_state(level, pos.offset(direction.to_offset())).to_block_id()
                    == BlockId::SCULK
            {
                state = Self::with_face(state, direction, false);
            }
        }
        if !Self::has_any_face(state) {
            state = if holds_no_fluid(level, pos) {
                Block::AIR.default_state.id
            } else {
                Block::WATER.default_state.id
            };
        }
        level.set_block_state(&pos.0, state.to_state());
    }

    /// Vanilla `SculkVeinBlock.hasSubstrateAccess`: a face of the vein grows on a block sculk can
    /// replace.
    pub fn has_substrate_access<T: GenerationCache>(
        level: &T,
        state: BlockStateId,
        pos: BlockPos,
    ) -> bool {
        if state.to_block_id() != BlockId::SCULK_VEIN {
            return false;
        }
        BlockDirection::all().into_iter().any(|direction| {
            Self::has_face(state, direction)
                && get_block_state(level, pos.offset(direction.to_offset()))
                    .to_block_id()
                    .has_tag(MINECRAFT_SCULK_REPLACEABLE)
        })
    }

    /// Vanilla `MultifaceSpreader.spreadAll` of a sculk vein spreader: spreads from every face of
    /// `state`, the source's state as the caller saw it, and returns how many veins it placed.
    pub fn spread_all<T: GenerationCache>(
        level: &mut T,
        state: BlockStateId,
        pos: BlockPos,
        post_process: bool,
        spread_types: &[SpreadType],
    ) -> u64 {
        let mut placed = 0;
        for face in BlockDirection::all() {
            if !Self::can_spread_from(state, face) {
                continue;
            }
            for spread_direction in BlockDirection::all() {
                if let Some((spread_pos, spread_face)) = Self::get_spread_from_face_toward_direction(
                    level,
                    state,
                    pos,
                    face,
                    spread_direction,
                    spread_types,
                ) && Self::spread_to_face(level, spread_pos, spread_face, post_process)
                {
                    placed += 1;
                }
            }
        }
        placed
    }

    /// Vanilla `SpreadConfig.canSpreadFrom`: a vein spreads from its faces, any other block from
    /// all of its sides.
    fn can_spread_from(state: BlockStateId, face: BlockDirection) -> bool {
        Self::is_other_block_valid_as_source(state) || Self::has_face(state, face)
    }

    /// Vanilla `SculkVeinSpreaderConfig.isOtherBlockValidAsSource`.
    fn is_other_block_valid_as_source(state: BlockStateId) -> bool {
        state.to_block_id() != BlockId::SCULK_VEIN
    }

    /// Vanilla `MultifaceSpreader.getSpreadFromFaceTowardDirection`: the first spread type whose
    /// position the vein can spread into.
    fn get_spread_from_face_toward_direction<T: GenerationCache>(
        level: &T,
        state: BlockStateId,
        pos: BlockPos,
        starting_face: BlockDirection,
        spread_direction: BlockDirection,
        spread_types: &[SpreadType],
    ) -> Option<(BlockPos, BlockDirection)> {
        if spread_direction.to_axis() == starting_face.to_axis() {
            return None;
        }
        let can_spread = Self::is_other_block_valid_as_source(state)
            || (Self::has_face(state, starting_face) && !Self::has_face(state, spread_direction));
        if !can_spread {
            return None;
        }
        spread_types
            .iter()
            .map(|spread_type| spread_type.spread_pos(pos, spread_direction, starting_face))
            .find(|&(spread_pos, spread_face)| {
                Self::can_spread_into(level, pos, spread_pos, spread_face)
            })
    }

    /// Vanilla `MultifaceSpreader.spreadToFace` with `DefaultSpreaderConfig.placeBlock`.
    fn spread_to_face<T: GenerationCache>(
        level: &mut T,
        spread_pos: BlockPos,
        face: BlockDirection,
        post_process: bool,
    ) -> bool {
        let old_state = get_block_state(level, spread_pos);
        let Some(spread_state) = Self::get_state_for_placement(level, old_state, spread_pos, face)
        else {
            return false;
        };
        if post_process {
            level.mark_pos_for_post_processing(&spread_pos.0);
        }
        level.set_block_state(&spread_pos.0, spread_state);
        true
    }

    /// Vanilla `DefaultSpreaderConfig.canSpreadInto` of a sculk vein spreader.
    pub fn can_spread_into<T: GenerationCache>(
        level: &T,
        source_pos: BlockPos,
        placement_pos: BlockPos,
        placement_face: BlockDirection,
    ) -> bool {
        let existing_state = get_block_state(level, placement_pos);
        Self::state_can_be_replaced(
            level,
            source_pos,
            placement_pos,
            placement_face,
            existing_state,
        ) && Self::is_valid_state_for_placement(
            level,
            existing_state,
            placement_pos,
            placement_face,
        )
    }

    /// Vanilla `SculkVeinSpreaderConfig.stateCanBeReplaced`.
    fn state_can_be_replaced<T: GenerationCache>(
        level: &T,
        source_pos: BlockPos,
        placement_pos: BlockPos,
        placement_direction: BlockDirection,
        existing_state: BlockStateId,
    ) -> bool {
        let against_state =
            get_block_state(level, placement_pos.offset(placement_direction.to_offset()));
        if matches!(
            against_state.to_block_id(),
            BlockId::SCULK | BlockId::SCULK_CATALYST | BlockId::MOVING_PISTON
        ) {
            return false;
        }
        if source_pos.manhattan_distance(placement_pos) == 2 {
            let neighbour_pos = source_pos.offset(placement_direction.opposite().to_offset());
            if get_block_state(level, neighbour_pos)
                .to_state()
                .is_side_solid(placement_direction)
            {
                return false;
            }
        }
        if !holds_no_fluid(level, placement_pos) && !holds_water_source(level, placement_pos) {
            return false;
        }
        if existing_state.to_block_id().has_tag(MINECRAFT_FIRE) {
            return false;
        }
        // `DefaultSpreaderConfig.stateCanBeReplaced` after the state's own replaceability.
        existing_state.to_state().replaceable()
            || existing_state.to_state().is_air()
            || existing_state.to_block_id() == BlockId::SCULK_VEIN
            || (existing_state.to_block_id() == BlockId::WATER
                && holds_water_source(level, placement_pos))
    }

    /// Vanilla `MultifaceBlock.isValidStateForPlacement` for a vein, which has every face.
    fn is_valid_state_for_placement<T: GenerationCache>(
        level: &T,
        old_state: BlockStateId,
        placement_pos: BlockPos,
        placement_direction: BlockDirection,
    ) -> bool {
        (old_state.to_block_id() != BlockId::SCULK_VEIN
            || !Self::has_face(old_state, placement_direction))
            && Self::can_attach_to(level, placement_pos, placement_direction)
    }

    /// Vanilla `MultifaceBlock.getStateForPlacement`: `old_state` with the face, or a new vein,
    /// waterlogged in a water source.
    fn get_state_for_placement<T: GenerationCache>(
        level: &T,
        old_state: BlockStateId,
        placement_pos: BlockPos,
        placement_direction: BlockDirection,
    ) -> Option<&'static BlockState> {
        if !Self::is_valid_state_for_placement(level, old_state, placement_pos, placement_direction)
        {
            return None;
        }
        let new_state = if old_state.to_block_id() == BlockId::SCULK_VEIN {
            old_state
        } else if holds_water_source(level, placement_pos) {
            Self::with_waterlogged(Block::SCULK_VEIN.default_state.id)
        } else {
            Block::SCULK_VEIN.default_state.id
        };
        Some(Self::with_face(new_state, placement_direction, true).to_state())
    }

    /// Vanilla `MultifaceBlock.canAttachTo`: the neighbour's support or collision shape covers
    /// the face towards `pos`.
    fn can_attach_to<T: GenerationCache>(
        level: &T,
        pos: BlockPos,
        direction: BlockDirection,
    ) -> bool {
        let neighbour = get_block_state(level, pos.offset(direction.to_offset())).to_state();
        neighbour.is_side_solid(direction.opposite()) || neighbour.is_full_cube()
    }

    /// Vanilla `MultifaceBlock.availableFaces`: the faces of a vein, none for other blocks.
    #[must_use]
    pub fn available_faces(state: BlockStateId) -> u8 {
        BlockDirection::all()
            .into_iter()
            .filter(|&direction| Self::has_face(state, direction))
            .fold(0, |faces, direction| faces | 1 << direction.to_index())
    }

    /// Returns whether `state` has the given face bit set.
    #[must_use]
    pub fn has_face(state: BlockStateId, face: BlockDirection) -> bool {
        if state.to_block_id() != BlockId::SCULK_VEIN {
            return false;
        }
        let props = GlowLichenLikeProperties::from_state_id(state);
        match face {
            BlockDirection::Down => props.down,
            BlockDirection::Up => props.up,
            BlockDirection::North => props.north,
            BlockDirection::South => props.south,
            BlockDirection::West => props.west,
            BlockDirection::East => props.east,
        }
    }

    /// Returns whether any face bit is set.
    #[must_use]
    pub fn has_any_face(state: BlockStateId) -> bool {
        BlockDirection::all()
            .into_iter()
            .any(|direction| Self::has_face(state, direction))
    }

    /// Returns a new state with the given face set to `value`.
    #[must_use]
    pub fn with_face(state: BlockStateId, face: BlockDirection, value: bool) -> BlockStateId {
        let mut props = GlowLichenLikeProperties::from_state_id(state);
        match face {
            BlockDirection::Down => props.down = value,
            BlockDirection::Up => props.up = value,
            BlockDirection::North => props.north = value,
            BlockDirection::South => props.south = value,
            BlockDirection::West => props.west = value,
            BlockDirection::East => props.east = value,
        }
        props.to_state_id(&Block::SCULK_VEIN)
    }

    fn with_waterlogged(state: BlockStateId) -> BlockStateId {
        let mut props = GlowLichenLikeProperties::from_state_id(state);
        props.waterlogged = true;
        props.to_state_id(&Block::SCULK_VEIN)
    }
}

#[cfg(test)]
mod tests {
    use pumpkin_util::math::vector3::Vector3;

    use super::super::test_utils::{empty_chunk, set};
    use super::*;

    #[test]
    fn spread_all_ignores_faces_added_during_call() {
        // The source state is a snapshot: the north face placed below must not become a source
        // for an up face within the same call.
        let mut level = empty_chunk();
        let pos = BlockPos::new(4, 60, 4);
        let base = VeinRules::with_face(
            Block::SCULK_VEIN.default_state.id,
            BlockDirection::Down,
            true,
        );
        set(&mut level, pos, base.to_state());
        set(&mut level, pos.down(), Block::STONE.default_state);
        set(&mut level, pos.north(), Block::STONE.default_state);
        set(&mut level, pos.up(), Block::STONE.default_state);
        assert!(VeinRules::spread_all(&mut level, base, pos, false, &DEFAULT_SPREAD_ORDER) > 0);
        let result = get_block_state(&level, pos);
        assert!(VeinRules::has_face(result, BlockDirection::Down));
        assert!(VeinRules::has_face(result, BlockDirection::North));
        assert!(!VeinRules::has_face(result, BlockDirection::Up));
    }

    #[test]
    fn world_gen_spreading_marks_veins_for_post_processing() {
        let mut level = empty_chunk();
        let pos = BlockPos::new(4, 60, 4);
        set(&mut level, pos.down(), Block::STONE.default_state);
        let air = Block::AIR.default_state.id;
        assert!(VeinRules::spread_all(&mut level, air, pos, true, &SAME_SPACE_SPREAD_ORDER) > 0);
        let section = ((pos.0.y - i32::from(level.bottom_y())) >> 4) as usize;
        let packed = ((pos.0.x & 15) | ((pos.0.y & 15) << 4) | ((pos.0.z & 15) << 8)) as u16;
        assert_eq!(level.post_processing[section], [packed]);
    }

    #[test]
    fn vein_spreading_into_seagrass_is_waterlogged() {
        // Seagrass can be replaced and holds a water source.
        let mut level = empty_chunk();
        let pos = BlockPos::new(4, 60, 4);
        set(&mut level, pos, Block::SEAGRASS.default_state);
        set(&mut level, pos.down(), Block::STONE.default_state);
        let seagrass = Block::SEAGRASS.default_state.id;
        VeinRules::spread_all(&mut level, seagrass, pos, false, &SAME_SPACE_SPREAD_ORDER);
        let down = VeinRules::with_face(
            Block::SCULK_VEIN.default_state.id,
            BlockDirection::Down,
            true,
        );
        assert_eq!(
            get_block_state(&level, pos),
            VeinRules::with_waterlogged(down)
        );
    }

    #[test]
    fn discharged_waterlogged_vein_restores_water() {
        let mut level = empty_chunk();
        let pos = BlockPos::new(4, 60, 4);
        let waterlogged = VeinRules::with_waterlogged(Block::SCULK_VEIN.default_state.id);
        set(&mut level, pos, waterlogged.to_state());
        VeinRules::on_discharged(&mut level, waterlogged, pos);
        assert_eq!(get_block_state(&level, pos), Block::WATER.default_state.id);
    }

    #[test]
    fn spread_type_same_position() {
        let pos = BlockPos::new(10, 60, 10);
        let (p, f) =
            SpreadType::SamePosition.spread_pos(pos, BlockDirection::Up, BlockDirection::North);
        assert_eq!(p, pos);
        assert_eq!(f, BlockDirection::Up);
    }

    #[test]
    fn spread_type_same_plane() {
        let pos = BlockPos::new(10, 60, 10);
        let (p, f) =
            SpreadType::SamePlane.spread_pos(pos, BlockDirection::East, BlockDirection::North);
        assert_eq!(p.0, Vector3::new(11, 60, 10));
        assert_eq!(f, BlockDirection::North);
    }

    #[test]
    fn spread_type_wrap_around() {
        let pos = BlockPos::new(10, 60, 10);
        let (p, f) =
            SpreadType::WrapAround.spread_pos(pos, BlockDirection::East, BlockDirection::North);
        assert_eq!(p.0, Vector3::new(11, 60, 9));
        assert_eq!(f, BlockDirection::West);
    }
}
