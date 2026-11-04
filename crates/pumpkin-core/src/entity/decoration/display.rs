//! Vanilla `Display`: the state and save data the three display entities share.

use std::sync::{
    Mutex,
    atomic::{AtomicI32, AtomicU8, Ordering},
};

use pumpkin_data::tracked_data::display as tracked;
use pumpkin_nbt::{compound::NbtCompound, tag::NbtTag};
use pumpkin_protocol::{
    codec::var_int::VarInt,
    java::client::play::MetadataSerializer,
    ser::{NetworkWriteExt, WritingError},
};
use pumpkin_util::math::vector3::Vector3;

use crate::entity::Entity;

mod block;
mod item;
mod text;
mod transformation;

pub use block::BlockDisplayEntity;
pub use item::ItemDisplayEntity;
pub use text::TextDisplayEntity;
use transformation::Transformation;

/// Vanilla `Display.NO_BRIGHTNESS_OVERRIDE`.
pub const NO_BRIGHTNESS_OVERRIDE: i32 = -1;
const NO_GLOW_COLOR_OVERRIDE: i32 = -1;
const INITIAL_SHADOW_RADIUS: f32 = 0.0;
const INITIAL_SHADOW_STRENGTH: f32 = 1.0;
const INITIAL_VIEW_RANGE: f32 = 1.0;
const INITIAL_WIDTH: f32 = 0.0;
const INITIAL_HEIGHT: f32 = 0.0;
/// The largest `teleport_duration` vanilla's `readAdditionalSaveData` keeps.
const MAX_POS_ROT_INTERPOLATION_DURATION: i32 = 59;
const MAX_LIGHT_LEVEL: i32 = 15;

/// Vanilla `BillboardConstraints`, by id.
const BILLBOARD_CONSTRAINTS: [&str; 4] = ["fixed", "vertical", "horizontal", "center"];

#[derive(Clone, Copy, Debug)]
pub struct Vector3fSerializer(pub f32, pub f32, pub f32);

impl MetadataSerializer for Vector3fSerializer {
    fn write_metadata(
        &self,
        writer: &mut impl std::io::Write,
        _version: &pumpkin_util::version::JavaMinecraftVersion,
    ) -> Result<(), WritingError> {
        writer.write_f32(self.0)?;
        writer.write_f32(self.1)?;
        writer.write_f32(self.2)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct QuaternionfSerializer(pub f32, pub f32, pub f32, pub f32);

impl MetadataSerializer for QuaternionfSerializer {
    fn write_metadata(
        &self,
        writer: &mut impl std::io::Write,
        _version: &pumpkin_util::version::JavaMinecraftVersion,
    ) -> Result<(), WritingError> {
        writer.write_f32(self.0)?;
        writer.write_f32(self.1)?;
        writer.write_f32(self.2)?;
        writer.write_f32(self.3)
    }
}

fn load<T: Copy>(value: &Mutex<T>) -> T {
    *value
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn store<T>(slot: &Mutex<T>, value: T) {
    *slot
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = value;
}

/// Vanilla `TagValueInput.getIntOr`: any number counts.
fn int_or(nbt: &NbtCompound, name: &str, default: i32) -> i32 {
    nbt.get(name)
        .and_then(NbtTag::as_numeric_double)
        .map_or(default, |value| value as i32)
}

/// Vanilla `LightCoordsUtil.pack`.
const fn pack_brightness(block: i32, sky: i32) -> i32 {
    block << 4 | sky << 20
}

/// Vanilla `Brightness.CODEC`: both light levels are required and between 0 and 15.
fn brightness_from_nbt(compound: &NbtCompound) -> Option<i32> {
    let level = |name: &str| {
        compound
            .get(name)
            .and_then(NbtTag::as_numeric_double)
            .map(|value| value as i32)
            .filter(|value| (0..=MAX_LIGHT_LEVEL).contains(value))
    };
    Some(pack_brightness(level("block")?, level("sky")?))
}

/// Vanilla `Brightness.unpack` as the save data.
fn brightness_to_nbt(packed: i32) -> NbtCompound {
    let mut compound = NbtCompound::new();
    compound.put_int("block", (packed >> 4) & MAX_LIGHT_LEVEL);
    compound.put_int("sky", (packed >> 20) & MAX_LIGHT_LEVEL);
    compound
}

/// Vanilla `Display`. `brightness` holds the packed `LightCoordsUtil` value of the synced data.
pub struct DisplayEntity {
    pub entity: Entity,
    pub interpolation_start_delta_ticks: AtomicI32,
    pub interpolation_duration: AtomicI32,
    pub teleport_duration: AtomicI32,
    pub view_range: Mutex<f32>,
    pub shadow_radius: Mutex<f32>,
    pub shadow_strength: Mutex<f32>,
    pub width: Mutex<f32>,
    pub height: Mutex<f32>,
    pub glow_color_override: AtomicI32,
    pub billboard: AtomicU8,
    pub brightness: AtomicI32,
    pub translation: Mutex<Vector3<f32>>,
    pub scale: Mutex<Vector3<f32>>,
    pub left_rotation: Mutex<[f32; 4]>,
    pub right_rotation: Mutex<[f32; 4]>,
}

impl DisplayEntity {
    pub fn new(entity: Entity) -> Self {
        entity.no_physics.store(true, Ordering::Relaxed);
        Self::define_synched_data(&entity);
        Self {
            entity,
            interpolation_start_delta_ticks: AtomicI32::new(0),
            interpolation_duration: AtomicI32::new(0),
            teleport_duration: AtomicI32::new(0),
            view_range: Mutex::new(INITIAL_VIEW_RANGE),
            shadow_radius: Mutex::new(INITIAL_SHADOW_RADIUS),
            shadow_strength: Mutex::new(INITIAL_SHADOW_STRENGTH),
            width: Mutex::new(INITIAL_WIDTH),
            height: Mutex::new(INITIAL_HEIGHT),
            glow_color_override: AtomicI32::new(NO_GLOW_COLOR_OVERRIDE),
            billboard: AtomicU8::new(0),
            brightness: AtomicI32::new(NO_BRIGHTNESS_OVERRIDE),
            translation: Mutex::new(Vector3::new(0.0, 0.0, 0.0)),
            scale: Mutex::new(Vector3::new(1.0, 1.0, 1.0)),
            left_rotation: Mutex::new(Transformation::IDENTITY.left_rotation),
            right_rotation: Mutex::new(Transformation::IDENTITY.right_rotation),
        }
    }

    /// Vanilla `Display.defineSynchedData`: a client assumes these, so only changes are sent.
    fn define_synched_data(entity: &Entity) {
        let data = &entity.synched_data;
        data.define(tracked::TELEPORT_DURATION, VarInt(0));
        data.define(tracked::START_INTERPOLATION, VarInt(0));
        data.define(tracked::INTERPOLATION_DURATION, VarInt(0));
        data.define(tracked::TRANSLATION, Vector3fSerializer(0.0, 0.0, 0.0));
        data.define(tracked::SCALE, Vector3fSerializer(1.0, 1.0, 1.0));
        let identity = QuaternionfSerializer(0.0, 0.0, 0.0, 1.0);
        data.define(tracked::RIGHT_ROTATION, identity);
        data.define(tracked::LEFT_ROTATION, identity);
        data.define(tracked::BILLBOARD, 0u8);
        data.define(tracked::BRIGHTNESS, VarInt(NO_BRIGHTNESS_OVERRIDE));
        data.define(tracked::VIEW_RANGE, INITIAL_VIEW_RANGE);
        data.define(tracked::SHADOW_RADIUS, INITIAL_SHADOW_RADIUS);
        data.define(tracked::SHADOW_STRENGTH, INITIAL_SHADOW_STRENGTH);
        data.define(tracked::WIDTH, INITIAL_WIDTH);
        data.define(tracked::HEIGHT, INITIAL_HEIGHT);
        data.define(tracked::GLOW_COLOR_OVERRIDE, VarInt(NO_GLOW_COLOR_OVERRIDE));
    }

    /// Vanilla `Display.tick`: nothing but leaving a vehicle that is gone. A display has no base
    /// tick, so it neither falls nor burns. It leaves in place, only living riders dismount.
    pub fn tick_display(&self) {
        if let Some(vehicle) = self.entity.get_vehicle()
            && vehicle.get_entity().is_removed()
        {
            vehicle
                .get_entity()
                .remove_passenger_before_teleport(self.entity.entity_id);
        }
    }

    #[must_use]
    pub fn get_interpolation_start_delta_ticks(&self) -> i32 {
        self.interpolation_start_delta_ticks.load(Ordering::Relaxed)
    }

    /// Vanilla `setTransformationInterpolationDelay`: forced, every write restarts the client's
    /// interpolation.
    pub fn set_interpolation_start_delta_ticks(&self, ticks: i32) {
        self.interpolation_start_delta_ticks
            .store(ticks, Ordering::Relaxed);
        self.entity
            .force_synced_data(tracked::START_INTERPOLATION, VarInt(ticks));
    }

    #[must_use]
    pub fn get_interpolation_duration(&self) -> i32 {
        self.interpolation_duration.load(Ordering::Relaxed)
    }

    pub fn set_interpolation_duration(&self, duration: i32) {
        self.interpolation_duration
            .store(duration, Ordering::Relaxed);
        self.entity
            .set_synced_data(tracked::INTERPOLATION_DURATION, VarInt(duration));
    }

    #[must_use]
    pub fn get_teleport_duration(&self) -> i32 {
        self.teleport_duration.load(Ordering::Relaxed)
    }

    pub fn set_teleport_duration(&self, duration: i32) {
        self.teleport_duration.store(duration, Ordering::Relaxed);
        self.entity
            .set_synced_data(tracked::TELEPORT_DURATION, VarInt(duration));
    }

    #[must_use]
    pub fn get_translation(&self) -> Vector3<f32> {
        load(&self.translation)
    }

    pub fn set_translation(&self, translation: Vector3<f32>) {
        store(&self.translation, translation);
        self.entity.set_synced_data(
            tracked::TRANSLATION,
            Vector3fSerializer(translation.x, translation.y, translation.z),
        );
    }

    #[must_use]
    pub fn get_scale(&self) -> Vector3<f32> {
        load(&self.scale)
    }

    pub fn set_scale(&self, scale: Vector3<f32>) {
        store(&self.scale, scale);
        self.entity
            .set_synced_data(tracked::SCALE, Vector3fSerializer(scale.x, scale.y, scale.z));
    }

    #[must_use]
    pub fn get_left_rotation(&self) -> [f32; 4] {
        load(&self.left_rotation)
    }

    pub fn set_left_rotation(&self, rotation: [f32; 4]) {
        store(&self.left_rotation, rotation);
        let [x, y, z, w] = rotation;
        self.entity
            .set_synced_data(tracked::LEFT_ROTATION, QuaternionfSerializer(x, y, z, w));
    }

    #[must_use]
    pub fn get_right_rotation(&self) -> [f32; 4] {
        load(&self.right_rotation)
    }

    pub fn set_right_rotation(&self, rotation: [f32; 4]) {
        store(&self.right_rotation, rotation);
        let [x, y, z, w] = rotation;
        self.entity
            .set_synced_data(tracked::RIGHT_ROTATION, QuaternionfSerializer(x, y, z, w));
    }

    #[must_use]
    pub fn get_billboard(&self) -> u8 {
        self.billboard.load(Ordering::Relaxed)
    }

    pub fn set_billboard(&self, billboard: u8) {
        self.billboard.store(billboard, Ordering::Relaxed);
        self.entity.set_synced_data(tracked::BILLBOARD, billboard);
    }

    /// The packed `LightCoordsUtil` value (`block << 4 | sky << 20`), or
    /// [`NO_BRIGHTNESS_OVERRIDE`].
    #[must_use]
    pub fn get_brightness(&self) -> i32 {
        self.brightness.load(Ordering::Relaxed)
    }

    /// Takes the packed `LightCoordsUtil` value (`block << 4 | sky << 20`), or
    /// [`NO_BRIGHTNESS_OVERRIDE`].
    pub fn set_brightness(&self, brightness: i32) {
        self.brightness.store(brightness, Ordering::Relaxed);
        self.entity
            .set_synced_data(tracked::BRIGHTNESS, VarInt(brightness));
    }

    #[must_use]
    pub fn get_view_range(&self) -> f32 {
        load(&self.view_range)
    }

    pub fn set_view_range(&self, view_range: f32) {
        store(&self.view_range, view_range);
        self.entity.set_synced_data(tracked::VIEW_RANGE, view_range);
    }

    #[must_use]
    pub fn get_shadow_radius(&self) -> f32 {
        load(&self.shadow_radius)
    }

    pub fn set_shadow_radius(&self, shadow_radius: f32) {
        store(&self.shadow_radius, shadow_radius);
        self.entity
            .set_synced_data(tracked::SHADOW_RADIUS, shadow_radius);
    }

    #[must_use]
    pub fn get_shadow_strength(&self) -> f32 {
        load(&self.shadow_strength)
    }

    pub fn set_shadow_strength(&self, shadow_strength: f32) {
        store(&self.shadow_strength, shadow_strength);
        self.entity
            .set_synced_data(tracked::SHADOW_STRENGTH, shadow_strength);
    }

    #[must_use]
    pub fn get_display_width(&self) -> f32 {
        load(&self.width)
    }

    pub fn set_display_width(&self, width: f32) {
        store(&self.width, width);
        self.entity.set_synced_data(tracked::WIDTH, width);
    }

    #[must_use]
    pub fn get_display_height(&self) -> f32 {
        load(&self.height)
    }

    pub fn set_display_height(&self, height: f32) {
        store(&self.height, height);
        self.entity.set_synced_data(tracked::HEIGHT, height);
    }

    #[must_use]
    pub fn get_glow_color_override(&self) -> i32 {
        self.glow_color_override.load(Ordering::Relaxed)
    }

    pub fn set_glow_color_override(&self, color: i32) {
        self.glow_color_override.store(color, Ordering::Relaxed);
        self.entity
            .set_synced_data(tracked::GLOW_COLOR_OVERRIDE, VarInt(color));
    }

    fn transformation(&self) -> Transformation {
        let translation = self.get_translation();
        let scale = self.get_scale();
        Transformation {
            translation: [translation.x, translation.y, translation.z],
            left_rotation: self.get_left_rotation(),
            scale: [scale.x, scale.y, scale.z],
            right_rotation: self.get_right_rotation(),
        }
    }

    /// Vanilla `Display.setTransformation`.
    fn set_transformation(&self, transformation: &Transformation) {
        let [x, y, z] = transformation.translation;
        self.set_translation(Vector3::new(x, y, z));
        self.set_left_rotation(transformation.left_rotation);
        let [x, y, z] = transformation.scale;
        self.set_scale(Vector3::new(x, y, z));
        self.set_right_rotation(transformation.right_rotation);
    }

    /// Vanilla `Display.addAdditionalSaveData`; `start_interpolation` is not saved.
    pub fn write_display_nbt(&self, nbt: &mut NbtCompound) {
        nbt.put_compound("transformation", self.transformation().to_nbt());
        let billboard = BILLBOARD_CONSTRAINTS
            .get(usize::from(self.get_billboard()))
            .copied()
            .unwrap_or(BILLBOARD_CONSTRAINTS[0]);
        nbt.put_string("billboard", billboard.to_string());
        nbt.put_int("interpolation_duration", self.get_interpolation_duration());
        nbt.put_int("teleport_duration", self.get_teleport_duration());
        nbt.put_float("view_range", self.get_view_range());
        nbt.put_float("shadow_radius", self.get_shadow_radius());
        nbt.put_float("shadow_strength", self.get_shadow_strength());
        nbt.put_float("width", self.get_display_width());
        nbt.put_float("height", self.get_display_height());
        nbt.put_int("glow_color_override", self.get_glow_color_override());
        let brightness = self.get_brightness();
        if brightness != NO_BRIGHTNESS_OVERRIDE {
            nbt.put_compound("brightness", brightness_to_nbt(brightness));
        }
    }

    /// Vanilla `Display.readAdditionalSaveData`: a missing or invalid value is the default, and
    /// each value goes through its setter so `/data` reaches the clients.
    pub fn read_display_nbt(&self, nbt: &NbtCompound) {
        let transformation = nbt
            .get("transformation")
            .and_then(Transformation::from_nbt)
            .unwrap_or(Transformation::IDENTITY);
        self.set_transformation(&transformation);
        self.set_interpolation_duration(int_or(nbt, "interpolation_duration", 0));
        self.set_interpolation_start_delta_ticks(int_or(nbt, "start_interpolation", 0));
        self.set_teleport_duration(
            int_or(nbt, "teleport_duration", 0).clamp(0, MAX_POS_ROT_INTERPOLATION_DURATION),
        );
        let billboard = nbt
            .get_string("billboard")
            .and_then(|name| BILLBOARD_CONSTRAINTS.iter().position(|&c| c == name))
            .unwrap_or(0);
        self.set_billboard(billboard as u8);
        self.set_view_range(
            nbt.get_numeric_float("view_range")
                .unwrap_or(INITIAL_VIEW_RANGE),
        );
        self.set_shadow_radius(
            nbt.get_numeric_float("shadow_radius")
                .unwrap_or(INITIAL_SHADOW_RADIUS),
        );
        self.set_shadow_strength(
            nbt.get_numeric_float("shadow_strength")
                .unwrap_or(INITIAL_SHADOW_STRENGTH),
        );
        self.set_display_width(nbt.get_numeric_float("width").unwrap_or(INITIAL_WIDTH));
        self.set_display_height(nbt.get_numeric_float("height").unwrap_or(INITIAL_HEIGHT));
        self.set_glow_color_override(int_or(nbt, "glow_color_override", NO_GLOW_COLOR_OVERRIDE));
        self.set_brightness(
            nbt.get_compound("brightness")
                .and_then(brightness_from_nbt)
                .unwrap_or(NO_BRIGHTNESS_OVERRIDE),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Plugins and the synced data use vanilla's packed form, not two nibbles.
    #[test]
    fn brightness_packs_like_light_coords() {
        let mut compound = NbtCompound::new();
        compound.put_int("block", 15);
        compound.put_int("sky", 7);
        let packed = brightness_from_nbt(&compound);
        assert_eq!(packed, Some(15 << 4 | 7 << 20));

        let saved = brightness_to_nbt(15 << 4 | 7 << 20);
        assert_eq!(saved.get_int("block"), Some(15));
        assert_eq!(saved.get_int("sky"), Some(7));
    }

    #[test]
    fn brightness_rejects_levels_out_of_range() {
        let mut compound = NbtCompound::new();
        compound.put_int("block", 16);
        compound.put_int("sky", 0);
        assert_eq!(brightness_from_nbt(&compound), None);
    }
}
