use crate::{
    argument_types::argument_type::{ArgumentType, JavaClientArgumentType},
    argument_types::block_predicate::{ERROR_UNCLOSED_PROPERTIES, parse_nbt},
    context::command_context::CommandContext,
    errors::command_syntax_error::CommandSyntaxError,
    errors::error_types::CommandErrorType,
    string_reader::StringReader,
    suggestion::suggestions::{Suggestions, SuggestionsBuilder},
};
use pumpkin_data::{Block, BlockStateId, Property, translation};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::text::TextComponent;

pub const INVALID_BLOCK_ERROR_TYPE: CommandErrorType<1> = CommandErrorType::new(
    translation::java::ARGUMENT_BLOCK_ID_INVALID,
    translation::java::ARGUMENT_BLOCK_ID_INVALID,
);

const ERROR_NO_TAGS_ALLOWED: CommandErrorType<0> = CommandErrorType::new(
    translation::java::ARGUMENT_BLOCK_TAG_DISALLOWED,
    translation::java::ARGUMENT_BLOCK_TAG_DISALLOWED,
);

const ERROR_UNKNOWN_PROPERTY: CommandErrorType<2> = CommandErrorType::new(
    translation::java::ARGUMENT_BLOCK_PROPERTY_UNKNOWN,
    translation::java::ARGUMENT_BLOCK_PROPERTY_UNKNOWN,
);

const ERROR_DUPLICATE_PROPERTY: CommandErrorType<2> = CommandErrorType::new(
    translation::java::ARGUMENT_BLOCK_PROPERTY_DUPLICATE,
    translation::java::ARGUMENT_BLOCK_PROPERTY_DUPLICATE,
);

const ERROR_INVALID_VALUE: CommandErrorType<3> = CommandErrorType::new(
    translation::java::ARGUMENT_BLOCK_PROPERTY_INVALID,
    translation::java::ARGUMENT_BLOCK_PROPERTY_INVALID,
);

const ERROR_EXPECTED_VALUE: CommandErrorType<2> = CommandErrorType::new(
    translation::java::ARGUMENT_BLOCK_PROPERTY_NOVALUE,
    translation::java::ARGUMENT_BLOCK_PROPERTY_NOVALUE,
);

/// Vanilla `BlockInput`: a block state, the properties the command named and block entity data.
#[derive(Clone, Debug)]
pub struct BlockInput {
    pub block: &'static Block,
    pub state_id: BlockStateId,
    /// The keys of the properties the command named; a shape update must not change those.
    pub properties: Vec<&'static str>,
    pub nbt: Option<NbtCompound>,
}

impl BlockInput {
    /// A plain state without named properties or block entity data, like `/fill`'s hollow core.
    #[must_use]
    pub const fn from_state(state_id: BlockStateId) -> Self {
        Self {
            block: Block::from_state_id(state_id),
            state_id,
            properties: Vec::new(),
            nbt: None,
        }
    }
}

/// The block state in brackets, read by vanilla `BlockStateParser.readProperties`.
pub(crate) struct ParsedProperties {
    pub state_id: BlockStateId,
    pub values: Vec<(&'static str, String)>,
}

fn block_id_text(block: &Block) -> TextComponent {
    TextComponent::text(format!("minecraft:{}", block.name))
}

/// Vanilla `BlockState.setValue`, for a value given by name. `None` when the block's own property
/// does not have that value: value names are shared, `straight` is no shape of a rail.
fn state_with_value(
    block: &'static Block,
    state_id: BlockStateId,
    key: &str,
    value: &str,
) -> Option<BlockStateId> {
    let new_value = block.states.iter().find_map(|state| {
        block
            .properties(state.id)?
            .to_props()
            .into_iter()
            .find(|property| property.key().to_str() == key && property.value_str() == value)
    })?;
    let mut properties = block.properties(state_id)?.to_props();
    let slot = properties
        .iter_mut()
        .find(|property| property.key().to_str() == key)?;
    *slot = new_value;
    Some(block.from_properties(&properties).to_state_id(block))
}

fn read_property(
    reader: &mut StringReader,
    block: &'static Block,
    own_properties: &[Property],
    parsed: &mut ParsedProperties,
) -> Result<(), CommandSyntaxError> {
    reader.skip_whitespace();
    let key_start = reader.cursor();
    let key = reader.read_string()?;
    let Some(known_key) = own_properties
        .iter()
        .map(|property| property.key().to_str())
        .find(|known| *known == key)
    else {
        reader.set_cursor(key_start);
        return Err(ERROR_UNKNOWN_PROPERTY.create(
            reader,
            block_id_text(block),
            TextComponent::text(key),
        ));
    };
    if parsed.values.iter().any(|(named, _)| *named == known_key) {
        reader.set_cursor(key_start);
        return Err(ERROR_DUPLICATE_PROPERTY.create(
            reader,
            TextComponent::text(key),
            block_id_text(block),
        ));
    }

    reader.skip_whitespace();
    if reader.peek() != Some('=') {
        return Err(ERROR_EXPECTED_VALUE.create(
            reader,
            TextComponent::text(key),
            block_id_text(block),
        ));
    }
    reader.skip();
    reader.skip_whitespace();

    let value_start = reader.cursor();
    let value = reader.read_string()?;
    let Some(state_id) = state_with_value(block, parsed.state_id, known_key, &value) else {
        reader.set_cursor(value_start);
        return Err(ERROR_INVALID_VALUE.create(
            reader,
            block_id_text(block),
            TextComponent::text(value),
            TextComponent::text(key),
        ));
    };
    parsed.state_id = state_id;
    parsed.values.push((known_key, value));
    Ok(())
}

/// Vanilla `BlockStateParser.readProperties`: the reader stands on the opening bracket.
pub(crate) fn read_properties(
    reader: &mut StringReader,
    block: &'static Block,
) -> Result<ParsedProperties, CommandSyntaxError> {
    let own_properties = block
        .properties(block.default_state.id)
        .map_or_else(Vec::new, |properties| properties.to_props());
    let mut parsed = ParsedProperties {
        state_id: block.default_state.id,
        values: Vec::new(),
    };
    reader.skip();
    reader.skip_whitespace();
    while reader.can_read_char() && reader.peek() != Some(']') {
        read_property(reader, block, &own_properties, &mut parsed)?;
        reader.skip_whitespace();
        match reader.peek() {
            Some(',') => reader.skip(),
            Some(']') | None => break,
            Some(_) => return Err(ERROR_UNCLOSED_PROPERTIES.create(reader)),
        }
    }
    if reader.peek() != Some(']') {
        return Err(ERROR_UNCLOSED_PROPERTIES.create(reader));
    }
    reader.skip();
    Ok(parsed)
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub struct BlockArgumentType;

impl<S: crate::source::CommandSource> ArgumentType<S> for BlockArgumentType {
    type Item = BlockInput;

    /// Vanilla `BlockStateParser.parseForBlock` with block entity data allowed.
    fn parse(&self, reader: &mut StringReader) -> Result<Self::Item, CommandSyntaxError> {
        if reader.peek() == Some('#') {
            return Err(ERROR_NO_TAGS_ALLOWED.create(reader));
        }
        let start = reader.cursor();
        while let Some(c) = reader.peek() {
            if c.is_alphanumeric() || c == '_' || c == ':' || c == '/' || c == '.' || c == '-' {
                reader.skip();
            } else {
                break;
            }
        }
        let block_name = &reader.string()[start..reader.cursor()];
        let normalized = if block_name.contains(':') {
            block_name.to_string()
        } else {
            format!("minecraft:{block_name}")
        };

        let Some(block) = Block::from_name(&normalized) else {
            reader.set_cursor(start);
            return Err(INVALID_BLOCK_ERROR_TYPE.create(reader, TextComponent::text(normalized)));
        };

        let parsed = if reader.peek() == Some('[') {
            read_properties(reader, block)?
        } else {
            ParsedProperties {
                state_id: block.default_state.id,
                values: Vec::new(),
            }
        };
        let nbt = parse_nbt(reader)?;

        Ok(BlockInput {
            block,
            state_id: parsed.state_id,
            properties: parsed.values.into_iter().map(|(key, _)| key).collect(),
            nbt,
        })
    }

    fn client_side_parser(&'_ self) -> JavaClientArgumentType {
        JavaClientArgumentType::BlockState
    }

    fn list_suggestions(
        &self,
        _context: &CommandContext<S>,
        builder: SuggestionsBuilder,
    ) -> Suggestions {
        builder.build()
    }
}

impl BlockArgumentType {
    pub fn get<S: crate::source::CommandSource>(
        context: &CommandContext<S>,
        name: &str,
    ) -> Result<BlockInput, CommandSyntaxError> {
        Ok(context.get_argument::<BlockInput>(name)?.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::DummySource;
    use pumpkin_data::block_properties::RailShape;

    fn parse(input: &str) -> Result<BlockInput, CommandSyntaxError> {
        ArgumentType::<DummySource>::parse(&BlockArgumentType, &mut StringReader::new(input))
    }

    #[test]
    fn named_properties_select_the_state_and_stay_listed() {
        let expected = Block::RAIL
            .from_properties(&[Property::RailShape(RailShape::EastWest)])
            .to_state_id(&Block::RAIL);
        assert!(parse("minecraft:rail[shape=east_west]").is_ok_and(|input| {
            input.state_id == expected && input.properties == ["shape"] && input.nbt.is_none()
        }));
        assert!(parse("rail[ shape = east_west ]").is_ok_and(|input| input.state_id == expected));
    }

    #[test]
    fn a_bare_block_is_its_default_state() {
        assert!(parse("rail").is_ok_and(|input| {
            input.state_id == Block::RAIL.default_state.id && input.properties.is_empty()
        }));
        assert!(parse("stone[]").is_ok_and(|input| *input.block == Block::STONE));
    }

    #[test]
    fn invalid_states_are_rejected_like_vanilla() {
        assert!(parse("rail[foo=bar]").is_err());
        assert!(parse("stone[foo=bar]").is_err());
        assert!(parse("rail[shape=sideways]").is_err());
        // A value of another block's property of the same name.
        assert!(parse("rail[shape=straight]").is_err());
        assert!(parse("rail[shape=east_west,shape=north_south]").is_err());
        assert!(parse("rail[shape]").is_err());
        assert!(parse("rail[shape=east_west").is_err());
        assert!(parse("#rails").is_err());
    }

    #[test]
    fn block_entity_data_follows_the_state() {
        assert!(parse("chest[facing=east]{Lock:\"a\"}").is_ok_and(|input| {
            input.properties == ["facing"]
                && input
                    .nbt
                    .is_some_and(|nbt| nbt.get_string("Lock") == Some("a"))
        }));
    }
}
