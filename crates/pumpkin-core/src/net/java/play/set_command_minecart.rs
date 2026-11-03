#[allow(clippy::wildcard_imports)]
use super::*;
use crate::entity::vehicle::minecart::MinecartEntity;
use pumpkin_data::translation;
use pumpkin_protocol::java::server::play::SSetCommandMinecart;

impl JavaClient {
    /// Vanilla `ServerGamePacketListenerImpl.handleSetCommandMinecart`.
    pub fn handle_set_command_minecart(&self, player: &Player, packet: &SSetCommandMinecart<'_>) {
        if !player.can_use_game_master_blocks() {
            player.send_system_message(&TextComponent::translate(
                translation::java::ADVMODE_NOTALLOWED,
                [],
            ));
            return;
        }

        let world = player.world();
        let Some(entity) = world.get_entity_by_id(packet.entity_id.0) else {
            return;
        };
        let enabled = world.level_info.load().game_rules.command_blocks_work;
        let is_command_minecart = entity
            .cast_any()
            .downcast_ref::<MinecartEntity>()
            .is_some_and(|minecart| {
                minecart.set_command(packet.command, packet.track_output, enabled)
            });
        if is_command_minecart && !packet.command.is_empty() {
            player.send_system_message(&TextComponent::translate(
                if enabled {
                    translation::java::ADVMODE_SETCOMMAND_SUCCESS
                } else {
                    translation::java::ADVMODE_SETCOMMAND_DISABLED
                },
                [TextComponent::text(packet.command.to_string())],
            ));
        }
    }
}
