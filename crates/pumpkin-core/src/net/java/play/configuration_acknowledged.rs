#[allow(clippy::wildcard_imports)]
use super::*;

impl JavaClient {
    /// A requested acknowledgement is read by the reconfiguration itself and never gets here.
    pub fn handle_configuration_acknowledged(&self, player: &Player) {
        // Vanilla `handleConfigurationAcknowledged`: an acknowledgement nobody asked for is an error.
        debug!(
            "Player {} acknowledged a configuration switch that was not requested",
            player.gameprofile.name
        );
        self.try_kick(&TextComponent::text(
            "Unexpected configuration acknowledgement",
        ));
    }
}
