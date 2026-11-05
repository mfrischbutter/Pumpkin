//! Vanilla `BlockInput.place` and what `/setblock` and `/fill` do around it.

use std::sync::Arc;

use pumpkin_data::{Block, BlockStateId};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::math::position::BlockPos;
use pumpkin_world::world::BlockFlags;

use crate::block::entities::{block_entity_from_nbt, ensure_block_entity};
use crate::command::argument_types::block::BlockInput;
use crate::world::World;

/// Both commands' update flags: `UPDATE_CLIENTS | UPDATE_SKIP_BLOCK_ENTITY_SIDEEFFECTS`, with
/// `strict` also `UPDATE_KNOWN_SHAPE | UPDATE_SUPPRESS_DROPS | UPDATE_SKIP_ON_PLACE`.
#[must_use]
pub(super) const fn place_flags(strict: bool) -> BlockFlags {
    let flags = BlockFlags::NOTIFY_LISTENERS.union(BlockFlags::SKIP_BLOCK_ENTITY_REPLACED_CALLBACK);
    if strict {
        flags
            .union(BlockFlags::UPDATE_KNOWN_SHAPE)
            .union(BlockFlags::SKIP_DROPS)
            .union(BlockFlags::SKIP_BLOCK_ADDED_CALLBACK)
    } else {
        flags
    }
}

/// Vanilla `BlockInput.overwriteWithDefinedProperties`: the properties the command named win
/// over what the neighbours would make of them.
fn overwrite_with_defined_properties(input: &BlockInput, shaped: BlockStateId) -> BlockStateId {
    if shaped == input.state_id || input.properties.is_empty() {
        return shaped;
    }
    let block = Block::from_state_id(shaped);
    let (Some(shaped_properties), Some(named)) = (
        block.properties(shaped),
        input.block.properties(input.state_id),
    ) else {
        return shaped;
    };
    let named = named.to_props();
    let mut properties = shaped_properties.to_props();
    for property in &mut properties {
        if input.properties.contains(&property.key().to_str())
            && let Some(value) = named.iter().find(|value| value.key() == property.key())
        {
            *property = *value;
        }
    }
    block.from_properties(&properties).to_state_id(block)
}

/// Vanilla `Block.updateFromNeighbourShapes`, as `BlockInput.place` uses it.
fn shaped_state(world: &Arc<World>, pos: &BlockPos, input: &BlockInput) -> BlockStateId {
    let shaped = world.update_from_neighbor_shapes(input.state_id, pos);
    if shaped.to_state().is_air() {
        input.state_id
    } else {
        overwrite_with_defined_properties(input, shaped)
    }
}

/// Vanilla `BlockEntity.loadWithComponents` for the command's data. Absent keys fall back to
/// their defaults as they do there, the position and id stay the block entity's own. Returns
/// whether that changed the block entity.
fn load_block_entity_data(world: &World, pos: &BlockPos, tag: &NbtCompound) -> bool {
    let Some(entity) = world.get_block_entity(pos) else {
        return false;
    };
    let Some(before) = world.get_block_entity_nbt(pos) else {
        return false;
    };
    let mut nbt = NbtCompound::new();
    nbt.put_string("id", entity.resource_location().to_string());
    nbt.put_int("x", pos.0.x);
    nbt.put_int("y", pos.0.y);
    nbt.put_int("z", pos.0.z);
    for (name, value) in &tag.child_tags {
        if !matches!(&**name, "id" | "x" | "y" | "z") {
            nbt.put(name, value.clone());
        }
    }
    let Some(loaded) = block_entity_from_nbt(&nbt) else {
        return false;
    };
    let mut after = NbtCompound::new();
    loaded.write_internal(&mut after);
    if after == before {
        return false;
    }
    world.add_block_entity(loaded);
    true
}

/// Vanilla `BlockInput.place`. Returns whether the block or its block entity changed.
pub(super) fn place(
    world: &Arc<World>,
    pos: &BlockPos,
    input: &BlockInput,
    flags: BlockFlags,
) -> bool {
    let state_id = if flags.contains(BlockFlags::UPDATE_KNOWN_SHAPE) {
        input.state_id
    } else {
        shaped_state(world, pos, input)
    };
    let placed = world.set_block_state(pos, state_id, flags) != state_id;
    // Vanilla makes the block entity in `LevelChunk.setBlockState`, also when `onPlace` is
    // skipped; here that is the placed callback.
    if placed && flags.contains(BlockFlags::SKIP_BLOCK_ADDED_CALLBACK) {
        ensure_block_entity(world, pos);
    }
    let loaded = input
        .nbt
        .as_ref()
        .is_some_and(|tag| load_block_entity_data(world, pos, tag));
    placed || loaded
}

/// Vanilla `ServerLevel.updateNeighboursOnBlockSet`.
pub(super) fn update_neighbours_on_block_set(
    world: &Arc<World>,
    pos: &BlockPos,
    old_state_id: BlockStateId,
) {
    let state_id = world.get_block_state_id(pos);
    let new_block = Block::from_state_id(state_id);
    let old_block = Block::from_state_id(old_state_id);
    if old_block != new_block {
        world
            .block_registry
            .on_state_replaced(world, old_block, pos, old_state_id, false);
    }
    world.update_neighbors_at(pos, new_block, None);
    if state_id.has_analog_output_signal() {
        world.update_neighbour_for_output_signal(pos, new_block);
    }
}
