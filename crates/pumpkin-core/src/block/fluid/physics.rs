use std::sync::Arc;

use pumpkin_data::block_properties::SlabType;
use pumpkin_data::tag::{self, Taggable};
use pumpkin_data::{Block, BlockDirection, BlockStateId, fluid::Fluid};
use pumpkin_util::math::position::BlockPos;
use pumpkin_world::{tick::TickPriority, world::BlockFlags};

use super::lava::MIN_LEVEL_CUTOFF;
use crate::world::World;

type SlabProperties = pumpkin_data::block_properties::WhiteWoolSlabLikeProperties;

/// Vanilla `LiquidBlockContainer`: a block that keeps a fluid in its own state instead of being
/// washed away by it. These are the waterloggable blocks, kelp and seagrass.
#[must_use]
pub fn is_liquid_block_container(block: &Block) -> bool {
    block.is_waterloggable()
        || block == &Block::KELP
        || block == &Block::KELP_PLANT
        || block == &Block::SEAGRASS
        || block == &Block::TALL_SEAGRASS
}

/// Vanilla `LiquidBlockContainer.canPlaceLiquid` without an entity, as a spreading fluid asks it.
#[must_use]
pub fn can_place_liquid(state_id: BlockStateId, fluid: &Fluid) -> bool {
    let block = state_id.to_block();
    // `SimpleWaterloggedBlock` takes only a water source. A barrier takes it only from a creative
    // player and a double slab never; kelp and seagrass take no fluid at all.
    fluid.id == Fluid::WATER.id
        && block.is_waterloggable()
        && block != &Block::BARRIER
        && !(block.has_tag(&tag::Block::MINECRAFT_SLABS)
            && SlabProperties::from_state_id(state_id).r#type == SlabType::Double)
}

/// Vanilla `FlowingFluid.canHoldAnyFluid`.
#[must_use]
pub fn can_hold_any_fluid(block: &Block) -> bool {
    is_liquid_block_container(block) || block.has_tag(&tag::Block::MINECRAFT_WASHED_AWAY_BY_FLUIDS)
}

/// Vanilla `FlowingFluid.canHoldSpecificFluid`.
#[must_use]
pub fn can_hold_specific_fluid(state_id: BlockStateId, fluid: &Fluid) -> bool {
    !is_liquid_block_container(state_id.to_block()) || can_place_liquid(state_id, fluid)
}

/// Vanilla `FlowingFluid.canHoldFluid`: whether `fluid` may flow into a block in `state_id`.
#[must_use]
pub fn can_hold_fluid(state_id: BlockStateId, fluid: &Fluid) -> bool {
    can_hold_any_fluid(state_id.to_block()) && can_hold_specific_fluid(state_id, fluid)
}

/// Vanilla `FluidState.canBeReplacedWith`: whether the fluid at `pos` gives way to `other`
/// flowing into it towards `direction`.
#[must_use]
pub fn can_be_replaced_with(
    world: &World,
    pos: &BlockPos,
    other: &Fluid,
    direction: BlockDirection,
) -> bool {
    let (fluid, state) = world.get_fluid_and_fluid_state(pos);
    let other_is_water = other.has_tag(&tag::Fluid::MINECRAFT_WATER);
    if fluid.matches_type(&Fluid::WATER) {
        // WaterFluid
        direction == BlockDirection::Down && !other_is_water
    } else if fluid.matches_type(&Fluid::LAVA) {
        // LavaFluid
        world.get_fluid_height(pos, fluid, &state) >= MIN_LEVEL_CUTOFF && other_is_water
    } else {
        // EmptyFluid
        true
    }
}

/// Vanilla `SimpleWaterloggedBlock.placeLiquid`: a dry waterloggable block takes a water source
/// by becoming waterlogged.
pub fn place_liquid(world: &Arc<World>, pos: &BlockPos, state_id: BlockStateId, fluid: &Fluid) {
    if fluid.id != Fluid::WATER.id {
        return;
    }
    let Some(waterlogged) = state_id.to_block().set_waterlogged(state_id, true) else {
        return;
    };
    world.set_block_state(pos, waterlogged, BlockFlags::NOTIFY_ALL);
    world.schedule_fluid_tick(
        &Fluid::WATER,
        *pos,
        Fluid::WATER.flow_speed as u8,
        TickPriority::Normal,
    );
}

#[cfg(test)]
mod tests {
    use pumpkin_data::block_properties::SlabType;
    use pumpkin_data::{Block, BlockStateId, fluid::Fluid};

    use super::{SlabProperties, can_hold_fluid};

    fn oak_slab(r#type: SlabType) -> BlockStateId {
        let mut props = SlabProperties::default(&Block::OAK_SLAB);
        props.r#type = r#type;
        props.to_state_id(&Block::OAK_SLAB)
    }

    #[test]
    fn flowing_fluids_wash_away_only_blocks_tagged_for_it() {
        let kept_out = [
            Block::GLOW_LICHEN.default_state.id,
            oak_slab(SlabType::Bottom),
            Block::OAK_FENCE.default_state.id,
            Block::PUMPKIN.default_state.id,
            Block::SUSPICIOUS_SAND.default_state.id,
        ];
        let washed_away = [
            Block::TORCH.default_state.id,
            Block::SHORT_GRASS.default_state.id,
            Block::END_ROD.default_state.id,
        ];
        for fluid in [&Fluid::FLOWING_WATER, &Fluid::FLOWING_LAVA] {
            for state_id in kept_out {
                assert!(
                    !can_hold_fluid(state_id, fluid),
                    "{} flows into {}",
                    fluid.name,
                    state_id.to_block().name
                );
            }
            for state_id in washed_away {
                assert!(
                    can_hold_fluid(state_id, fluid),
                    "{} does not wash away {}",
                    fluid.name,
                    state_id.to_block().name
                );
            }
        }
    }

    #[test]
    fn only_a_water_source_waterlogs_a_block() {
        for state_id in [
            oak_slab(SlabType::Bottom),
            Block::OAK_FENCE.default_state.id,
            Block::GLOW_LICHEN.default_state.id,
        ] {
            assert!(can_hold_fluid(state_id, &Fluid::WATER));
            assert!(!can_hold_fluid(state_id, &Fluid::LAVA));
        }
        for state_id in [
            oak_slab(SlabType::Double),
            Block::BARRIER.default_state.id,
            Block::KELP.default_state.id,
        ] {
            assert!(!can_hold_fluid(state_id, &Fluid::WATER));
        }
    }
}
