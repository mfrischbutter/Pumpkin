use pumpkin_data::BlockStateId;
use pumpkin_util::math::position::BlockPos;

use super::at_y;
use crate::generation::proto_chunk::GenerationCache;

/// Vanilla's `levelgen.Column`: the open blocks around a position, bounded by the floor and the
/// ceiling block the scan ended on, if any.
#[derive(Clone, Copy)]
pub(super) struct Column {
    pub floor: Option<i32>,
    pub ceiling: Option<i32>,
}

impl Column {
    /// Vanilla's `Column.scan`: walks up and then down from `pos` while `inside_column` accepts the
    /// block, at most `search_range - 1` steps each way, and keeps each block it stops on that
    /// `valid_edge` accepts. `None` when `pos` itself is outside a column.
    pub fn scan<T: GenerationCache>(
        chunk: &T,
        pos: BlockPos,
        search_range: i32,
        inside_column: impl Fn(BlockStateId) -> bool,
        valid_edge: impl Fn(BlockStateId) -> bool,
    ) -> Option<Self> {
        let state_at = |y| GenerationCache::get_block_state(chunk, &at_y(pos, y).0);
        if !inside_column(state_at(pos.0.y)) {
            return None;
        }
        let scan_direction = |step: i32| {
            let mut y = pos.0.y;
            for _ in 1..search_range {
                if !inside_column(state_at(y)) {
                    break;
                }
                y += step;
            }
            valid_edge(state_at(y)).then_some(y)
        };
        let ceiling = scan_direction(1);
        let floor = scan_direction(-1);
        Some(Self { floor, ceiling })
    }

    /// Vanilla's `Column.getHeight`: the number of blocks between the floor and the ceiling.
    pub fn height(self) -> Option<i32> {
        Some(self.ceiling? - self.floor? - 1)
    }
}
