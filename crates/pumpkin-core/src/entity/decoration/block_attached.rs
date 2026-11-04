//! Vanilla `BlockAttachedEntity` and `HangingEntity`, shared by paintings, item frames and
//! leash knots.

use std::sync::atomic::{AtomicI32, Ordering};

use crossbeam::atomic::AtomicCell;
use pumpkin_data::damage::DamageType;
use pumpkin_data::game_event::GameEvent;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::packet::CURRENT_MC_VERSION;
use pumpkin_data::tracked_data;
use pumpkin_data::{Block, BlockDirection, BlockState};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_protocol::java::client::play::CSetEntityMetadata;
use pumpkin_util::math::boundingbox::BoundingBox;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;

use crate::entity::ai::brain::memory::codec::{block_pos_from_tag, block_pos_to_tag};
use crate::entity::decoration::{item_frame::ItemFrameEntity, painting::PaintingEntity};
use crate::entity::player::Player;
use crate::entity::{Entity, EntityBase};
use crate::net::java::JavaClient;
use crate::world::World;
use crate::world::game_event::GameEventContext;

/// Vanilla `BlockAttachedEntity.CHECK_INTERVAL`.
const CHECK_INTERVAL: i32 = 100;
/// Vanilla `BlockAttachedEntity.TAG_BLOCK_POS`.
const TAG_BLOCK_POS: &str = "block_pos";
/// Vanilla `readAdditionalSaveData`: a saved block further than this from the entity is refused.
const MAX_STORED_POS_DISTANCE: i64 = 16;
/// Vanilla `createBoundingBox` and `calculateBoundingBox`: the entity's center off the block's.
pub(super) const SHIFT_TO_BLOCK_WALL: f64 = 0.46875;
/// Vanilla `Painting.DEPTH` and `ItemFrame.DEPTH`.
pub(super) const DEPTH: f64 = 0.0625;
/// Vanilla `HangingEntity.spawnAtLocation`: how far from the wall a drop starts.
const DROP_WALL_OFFSET: f64 = 0.15;
/// Vanilla `calculateSupportBox`.
const SUPPORT_BOX_DEFLATE: f64 = 1.0E-7;

/// The state vanilla `BlockAttachedEntity` adds to an entity: the block it is attached at and
/// the countdown of its survival check.
pub struct BlockAttachment {
    pos: AtomicCell<BlockPos>,
    ticks_since_last_check: AtomicI32,
}

impl BlockAttachment {
    #[must_use]
    pub const fn new(pos: BlockPos) -> Self {
        Self {
            pos: AtomicCell::new(pos),
            ticks_since_last_check: AtomicI32::new(0),
        }
    }

    /// Vanilla `getPos`.
    #[must_use]
    pub fn pos(&self) -> BlockPos {
        self.pos.load()
    }

    pub fn set_pos(&self, pos: BlockPos) {
        self.pos.store(pos);
    }

    /// Vanilla `addAdditionalSaveData`.
    pub fn write_nbt(&self, nbt: &mut NbtCompound) {
        nbt.put(TAG_BLOCK_POS, block_pos_to_tag(self.pos()));
    }

    /// Vanilla `readAdditionalSaveData`. Returns whether the compound had a block position, so
    /// the caller can derive one for saves from before it was stored.
    pub fn read_nbt(&self, nbt: &NbtCompound, entity_pos: BlockPos) -> bool {
        let Some(stored) = nbt.get(TAG_BLOCK_POS).and_then(block_pos_from_tag) else {
            return false;
        };
        let x = i64::from(stored.0.x) - i64::from(entity_pos.0.x);
        let y = i64::from(stored.0.y) - i64::from(entity_pos.0.y);
        let z = i64::from(stored.0.z) - i64::from(entity_pos.0.z);
        if x * x + y * y + z * z < MAX_STORED_POS_DISTANCE * MAX_STORED_POS_DISTANCE {
            self.set_pos(stored);
        } else {
            tracing::error!("Block-attached entity at invalid position: {stored:?}");
        }
        true
    }
}

/// Vanilla `BlockAttachedEntity`'s behaviour.
pub trait BlockAttached: EntityBase + Sized {
    fn attachment(&self) -> &BlockAttachment;

    /// Vanilla `survives`.
    fn survives(&self) -> bool;

    /// Vanilla `dropItem`.
    fn drop_attached_item(&self, caused_by: Option<&dyn EntityBase>);

    /// Vanilla `tick`: the entity does none of the base entity's tick.
    fn tick_attached(&self, caller: &dyn EntityBase) {
        let entity = self.get_entity();
        entity.check_out_of_world(caller);
        let attachment = self.attachment();
        if attachment
            .ticks_since_last_check
            .fetch_add(1, Ordering::Relaxed)
            >= CHECK_INTERVAL
        {
            attachment
                .ticks_since_last_check
                .store(0, Ordering::Relaxed);
            if !entity.is_removed() && !self.survives() {
                entity.remove();
                self.drop_attached_item(None);
            }
        }
    }

    /// Vanilla `kill`.
    fn kill_attached(&self, attributed_to: Option<&dyn EntityBase>) {
        let entity = self.get_entity();
        entity.remove();
        emit_game_event(
            entity,
            GameEvent::EntityDie,
            attributed_to.or(Some(self as &dyn EntityBase)),
        );
    }

    /// Vanilla `hurtServer`.
    fn hurt_attached(&self, damage_type: DamageType, cause: Option<&dyn EntityBase>) -> bool {
        let entity = self.get_entity();
        if entity.is_invulnerable_to(&damage_type, cause) {
            return false;
        }
        let griefing = entity.world.load().level_info.load().game_rules.mob_griefing;
        if !griefing && cause.is_some_and(|cause| cause.get_mob().is_some()) {
            return false;
        }
        if !entity.is_removed() {
            self.kill_attached(cause);
            self.drop_attached_item(cause);
        }
        true
    }

    /// Vanilla `move` and `push`: any movement takes the entity off its block.
    fn moved_off_block(&self, delta: Vector3<f64>) {
        if !self.get_entity().is_removed() && delta.length_squared() > 0.0 {
            self.kill_attached(None);
            self.drop_attached_item(None);
        }
    }

    /// Vanilla `skipAttackInteraction`.
    fn skip_attack_on_attached(&self, player: &Player) -> bool {
        let world = self.get_entity().world.load();
        !may_interact(&world, player, &self.attachment().pos())
            || self.damage_with_context(
                self,
                0.0,
                DamageType::PLAYER_ATTACK,
                None,
                Some(player),
                Some(player),
            )
    }
}

/// Vanilla `BlockAttachedEntity.ignoreExplosion`.
pub(super) const fn attached_ignore_explosion(
    affects_blocklike_entities: bool,
    source_in_water: bool,
) -> bool {
    source_in_water || !affects_blocklike_entities
}

/// Vanilla `Level.mayInteract`.
pub(super) fn may_interact(world: &World, player: &Player, pos: &BlockPos) -> bool {
    !world.is_in_spawn_protection(player, pos)
        && world
            .worldborder
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains_block(pos.0.x, pos.0.z)
}

/// Vanilla `Entity.gameEvent(event, sourceEntity)`.
pub(super) fn emit_game_event(
    entity: &Entity,
    event: GameEvent,
    source: Option<&dyn EntityBase>,
) {
    entity.world.load().emit_game_event_with_context(
        event.name(),
        entity.pos.load(),
        &GameEventContext {
            source_entity: source,
        },
    );
}

/// Vanilla `player.hasInfiniteMaterials`, for the entity a drop is attributed to.
pub(super) fn is_creative_player(entity: Option<&dyn EntityBase>) -> bool {
    entity
        .and_then(EntityBase::get_player)
        .is_some_and(Player::is_creative)
}

/// Vanilla `itemStack.set(DataComponents.CUSTOM_NAME, getCustomName())`.
pub(super) fn copy_custom_name(entity: &Entity, stack: &mut ItemStack) {
    if let Some(name) = entity.custom_name.load().as_ref().clone() {
        stack.set_custom_name_component(name);
    }
}

/// Vanilla `HangingEntity.spawnAtLocation`: drops leave from just off the wall.
pub(super) fn spawn_hanging_drop(entity: &Entity, direction: BlockDirection, stack: ItemStack) {
    let pos = entity.pos.load();
    let step = direction.to_offset().to_f64();
    entity.world.load().drop_stack_at(
        Vector3::new(
            pos.x + step.x * DROP_WALL_OFFSET,
            pos.y,
            pos.z + step.z * DROP_WALL_OFFSET,
        ),
        stack,
    );
}

/// Vanilla `setDirectionRaw`. The spawn packet carries the same direction in its data field.
pub(super) fn sync_direction(entity: &Entity, direction: BlockDirection) {
    let index = i32::from(direction.to_index());
    entity.data.store(index, Ordering::Relaxed);
    entity.set_synced_data(
        tracked_data::hanging_entity::DATA_DIRECTION,
        VarInt(index),
    );
}

/// Vanilla `recalculateBoundingBox`: the entity's position follows from its box.
pub(super) fn apply_bounding_box(entity: &Entity, center: Vector3<f64>, bounds: BoundingBox) {
    entity.set_pos(center);
    entity.bounding_box.store(bounds);
}

/// Vanilla `Vec3.atCenterOf(pos).relative(direction, -0.46875)`.
pub(super) fn wall_center(pos: BlockPos, direction: BlockDirection) -> Vector3<f64> {
    let center = pos.to_centered_f64();
    let step = direction.to_offset().to_f64();
    Vector3::new(
        center.x - step.x * SHIFT_TO_BLOCK_WALL,
        center.y - step.y * SHIFT_TO_BLOCK_WALL,
        center.z - step.z * SHIFT_TO_BLOCK_WALL,
    )
}

/// Vanilla `DiodeBlock.isDiode`.
pub(super) fn is_diode(block: &Block) -> bool {
    block == &Block::REPEATER || block == &Block::COMPARATOR
}

/// Vanilla `HangingEntity.isSupportingBlock`.
fn is_supporting_block(block: &Block, state: &BlockState) -> bool {
    state.is_solid() || is_diode(block)
}

/// Vanilla `HangingEntity.calculateSupportBox` with the check that every block in it supports.
pub(super) fn is_supported(world: &World, bounds: BoundingBox, direction: BlockDirection) -> bool {
    let step = direction.to_offset().to_f64();
    let support = bounds
        .shift(Vector3::new(-0.5 * step.x, -0.5 * step.y, -0.5 * step.z))
        .contract_all(SUPPORT_BOX_DEFLATE);
    BlockPos::iterate(support.min_block_pos(), support.max_block_pos()).all(|pos| {
        let (block, state) = world.get_block_and_state(&pos);
        is_supporting_block(block, state)
    })
}

/// Vanilla `HangingEntity.hasLevelCollision`.
pub(super) fn has_level_collision(world: &World, pop_box: BoundingBox) -> bool {
    if !world.is_space_empty(pop_box) {
        return true;
    }
    let border = world
        .worldborder
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    !border.contains(pop_box.min.x, pop_box.min.z) || !border.contains(pop_box.max.x, pop_box.max.z)
}

/// The direction of a vanilla `HangingEntity`, for the entities that are one.
fn hanging_direction(entity: &dyn EntityBase) -> Option<BlockDirection> {
    let any = entity.cast_any();
    any.downcast_ref::<PaintingEntity>()
        .map(PaintingEntity::direction)
        .or_else(|| {
            any.downcast_ref::<ItemFrameEntity>()
                .map(ItemFrameEntity::get_facing)
        })
}

/// Vanilla `HangingEntity.canCoexist`. `own` names the entity to look past, if it exists yet.
pub(super) fn can_coexist(
    world: &World,
    own: Option<i32>,
    own_type: &pumpkin_data::entity::EntityType,
    direction: BlockDirection,
    pop_box: &BoundingBox,
    allow_intersecting_same_type: bool,
) -> bool {
    !world.get_entities_at_box(pop_box).iter().any(|other| {
        let Some(other_direction) = hanging_direction(other.as_ref()) else {
            return false;
        };
        let other = other.get_entity();
        let intersects_same_type = !allow_intersecting_same_type && other.entity_type == own_type;
        Some(other.entity_id) != own && (intersects_same_type || other_direction == direction)
    })
}

/// Vanilla `getAddEntityPacket`: these entities spawn at their block, not at their center, since
/// the client derives the center again from the block and the direction.
pub(super) fn send_spawn_at_block(this: &dyn EntityBase, pos: BlockPos, client: &JavaClient) {
    let entity = this.get_entity();
    let metadata = this.java_spawn_metadata(CURRENT_MC_VERSION);
    let mut packet = entity.create_spawn_packet();
    packet.position = pos.0.to_f64();
    if let Ok(data) = client.serialize_packet(&packet) {
        client.try_enqueue_packet(data);
    }
    if let Some(metadata) = metadata {
        let metadata = CSetEntityMetadata::new(entity.entity_id.into(), metadata);
        if let Ok(data) = client.serialize_packet(&metadata) {
            client.try_enqueue_packet(data);
        }
    }
}
