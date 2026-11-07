//! Tree edges against vanilla 26.3: `TreeFeature.place` ends with `updateShapeAtEdge`.

use pumpkin_data::Block;
use pumpkin_data::block_properties::{
    BlockProperties, DoubleBlockHalf, TallSeagrassLikeProperties,
};
use pumpkin_data::dimension::Dimension;
use pumpkin_util::math::vector3::Vector3;

use super::stage_parity::decorate_chunk;

/// Vanilla 26.3, seed -4172144997902289642, no generation secret: oak leaves at y 65 replace
/// the upper halves of peonies, and vanilla removes the lower halves at y 64 as well.
#[test]
fn tree_leaves_remove_the_double_plant_half_below_them() {
    let (cx, cz) = (-157, -57);
    let grid = decorate_chunk(
        -4_172_144_997_902_289_642i64 as u64,
        Dimension::OVERWORLD,
        cx,
        cz,
        |_, _| {},
    );
    let chunk = grid.chunks.get(&(cx, cz)).unwrap();
    let block_at = |x: i32, y: i32, z: i32| {
        let state = chunk.get_block_state(&Vector3::new(x, y, z));
        (Block::from_state_id(state), state)
    };

    for (x, z) in [
        (-2510, -907),
        (-2508, -910),
        (-2507, -907),
        (-2506, -909),
        (-2506, -903),
    ] {
        let (block, _) = block_at(x, 64, z);
        assert_ne!(block.id, Block::PEONY.id, "peony left at ({x}, 64, {z})");
        if (x, z) != (-2506, -909) {
            assert_eq!(
                block.id,
                Block::AIR.id,
                "expected air at ({x}, 64, {z}), got {}",
                block.name
            );
        }
    }

    for x in cx * 16..cx * 16 + 16 {
        for z in cz * 16..cz * 16 + 16 {
            for y in 40..120 {
                let (block, state) = block_at(x, y, z);
                if !TallSeagrassLikeProperties::handles_block_id(block.id) {
                    continue;
                }
                let (other_y, other_half) =
                    match TallSeagrassLikeProperties::from_state_id(state).half {
                        DoubleBlockHalf::Lower => (y + 1, DoubleBlockHalf::Upper),
                        DoubleBlockHalf::Upper => (y - 1, DoubleBlockHalf::Lower),
                    };
                let (other, other_state) = block_at(x, other_y, z);
                assert!(
                    other.id == block.id
                        && TallSeagrassLikeProperties::from_state_id(other_state).half
                            == other_half,
                    "{} at ({x}, {y}, {z}) misses its other half, found {} at y {other_y}",
                    block.name,
                    other.name
                );
            }
        }
    }
}
