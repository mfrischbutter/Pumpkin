#[allow(clippy::wildcard_imports)]
use super::*;
use pumpkin_protocol::java::server::config::ResourcePackResponseResult;
use pumpkin_protocol::java::server::play::{PlayResourcePackResult, SPlayResourcePack};
use pumpkin_util::text::TextComponent;
use uuid::Uuid;

use crate::plugin::api::events::player::player_resource_pack_status::PlayerResourcePackStatusEvent;

impl JavaClient {
    pub fn handle_play_resource_pack_response(
        &self,
        server: &Arc<Server>,
        player: &Arc<Player>,
        packet: &SPlayResourcePack,
    ) {
        let result = packet.response_result();
        debug!(
            "Player {} resource pack response for {}: {:?}",
            player.gameprofile.name, packet.uuid, result
        );

        let mut event = PlayerResourcePackStatusEvent::new(
            player.clone(),
            packet.uuid.to_string(),
            format!("{result:?}"),
        );
        server.plugin_manager.fire_blocking(server, &mut event);

        // Only the server's own pack; plugins choose `forced` for the packs they push.
        let resource_config = &server.advanced_config.resource_pack.java;
        if resource_config.enabled
            && resource_config.force
            && packet.uuid == resource_config.uuid()
            && (result == PlayResourcePackResult::Declined
                || result == PlayResourcePackResult::DownloadFail)
        {
            self.try_kick(&TextComponent::text(
                "You must accept the resource pack to play on this server.",
            ));
        }
    }
}

/// Fires the resource pack statuses the client sent in the configuration phase, once the player
/// exists. The configuration phase already acted on them, so cancelling changes nothing.
pub async fn fire_configuration_resource_pack_statuses(
    server: &Arc<Server>,
    player: &Arc<Player>,
    statuses: Vec<(Uuid, ResourcePackResponseResult)>,
) {
    if statuses.is_empty()
        || !server
            .plugin_manager
            .has_handlers::<PlayerResourcePackStatusEvent>()
    {
        return;
    }
    for (uuid, result) in statuses {
        let mut event = PlayerResourcePackStatusEvent::new(
            player.clone(),
            uuid.to_string(),
            format!("{result:?}"),
        );
        server.plugin_manager.fire(server, &mut event).await;
    }
}
