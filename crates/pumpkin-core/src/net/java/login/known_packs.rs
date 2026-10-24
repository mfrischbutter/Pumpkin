#[allow(clippy::wildcard_imports)]
use super::*;
use pumpkin_data::registry::Registry;
use pumpkin_data::tag::RegistryKey;

impl PendingConnection {
    pub async fn handle_known_packs(&mut self, server: &Server) {
        for reg in synced_registries(server) {
            self.send_packet_now(&CRegistryData::new(&reg.registry_id, &reg.registry_entries))
                .await;
        }
        self.send_packet_now(&CUpdateTags::new(&synced_tag_registries()))
            .await;
        self.send_packet_now(&CFinishConfig).await;
    }
}

/// The synced registries as the configuration phase sends them, with datapack and plugin
/// entries merged in.
#[must_use]
pub fn synced_registries(server: &Server) -> Vec<Registry> {
    Registry::get_synced(CURRENT_MC_VERSION)
        .into_iter()
        .map(|reg| {
            let registry_entries = if reg.registry_id == "minecraft:damage_type" {
                server
                    .datapack_manager
                    .merge_damage_type_entries(&reg.registry_entries)
            } else {
                server
                    .datapack_manager
                    .merge_registry_entries(&reg.registry_id, &reg.registry_entries)
            };
            Registry {
                registry_id: reg.registry_id,
                registry_entries,
            }
        })
        .collect()
}

/// The network registries that have tags to send in `CUpdateTags`.
#[must_use]
pub fn synced_tag_registries() -> Vec<RegistryKey> {
    let mut tags = Vec::new();
    for &key in RegistryKey::NETWORK_KEYS {
        if pumpkin_data::tag::get_registry_key_tags(CURRENT_MC_VERSION, key)
            .is_some_and(|map| !map.is_empty())
        {
            tags.push(key);
        }
    }
    tags
}
