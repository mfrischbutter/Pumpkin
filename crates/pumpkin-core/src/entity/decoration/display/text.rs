use std::sync::{
    Arc, Mutex,
    atomic::{AtomicI8, AtomicI32, AtomicU8, Ordering},
};

use pumpkin_data::tracked_data::text_display as tracked;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_util::text::TextComponent;

use super::{DisplayEntity, int_or};
use crate::{
    entity::{
        Entity, EntityBase, living::LivingEntity, text_component_from_nbt, text_component_to_nbt,
    },
    server::Server,
};

const FLAG_SHADOW: u8 = 1;
const FLAG_SEE_THROUGH: u8 = 2;
const FLAG_USE_DEFAULT_BACKGROUND: u8 = 4;
const FLAG_ALIGN_LEFT: u8 = 8;
const FLAG_ALIGN_RIGHT: u8 = 16;
const INITIAL_TEXT_OPACITY: i8 = -1;
const INITIAL_BACKGROUND: i32 = 1_073_741_824;
const INITIAL_LINE_WIDTH: i32 = 200;

/// Vanilla `Display.TextDisplay`.
pub struct TextDisplayEntity {
    pub display: DisplayEntity,
    pub text: Mutex<TextComponent>,
    pub line_width: AtomicI32,
    pub background: AtomicI32,
    pub text_opacity: AtomicI8,
    pub flags: AtomicU8,
}

impl TextDisplayEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let data = &entity.synched_data;
        data.define(tracked::TEXT, TextComponent::empty());
        data.define(tracked::LINE_WIDTH, VarInt(INITIAL_LINE_WIDTH));
        data.define(tracked::BACKGROUND, VarInt(INITIAL_BACKGROUND));
        data.define(tracked::TEXT_OPACITY, INITIAL_TEXT_OPACITY as u8);
        data.define(tracked::TEXT_DISPLAY_FLAGS, 0u8);
        Arc::new(Self {
            display: DisplayEntity::new(entity),
            text: Mutex::new(TextComponent::empty()),
            line_width: AtomicI32::new(INITIAL_LINE_WIDTH),
            background: AtomicI32::new(INITIAL_BACKGROUND),
            text_opacity: AtomicI8::new(INITIAL_TEXT_OPACITY),
            flags: AtomicU8::new(0),
        })
    }

    #[must_use]
    pub fn get_text(&self) -> TextComponent {
        self.text
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    pub fn set_text(&self, text: TextComponent) {
        *self
            .text
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = text.clone();
        self.display.entity.set_synced_data(tracked::TEXT, text);
    }

    #[must_use]
    pub fn get_line_width(&self) -> i32 {
        self.line_width.load(Ordering::Relaxed)
    }

    pub fn set_line_width(&self, width: i32) {
        self.line_width.store(width, Ordering::Relaxed);
        self.display
            .entity
            .set_synced_data(tracked::LINE_WIDTH, VarInt(width));
    }

    #[must_use]
    pub fn get_background_color(&self) -> i32 {
        self.background.load(Ordering::Relaxed)
    }

    pub fn set_background_color(&self, color: i32) {
        self.background.store(color, Ordering::Relaxed);
        self.display
            .entity
            .set_synced_data(tracked::BACKGROUND, VarInt(color));
    }

    #[must_use]
    pub fn get_text_opacity(&self) -> i8 {
        self.text_opacity.load(Ordering::Relaxed)
    }

    pub fn set_text_opacity(&self, opacity: i8) {
        self.text_opacity.store(opacity, Ordering::Relaxed);
        self.display
            .entity
            .set_synced_data(tracked::TEXT_OPACITY, opacity as u8);
    }

    fn has_flag(&self, mask: u8) -> bool {
        self.flags.load(Ordering::Relaxed) & mask != 0
    }

    /// Vanilla `TextDisplay.setFlags`.
    fn set_flags(&self, flags: u8) {
        self.flags.store(flags, Ordering::Relaxed);
        self.display
            .entity
            .set_synced_data(tracked::TEXT_DISPLAY_FLAGS, flags);
    }

    fn set_flag(&self, mask: u8, value: bool) {
        let flags = self.flags.load(Ordering::Relaxed);
        self.set_flags(if value { flags | mask } else { flags & !mask });
    }

    #[must_use]
    pub fn get_shadow(&self) -> bool {
        self.has_flag(FLAG_SHADOW)
    }

    pub fn set_shadow(&self, shadow: bool) {
        self.set_flag(FLAG_SHADOW, shadow);
    }

    #[must_use]
    pub fn get_see_through(&self) -> bool {
        self.has_flag(FLAG_SEE_THROUGH)
    }

    pub fn set_see_through(&self, see_through: bool) {
        self.set_flag(FLAG_SEE_THROUGH, see_through);
    }

    #[must_use]
    pub fn get_use_default_background(&self) -> bool {
        self.has_flag(FLAG_USE_DEFAULT_BACKGROUND)
    }

    pub fn set_use_default_background(&self, default_background: bool) {
        self.set_flag(FLAG_USE_DEFAULT_BACKGROUND, default_background);
    }

    /// Vanilla `TextDisplay.getAlign`: 0 centered, 1 left, 2 right.
    #[must_use]
    pub fn get_alignment(&self) -> u8 {
        if self.has_flag(FLAG_ALIGN_LEFT) {
            1
        } else if self.has_flag(FLAG_ALIGN_RIGHT) {
            2
        } else {
            0
        }
    }

    pub fn set_alignment(&self, align: u8) {
        let flags = self.flags.load(Ordering::Relaxed) & !(FLAG_ALIGN_LEFT | FLAG_ALIGN_RIGHT);
        self.set_flags(match align {
            1 => flags | FLAG_ALIGN_LEFT,
            2 => flags | FLAG_ALIGN_RIGHT,
            _ => flags,
        });
    }
}

impl EntityBase for TextDisplayEntity {
    /// Vanilla `TextDisplay.addAdditionalSaveData`: the text is a component tag, not JSON.
    fn write_custom_nbt(&self, nbt: &mut NbtCompound) {
        self.display.write_display_nbt(nbt);
        nbt.put("text", text_component_to_nbt(&self.get_text()));
        nbt.put_int("line_width", self.get_line_width());
        nbt.put_int("background", self.get_background_color());
        nbt.put_byte("text_opacity", self.get_text_opacity());
        nbt.put_bool("shadow", self.get_shadow());
        nbt.put_bool("see_through", self.get_see_through());
        nbt.put_bool("default_background", self.get_use_default_background());
        let alignment = match self.get_alignment() {
            1 => "left",
            2 => "right",
            _ => "center",
        };
        nbt.put_string("alignment", alignment.to_string());
    }

    /// Vanilla `TextDisplay.readAdditionalSaveData`: only a present `text` replaces the text.
    fn read_custom_nbt(&self, nbt: &NbtCompound) {
        self.display.read_display_nbt(nbt);
        self.set_line_width(int_or(nbt, "line_width", INITIAL_LINE_WIDTH));
        self.set_text_opacity(int_or(nbt, "text_opacity", i32::from(INITIAL_TEXT_OPACITY)) as i8);
        self.set_background_color(int_or(nbt, "background", INITIAL_BACKGROUND));

        let flag = |name: &str, mask: u8| {
            if nbt.get_bool(name).unwrap_or(false) {
                mask
            } else {
                0
            }
        };
        let alignment = match nbt.get_string("alignment") {
            Some("left") => FLAG_ALIGN_LEFT,
            Some("right") => FLAG_ALIGN_RIGHT,
            _ => 0,
        };
        self.set_flags(
            flag("shadow", FLAG_SHADOW)
                | flag("see_through", FLAG_SEE_THROUGH)
                | flag("default_background", FLAG_USE_DEFAULT_BACKGROUND)
                | alignment,
        );

        if let Some(text) = nbt.get("text") {
            self.set_text(text_component_from_nbt(text));
        }
    }

    fn tick(&self, _caller: &dyn EntityBase, _server: &Server) {
        self.display.tick_display();
    }

    /// Every value already reached the synced data through its setter.
    fn init_data_tracker(&self) {}

    fn get_entity(&self) -> &Entity {
        &self.display.entity
    }

    fn get_living_entity(&self) -> Option<&LivingEntity> {
        None
    }

    fn cast_any(&self) -> &dyn std::any::Any {
        self
    }

    fn is_pushed_by_fluids(&self) -> bool {
        false
    }

    fn is_immune_to_explosion(&self) -> bool {
        true
    }
}
