#[allow(clippy::wildcard_imports)]
use super::*;
use pumpkin_protocol::java::client::config::{
    CFeatureFlags, CFinishConfig, CKnownPacks, CRegistryData, CUpdateTags,
};
use pumpkin_protocol::java::client::play::CStartConfiguration;
use pumpkin_protocol::java::server::config::{
    SAcceptCodeOfConduct, SAcknowledgeFinishConfig, SClientInformationConfig,
    SConfigCookieResponse, SConfigPong, SConfigResourcePack, SCustomClickAction, SKeepAlive,
    SKnownPacks, SPluginMessage,
};

use super::login::known_packs::{synced_registries, synced_tag_registries};

/// How long a client may stay silent while it is configured again.
const RECONFIGURATION_IDLE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

impl JavaClient {
    /// Asks to send the client back to the configuration phase, which resends registries and
    /// tags. The player leaves the world and joins it again afterwards, like vanilla
    /// `ServerGamePacketListenerImpl.switchToConfig`.
    ///
    /// Returns `false` when the client is not in play, is already being configured again, or
    /// speaks an older protocol through the packet events.
    pub fn request_reconfiguration(&self) -> bool {
        if self.version.load() != CURRENT_MC_VERSION
            || self.connection_state.load() != ConnectionState::Play
            || self.reconfiguring.swap(true, Ordering::AcqRel)
        {
            return false;
        }
        self.reconfigure_requested.notify_one();
        true
    }

    /// Whether the outbound side is in the configuration phase. Play packets queued then would
    /// be read as configuration packets by the client, so the play send paths drop them.
    pub(super) fn is_configuring(&self) -> bool {
        self.connection_state.load() == ConnectionState::Config
    }

    /// Runs the configuration phase again after [`Self::progress_player_packets`] returned
    /// [`PlayPhaseEnd::Reconfigure`] and the player was taken out of the world.
    ///
    /// Returns `false` when the connection closed or failed on the way.
    pub async fn reconfigure(&self, server: &Server) -> bool {
        let reader = self
            .network_reader
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        let Some(mut network_reader) = reader else {
            return false;
        };
        let configured = self
            .run_reconfiguration(server, &mut network_reader)
            .await
            .is_some();
        *self
            .network_reader
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(network_reader);

        // Keep-alives sent before the switch are never answered.
        self.pending_keep_alives
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
        self.wait_for_keep_alive.store(false, Ordering::Relaxed);
        self.last_packet_time.store(Instant::now());
        self.reconfiguring.store(false, Ordering::Release);
        configured
    }

    async fn run_reconfiguration(
        &self,
        server: &Server,
        network_reader: &mut TCPNetworkDecoder<BufReader<OwnedReadHalf>>,
    ) -> Option<()> {
        let version = CURRENT_MC_VERSION;

        self.send_packet(&CStartConfiguration).await;
        // Vanilla switches the outbound protocol right after this packet.
        self.connection_state.store(ConnectionState::Config);

        // Play packets the client sent before it got the switch are dropped: the player is
        // no longer in a world.
        loop {
            let packet = self.next_configuration_packet(server, network_reader).await?;
            if packet.id == SConfigurationAcknowledged::to_id(version) {
                break;
            }
        }
        debug!(
            "Player {} acknowledged the configuration switch",
            self.gameprofile.name
        );

        // Vanilla `ServerConfigurationPacketListenerImpl.startConfiguration` without the brand,
        // server links and resource packs, which the client keeps across the switch.
        let features = server.get_enabled_features();
        self.send_config_packet(&CFeatureFlags::new(&features))
            .await;
        let version_str = version.to_string();
        let loaded_packs = server.datapack_manager.get_loaded_packs();
        let known_packs = server.get_known_packs(&version_str, &loaded_packs);
        self.send_config_packet(&CKnownPacks::new(&known_packs))
            .await;

        loop {
            let packet = self.next_configuration_packet(server, network_reader).await?;
            match packet.id {
                id if id == SKnownPacks::to_id(version) => {
                    for registry in synced_registries(server) {
                        self.send_config_packet(&CRegistryData::new(
                            &registry.registry_id,
                            &registry.registry_entries,
                        ))
                        .await;
                    }
                    self.send_config_packet(&CUpdateTags::new(&synced_tag_registries()))
                        .await;
                    self.send_config_packet(&CFinishConfig).await;
                }
                id if id == SAcknowledgeFinishConfig::to_id(version) => {
                    self.connection_state.store(ConnectionState::Play);
                    return Some(());
                }
                // Client settings, brand and pack statuses keep what the first configuration set.
                id if id == SClientInformationConfig::to_id(version)
                    || id == SPluginMessage::to_id(version)
                    || id == SConfigResourcePack::to_id(version)
                    || id == SConfigCookieResponse::to_id(version)
                    || id == SKeepAlive::to_id(version)
                    || id == SConfigPong::to_id(version)
                    || id == SAcceptCodeOfConduct::to_id(version)
                    || id == SCustomClickAction::to_id(version) => {}
                id => {
                    self.kick(TextComponent::text(format!(
                        "Failed to handle packet id {id} in Config State"
                    )))
                    .await;
                    return None;
                }
            }
        }
    }

    /// The next packet while the client is configured again. `None` when the connection closed.
    async fn next_configuration_packet(
        &self,
        server: &Server,
        network_reader: &mut TCPNetworkDecoder<BufReader<OwnedReadHalf>>,
    ) -> Option<RawPacket> {
        let Ok(packet) = tokio::time::timeout(
            RECONFIGURATION_IDLE_TIMEOUT,
            self.get_packet_with_reader(network_reader),
        )
        .await
        else {
            self.kick(pumpkin_macros::translate_cross!(
                translation::java::DISCONNECT_TIMEOUT,
                translation::bedrock::DISCONNECT_TIMEOUT
            ))
            .await;
            return None;
        };
        let packet = packet?;
        self.last_packet_time.store(Instant::now());
        if !self.packet_limiter.check_packet() {
            warn!(
                "Client {} ({}) exceeded packet rate limit (rate: {}/s)",
                self.id,
                self.gameprofile.name,
                self.packet_limiter.max_rate()
            );
            self.kick(TextComponent::text(
                server
                    .advanced_config
                    .networking
                    .java
                    .packet_limiter
                    .kick_message
                    .clone(),
            ))
            .await;
            return None;
        }
        Some(packet)
    }

    /// Sends a configuration packet past the play packet gate of [`Self::is_configuring`].
    async fn send_config_packet<P: ClientPacket>(&self, packet: &P) {
        if let Ok(data) = self.serialize_packet(packet) {
            self.send_and_wait(data, OutgoingPacket::high_priority)
                .await;
        }
    }
}
