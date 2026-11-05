//! Vanilla `VibrationSystem` for entities: what reaches them, its way there and its arrival.

use std::sync::{Arc, Mutex, MutexGuard};

use pumpkin_data::{
    BlockDirection,
    game_event::{GameEvent, PositionSourceType},
    particle::Particle,
    tag::{self, Taggable},
};
use pumpkin_nbt::{compound::NbtCompound, tag::NbtTag};
use pumpkin_protocol::{codec::var_int::VarInt, java::client::play::CParticle, ser::WritingError};
use pumpkin_util::math::{position::BlockPos, vector3::Vector3};
use uuid::Uuid;

use crate::entity::{Entity, EntityBase};
use crate::world::{
    World,
    game_event::{GameEventContext, GameEventListener},
};

/// Vanilla `VibrationSystem.User.getListenerRadius`, 16 for every entity that listens.
const VIBRATION_EVENT_LISTENER_RANGE: i32 = 16;
const PARTICLE_SEND_DISTANCE: f64 = 32.0;

/// Vanilla `VibrationSystem.User`.
pub trait VibrationUser: Send + Sync {
    fn vibration_entity(&self) -> &Entity;

    fn vibration_data(&self) -> &Mutex<VibrationData>;

    /// Vanilla `getListenableEvents`.
    fn can_listen_to(&self, event: GameEvent) -> bool;

    fn can_receive_vibration(
        &self,
        world: &World,
        pos: BlockPos,
        event: GameEvent,
        context: &GameEventContext<'_>,
    ) -> bool;

    fn on_receive_vibration(
        &self,
        world: &World,
        pos: BlockPos,
        event: GameEvent,
        source: Option<Arc<dyn EntityBase>>,
        projectile_owner: Option<Arc<dyn EntityBase>>,
    );
}

#[derive(Clone)]
pub(crate) struct VibrationInfo {
    pub(crate) event: GameEvent,
    pub(crate) distance: f32,
    pub(crate) pos: Vector3<f64>,
    pub(crate) source: Option<Uuid>,
    pub(crate) projectile_owner: Option<Uuid>,
}

impl VibrationInfo {
    fn read_nbt(nbt: &NbtCompound) -> Option<Self> {
        let event = GameEvent::from_name(nbt.get_string("game_event")?)?;
        let distance = nbt.get_float("distance")?;
        let pos = nbt.get_list("pos")?;
        if pos.len() != 3 || !distance.is_finite() || distance < 0.0 {
            return None;
        }
        let pos = Vector3::new(
            pos[0].extract_double()?,
            pos[1].extract_double()?,
            pos[2].extract_double()?,
        );
        if !pos.x.is_finite() || !pos.y.is_finite() || !pos.z.is_finite() {
            return None;
        }
        Some(Self {
            event,
            distance,
            pos,
            source: nbt.get_uuid("source"),
            projectile_owner: nbt.get_uuid("projectile_owner"),
        })
    }

    fn write_nbt(&self) -> NbtCompound {
        let mut nbt = NbtCompound::new();
        nbt.put_string("game_event", format!("minecraft:{}", self.event.name()));
        nbt.put_float("distance", self.distance);
        nbt.put_list(
            "pos",
            vec![
                NbtTag::Double(self.pos.x),
                NbtTag::Double(self.pos.y),
                NbtTag::Double(self.pos.z),
            ],
        );
        if let Some(source) = self.source {
            nbt.put_uuid("source", source);
        }
        if let Some(owner) = self.projectile_owner {
            nbt.put_uuid("projectile_owner", owner);
        }
        nbt
    }
}

#[derive(Default)]
pub struct VibrationData {
    pub(crate) current: Option<VibrationInfo>,
    pub(crate) candidate: Option<(VibrationInfo, i64)>,
    pub(crate) delay: i32,
    pub(crate) reload_particle: bool,
}

impl VibrationData {
    #[must_use]
    pub fn read_nbt(nbt: &NbtCompound) -> Self {
        let Some(selector) = nbt.get_compound("selector") else {
            return Self::default();
        };
        let Some(tick) = selector.get_long("tick") else {
            return Self::default();
        };
        let delay = nbt.get_int("event_delay").unwrap_or(0);
        if delay < 0 {
            return Self::default();
        }
        Self {
            current: nbt.get_compound("event").and_then(VibrationInfo::read_nbt),
            candidate: selector
                .get_compound("event")
                .and_then(VibrationInfo::read_nbt)
                .map(|event| (event, tick)),
            delay,
            reload_particle: true,
        }
    }

    #[must_use]
    pub fn write_nbt(&self) -> NbtCompound {
        let mut nbt = NbtCompound::new();
        if let Some(event) = &self.current {
            nbt.put_compound("event", event.write_nbt());
        }
        let mut selector = NbtCompound::new();
        selector.put_long(
            "tick",
            self.candidate.as_ref().map_or(-1, |(_, tick)| *tick),
        );
        if let Some((event, _)) = &self.candidate {
            selector.put_compound("event", event.write_nbt());
        }
        nbt.put_compound("selector", selector);
        nbt.put_int("event_delay", self.delay);
        nbt
    }
}

/// Vanilla `VibrationSystem.Listener`.
pub struct VibrationListener<'a, U: VibrationUser>(pub &'a U);

fn data_of<U: VibrationUser>(user: &U) -> MutexGuard<'_, VibrationData> {
    user.vibration_data()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn eye_position(entity: &Entity) -> Vector3<f64> {
    let pos = entity.pos.load();
    Vector3::new(pos.x, entity.get_eye_y(), pos.z)
}

impl<U: VibrationUser> GameEventListener for VibrationListener<'_, U> {
    fn listener_position(&self) -> Vector3<f64> {
        eye_position(self.0.vibration_entity())
    }

    fn listener_radius(&self) -> i32 {
        VIBRATION_EVENT_LISTENER_RANGE
    }

    fn handle_game_event(
        &self,
        world: &World,
        event: GameEvent,
        context: &GameEventContext<'_>,
        source_position: Vector3<f64>,
    ) -> bool {
        let user = self.0;
        if data_of(user).current.is_some() || !is_valid_vibration(user, event, context) {
            return false;
        }
        let destination = eye_position(user.vibration_entity());
        let pos = BlockPos::floored_v(source_position);
        if !user.can_receive_vibration(world, pos, event, context)
            || is_occluded(world, source_position, destination)
        {
            return false;
        }
        let time = world.get_world_age();
        let distance = (source_position - destination).length() as f32;
        let source = context.source_entity;
        let projectile_owner = source
            .and_then(|source| source.get_projectile_owner())
            .and_then(|owner| owner.get(world))
            .map(|owner| owner.get_entity().entity_uuid);
        let mut data = data_of(user);
        if data
            .candidate
            .as_ref()
            .is_none_or(|(previous, tick)| *tick == time && distance < previous.distance)
        {
            data.candidate = Some((
                VibrationInfo {
                    event,
                    distance,
                    pos: source_position,
                    source: source.map(|source| source.get_entity().entity_uuid),
                    projectile_owner,
                },
                time,
            ));
        }
        true
    }
}

fn is_valid_vibration<U: VibrationUser>(
    user: &U,
    event: GameEvent,
    context: &GameEventContext<'_>,
) -> bool {
    if !user.can_listen_to(event) {
        return false;
    }
    let Some(source) = context.source_entity else {
        return true;
    };
    let ignored_while_sneaking = tag::GameEvent::MINECRAFT_IGNORE_VIBRATIONS_SNEAKING
        .0
        .contains(&event.name());
    !source.is_spectator()
        && (!source.is_stepping_carefully() || !ignored_while_sneaking)
        && !source.dampens_vibrations()
}

fn is_occluded(world: &World, origin: Vector3<f64>, destination: Vector3<f64>) -> bool {
    let from = BlockPos::floored_v(origin).to_centered_f64();
    let to = BlockPos::floored_v(destination).to_centered_f64();
    BlockDirection::all().iter().all(|direction| {
        let nudged_source = from + direction.to_offset().to_f64() * f64::from(1.0e-5f32);
        pumpkin_world::clip::traverse_blocks(nudged_source, to, |pos| {
            world
                .get_block(pos)
                .has_tag(&tag::Block::MINECRAFT_OCCLUDES_VIBRATION_SIGNALS)
                .then_some(())
        })
        .is_some()
    })
}

fn find_by_uuid(world: &World, uuid: Option<Uuid>) -> Option<Arc<dyn EntityBase>> {
    world.get_entity_by_uuid(uuid?)
}

/// Vanilla `VibrationSystem.Ticker`: picks the nearest candidate, sends it on its way and hands
/// it over when it arrives.
pub fn tick_vibration<U: VibrationUser>(user: &U) {
    let entity = user.vibration_entity();
    let world = entity.world.load();
    let mut data = data_of(user);
    if data.current.is_none()
        && data
            .candidate
            .as_ref()
            .is_some_and(|(_, time)| *time < world.get_world_age())
        && let Some((event, _)) = data.candidate.take()
    {
        data.delay = event.distance.floor() as i32;
        send_vibration_particle(entity, event.pos, data.delay);
        data.current = Some(event);
    }
    if data.reload_particle {
        if let Some(event) = &data.current {
            let initial = event.distance.floor() as i32;
            let alpha = 1.0 - f64::from(data.delay) / f64::from(initial);
            let pos = event.pos.lerp(&eye_position(entity), alpha);
            if send_vibration_particle(entity, pos, data.delay) {
                data.reload_particle = false;
            }
        } else {
            data.reload_particle = false;
        }
    }
    if data.current.is_none() {
        return;
    }
    data.delay = (data.delay - 1).max(0);
    if data.delay == 0 {
        let event = data.current.take();
        drop(data);
        if let Some(event) = event {
            user.on_receive_vibration(
                &world,
                BlockPos::floored_v(event.pos),
                event.event,
                find_by_uuid(&world, event.source),
                find_by_uuid(&world, event.projectile_owner),
            );
        }
    }
}

fn send_vibration_particle(entity: &Entity, pos: Vector3<f64>, delay: i32) -> bool {
    let Ok(payload) = vibration_payload(entity.entity_id, entity.get_eye_height() as f32, delay)
    else {
        return false;
    };
    let packet = CParticle::new(
        false,
        false,
        pos,
        Vector3::new(0.0, 0.0, 0.0),
        0.0,
        1,
        VarInt(i32::from(Particle::Vibration.to_id())),
        &payload,
    );
    let world = entity.world.load();
    let mut sent = false;
    for player in world.players.load().iter() {
        if player.position().squared_distance_to_vec(&pos)
            <= PARTICLE_SEND_DISTANCE * PARTICLE_SEND_DISTANCE
        {
            player.try_send_client_packet(&packet);
            sent = true;
        }
    }
    sent
}

pub(crate) fn vibration_payload(
    entity_id: i32,
    eye_offset: f32,
    delay: i32,
) -> Result<Vec<u8>, WritingError> {
    let mut payload = Vec::new();
    VarInt(PositionSourceType::Entity.to_id()).encode(&mut payload)?;
    VarInt(entity_id).encode(&mut payload)?;
    payload.extend_from_slice(&eye_offset.to_be_bytes());
    VarInt(delay).encode(&mut payload)?;
    Ok(payload)
}
