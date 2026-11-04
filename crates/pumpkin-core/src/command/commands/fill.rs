use std::sync::Arc;

use pumpkin_data::BlockStateId;
use pumpkin_data::translation;
use pumpkin_util::PermissionLvl;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_util::permission::{Permission, PermissionDefault, PermissionRegistry};
use pumpkin_util::text::TextComponent;
use pumpkin_world::lighting::LightEngine;
use pumpkin_world::world::BlockFlags;

use super::block_input;
use crate::command::argument_builder::{
    ArgumentBuilder, RequiredArgumentBuilder, argument, command, literal,
};
use crate::command::argument_types::block::{BlockArgumentType, BlockInput};
use crate::command::argument_types::block_predicate::{BlockPredicate, BlockPredicateArgumentType};
use crate::command::argument_types::coordinates::block_pos::BlockPosArgumentType;
use crate::command::context::command_context::CommandContext;
use crate::command::context::command_source::CommandSource;
use crate::command::errors::command_syntax_error::CommandSyntaxError;
use crate::command::errors::error_types::CommandErrorType;
use crate::command::node::dispatcher::CommandDispatcher;
use crate::command::node::{CommandExecutor, CommandExecutorResult};
use crate::world::World;

const DESCRIPTION: &str = "Fills all or parts of a region with a specific block.";
const PERMISSION: &str = "minecraft:command.fill";

const ERROR_AREA_TOO_LARGE: CommandErrorType<2> = CommandErrorType::new(
    translation::java::COMMANDS_FILL_TOOBIG,
    translation::java::COMMANDS_FILL_TOOBIG,
);

const ERROR_FAILED: CommandErrorType<0> = CommandErrorType::new(
    translation::java::COMMANDS_FILL_FAILED,
    translation::java::COMMANDS_FILL_FAILED,
);

#[derive(Clone, Copy, PartialEq, Eq)]
enum FillMode {
    Replace,
    Outline,
    Hollow,
    Destroy,
    Keep,
}

#[derive(Clone, Copy)]
enum FilterMode {
    None,
    WithFilter,
    KeepAir,
}

/// The box of vanilla `BoundingBox.fromCorners`.
struct Region {
    min: Vector3<i32>,
    max: Vector3<i32>,
}

impl Region {
    fn from_corners(from: BlockPos, to: BlockPos) -> Self {
        Self {
            min: Vector3::new(
                from.0.x.min(to.0.x),
                from.0.y.min(to.0.y),
                from.0.z.min(to.0.z),
            ),
            max: Vector3::new(
                from.0.x.max(to.0.x),
                from.0.y.max(to.0.y),
                from.0.z.max(to.0.z),
            ),
        }
    }

    fn area(&self) -> i64 {
        let x_span = i64::from(self.max.x - self.min.x + 1);
        let y_span = i64::from(self.max.y - self.min.y + 1);
        let z_span = i64::from(self.max.z - self.min.z + 1);
        x_span * y_span * z_span
    }

    const fn is_edge(&self, pos: &BlockPos) -> bool {
        pos.0.x == self.min.x
            || pos.0.x == self.max.x
            || pos.0.y == self.min.y
            || pos.0.y == self.max.y
            || pos.0.z == self.min.z
            || pos.0.z == self.max.z
    }
}

/// One fill's fixed inputs.
struct Fill<'a> {
    world: &'a Arc<World>,
    region: Region,
    target: &'a BlockInput,
    hollow_core: BlockInput,
    mode: FillMode,
    filter: Option<&'a BlockPredicate>,
    flags: BlockFlags,
}

impl Fill<'_> {
    fn passes_filter(&self, pos: &BlockPos, state_id: BlockStateId) -> bool {
        if self.mode == FillMode::Keep {
            return state_id.to_state().is_air();
        }
        self.filter.is_none_or(|filter| {
            filter.test(state_id, || self.world.get_block_entity_nbt(pos))
        })
    }

    /// Vanilla `FillCommand.Mode.filter`.
    fn block_for(&self, pos: &BlockPos) -> Option<&BlockInput> {
        match self.mode {
            FillMode::Outline => self.region.is_edge(pos).then_some(self.target),
            FillMode::Hollow => Some(if self.region.is_edge(pos) {
                self.target
            } else {
                &self.hollow_core
            }),
            FillMode::Replace | FillMode::Destroy | FillMode::Keep => Some(self.target),
        }
    }

    /// One position of vanilla `FillCommand.fillBlocks`. Returns whether it counts as affected.
    fn fill_position(
        &self,
        pos: &BlockPos,
        old_state_id: BlockStateId,
        placed: &mut Vec<(BlockPos, BlockStateId)>,
        light_changes: &mut Vec<BlockPos>,
    ) -> bool {
        // Vanilla `Level.destroyBlock` with its drops: no player, so a plain break.
        let destroyed = self.mode == FillMode::Destroy
            && self
                .world
                .break_block(pos, None, BlockFlags::NOTIFY_ALL)
                .is_some();
        let Some(block) = self.block_for(pos) else {
            return destroyed;
        };
        if !block_input::place(self.world, pos, block, self.flags) {
            return destroyed;
        }
        placed.push((*pos, old_state_id));
        let state_id = self.world.get_block_state_id(pos);
        if LightEngine::has_different_light_properties(old_state_id.to_state(), state_id.to_state())
        {
            light_changes.push(*pos);
        }
        true
    }
}

fn fill_blocks(
    source: &CommandSource,
    from: BlockPos,
    to: BlockPos,
    target: &BlockInput,
    mode: FillMode,
    filter: Option<&BlockPredicate>,
    strict: bool,
) -> Result<i32, CommandSyntaxError> {
    let region = Region::from_corners(from, to);
    let area = region.area();

    let world = source.world().clone();
    let max_block_modifications = {
        let level_info = world.level_info.load();
        level_info.game_rules.max_block_modifications
    };

    if area > max_block_modifications {
        return Err(ERROR_AREA_TOO_LARGE.create_without_context(
            TextComponent::text(max_block_modifications.to_string()),
            TextComponent::text(area.to_string()),
        ));
    }

    let fill = Fill {
        world: &world,
        hollow_core: BlockInput::from_state(BlockStateId::AIR),
        target,
        mode,
        filter,
        // Light spreads once below, not after every block of a big fill.
        flags: block_input::place_flags(strict) | BlockFlags::DEFER_LIGHT_UPDATE,
        region,
    };
    let (min, max) = (fill.region.min, fill.region.max);

    let mut count = 0;
    let mut placed = Vec::new();
    let mut light_changes = Vec::new();

    // Vanilla's `BlockPos.betweenClosed` order: a later block sees the shapes of the earlier ones.
    for z in min.z..=max.z {
        for y in min.y..=max.y {
            for x in min.x..=max.x {
                let pos = BlockPos(Vector3::new(x, y, z));
                let Some(old_state_id) = world.get_block_state_id_if_loaded(&pos) else {
                    continue;
                };
                if fill.passes_filter(&pos, old_state_id)
                    && fill.fill_position(&pos, old_state_id, &mut placed, &mut light_changes)
                {
                    count += 1;
                }
            }
        }
    }

    world
        .level
        .light_engine
        .update_lighting_batch(&world.level, &light_changes);

    if !strict {
        for (pos, old_state_id) in placed {
            block_input::update_neighbours_on_block_set(&world, &pos, old_state_id);
        }
    }
    world.flush_block_updates();

    if count == 0 {
        return Err(ERROR_FAILED.create_without_context());
    }

    source.send_feedback(
        TextComponent::translate_cross(
            translation::java::COMMANDS_FILL_SUCCESS,
            translation::java::COMMANDS_FILL_SUCCESS,
            [TextComponent::text(count.to_string())],
        ),
        true,
    );

    Ok(count)
}

struct FillExecutor {
    mode: FillMode,
    filter_mode: FilterMode,
    strict: bool,
}

impl CommandExecutor for FillExecutor {
    fn execute(&self, context: &CommandContext) -> CommandExecutorResult {
        let from = BlockPosArgumentType::get_loaded_block_pos(context, "from")?;
        let to = BlockPosArgumentType::get_loaded_block_pos(context, "to")?;
        let block = BlockArgumentType::get(context, "block")?;

        let filter = if matches!(self.filter_mode, FilterMode::WithFilter) {
            Some(BlockPredicateArgumentType::get(context, "filter")?)
        } else {
            None
        };

        fill_blocks(
            &context.source,
            from,
            to,
            &block,
            self.mode,
            filter.as_ref(),
            self.strict,
        )
    }
}

fn wrap_with_mode(builder: RequiredArgumentBuilder, has_filter: bool) -> RequiredArgumentBuilder {
    let filter_mode = if has_filter {
        FilterMode::WithFilter
    } else {
        FilterMode::None
    };

    builder
        .executes(FillExecutor {
            mode: FillMode::Replace,
            filter_mode,
            strict: false,
        })
        .then(literal("outline").executes(FillExecutor {
            mode: FillMode::Outline,
            filter_mode,
            strict: false,
        }))
        .then(literal("hollow").executes(FillExecutor {
            mode: FillMode::Hollow,
            filter_mode,
            strict: false,
        }))
        .then(literal("destroy").executes(FillExecutor {
            mode: FillMode::Destroy,
            filter_mode,
            strict: false,
        }))
        .then(literal("strict").executes(FillExecutor {
            mode: FillMode::Replace,
            filter_mode,
            strict: true,
        }))
}

pub fn register(dispatcher: &mut CommandDispatcher, registry: &PermissionRegistry) {
    registry.register_permission_or_panic(Permission::new(
        PERMISSION,
        DESCRIPTION,
        PermissionDefault::Op(PermissionLvl::Two),
    ));

    let filter_arg = wrap_with_mode(argument("filter", BlockPredicateArgumentType), true);

    let replace_literal = literal("replace")
        .executes(FillExecutor {
            mode: FillMode::Replace,
            filter_mode: FilterMode::None,
            strict: false,
        })
        .then(filter_arg);

    let keep_literal = literal("keep").executes(FillExecutor {
        mode: FillMode::Keep,
        filter_mode: FilterMode::KeepAir,
        strict: false,
    });

    let block_arg = wrap_with_mode(argument("block", BlockArgumentType), false)
        .then(replace_literal)
        .then(keep_literal);

    dispatcher.register(
        command("fill", DESCRIPTION).requires(PERMISSION).then(
            argument("from", BlockPosArgumentType)
                .then(argument("to", BlockPosArgumentType).then(block_arg)),
        ),
    );
}
