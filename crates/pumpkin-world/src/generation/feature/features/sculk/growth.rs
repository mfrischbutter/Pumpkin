//! Sculk spending a cursor's charge on sensors and shriekers (vanilla `SculkBlock`).

use pumpkin_data::Block;
use pumpkin_data::BlockId;
use pumpkin_data::BlockState;
use pumpkin_data::block_properties::SculkSensorLikeProperties;
use pumpkin_data::block_properties::SculkShriekerLikeProperties;
use pumpkin_data::tag::Block::MINECRAFT_SCULK_GROWTH_INHIBITORS;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::random::RandomGenerator;
use pumpkin_util::random::RandomImpl;

use super::get_block_state;
use super::holds_no_fluid;
use super::holds_water_source;
use super::spreader::ChargeCursor;
use super::spreader::SculkSpreader;
use crate::generation::proto_chunk::GenerationCache;

/// Vanilla `SculkSpreader.MAX_GROWTH_RATE_RADIUS`: past this distance from the origin a decaying
/// cursor loses the most charge.
const MAX_GROWTH_RATE_RADIUS: i32 = 24;
/// Vanilla `SculkSpreader.MAX_DECAY_FACTOR`.
const MAX_DECAY_FACTOR: f32 = 0.5;
/// Vanilla `SculkSpreader.SHRIEKER_PLACEMENT_RATE`: one growth in this many is a shrieker.
const SHRIEKER_PLACEMENT_RATE: i32 = 11;
/// Vanilla `SculkBlock.GROWTH_INHIBITOR_RANGE`: the horizontal reach of the growth check.
const GROWTH_INHIBITOR_RANGE: i32 = 4;
/// How far above the sculk the growth check reaches.
const GROWTH_INHIBITOR_HEIGHT: i32 = 2;
/// How many growth inhibitors may be in reach of a new growth.
const MAX_GROWTH_INHIBITORS: i32 = 2;

/// Rules of sculk using a cursor's charge.
pub struct GrowthRules;

impl GrowthRules {
    /// Vanilla `SculkBlock.attemptUseCharge`: grows a sensor or shrieker above the cursor or lets
    /// its charge decay with the distance from the origin.
    pub fn attempt_use_charge<T: GenerationCache>(
        cursor: &ChargeCursor,
        level: &mut T,
        origin_pos: BlockPos,
        random: &mut RandomGenerator,
        spreader: &SculkSpreader,
    ) -> i32 {
        let charge = cursor.charge;
        if charge == 0 || random.next_bounded_i32(spreader.charge_decay_rate()) != 0 {
            return charge;
        }

        let charge_pos = cursor.pos;
        let is_close_to_catalyst =
            Self::is_close_to_catalyst(charge_pos, origin_pos, spreader.no_growth_radius());
        if !is_close_to_catalyst && Self::can_place_growth(level, charge_pos) {
            let xp_per_growth_spawn = spreader.growth_spawn_cost();
            if random.next_bounded_i32(xp_per_growth_spawn) < charge {
                let growth_placement = charge_pos.up();
                let growth_state = Self::random_growth_state(
                    level,
                    growth_placement,
                    random,
                    spreader.is_world_generation(),
                );
                level.set_block_state(&growth_placement.0, growth_state);
            }
            return (charge - xp_per_growth_spawn).max(0);
        }

        if random.next_bounded_i32(spreader.additional_decay_rate()) != 0 {
            charge
        } else if is_close_to_catalyst {
            charge - 1
        } else {
            charge - Self::decay_penalty(spreader, charge_pos, origin_pos, charge)
        }
    }

    /// Vanilla `BlockPos.closerThan` with the spreader's no-growth radius.
    fn is_close_to_catalyst(pos: BlockPos, origin_pos: BlockPos, no_growth_radius: i32) -> bool {
        pos.squared_distance(&origin_pos) < no_growth_radius * no_growth_radius
    }

    /// Vanilla `SculkBlock.getDecayPenalty`.
    fn decay_penalty(
        spreader: &SculkSpreader,
        pos: BlockPos,
        origin_pos: BlockPos,
        charge: i32,
    ) -> i32 {
        let no_growth_radius = spreader.no_growth_radius();
        let outer_distance =
            f64::from(pos.squared_distance(&origin_pos)).sqrt() as f32 - no_growth_radius as f32;
        let outer_distance_squared = outer_distance * outer_distance;
        let max_reach = MAX_GROWTH_RATE_RADIUS - no_growth_radius;
        let max_reach_squared = max_reach * max_reach;
        let distance_factor = (outer_distance_squared / max_reach_squared as f32).min(1.0);
        ((charge as f32 * distance_factor * MAX_DECAY_FACTOR) as i32).max(1)
    }

    /// Vanilla `SculkBlock.getRandomGrowthState`: a sensor, or one time in
    /// [`SHRIEKER_PLACEMENT_RATE`] a shrieker, waterlogged if `pos` holds a fluid.
    fn random_growth_state<T: GenerationCache>(
        level: &T,
        pos: BlockPos,
        random: &mut RandomGenerator,
        is_world_gen: bool,
    ) -> &'static BlockState {
        let waterlogged = !holds_no_fluid(level, pos);
        if random.next_bounded_i32(SHRIEKER_PLACEMENT_RATE) == 0 {
            let mut props = SculkShriekerLikeProperties::default(&Block::SCULK_SHRIEKER);
            props.can_summon = is_world_gen;
            props.waterlogged = waterlogged;
            props.to_state_id(&Block::SCULK_SHRIEKER).to_state()
        } else {
            let mut props = SculkSensorLikeProperties::default(&Block::SCULK_SENSOR);
            props.waterlogged = waterlogged;
            props.to_state_id(&Block::SCULK_SENSOR).to_state()
        }
    }

    /// Vanilla `SculkBlock.canPlaceGrowth`: air or a water source above, and at most
    /// [`MAX_GROWTH_INHIBITORS`] sensors and shriekers in reach.
    fn can_place_growth<T: GenerationCache>(level: &T, pos: BlockPos) -> bool {
        let above = pos.up();
        let state_above = get_block_state(level, above);
        if !(state_above.to_state().is_air()
            || (state_above.to_block_id() == BlockId::WATER && holds_water_source(level, above)))
        {
            return false;
        }

        let mut inhibitors = 0;
        for dy in 0..=GROWTH_INHIBITOR_HEIGHT {
            for dz in -GROWTH_INHIBITOR_RANGE..=GROWTH_INHIBITOR_RANGE {
                for dx in -GROWTH_INHIBITOR_RANGE..=GROWTH_INHIBITOR_RANGE {
                    let state = get_block_state(level, pos.add(dx, dy, dz));
                    if state
                        .to_block_id()
                        .has_tag(MINECRAFT_SCULK_GROWTH_INHIBITORS)
                    {
                        inhibitors += 1;
                        if inhibitors > MAX_GROWTH_INHIBITORS {
                            return false;
                        }
                    }
                }
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_utils::{empty_chunk, set};
    use super::*;

    #[test]
    fn can_place_growth_accepts_air_like_states() {
        // Cave and void air count as air, not only `minecraft:air`.
        let mut level = empty_chunk();
        let pos = BlockPos::new(4, 60, 4);
        set(&mut level, pos.up(), Block::CAVE_AIR.default_state);
        assert!(GrowthRules::can_place_growth(&level, pos));
    }

    #[test]
    fn can_place_growth_accepts_water_source_above() {
        let mut level = empty_chunk();
        let pos = BlockPos::new(4, 60, 4);
        set(&mut level, pos.up(), Block::WATER.default_state);
        assert!(GrowthRules::can_place_growth(&level, pos));
    }

    #[test]
    fn density_ignores_calibrated_sensors() {
        let mut level = empty_chunk();
        let pos = BlockPos::new(4, 60, 4);
        for dx in 0..3 {
            set(
                &mut level,
                pos.add(dx, 0, 0),
                Block::CALIBRATED_SCULK_SENSOR.default_state,
            );
        }
        assert!(GrowthRules::can_place_growth(&level, pos));
    }

    #[test]
    fn density_blocks_after_two_sensors_or_shriekers() {
        let mut level = empty_chunk();
        let pos = BlockPos::new(4, 60, 4);
        for dx in 0..2 {
            set(
                &mut level,
                pos.add(dx, 0, 0),
                Block::SCULK_SENSOR.default_state,
            );
        }
        assert!(GrowthRules::can_place_growth(&level, pos));
        set(
            &mut level,
            pos.add(2, 0, 0),
            Block::SCULK_SHRIEKER.default_state,
        );
        assert!(!GrowthRules::can_place_growth(&level, pos));
    }

    #[test]
    fn close_to_catalyst_is_strict() {
        let origin = BlockPos::new(0, 60, 0);
        // `closerThan` uses `< radius²`: exactly on the boundary is not close.
        assert!(!GrowthRules::is_close_to_catalyst(
            BlockPos::new(4, 60, 0),
            origin,
            4
        ));
        assert!(GrowthRules::is_close_to_catalyst(
            BlockPos::new(3, 60, 0),
            origin,
            4
        ));
        assert!(GrowthRules::is_close_to_catalyst(
            BlockPos::new(2, 62, 2),
            origin,
            4
        ));
    }
}
