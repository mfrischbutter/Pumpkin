//! Rail lookups both vanilla minecart behaviours share.

use pumpkin_data::block_properties::{PoweredRailLikeProperties, RailLikeProperties, RailShape};
use pumpkin_data::tag::{self, Taggable};
use pumpkin_data::{Block, BlockStateId};
use pumpkin_util::math::vector3::Vector3;

use crate::block::blocks::redstone::rails::StraightRailShapeExt;

/// Vanilla `BaseRailBlock.isRail`.
pub(super) fn is_rail(block: &Block) -> bool {
    block.has_tag(&tag::Block::MINECRAFT_RAILS)
}

/// The shape of a rail block's state, whichever of the two rail property types it uses.
pub(super) fn rail_shape(block: &Block, state_id: BlockStateId) -> RailShape {
    if block == &Block::RAIL {
        RailLikeProperties::from_state_id(state_id).shape
    } else {
        PoweredRailLikeProperties::from_state_id(state_id)
            .shape
            .as_shape()
    }
}

/// Vanilla `AbstractMinecart.EXITS`: the two ends of a rail, the low end of a slope one down.
pub(super) const fn exits(shape: RailShape) -> (Vector3<i32>, Vector3<i32>) {
    const X_NEG: Vector3<i32> = Vector3::new(-1, 0, 0);
    const X_POS: Vector3<i32> = Vector3::new(1, 0, 0);
    const Z_NEG: Vector3<i32> = Vector3::new(0, 0, -1);
    const Z_POS: Vector3<i32> = Vector3::new(0, 0, 1);
    const X_NEG_BELOW: Vector3<i32> = Vector3::new(-1, -1, 0);
    const X_POS_BELOW: Vector3<i32> = Vector3::new(1, -1, 0);
    const Z_NEG_BELOW: Vector3<i32> = Vector3::new(0, -1, -1);
    const Z_POS_BELOW: Vector3<i32> = Vector3::new(0, -1, 1);
    match shape {
        RailShape::NorthSouth => (Z_NEG, Z_POS),
        RailShape::EastWest => (X_NEG, X_POS),
        RailShape::AscendingEast => (X_NEG_BELOW, X_POS),
        RailShape::AscendingWest => (X_NEG, X_POS_BELOW),
        RailShape::AscendingNorth => (Z_NEG, Z_POS_BELOW),
        RailShape::AscendingSouth => (Z_NEG_BELOW, Z_POS),
        RailShape::SouthEast => (Z_POS, X_POS),
        RailShape::SouthWest => (Z_POS, X_NEG),
        RailShape::NorthWest => (Z_NEG, X_NEG),
        RailShape::NorthEast => (Z_NEG, X_POS),
    }
}

#[cfg(test)]
mod tests {
    use super::exits;
    use pumpkin_data::block_properties::RailShape;

    #[test]
    fn slope_exits_drop_on_the_low_side_like_vanilla() {
        let (low, high) = exits(RailShape::AscendingEast);
        assert_eq!((low.x, low.y), (-1, -1));
        assert_eq!((high.x, high.y), (1, 0));
        let (high, low) = exits(RailShape::AscendingNorth);
        assert_eq!((high.z, high.y), (-1, 0));
        assert_eq!((low.z, low.y), (1, -1));
    }
}
