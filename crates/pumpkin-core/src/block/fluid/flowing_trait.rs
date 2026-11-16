use super::{pathfinder, physics};
use crate::world::World;
use pumpkin_data::{
    Block, BlockDirection, BlockStateId,
    fluid::{EnumVariants, Falling, Fluid, FluidProperties, Level},
    world::WorldEvent,
};
use pumpkin_util::math::position::BlockPos;
use pumpkin_world::{tick::TickPriority, world::BlockFlags};
use std::sync::Arc;
pub type FlowingFluidProperties = pumpkin_data::fluid::FlowingWaterLikeFluidProperties;

#[allow(async_fn_in_trait)]
/// Blocks whose fluid state is always a water source in vanilla (their `getFluidState` returns
/// `Fluids.WATER.getSource(false)`): water neither flows into them nor drains them.
fn holds_water_source(block: &Block) -> bool {
    block == &Block::BUBBLE_COLUMN
        || block == &Block::KELP
        || block == &Block::KELP_PLANT
        || block == &Block::SEAGRASS
        || block == &Block::TALL_SEAGRASS
}

pub trait FlowingFluid: Send + Sync {
    fn get_level_decrease_per_block(&self, world: &World) -> i32;
    fn get_flow_speed(&self, world: &World) -> u8;

    fn get_source(&self, fluid: &Fluid, falling: bool) -> FlowingFluidProperties {
        let mut source_props = FlowingFluidProperties::default(fluid);
        source_props.level = Level::L8;
        source_props.falling = if falling {
            Falling::True
        } else {
            Falling::False
        };
        source_props
    }

    fn get_flowing(&self, fluid: &Fluid, level: Level, falling: bool) -> FlowingFluidProperties {
        let mut flowing_props = FlowingFluidProperties::default(fluid);
        flowing_props.level = level;
        flowing_props.falling = if falling {
            Falling::True
        } else {
            Falling::False
        };
        flowing_props
    }

    fn get_max_flow_distance(&self, world: &World) -> i32;
    fn can_convert_to_source(&self, world: &Arc<World>) -> bool;

    /// Returns true if `state_id` represents the given fluid — either as a direct fluid state
    /// or as a waterlogged block (when the fluid is water-type).
    fn has_fluid_at(&self, fluid: &Fluid, state_id: BlockStateId) -> bool {
        self.get_effective_props(fluid, state_id).is_some()
    }

    /// Returns correct fluid properties for a state, treating waterlogged blocks as water sources
    /// (level 8, non-falling). Returns `None` if the state doesn't contain this fluid.
    fn get_effective_props(
        &self,
        fluid: &Fluid,
        state_id: BlockStateId,
    ) -> Option<FlowingFluidProperties> {
        if fluid.states.iter().any(|s| s.block_state_id == state_id) {
            return Some(FlowingFluidProperties::from_state_id(state_id, fluid));
        }
        if fluid.id == Fluid::FLOWING_WATER.id || fluid.id == Fluid::WATER.id {
            let block = Block::from_state_id(state_id);
            if block.is_waterlogged(state_id) || holds_water_source(block) {
                return Some(self.get_source(fluid, false));
            }
        }
        None
    }

    /// Vanilla `FluidState.getType()` of `props`: the still fluid for a source, `fluid` otherwise.
    fn fluid_type<'a>(&self, fluid: &'a Fluid, props: &FlowingFluidProperties) -> &'a Fluid {
        Fluid::from_state_id(props.to_state_id(fluid)).unwrap_or(fluid)
    }

    /// Vanilla `FlowingFluid.isSourceBlockOfThisType`, waterlogged blocks included.
    fn is_source_block_of_this_type(&self, fluid: &Fluid, state_id: BlockStateId) -> bool {
        self.get_effective_props(fluid, state_id)
            .is_some_and(|props| props.level == Level::L8 && props.falling == Falling::False)
    }

    /// Vanilla `FlowingFluid.canMaybePassThrough`, without the collision shapes that
    /// `canPassThroughWall` compares.
    fn can_maybe_pass_through(&self, fluid: &Fluid, state_id: BlockStateId) -> bool {
        !self.is_source_block_of_this_type(fluid, state_id)
            && physics::can_hold_any_fluid(state_id.to_block())
    }

    /// Vanilla `FlowingFluid.canPassThrough` for the flowing fluid, as the slope search asks it.
    fn can_pass_through(&self, fluid: &Fluid, state_id: BlockStateId) -> bool {
        self.can_maybe_pass_through(fluid, state_id)
            && physics::can_hold_specific_fluid(state_id, fluid)
    }

    /// Vanilla `FlowingFluid.isWaterHole`, without the collision shapes that
    /// `canPassThroughWall` compares: whether the fluid would flow down into `below_state_id`.
    fn is_water_hole(&self, fluid: &Fluid, below_state_id: BlockStateId) -> bool {
        self.has_fluid_at(fluid, below_state_id) || physics::can_hold_fluid(below_state_id, fluid)
    }

    /// Core fluid tick handler that updates fluid state and triggers spreading.
    ///
    /// Processes scheduled fluid ticks by:
    /// 1. Validating the block contains fluid
    /// 2. Updating non-source fluid levels based on neighbors
    /// 3. Triggering fluid spread to adjacent positions
    ///
    /// Sources (level 8, non-falling) always spread without state changes.
    fn on_scheduled_tick_internal(&self, world: &Arc<World>, fluid: &Fluid, block_pos: &BlockPos) {
        let current_block_state_id = world.get_block_state_id(block_pos);
        let block = Block::from_state_id(current_block_state_id);

        if !self.has_fluid_at(fluid, current_block_state_id) {
            return;
        }

        let waterlogged = block.is_waterlogged(current_block_state_id);
        let Some(current_fluid_state) = self.get_effective_props(fluid, current_block_state_id)
        else {
            return;
        };
        let is_source =
            current_fluid_state.level == Level::L8 && current_fluid_state.falling != Falling::True;
        let state_for_spreading: FlowingFluidProperties;

        // Update state if non-source
        if !is_source && !waterlogged {
            let new_fluid_state = self.get_new_liquid(world, fluid, block_pos);

            if let Some(mut new_state) = new_fluid_state {
                let new_state_id = new_state.to_state_id(fluid);

                if new_state_id != current_block_state_id {
                    let Some(state_id) = fire_level_change(world, block_pos, new_state_id) else {
                        return;
                    };
                    world.set_block_state(block_pos, state_id, BlockFlags::NOTIFY_ALL);

                    // Schedule next tick for this position
                    let tick_delay = self.get_flow_speed(world);
                    world.schedule_fluid_tick(fluid, *block_pos, tick_delay, TickPriority::Normal);

                    // A plugin replaced the new level: spread from what is there now.
                    if state_id != new_state_id {
                        let Some(set_state) = self.get_effective_props(fluid, state_id) else {
                            return;
                        };
                        new_state = set_state;
                    }
                }

                // Use the new state for spreading
                state_for_spreading = new_state;
            } else {
                if let Some(state_id) =
                    fire_level_change(world, block_pos, Block::AIR.default_state.id)
                {
                    world.set_block_state(block_pos, state_id, BlockFlags::NOTIFY_ALL);
                }
                return; // Don't spread if fluid is gone
            }
        } else {
            // Sources use their current state
            state_for_spreading = current_fluid_state;
        }

        // Then, spread using the appropriate state
        self.try_flow(world, fluid, block_pos, &state_for_spreading);
    }

    /// Vanilla `FlowingFluid.spread`: flows down into the block below when that block takes the
    /// fluid, and to the sides as well next to three sources. Otherwise a source, or a fluid
    /// with no hole below, flows to the sides.
    fn try_flow(
        &self,
        world: &Arc<World>,
        fluid: &Fluid,
        block_pos: &BlockPos,
        props: &FlowingFluidProperties,
    ) {
        let below_pos = block_pos.down();
        let below_state_id = world.get_block_state_id(&below_pos);
        if self.can_maybe_pass_through(fluid, below_state_id)
            && physics::can_be_replaced_with(world, &below_pos, fluid, BlockDirection::Down)
            && let Some(new_below) = self.get_new_liquid(world, fluid, &below_pos)
            && physics::can_hold_specific_fluid(below_state_id, self.fluid_type(fluid, &new_below))
        {
            self.spread_to(
                world,
                fluid,
                block_pos,
                &below_pos,
                BlockDirection::Down,
                new_below.to_state_id(fluid),
            );
            if self.count_source_neighbors(world, fluid, block_pos) >= 3 {
                self.flow_to_sides(world, fluid, block_pos, props);
            }
            return;
        }

        let is_source = props.level == Level::L8 && props.falling == Falling::False;
        if is_source || !self.is_water_hole(fluid, below_state_id) {
            self.flow_to_sides(world, fluid, block_pos, props);
        }
    }

    fn count_source_neighbors(
        &self,
        world: &Arc<World>,
        fluid: &Fluid,
        block_pos: &BlockPos,
    ) -> i32 {
        let mut count = 0;
        for direction in [
            BlockDirection::North,
            BlockDirection::South,
            BlockDirection::West,
            BlockDirection::East,
        ] {
            let neighbor_pos = block_pos.offset(direction.to_offset());
            let neighbor_id = world.get_block_state_id(&neighbor_pos);
            if self
                .get_effective_props(fluid, neighbor_id)
                .is_some_and(|p| p.level == Level::L8 && p.falling == Falling::False)
            {
                count += 1;
            }
        }
        count
    }

    /// Calculates the new fluid state for a position based on neighbors and environment.
    ///
    /// Priority order:
    /// 1. Sources remain unchanged
    /// 2. Infinite source formation (2+ adjacent sources + solid/source below)
    /// 3. Fluid above forces falling state (level 8, falling)
    /// 4. Standard flow calculation from highest neighbor minus dropoff
    ///
    /// # Returns
    /// New fluid properties, or None if fluid should drain
    fn get_new_liquid(
        &self,
        world: &Arc<World>,
        fluid: &Fluid,
        block_pos: &BlockPos,
    ) -> Option<FlowingFluidProperties> {
        let current_state_id = world.get_block_state_id(block_pos);
        let current_props = FlowingFluidProperties::from_state_id(current_state_id, fluid);

        // Sources never change
        if current_props.level == Level::L8 && current_props.falling != Falling::True {
            return Some(current_props);
        }

        // First: check horizontal neighbors for infinite source formation
        let mut highest_neighbor = 0;
        let mut neighbor_source_count = 0;
        for direction in [
            BlockDirection::North,
            BlockDirection::South,
            BlockDirection::West,
            BlockDirection::East,
        ] {
            let neighbor_pos = block_pos.offset(direction.to_offset());
            let neighbor_state_id = world.get_block_state_id(&neighbor_pos);
            let Some(neighbor_props) = self.get_effective_props(fluid, neighbor_state_id) else {
                continue;
            };

            // Count horizontal non-falling sources for infinite source formation
            if neighbor_props.level == Level::L8 && neighbor_props.falling == Falling::False {
                neighbor_source_count += 1;
            }

            // Falling water from the side counts as level 8
            let neighbor_level = if neighbor_props.falling == Falling::True {
                8
            } else {
                i32::from(neighbor_props.level.to_index()) + 1
            };

            highest_neighbor = highest_neighbor.max(neighbor_level);
        }

        // Attempt infinite source formation first
        if self.can_convert_to_source(world) && neighbor_source_count >= 2 {
            let below_pos = block_pos.down();
            let below_state = world.get_block_state(&below_pos);
            let below_state_id = below_state.id;

            // Check if block below is a stable source of the same fluid
            let below_is_same_source = self
                .get_effective_props(fluid, below_state_id)
                .is_some_and(|p| p.level == Level::L8 && p.falling == Falling::False);

            // If the block below is solid (solid block) or a source of same fluid, form a source here.
            if below_is_same_source || below_state.is_solid_block() {
                return Some(self.get_source(fluid, false));
            }
            // Otherwise continue to standard falling/flowing logic
        }

        // Then: if there's water above, this block is ALWAYS level 8, falling=true
        let above_pos = block_pos.up();
        let above_state_id = world.get_block_state_id(&above_pos);

        if self.has_fluid_at(fluid, above_state_id) {
            return Some(self.get_flowing(fluid, Level::L8, true));
        }

        // Standard flowing calculation
        let drop_off = self.get_level_decrease_per_block(world);
        let new_level = highest_neighbor - drop_off;

        if new_level <= 0 {
            None
        } else {
            Some(self.get_flowing(fluid, Level::from_index(new_level as u16 - 1), false))
        }
    }

    /// Core spread logic with quiescence checks and state updates.
    ///
    /// Implements:
    /// - Quiescence: prevents unnecessary updates (e.g., source blocks, lower levels)
    /// - Infinite source formation checks (before and after placement)
    /// - Block replacement for non-fluid blocks, waterlogging for `LiquidBlockContainer`s
    /// - Fluid tick scheduling for non-source blocks
    ///
    /// Called by `spread_to` implementations after fluid-specific pre-checks.
    fn apply_spread(
        &self,
        world: &Arc<World>,
        fluid: &Fluid,
        from_pos: &BlockPos,
        pos: &BlockPos,
        state_id: BlockStateId,
        new_props: FlowingFluidProperties,
    ) {
        let current_state_id = world.get_block_state_id(pos);
        let mut replace_block = false;
        if let Some(current_props) = self.get_effective_props(fluid, current_state_id) {
            let current_level = i32::from(current_props.level.to_index()) + 1;
            let new_level = i32::from(new_props.level.to_index()) + 1;
            let current_is_source =
                current_props.level == Level::L8 && current_props.falling == Falling::False;
            let new_is_source = new_props.level == Level::L8 && new_props.falling == Falling::False;

            // Never overwrite a source with anything
            if current_is_source {
                return;
            }

            // Check for infinite source formation before quiescence checks
            if !current_is_source && self.can_convert_to_source(world) {
                let should_convert = self.check_infinite_source_formation(world, fluid, pos);

                if should_convert {
                    let source_props = self.get_source(fluid, false);
                    let source_state_id = source_props.to_state_id(fluid);
                    world.set_block_state(pos, source_state_id, BlockFlags::NOTIFY_ALL);

                    // Sources don't need ticks
                    return;
                }
            }

            // If new is a source, always accept it
            if new_is_source {
                // Continue to set state below
            } else if current_props.falling == new_props.falling {
                // Same falling state - check level
                if new_level <= current_level {
                    return;
                }
            }
        } else {
            // Replace non-fluid blocks
            replace_block = world.get_block(pos).id != Block::AIR.id;
        }

        let block = if fluid.id == Fluid::FLOWING_WATER.id || fluid.id == Fluid::WATER.id {
            &Block::WATER
        } else {
            &Block::LAVA
        };
        let mut event = crate::plugin::api::events::block::block_from_to::BlockFromToEvent::new(
            world.clone(),
            *from_pos,
            *pos,
            block,
        );
        if let Some(server) = world.server.upgrade() {
            server.plugin_manager.fire_blocking(&server, &mut event);
        }
        if event.cancelled {
            return;
        }

        // Vanilla `LiquidBlockContainer.placeLiquid`: the block takes the fluid in, if it can, and
        // is never replaced.
        if physics::is_liquid_block_container(current_state_id.to_block()) {
            if world.get_block_state_id(pos) == current_state_id {
                physics::place_liquid(
                    world,
                    pos,
                    current_state_id,
                    self.fluid_type(fluid, &new_props),
                );
            }
            return;
        }

        // Break only the block we observed, after the event (vanilla's beforeDestroyingBlock
        // order): a handler may have replaced it during dispatch.
        if replace_block && world.get_block_state_id(pos) == current_state_id {
            if block == &Block::LAVA {
                // Vanilla `LavaFluid.beforeDestroyingBlock`: the lava fizzes, nothing drops.
                world.sync_world_event(WorldEvent::LavaFizz, *pos, 0);
                world.break_block(pos, None, BlockFlags::NOTIFY_ALL | BlockFlags::SKIP_DROPS);
            } else {
                world.break_block(pos, None, BlockFlags::NOTIFY_ALL);
            }
        }

        world.set_block_state(pos, state_id, BlockFlags::NOTIFY_ALL);

        // Check for infinite source formation after placing new fluid
        if self.can_convert_to_source(world) {
            let should_convert = self.check_infinite_source_formation(world, fluid, pos);

            if should_convert {
                let source_props = self.get_source(fluid, false);
                let source_state_id = source_props.to_state_id(fluid);
                world.set_block_state(pos, source_state_id, BlockFlags::NOTIFY_ALL);

                // Sources don't need ticks
                return;
            }
        }

        // Only schedule tick if not a source
        let is_source = new_props.level == Level::L8 && new_props.falling == Falling::False;

        if !is_source {
            let tick_delay = self.get_flow_speed(world);
            world.schedule_fluid_tick(fluid, *pos, tick_delay, TickPriority::Normal);
        }
    }

    /// Checks if infinite source formation conditions are met.
    ///
    /// Requirements:
    /// - 2+ horizontally adjacent source blocks (level 8, non-falling)
    /// - Block below is either solid OR a source of the same fluid
    ///
    /// # Returns
    /// `true` if position should convert to a source block
    fn check_infinite_source_formation(
        &self,
        world: &Arc<World>,
        fluid: &Fluid,
        pos: &BlockPos,
    ) -> bool {
        // Count adjacent horizontal source blocks
        let mut source_count = 0;
        for direction in [
            BlockDirection::North,
            BlockDirection::South,
            BlockDirection::West,
            BlockDirection::East,
        ] {
            let neighbor_pos = pos.offset(direction.to_offset());
            let neighbor_state_id = world.get_block_state_id(&neighbor_pos);

            if self
                .get_effective_props(fluid, neighbor_state_id)
                .is_some_and(|p| p.level == Level::L8 && p.falling == Falling::False)
            {
                source_count += 1;
            }
        }

        // Need at least 2 source neighbors
        if source_count < 2 {
            return false;
        }

        // Check the block below
        let below_pos = pos.down();
        let below_state = world.get_block_state(&below_pos);
        let below_state_id = below_state.id;

        // Check if block below is a stable source of the same fluid
        let below_is_same_source = self
            .get_effective_props(fluid, below_state_id)
            .is_some_and(|p| p.level == Level::L8 && p.falling == Falling::False);

        // Convert to source if below is solid or a source of same fluid
        below_is_same_source || below_state.is_solid_block()
    }

    /// Spreads fluid to a target position with the given state.
    ///
    /// Default implementation delegates to `apply_spread`. Implementations like
    /// lava can override to add fluid-specific logic (e.g., water -> stone conversion).
    fn spread_to(
        &self,
        world: &Arc<World>,
        fluid: &Fluid,
        from_pos: &BlockPos,
        pos: &BlockPos,
        _direction: BlockDirection,
        state_id: BlockStateId,
    ) {
        let new_props = FlowingFluidProperties::from_state_id(state_id, fluid);
        self.apply_spread(world, fluid, from_pos, pos, state_id, new_props);
    }

    /// Spreads fluid horizontally to adjacent positions using pathfinding.
    ///
    /// Uses `get_spread` to find optimal flow directions (shortest distance to holes)
    /// and the computed fluid state for each target position.
    fn flow_to_sides(
        &self,
        world: &Arc<World>,
        fluid: &Fluid,
        block_pos: &BlockPos,
        props: &FlowingFluidProperties,
    ) {
        let drop_off = self.get_level_decrease_per_block(world);
        let current_level = i32::from(props.level.to_index()) + 1;
        let effective_level = if props.falling == Falling::True {
            7
        } else {
            current_level - drop_off
        };

        if effective_level <= 0 {
            return;
        }

        let (spread_dirs, count) = pathfinder::get_spread(self, world, fluid, block_pos);

        for &(direction, state_id) in spread_dirs.iter().take(count) {
            let side_pos = block_pos.offset(direction.to_offset());

            self.spread_to(world, fluid, block_pos, &side_pos, direction, state_id);
        }
    }
}

/// Fires `FluidLevelChangeEvent` for a flowing fluid that changes its level or dries out.
/// Returns the state to set, or `None` when a plugin cancelled the change.
fn fire_level_change(
    world: &Arc<World>,
    pos: &BlockPos,
    new_state_id: BlockStateId,
) -> Option<BlockStateId> {
    use crate::plugin::api::events::block::fluid_level_change::FluidLevelChangeEvent;

    // Fluid ticks are hot: only build the event when a plugin listens.
    let Some(server) = world.server.upgrade() else {
        return Some(new_state_id);
    };
    if !server
        .plugin_manager
        .has_handlers::<FluidLevelChangeEvent>()
    {
        return Some(new_state_id);
    }
    let mut event = FluidLevelChangeEvent::new(*pos, world.clone(), new_state_id);
    server.plugin_manager.fire_blocking(&server, &mut event);
    (!event.cancelled).then_some(event.new_state_id)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use pumpkin_data::block_properties::GlowLichenProperties;
    use pumpkin_data::{Block, fluid::Fluid};
    use pumpkin_util::math::{position::BlockPos, vector2::Vector2};
    use pumpkin_world::{chunk::ChunkData, world::BlockFlags};

    use super::FlowingFluid;
    use crate::block::fluid::water::FlowingWater;
    use crate::entity::ai::test_support::world;
    use crate::world::World;

    /// Loads chunk 0 0 with a stone floor at y 64, so that no hole is in reach of the flow.
    fn stone_floor(world: &Arc<World>) {
        world
            .level
            .loaded_chunks
            .insert(Vector2::new(0, 0), ChunkData::empty_sync(0, 0));
        for x in 0..16 {
            for z in 0..16 {
                world.set_block_state(
                    &BlockPos::new(x, 64, z),
                    Block::STONE.default_state.id,
                    BlockFlags::empty(),
                );
            }
        }
    }

    #[tokio::test]
    async fn water_does_not_flow_down_into_a_glow_lichen() -> Result<(), Box<dyn std::error::Error>>
    {
        let folder = tempfile::tempdir()?;
        let world = world(&folder);
        stone_floor(&world);
        let lichen = BlockPos::new(8, 65, 8);
        let mut on_floor = GlowLichenProperties::default(&Block::GLOW_LICHEN);
        on_floor.down = true;
        let lichen_state = on_floor.to_state_id(&Block::GLOW_LICHEN);
        world.set_block_state(&lichen, lichen_state, BlockFlags::empty());
        let water = lichen.up();
        world.set_block_state(&water, Block::WATER.default_state.id, BlockFlags::empty());

        FlowingWater.on_scheduled_tick_internal(&world, &Fluid::FLOWING_WATER, &water);

        assert_eq!(world.get_block_state_id(&lichen), lichen_state);
        world.level.shutdown().await;
        Ok(())
    }

    #[tokio::test]
    async fn water_source_between_two_sources_waterlogs_a_slab()
    -> Result<(), Box<dyn std::error::Error>> {
        let folder = tempfile::tempdir()?;
        let world = world(&folder);
        stone_floor(&world);
        let slab = BlockPos::new(8, 65, 8);
        let dry_slab = Block::OAK_SLAB.default_state.id;
        world.set_block_state(&slab, dry_slab, BlockFlags::empty());
        let sources = [BlockPos::new(7, 65, 8), BlockPos::new(9, 65, 8)];
        for source in &sources {
            world.set_block_state(source, Block::WATER.default_state.id, BlockFlags::empty());
        }

        FlowingWater.on_scheduled_tick_internal(&world, &Fluid::FLOWING_WATER, &sources[0]);

        assert_eq!(
            Some(world.get_block_state_id(&slab)),
            Block::OAK_SLAB.set_waterlogged(dry_slab, true)
        );
        world.level.shutdown().await;
        Ok(())
    }
}
