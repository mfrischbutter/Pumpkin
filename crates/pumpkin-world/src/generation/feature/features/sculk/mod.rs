//! Vanilla's sculk spreading for world generation: a [`spreader::SculkSpreader`] moves charge
//! cursors over sculk, and each cursor acts through the [`SculkBehaviour`] of the block it is on.
//!
//! - [`spreader`]: the spreader and its charge cursors (`SculkSpreader`).
//! - [`growth`]: sculk spending charge on sensors and shriekers (`SculkBlock`).
//! - [`vein`]: sculk veins and their spreading over faces (`SculkVeinBlock`, `MultifaceSpreader`).

use pumpkin_data::BlockDirection;
use pumpkin_data::BlockId;
use pumpkin_data::BlockStateId;
use pumpkin_data::fluid::Fluid;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::random::RandomGenerator;
use pumpkin_util::random::RandomImpl;

use crate::generation::proto_chunk::GenerationCache;

use self::growth::GrowthRules;
use self::spreader::ChargeCursor;
use self::spreader::SculkSpreader;
use self::vein::DEFAULT_SPREAD_ORDER;
use self::vein::SAME_SPACE_SPREAD_ORDER;
use self::vein::VeinRules;

pub mod growth;
pub mod spreader;
pub mod vein;

/// What a block does with a cursor on it (vanilla `SculkBehaviour`): sculk and sculk veins have
/// their own behaviour, every other block the default one.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SculkBehaviour {
    Default,
    Sculk,
    SculkVein,
}

impl SculkBehaviour {
    #[must_use]
    pub const fn of(state: BlockStateId) -> Self {
        match state.to_block_id() {
            BlockId::SCULK => Self::Sculk,
            BlockId::SCULK_VEIN => Self::SculkVein,
            _ => Self::Default,
        }
    }

    /// Vanilla `SculkBehaviour.attemptSpreadVein`: whether veins grew at or around `pos`.
    pub fn attempt_spread_vein<T: GenerationCache>(
        self,
        level: &mut T,
        pos: BlockPos,
        state: BlockStateId,
        facings: Option<u8>,
        post_process: bool,
    ) -> bool {
        match (self, facings) {
            (Self::Default, None) => {
                let state = get_block_state(level, pos);
                VeinRules::spread_all(level, state, pos, post_process, &SAME_SPACE_SPREAD_ORDER) > 0
            }
            (Self::Default, Some(faces)) if faces != 0 => {
                (state.to_state().is_air() || holds_water_source(level, pos))
                    && VeinRules::regrow(level, pos, faces)
            }
            _ => VeinRules::spread_all(level, state, pos, post_process, &DEFAULT_SPREAD_ORDER) > 0,
        }
    }

    /// Vanilla `SculkBehaviour.canChangeBlockStateOnSpread`: spreading never changes sculk.
    #[must_use]
    pub const fn can_change_block_state_on_spread(self) -> bool {
        !matches!(self, Self::Sculk)
    }

    /// Vanilla `SculkBehaviour.attemptUseCharge`: the cursor's charge after the block used it.
    pub fn attempt_use_charge<T: GenerationCache>(
        self,
        cursor: &ChargeCursor,
        level: &mut T,
        origin_pos: BlockPos,
        random: &mut RandomGenerator,
        spreader: &SculkSpreader,
        spread_veins: bool,
    ) -> i32 {
        match self {
            Self::Default => {
                if cursor.decay_delay > 0 {
                    cursor.charge
                } else {
                    0
                }
            }
            Self::Sculk => {
                GrowthRules::attempt_use_charge(cursor, level, origin_pos, random, spreader)
            }
            Self::SculkVein => {
                VeinRules::attempt_use_charge(cursor, level, random, spreader, spread_veins)
            }
        }
    }

    /// Vanilla `SculkBehaviour.onDischarged`, given the state the cursor saw at `pos`.
    pub fn on_discharged<T: GenerationCache>(
        self,
        level: &mut T,
        state: BlockStateId,
        pos: BlockPos,
    ) {
        if self == Self::SculkVein {
            VeinRules::on_discharged(level, state, pos);
        }
    }

    /// Vanilla `SculkBehaviour.updateDecayDelay`.
    #[must_use]
    pub fn update_decay_delay(self, age: i32) -> i32 {
        match self {
            Self::Default => (age - 1).max(0),
            Self::Sculk | Self::SculkVein => 1,
        }
    }
}

/// Whether the block is sculk or a sculk vein (vanilla `instanceof SculkBehaviour`).
#[must_use]
pub const fn is_sculk_behaviour(id: BlockId) -> bool {
    matches!(id, BlockId::SCULK | BlockId::SCULK_VEIN)
}

/// Vanilla `SculkPatchFeature.canSpreadFrom`: sculk, or air or a water source next to a full block.
pub fn can_spread_from<T: GenerationCache>(level: &T, origin: BlockPos) -> bool {
    let start = get_block_state(level, origin);
    if is_sculk_behaviour(start.to_block_id()) {
        return true;
    }
    let is_water_source =
        start.to_block_id() == BlockId::WATER && holds_water_source(level, origin);
    if !start.to_state().is_air() && !is_water_source {
        return false;
    }
    BlockDirection::all().into_iter().any(|direction| {
        get_block_state(level, origin.offset(direction.to_offset()))
            .to_state()
            .is_full_cube()
    })
}

fn get_block_state<T: GenerationCache>(level: &T, pos: BlockPos) -> BlockStateId {
    GenerationCache::get_block_state(level, &pos.0)
}

/// Vanilla `FluidState.isEmpty` of the block at `pos`.
fn holds_no_fluid<T: GenerationCache>(level: &T, pos: BlockPos) -> bool {
    level.get_fluid_and_fluid_state(&pos.0).0.id == Fluid::EMPTY.id
}

/// Vanilla `FluidState.is(Fluids.WATER)` of the block at `pos`: it holds a water source.
fn holds_water_source<T: GenerationCache>(level: &T, pos: BlockPos) -> bool {
    level.get_fluid_and_fluid_state(&pos.0).0.id == Fluid::WATER.id
}

/// Vanilla `Util.shuffle`.
fn shuffle<V>(values: &mut [V], random: &mut RandomGenerator) {
    for i in (1..values.len()).rev() {
        let swap_to = random.next_bounded_i32(i as i32 + 1) as usize;
        values.swap(i, swap_to);
    }
}

#[cfg(test)]
pub(crate) mod test_utils {
    use pumpkin_data::BlockState;
    use pumpkin_data::dimension::Dimension;
    use pumpkin_util::math::position::BlockPos;
    use pumpkin_util::world_seed::Seed;

    use crate::chunk_system::{Cache, Chunk};
    use crate::generation::get_world_gen;
    use crate::generation::proto_chunk::{GenerationCache, ProtoChunk};

    /// The chunk at 0 0 with nothing in it, as a one-chunk generation cache.
    pub fn empty_chunk() -> ProtoChunk {
        let world_gen = get_world_gen(
            Seed(1),
            Dimension::OVERWORLD,
            false,
            Vec::new(),
            String::new(),
        );
        ProtoChunk::new(0, 0, &world_gen)
    }

    /// The nine chunks from 0 0 to 2 2 with nothing in them, centred on chunk 1 1.
    pub fn empty_region() -> Cache {
        let world_gen = get_world_gen(
            Seed(1),
            Dimension::OVERWORLD,
            false,
            Vec::new(),
            String::new(),
        );
        let mut cache = Cache::new(0, 0, 3);
        for x in 0..3 {
            for z in 0..3 {
                cache
                    .chunks
                    .push(Chunk::Proto(Box::new(ProtoChunk::new(x, z, &world_gen))));
            }
        }
        cache
    }

    pub fn set<T: GenerationCache>(level: &mut T, pos: BlockPos, state: &BlockState) {
        level.set_block_state(&pos.0, state);
    }
}

#[cfg(test)]
mod tests {
    use pumpkin_data::Block;
    use pumpkin_data::block_properties::GlowLichenLikeProperties;

    use super::test_utils::{empty_chunk, set};
    use super::*;

    #[test]
    fn can_spread_from_water_source_origin() {
        let mut level = empty_chunk();
        let origin = BlockPos::new(0, 60, 0);
        set(&mut level, origin, Block::WATER.default_state);
        set(&mut level, origin.up(), Block::STONE.default_state);
        assert!(can_spread_from(&level, origin));
    }

    #[test]
    fn can_spread_from_rejects_solid_origin() {
        let mut level = empty_chunk();
        let origin = BlockPos::new(0, 60, 0);
        set(&mut level, origin, Block::STONE.default_state);
        set(&mut level, origin.up(), Block::STONE.default_state);
        assert!(!can_spread_from(&level, origin));
    }

    #[test]
    fn can_spread_from_rejects_waterlogged_non_water_origin() {
        // Only air or a water block holding a source qualifies: waterlogged glow lichen holds a
        // water source but is no water block.
        let mut level = empty_chunk();
        let origin = BlockPos::new(0, 60, 0);
        let mut lichen = GlowLichenLikeProperties::default(&Block::GLOW_LICHEN);
        lichen.waterlogged = true;
        set(
            &mut level,
            origin,
            lichen.to_state_id(&Block::GLOW_LICHEN).to_state(),
        );
        set(&mut level, origin.up(), Block::STONE.default_state);
        assert!(holds_water_source(&level, origin));
        assert!(!can_spread_from(&level, origin));
    }

    #[test]
    fn can_spread_from_rejects_sensor_origin() {
        let mut level = empty_chunk();
        let origin = BlockPos::new(0, 60, 0);
        set(&mut level, origin, Block::SCULK_SENSOR.default_state);
        set(&mut level, origin.up(), Block::STONE.default_state);
        assert!(!can_spread_from(&level, origin));
    }

    #[test]
    fn only_sculk_and_veins_have_their_own_behaviour() {
        assert_eq!(
            SculkBehaviour::of(Block::SCULK.default_state.id),
            SculkBehaviour::Sculk
        );
        assert_eq!(
            SculkBehaviour::of(Block::SCULK_VEIN.default_state.id),
            SculkBehaviour::SculkVein
        );
        for block in [
            Block::SCULK_CATALYST,
            Block::SCULK_SENSOR,
            Block::CALIBRATED_SCULK_SENSOR,
            Block::SCULK_SHRIEKER,
        ] {
            assert_eq!(
                SculkBehaviour::of(block.default_state.id),
                SculkBehaviour::Default
            );
        }
    }
}
