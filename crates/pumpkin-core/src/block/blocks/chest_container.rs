use std::sync::Arc;

use pumpkin_data::BlockStateId;
use pumpkin_data::block_properties::{ChestLikeProperties, ChestType};
use pumpkin_inventory::Inventory;
use pumpkin_inventory::double::DoubleInventory;
use pumpkin_util::math::boundingbox::BoundingBox;
use pumpkin_util::math::position::BlockPos;

use crate::block::entities::BlockEntity;
use crate::block::entities::chest::ChestBlockEntity;
use crate::block::entities::trapped_chest::TrappedChestBlockEntity;
use crate::entity::passive::cat::CatEntity;
use crate::entity::passive::tamable::TamableAnimal;
use crate::world::World;

/// Vanilla `unpackLootTable(null)` for a chest and its other half, on a mob's container access.
pub(crate) fn unpack_chest_loot(world: &Arc<World>, position: &BlockPos) {
    if let Some(entity) = world.get_block_entity(position) {
        entity.unpack_loot_table(world, None);
    }
    if let Some(connected) = connected_chest_pos(position, world.get_block_state_id(position))
        && let Some(entity) = world.get_block_entity(&connected)
    {
        entity.unpack_loot_table(world, None);
    }
}

/// Vanilla `ChestBlock.getConnectedBlockPos`; `None` for a single chest.
pub(crate) fn connected_chest_pos(position: &BlockPos, state_id: BlockStateId) -> Option<BlockPos> {
    let props = ChestLikeProperties::from_state_id(state_id);
    let towards = match props.r#type {
        ChestType::Single => return None,
        ChestType::Left => props.facing.rotate_clockwise(),
        ChestType::Right => props.facing.rotate_counter_clockwise(),
    };
    Some(position.offset(towards.to_offset()))
}

/// Vanilla `ChestBlock.getContainer` with `ignoreBeingBlocked` false: `None` when a half is
/// blocked, both halves' inventories for a double chest.
pub(crate) fn chest_container(world: &World, position: &BlockPos) -> Option<Arc<dyn Inventory>> {
    let (block, state_id) = world.get_block_and_state_id(position);
    let first = world
        .get_block_entity(position)
        .and_then(BlockEntity::get_inventory)?;
    if is_chest_blocked(world, position) {
        return None;
    }
    let Some(connected) = connected_chest_pos(position, state_id) else {
        return Some(first);
    };
    let props = ChestLikeProperties::from_state_id(state_id);
    let (neighbor_block, neighbor_state_id) = world.get_block_and_state_id(&connected);
    let neighbor_props = (neighbor_block == block)
        .then(|| ChestLikeProperties::from_state_id(neighbor_state_id))
        .filter(|neighbor| {
            neighbor.r#type != ChestType::Single
                && neighbor.r#type != props.r#type
                && neighbor.facing == props.facing
        });
    if neighbor_props.is_none() {
        return Some(first);
    }
    if is_chest_blocked(world, &connected) {
        return None;
    }
    let Some(second) = world
        .get_block_entity(&connected)
        .and_then(BlockEntity::get_inventory)
    else {
        return Some(first);
    };
    let double: Arc<dyn Inventory> = if matches!(props.r#type, ChestType::Right) {
        DoubleInventory::new(first, second)
    } else {
        DoubleInventory::new(second, first)
    };
    Some(double)
}

/// The opener count of vanilla `ChestBlockEntity`, trapped and copper chests included; `None`
/// for any other block entity.
pub(crate) fn chest_opener_count(block_entity: &dyn BlockEntity) -> Option<u16> {
    let any = block_entity.as_any();
    any.downcast_ref::<ChestBlockEntity>()
        .map(ChestBlockEntity::viewer_count)
        .or_else(|| {
            any.downcast_ref::<TrappedChestBlockEntity>()
                .map(TrappedChestBlockEntity::get_viewer_count)
        })
}

/// Vanilla `ChestBlock.isChestBlockedAt`.
pub(crate) fn is_chest_blocked(world: &World, block_pos: &BlockPos) -> bool {
    has_block_on_top(world, block_pos) || is_cat_sitting_on_chest(world, block_pos)
}

/// Vanilla `ChestBlock.isCatSittingOnChest`.
fn is_cat_sitting_on_chest(world: &World, block_pos: &BlockPos) -> bool {
    world
        .get_entities_at_box(&BoundingBox::from_block(&block_pos.up()))
        .iter()
        .filter_map(|entity| entity.cast_any().downcast_ref::<CatEntity>())
        .any(TamableAnimal::is_in_sitting_pose)
}

fn has_block_on_top(world: &World, block_pos: &BlockPos) -> bool {
    let above_pos = block_pos.up();
    let above_state = world.get_block_state(&above_pos);
    above_state.is_solid_block()
}
