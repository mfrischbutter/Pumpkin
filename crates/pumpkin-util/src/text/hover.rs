use std::{borrow::Cow, vec};

use serde::{Deserialize, Deserializer, Serialize};

use super::{TextComponent, TextComponentBase};

/// Represents the hover event action in a chat component.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum HoverEvent {
    /// Displays a tooltip with the given text.
    ShowText {
        #[serde(deserialize_with = "deserialize_components")]
        value: Vec<TextComponentBase>,
    },
    /// Shows an item.
    ShowItem {
        /// Resource identifier of the item.
        id: Cow<'static, str>,
        /// Number of the items in the stack.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        count: Option<i32>,
        // #[serde(default, skip_serializing_if = "Option::is_none")]
        // components: Option<Cow<'static, str>>,
    },
    /// Shows an entity.
    ShowEntity {
        /// The entity's ID Entity Type.
        id: Cow<'static, str>,
        /// The entity's UUID
        /// The UUID cannot use `uuid::Uuid` because its serialization parses it into bytes, so its double bytes serialized.
        uuid: Cow<'static, str>,
        /// Optional custom name for the entity.
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "deserialize_optional_components"
        )]
        name: Option<Vec<TextComponentBase>>,
    },
}

/// Reads a component in any vanilla form (string, object or list) as a list of components.
fn deserialize_components<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Vec<TextComponentBase>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Components {
        List(Vec<TextComponent>),
        One(TextComponent),
    }

    Ok(match Components::deserialize(deserializer)? {
        Components::List(list) => list.into_iter().map(|component| component.0).collect(),
        Components::One(component) => vec![component.0],
    })
}

fn deserialize_optional_components<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Vec<TextComponentBase>>, D::Error> {
    deserialize_components(deserializer).map(Some)
}

impl HoverEvent {
    /// Creates a new hover event that displays text.
    ///
    /// # Arguments
    /// - `text` – The text component to display in the tooltip.
    ///
    /// # Returns
    /// A `HoverEvent::ShowText` variant containing the provided text.
    #[must_use]
    pub fn show_text(text: TextComponent) -> Self {
        Self::ShowText {
            value: vec![text.0],
        }
    }

    /// Creates a new hover event that displays entity information.
    ///
    /// # Arguments
    /// - `uuid` – The entity's UUID as a string.
    /// - `kind` – The entity type identifier (e.g., "minecraft:pig").
    /// - `name` – Optional custom name for the entity.
    ///
    /// # Returns
    /// A `HoverEvent::ShowEntity` variant containing the entity information.
    pub fn show_entity<P: Into<Cow<'static, str>>>(
        uuid: P,
        kind: P,
        name: Option<TextComponent>,
    ) -> Self {
        Self::ShowEntity {
            id: kind.into(),
            uuid: uuid.into(),
            name: match name {
                Some(name) => Some(vec![name.0]),
                None => None,
            },
        }
    }
}
