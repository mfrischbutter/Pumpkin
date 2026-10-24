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
    java::client::play::{CResetScore, CUpdateObjectives, CUpdateScore},
};
use pumpkin_util::{text::TextComponent, version::JavaMinecraftVersion};

use crate::pumpkin::plugin::java_packets::ClientboundPacket;

/// Serializes the packets the generated mapping skips. Text fields are plain text, like in the
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

fn write<P: ClientPacket>(packet: &P, version: JavaMinecraftVersion) -> Option<Bytes> {
    let mut buf = Vec::new();
    pumpkin_core::net::java::JavaClient::write_packet_for_version(packet, version, &mut buf)
        .ok()?;
    Some(buf.into())
}
