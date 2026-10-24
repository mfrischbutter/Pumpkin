#[allow(clippy::wildcard_imports)]
use super::*;
use crate::net::java::pending::PendingResourcePack;
use crate::server::LoginResourcePack;

impl PendingConnection {
    pub async fn handle_login_acknowledged(
        &mut self,
        server: &Server,
    ) -> Option<PacketHandlerResult> {
        debug!("Handling login acknowledgement");
        // Only valid once `CLoginSuccess` went out. Without this the claimed
        // profile from `SLoginStart` reaches the config phase, and from there the
        // play phase, without ever being authenticated.
        if !self.login_success_sent {
            self.kick(TextComponent::text("Unexpected login acknowledgement"))
                .await;
            return Some(PacketHandlerResult::Stop);
        }
        self.connection_state.store(ConnectionState::Config);
        self.send_packet_now(&server.get_branding()).await;

        if server.advanced_config.server_links.enabled {
            let mut links: Vec<Link> = Vec::new();

            let bug_report = &server.advanced_config.server_links.bug_report;
            if !bug_report.is_empty() {
                links.push(Link::new(Label::BuiltIn(LinkType::BugReport), bug_report));
            }

            let support = &server.advanced_config.server_links.support;
            if !support.is_empty() {
                links.push(Link::new(Label::BuiltIn(LinkType::Support), support));
            }

            let status = &server.advanced_config.server_links.status;
            if !status.is_empty() {
                links.push(Link::new(Label::BuiltIn(LinkType::Status), status));
            }

            let feedback = &server.advanced_config.server_links.feedback;
            if !feedback.is_empty() {
                links.push(Link::new(Label::BuiltIn(LinkType::Feedback), feedback));
            }

            let community = &server.advanced_config.server_links.community;
            if !community.is_empty() {
                links.push(Link::new(Label::BuiltIn(LinkType::Community), community));
            }

            let website = &server.advanced_config.server_links.website;
            if !website.is_empty() {
                links.push(Link::new(Label::BuiltIn(LinkType::Website), website));
            }

            let forums = &server.advanced_config.server_links.forums;
            if !forums.is_empty() {
                links.push(Link::new(Label::BuiltIn(LinkType::Forums), forums));
            }

            let news = &server.advanced_config.server_links.news;
            if !news.is_empty() {
                links.push(Link::new(Label::BuiltIn(LinkType::News), news));
            }

            let announcements = &server.advanced_config.server_links.announcements;
            if !announcements.is_empty() {
                links.push(Link::new(
                    Label::BuiltIn(LinkType::Announcements),
                    announcements,
                ));
            }

            for (key, value) in &server.advanced_config.server_links.custom {
                links.push(Link::new(
                    Label::TextComponent(TextComponent::text(key.clone()).into()),
                    value,
                ));
            }

            self.send_packet_now(&CConfigServerLinks::new(&links)).await;
        }

        let packs = Self::configuration_resource_packs(server);
        if packs.is_empty() {
            self.send_known_packs(server).await;
        }
        for (pack, from_config) in packs {
            self.send_packet_now(&CConfigAddResourcePack::new(
                &pack.uuid,
                &pack.url,
                &pack.hash,
                pack.forced,
                pack.prompt_message,
            ))
            .await;
            self.pending_resource_packs.push(PendingResourcePack {
                uuid: pack.uuid,
                forced: pack.forced,
                from_config,
            });
        }
        debug!("login acknowledged");
        None
    }

    /// The `[resource_pack.java]` pack, then the packs plugins set, each paired with whether it
    /// comes from the config. A UUID that is already taken is sent once.
    fn configuration_resource_packs(server: &Server) -> Vec<(LoginResourcePack, bool)> {
        let mut packs = Vec::new();
        let resource_config = &server.advanced_config.resource_pack.java;
        if resource_config.enabled {
            let config_pack = LoginResourcePack {
                uuid: resource_config.uuid(),
                url: resource_config.url.clone(),
                hash: resource_config.sha1.clone(),
                forced: resource_config.force,
                prompt_message: (!resource_config.prompt_message.is_empty())
                    .then(|| TextComponent::text(resource_config.prompt_message.clone())),
            };
            packs.push((config_pack, true));
        }
        let plugin_packs = server.login_resource_packs.load_full();
        for pack in plugin_packs.iter().flat_map(|(_, list)| list) {
            if packs.iter().any(|(sent, _)| sent.uuid == pack.uuid) {
                debug!(
                    "Skipping login resource pack {}: UUID already sent",
                    pack.uuid
                );
                continue;
            }
            packs.push((pack.clone(), false));
        }
        packs
    }

    pub async fn send_known_packs(&mut self, server: &Server) {
        let features = server.get_enabled_features();
        self.send_packet_now(&CFeatureFlags::new(&features)).await;
        let version_str = CURRENT_MC_VERSION.to_string();
        let loaded_packs = server.datapack_manager.get_loaded_packs();
        let known_packs = server.get_known_packs(&version_str, &loaded_packs);
        self.send_packet_now(&CKnownPacks::new(&known_packs)).await;
    }
}
