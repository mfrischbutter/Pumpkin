//! Clientbound Java packets that `generated_packets` cannot serialize, written by hand.

use std::{
    collections::HashSet,
    mem::Discriminant,
    sync::{LazyLock, Mutex, PoisonError},
};

use bytes::Bytes;
use pumpkin_protocol::{
    ClientPacket, NumberFormat,
    codec::var_int::VarInt,
    java::client::play::{
        CPlayerInfoUpdate, CResetScore, CUpdateObjectives, CUpdateScore, InitChat, Player,
        PlayerAction,
    },
};
use pumpkin_util::{text::TextComponent, version::JavaMinecraftVersion};

use crate::{
    pumpkin::plugin::{
        java_packets::{self, ClientboundPacket},
        uuid::Uuid,
    },
    uuid::UuidExt,
};

/// Serializes the packets the generated mapping skips. Score texts are plain text, like in the
/// generated mapping, and a `number-format` string is shown as fixed text.
#[must_use]
pub fn serialize_java_packet(
    packet: &ClientboundPacket,
    version: JavaMinecraftVersion,
) -> Option<Bytes> {
    match packet {
        ClientboundPacket::CUpdateObjectives(data) => write(
            &CUpdateObjectives {
                objective_name: data.objective_name.clone(),
                mode: data.mode,
                display_name: TextComponent::text(data.display_name.clone()),
                render_type: VarInt(data.render_type),
                number_format: data.number_format.as_deref().map(fixed_number_format),
            },
            version,
        ),
        ClientboundPacket::CUpdateScore(data) => write(
            &CUpdateScore::new(
                data.entity_name.clone(),
                data.objective_name.clone(),
                VarInt(data.value),
                data.display_name.clone().map(TextComponent::text),
                data.number_format.as_deref().map(fixed_number_format),
            ),
            version,
        ),
        ClientboundPacket::CResetScore(data) => write(
            &CResetScore::new(data.entity_name.clone(), data.objective_name.clone()),
            version,
        ),
        ClientboundPacket::CPlayerInfoUpdate(data) => player_info_update(data, version),
        _ => None,
    }
}

/// Logs once per packet variant that a plugin sent a packet the host cannot serialize.
pub fn warn_unsupported(packet: &ClientboundPacket) {
    static WARNED: LazyLock<Mutex<HashSet<Discriminant<ClientboundPacket>>>> =
        LazyLock::new(|| Mutex::new(HashSet::new()));

    let variant = std::mem::discriminant(packet);
    let first = WARNED
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(variant);
    if first {
        tracing::warn!(
            ?variant,
            "Wasm plugin sent a Java packet the host cannot serialize; it was dropped"
        );
    }
}

fn fixed_number_format(text: &str) -> NumberFormat {
    NumberFormat::Fixed(TextComponent::text(text.to_owned()))
}

/// Display names are read as JSON text, the format the generated mapping gives plugins for text
/// fields and `text.from-json` takes; any other string is plain text. `add-player` sends no
/// properties: its WIT `property` record is the attribute one and has no profile fields.
fn player_info_update(
    data: &java_packets::CPlayerInfoUpdate,
    version: JavaMinecraftVersion,
) -> Option<Bytes> {
    use java_packets::PlayerAction as WitPlayerAction;

    // The protocol actions borrow their display names, so those are built first.
    let display_names: Vec<Vec<Option<TextComponent>>> = data
        .players
        .iter()
        .map(|player| {
            player
                .actions
                .iter()
                .map(|action| match action {
                    WitPlayerAction::UpdateDisplayName(name) => name.as_deref().map(text_from_json),
                    _ => None,
                })
                .collect()
        })
        .collect();
    let actions: Vec<Vec<PlayerAction>> = data
        .players
        .iter()
        .zip(&display_names)
        .map(|(player, names)| {
            player
                .actions
                .iter()
                .zip(names)
                .map(|(action, display_name)| match action {
                    WitPlayerAction::AddPlayer(add) => PlayerAction::AddPlayer {
                        name: &add.name,
                        properties: &[],
                    },
                    WitPlayerAction::InitializeChat(chat) => {
                        PlayerAction::InitializeChat(chat.as_ref().map(|chat| InitChat {
                            session_id: Uuid::from_wit(&chat.session_id),
                            expires_at: chat.expires_at,
                            public_key: chat.public_key.clone().into_boxed_slice(),
                            signature: chat.signature.clone().into_boxed_slice(),
                        }))
                    }
                    WitPlayerAction::UpdateGameMode(game_mode) => {
                        PlayerAction::UpdateGameMode(VarInt(*game_mode))
                    }
                    WitPlayerAction::UpdateListed(listed) => PlayerAction::UpdateListed(*listed),
                    WitPlayerAction::UpdateLatency(latency) => {
                        PlayerAction::UpdateLatency(VarInt(*latency))
                    }
                    WitPlayerAction::UpdateDisplayName(_) => {
                        PlayerAction::UpdateDisplayName(display_name.as_ref())
                    }
                    WitPlayerAction::UpdateListOrder(order) => {
                        PlayerAction::UpdateListOrder(VarInt(*order))
                    }
                    WitPlayerAction::UpdateHat(show_hat) => PlayerAction::UpdateHat(*show_hat),
                })
                .collect()
        })
        .collect();
    let players: Vec<Player> = data
        .players
        .iter()
        .zip(&actions)
        .map(|(player, actions)| Player {
            uuid: Uuid::from_wit(&player.uuid),
            actions,
        })
        .collect();
    write(&CPlayerInfoUpdate::new(data.actions, &players), version)
}

fn text_from_json(text: &str) -> TextComponent {
    serde_json::from_str(text).unwrap_or_else(|_| TextComponent::text(text.to_owned()))
}

fn write<P: ClientPacket>(packet: &P, version: JavaMinecraftVersion) -> Option<Bytes> {
    let mut buf = Vec::new();
    pumpkin_core::net::java::JavaClient::write_packet_for_version(packet, version, &mut buf)
        .ok()?;
    Some(buf.into())
}
