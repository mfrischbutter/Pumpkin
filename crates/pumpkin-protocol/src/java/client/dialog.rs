use pumpkin_util::text::TextComponent;

use crate::ser::NetworkWriteExt;
use pumpkin_util::version::JavaMinecraftVersion;

/// Parses a dialog in vanilla's JSON format into the compound the client reads.
///
/// Only the shape is checked. The client validates the rest and disconnects on a dialog it
/// cannot decode.
pub fn dialog_compound_from_json(json: &str) -> Result<pumpkin_nbt::compound::NbtCompound, String> {
    let value: serde_json::Value = serde_json::from_str(json).map_err(|e| e.to_string())?;
    if !value.get("type").is_some_and(serde_json::Value::is_string) {
        return Err("a dialog needs a string field `type`".to_string());
    }
    if value.get("title").is_none() {
        return Err("a dialog needs a field `title`".to_string());
    }
    match pumpkin_util::text::hover::json_to_nbt(&value) {
        pumpkin_nbt::tag::NbtTag::Compound(compound) => Ok(compound),
        _ => Err("a dialog must be a JSON object".to_string()),
    }
}

pub struct DialogNBT<'a>(pub DialogNBTSource<'a>);

impl<'a> DialogNBT<'a> {
    #[must_use]
    pub const fn from_dialog(dialog: &'a Dialog) -> Self {
        Self(DialogNBTSource::Struct(dialog))
    }

    #[must_use]
    pub const fn from_nbt(compound: &'a pumpkin_nbt::compound::NbtCompound) -> Self {
        Self(DialogNBTSource::Nbt(compound))
    }

    pub fn write_packet_data(
        &self,
        mut write: impl std::io::Write,
        version: &JavaMinecraftVersion,
    ) -> Result<(), crate::ser::WritingError> {
        match &self.0 {
            DialogNBTSource::Struct(_dialog) => Err(crate::ser::WritingError::Message(
                "Serializing Dialog struct to NBT is not supported".to_string(),
            )),
            DialogNBTSource::Nbt(nbt) => {
                let tag = pumpkin_nbt::tag::NbtTag::Compound((*nbt).clone());
                write.write_nbt_with_version(Some(&tag), version)?;
                Ok(())
            }
        }
    }
}

pub enum DialogNBTSource<'a> {
    Struct(&'a Dialog),
    Nbt(&'a pumpkin_nbt::compound::NbtCompound),
}

#[derive(Clone, Debug)]
pub struct Dialog {
    pub r#type: String,
    pub title: TextComponent,
    pub body: Vec<DialogBody>,
    pub inputs: Vec<DialogInput>,
    pub buttons: Vec<ActionButton>,
    pub links: Vec<DialogLink>,
    pub exit_action: Option<DialogAction>,
    pub after_action: Option<String>,
    pub can_close_with_escape: bool,
    pub external_title: Option<TextComponent>,
}

#[derive(Clone, Debug)]
pub enum DialogBody {
    PlainMessage { contents: TextComponent },
    Item { item: i32 }, // TODO: ItemStack serialization to NBT
}

#[derive(Clone, Debug)]
pub enum DialogInput {
    Boolean {
        label: TextComponent,
        default_value: bool,
    },
    Text {
        label: TextComponent,
        placeholder: TextComponent,
        default_value: String,
    },
    NumberRange {
        label: TextComponent,
        min: f32,
        max: f32,
        initial: f32,
        step: f32,
        label_format: Option<String>,
    },
    SingleOption {
        label: TextComponent,
        options: Vec<TextComponent>,
        initial_index: u32,
    },
}

#[derive(Clone, Debug)]
pub struct ActionButton {
    pub text: TextComponent,
    pub tooltip: Option<TextComponent>,
    pub width: Option<u32>,
    pub action: DialogAction,
}

#[derive(Clone, Debug)]
pub enum DialogAction {
    OpenUrl {
        url: String,
    },
    Custom {
        id: String,
        payload: Option<Vec<u8>>,
    },
}

#[derive(Clone, Debug)]
pub struct DialogLink {
    pub label: crate::Label,
    pub url: String,
}

#[cfg(test)]
mod tests {
    use super::dialog_compound_from_json;

    #[test]
    fn dialog_json_keeps_nested_fields() {
        let json = r#"{"type":"minecraft:notice","title":{"text":"A"},"pause":false,
            "body":{"type":"minecraft:plain_message","width":431,"contents":{"text":"","extra":[{"text":"B"}]}}}"#;
        let compound = dialog_compound_from_json(json).unwrap();
        let body = compound.get_compound("body").unwrap();
        assert_eq!(body.get_short("width"), Some(431));
        assert_eq!(compound.get_byte("pause"), Some(0));
    }

    #[test]
    fn dialog_json_rejects_what_is_no_dialog() {
        assert!(dialog_compound_from_json("not json").is_err());
        assert!(dialog_compound_from_json("[1]").is_err());
        assert!(dialog_compound_from_json(r#"{"title":"A"}"#).is_err());
        assert!(dialog_compound_from_json(r#"{"type":"minecraft:notice"}"#).is_err());
    }
}
