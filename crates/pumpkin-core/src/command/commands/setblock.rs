use pumpkin_data::translation;
use pumpkin_util::PermissionLvl;
use pumpkin_util::permission::{Permission, PermissionDefault, PermissionRegistry};
use pumpkin_util::text::TextComponent;
use pumpkin_world::world::BlockFlags;

use super::block_input;
use crate::command::argument_builder::{ArgumentBuilder, argument, command, literal};
use crate::command::argument_types::block::BlockArgumentType;
use crate::command::argument_types::coordinates::block_pos::BlockPosArgumentType;
use crate::command::context::command_context::CommandContext;
use crate::command::errors::error_types::CommandErrorType;
use crate::command::node::dispatcher::CommandDispatcher;
use crate::command::node::{CommandExecutor, CommandExecutorResult};

const DESCRIPTION: &str = "Place a block.";
const PERMISSION: &str = "minecraft:command.setblock";

const ERROR_FAILED: CommandErrorType<0> = CommandErrorType::new(
    translation::java::COMMANDS_SETBLOCK_FAILED,
    translation::java::COMMANDS_SETBLOCK_FAILED,
);

#[derive(Clone, Copy)]
enum Mode {
    /// with particles + item drops
    Destroy,

    /// only replaces air
    Keep,

    /// default; without particles
    Replace,

    /// places block without triggering updates around it
    Strict,
}

struct SetBlockExecutor(Mode);

impl CommandExecutor for SetBlockExecutor {
    fn execute(&self, context: &CommandContext) -> CommandExecutorResult {
        let block = BlockArgumentType::get(context, "block")?;
        let mode = self.0;
        let world = context.source.world();
        let pos = BlockPosArgumentType::get_loaded_block_pos(context, "pos")?;

        if matches!(mode, Mode::Keep) && !world.get_block_state(&pos).is_air() {
            return Err(ERROR_FAILED.create_without_context());
        }

        let place_needed = if matches!(mode, Mode::Destroy) {
            // Vanilla `Level.destroyBlock` with its drops: no player, so a plain break.
            world.break_block(&pos, None, BlockFlags::NOTIFY_ALL);
            !block.state_id.to_state().is_air() || !world.get_block_state(&pos).is_air()
        } else {
            true
        };

        let strict = matches!(mode, Mode::Strict);
        let old_state_id = world.get_block_state_id(&pos);
        if place_needed
            && !block_input::place(world, &pos, &block, block_input::place_flags(strict))
        {
            world.flush_block_updates();
            return Err(ERROR_FAILED.create_without_context());
        }
        if !strict {
            block_input::update_neighbours_on_block_set(world, &pos, old_state_id);
        }

        world.flush_block_updates();
        context.source.send_feedback(
            TextComponent::translate_cross(
                pumpkin_data::translation::java::COMMANDS_SETBLOCK_SUCCESS,
                pumpkin_data::translation::bedrock::COMMANDS_SETBLOCK_SUCCESS,
                [
                    TextComponent::text(pos.0.x.to_string()),
                    TextComponent::text(pos.0.y.to_string()),
                    TextComponent::text(pos.0.z.to_string()),
                ],
            ),
            true,
        );
        Ok(1)
    }
}

pub fn register(dispatcher: &mut CommandDispatcher, registry: &PermissionRegistry) {
    registry.register_permission_or_panic(Permission::new(
        PERMISSION,
        DESCRIPTION,
        PermissionDefault::Op(PermissionLvl::Two),
    ));

    dispatcher.register(
        command("setblock", DESCRIPTION).requires(PERMISSION).then(
            argument("pos", BlockPosArgumentType).then(
                argument("block", BlockArgumentType)
                    .executes(SetBlockExecutor(Mode::Replace))
                    .then(literal("destroy").executes(SetBlockExecutor(Mode::Destroy)))
                    .then(literal("keep").executes(SetBlockExecutor(Mode::Keep)))
                    .then(literal("replace").executes(SetBlockExecutor(Mode::Replace)))
                    .then(literal("strict").executes(SetBlockExecutor(Mode::Strict))),
            ),
        ),
    );
}
