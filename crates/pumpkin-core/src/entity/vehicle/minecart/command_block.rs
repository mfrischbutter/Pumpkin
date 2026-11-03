//! Vanilla `MinecartCommandBlock` with the `BaseCommandBlock` it holds.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicI64, Ordering};

use pumpkin_data::tracked_data::command_block_minecart::{ID_COMMAND_NAME, ID_LAST_OUTPUT};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector2::Vector2;
use pumpkin_util::text::TextComponent;

use crate::block::entities::command_block::CommandBlockEntity;
use crate::command::CommandSender;
use crate::command::context::command_source::CommandSource;
use crate::entity::{Entity, EntityBase};

/// Vanilla `MinecartCommandBlock.ACTIVATION_DELAY`.
const ACTIVATION_DELAY: i32 = 4;
/// Vanilla's `lastExecution` when it has none.
const NO_EXECUTION: i64 = -1;

pub(super) struct CommandBlockMinecart {
    /// The command, output and success count. Its block position stays unused, the command
    /// source gets the cart's position.
    command_block: Arc<CommandBlockEntity>,
    last_activated: AtomicI32,
    last_execution: AtomicI64,
    update_last_execution: AtomicBool,
}

impl CommandBlockMinecart {
    pub(super) fn new() -> Self {
        Self {
            command_block: Arc::new(CommandBlockEntity::new(BlockPos::new(0, 0, 0), true, false)),
            last_activated: AtomicI32::new(0),
            last_execution: AtomicI64::new(NO_EXECUTION),
            update_last_execution: AtomicBool::new(true),
        }
    }

    fn command(&self) -> String {
        self.command_block
            .command
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    fn set_last_output(&self, output: String) {
        *self
            .command_block
            .last_output
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = output;
    }

    fn last_output(&self) -> String {
        self.command_block
            .last_output
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    pub(super) fn success_count(&self) -> u32 {
        self.command_block.success_count.load(Ordering::SeqCst)
    }

    /// Vanilla `MinecartCommandBase.onUpdated`.
    pub(super) fn sync_entity_data(&self, entity: &Entity) {
        entity.set_synced_data(ID_COMMAND_NAME, self.command());
        entity.set_synced_data(ID_LAST_OUTPUT, TextComponent::text(self.last_output()));
    }

    /// The fields vanilla `handleSetCommandMinecart` sets.
    pub(super) fn set_command(&self, command: &str, track_output: bool) {
        *self
            .command_block
            .command
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = command.to_string();
        self.command_block
            .track_output
            .store(track_output, Ordering::SeqCst);
        if !track_output {
            self.set_last_output(String::new());
        }
    }

    /// Vanilla `MinecartCommandBlock.activateMinecart`.
    pub(super) fn activate(&self, entity: &Entity, powered: bool) {
        let tick_count = entity.tick_count.load(Ordering::Relaxed);
        if powered && tick_count - self.last_activated.load(Ordering::Relaxed) >= ACTIVATION_DELAY {
            self.perform_command(entity);
            self.last_activated.store(tick_count, Ordering::Relaxed);
        }
    }

    /// Vanilla `BaseCommandBlock.performCommand`.
    fn perform_command(&self, entity: &Entity) {
        let world = entity.world.load_full();
        let game_time = world.get_world_age();
        if game_time == self.last_execution.load(Ordering::Relaxed) {
            return;
        }
        let command = self.command();
        if command.eq_ignore_ascii_case("Searge") {
            self.set_last_output("#itzlipofutzli".to_string());
            self.command_block.success_count.store(1, Ordering::SeqCst);
            self.sync_entity_data(entity);
            return;
        }
        self.command_block.success_count.store(0, Ordering::SeqCst);
        if world.level_info.load().game_rules.command_blocks_work && !command.is_empty() {
            self.set_last_output(String::new());
            self.run(entity, &world, &command);
        }
        self.last_execution.store(
            if self.update_last_execution.load(Ordering::Relaxed) {
                game_time
            } else {
                NO_EXECUTION
            },
            Ordering::Relaxed,
        );
        self.sync_entity_data(entity);
    }

    /// Vanilla `MinecartCommandBase.createCommandSourceStack`: at the cart, as the cart, and
    /// `performPrefixedCommand`, which drops a leading slash.
    fn run(&self, entity: &Entity, world: &Arc<crate::world::World>, command: &str) {
        let Some(server) = world.server.upgrade() else {
            return;
        };
        let Some(minecart) = world.get_entity_by_id(entity.entity_id) else {
            return;
        };
        let source = CommandSource::new(
            CommandSender::CommandBlock(self.command_block.clone(), world.clone()),
            world.clone(),
            Some(minecart.clone()),
            entity.pos.load(),
            Vector2::new(entity.pitch.load(), entity.yaw.load()),
            minecart.get_name().get_text(),
            minecart.get_display_name(),
            server.clone(),
        );
        server
            .command_dispatcher
            .load()
            .handle_command(&source, command.strip_prefix('/').unwrap_or(command));
    }

    /// Vanilla `BaseCommandBlock.save`. The custom name is saved by the entity itself.
    pub(super) fn write_nbt(&self, nbt: &mut NbtCompound) {
        nbt.put_string("Command", self.command());
        nbt.put_int("SuccessCount", self.success_count() as i32);
        let track_output = self.command_block.track_output.load(Ordering::SeqCst);
        nbt.put_bool("TrackOutput", track_output);
        let last_output = self.last_output();
        if track_output && !last_output.is_empty() {
            nbt.put_string("LastOutput", last_output);
        }
        let update = self.update_last_execution.load(Ordering::Relaxed);
        nbt.put_bool("UpdateLastExecution", update);
        let last_execution = self.last_execution.load(Ordering::Relaxed);
        if update && last_execution != NO_EXECUTION {
            nbt.put_long("LastExecution", last_execution);
        }
    }

    /// Vanilla `BaseCommandBlock.load` and what `MinecartCommandBlock` syncs after it.
    pub(super) fn read_nbt(&self, entity: &Entity, nbt: &NbtCompound) {
        let track_output = nbt.get_bool("TrackOutput").unwrap_or(true);
        self.set_command(nbt.get_string("Command").unwrap_or(""), track_output);
        self.command_block.success_count.store(
            nbt.get_int("SuccessCount").unwrap_or(0) as u32,
            Ordering::SeqCst,
        );
        if track_output {
            self.set_last_output(nbt.get_string("LastOutput").unwrap_or("").to_string());
        }
        let update = nbt.get_bool("UpdateLastExecution").unwrap_or(true);
        self.update_last_execution.store(update, Ordering::Relaxed);
        self.last_execution.store(
            if update {
                nbt.get_long("LastExecution").unwrap_or(NO_EXECUTION)
            } else {
                NO_EXECUTION
            },
            Ordering::Relaxed,
        );
        self.sync_entity_data(entity);
    }
}
