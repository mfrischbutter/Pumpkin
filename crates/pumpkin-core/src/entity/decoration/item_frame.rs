use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};

use crate::entity::decoration::block_attached::{
    BlockAttached, BlockAttachment, DEPTH, apply_bounding_box, attached_ignore_explosion,
    can_coexist, copy_custom_name, emit_game_event, has_level_collision, is_creative_player,
    is_diode, send_spawn_at_block, spawn_hanging_drop, sync_direction, wall_center,
};
use crate::entity::decoration::painting::facing_to_horizontal;
use crate::entity::player::Player;
use crate::entity::{Entity, EntityBase, living::LivingEntity};
use crate::net::java::JavaClient;
use crate::server::Server;
use crate::world::World;
use crossbeam::atomic::AtomicCell;
use pumpkin_data::damage::DamageType;
use pumpkin_data::data_component_impl::MapIdImpl;
use pumpkin_data::entity::EntityType;
use pumpkin_data::game_event::GameEvent;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::Sound;
use pumpkin_data::tag::{self, Taggable};
use pumpkin_data::tracked_data::item_frame;
use pumpkin_data::{Block, BlockDirection};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_protocol::codec::item_stack_seralizer::ItemStackSerializer;
use pumpkin_util::math::boundingbox::BoundingBox;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;

/// Vanilla `ItemFrame.NUM_ROTATIONS`.
const NUM_ROTATIONS: u8 = 8;
/// Vanilla `ItemFrame.WIDTH` and `ItemFrame.HEIGHT`, for a frame without a map.
const FRAME_SIZE: f64 = 0.75;
/// The width and height of a frame showing a map.
const MAP_FRAME_SIZE: f64 = 1.0;

/// An item frame or glow item frame.
///
/// Holds the displayed item and its rotation so that comparators can read the
/// frame's analog output and so frames from vanilla worlds keep their data
/// across save cycles.
pub struct ItemFrameEntity {
    entity: Entity,
    attachment: BlockAttachment,
    item_stack: Mutex<ItemStack>,
    /// Rotation of the displayed item, always in `0..8`.
    rotation: AtomicU8,
    /// The direction the frame faces, i.e. the axis pointing away from the
    /// block it hangs on. Stored as the vanilla 3D direction index
    /// (0 = down, 1 = up, 2 = north, 3 = south, 4 = west, 5 = east).
    facing: AtomicU8,
    item_drop_chance: AtomicCell<f32>,
    fixed: AtomicBool,
}

impl ItemFrameEntity {
    /// Facing used when a frame is created without NBT, matching vanilla.
    const DEFAULT_FACING: BlockDirection = BlockDirection::South;

    #[must_use]
    pub fn new(entity: Entity) -> Self {
        // The spawn packet reads the direction from the entity data field, so
        // it has to agree with `facing` or the frame spawns facing elsewhere.
        sync_direction(&entity, Self::DEFAULT_FACING);
        entity.synched_data.define(
            item_frame::ITEM,
            ItemStackSerializer::from(ItemStack::EMPTY.clone()),
        );
        entity.synched_data.define(item_frame::ROTATION, 0i32);
        let pos = entity.block_pos.load();
        Self {
            entity,
            attachment: BlockAttachment::new(pos),
            item_stack: Mutex::new(ItemStack::EMPTY.clone()),
            rotation: AtomicU8::new(0),
            facing: AtomicU8::new(Self::DEFAULT_FACING.to_index()),
            item_drop_chance: AtomicCell::new(1.0),
            fixed: AtomicBool::new(false),
        }
    }

    /// Vanilla `ItemFrame(EntityType, Level, BlockPos, Direction)`, in the block `pos`.
    #[must_use]
    pub fn create(
        world: &Arc<World>,
        entity_type: &'static EntityType,
        pos: BlockPos,
        direction: BlockDirection,
    ) -> Self {
        let entity = Entity::new(world.clone(), wall_center(pos, direction), entity_type);
        let frame = Self::new(entity);
        frame.attachment.set_pos(pos);
        frame.set_facing(direction);
        frame
    }

    #[must_use]
    pub const fn is_glow(&self) -> bool {
        self.entity.entity_type.id == EntityType::GLOW_ITEM_FRAME.id
    }

    #[must_use]
    pub const fn get_add_item_sound(&self) -> Sound {
        if self.is_glow() {
            Sound::EntityGlowItemFrameAddItem
        } else {
            Sound::EntityItemFrameAddItem
        }
    }

    #[must_use]
    pub const fn get_remove_item_sound(&self) -> Sound {
        if self.is_glow() {
            Sound::EntityGlowItemFrameRemoveItem
        } else {
            Sound::EntityItemFrameRemoveItem
        }
    }

    #[must_use]
    pub const fn get_rotate_item_sound(&self) -> Sound {
        if self.is_glow() {
            Sound::EntityGlowItemFrameRotateItem
        } else {
            Sound::EntityItemFrameRotateItem
        }
    }

    #[must_use]
    pub const fn get_break_sound(&self) -> Sound {
        if self.is_glow() {
            Sound::EntityGlowItemFrameBreak
        } else {
            Sound::EntityItemFrameBreak
        }
    }

    #[must_use]
    pub const fn get_place_sound(&self) -> Sound {
        if self.is_glow() {
            Sound::EntityGlowItemFramePlace
        } else {
            Sound::EntityItemFramePlace
        }
    }

    #[must_use]
    pub fn get_facing(&self) -> BlockDirection {
        BlockDirection::from_index(self.facing.load(Ordering::Relaxed))
            .unwrap_or(Self::DEFAULT_FACING)
    }

    /// Vanilla `ItemFrame.setDirection`.
    pub fn set_facing(&self, facing: BlockDirection) {
        self.facing.store(facing.to_index(), Ordering::Relaxed);
        sync_direction(&self.entity, facing);
        let (yaw, pitch) = match facing {
            BlockDirection::Up => (0.0, -90.0),
            BlockDirection::Down => (0.0, 90.0),
            horizontal => (f32::from(facing_to_horizontal(horizontal)) * 90.0, 0.0),
        };
        self.entity.set_rotation(yaw, pitch);
        self.recalculate_bounding_box();
    }

    /// Vanilla `createBoundingBox`.
    fn create_bounding_box(
        pos: BlockPos,
        direction: BlockDirection,
        has_framed_map: bool,
    ) -> BoundingBox {
        let size = if has_framed_map {
            MAP_FRAME_SIZE
        } else {
            FRAME_SIZE
        };
        let step = direction.to_offset();
        BoundingBox::of_size(
            wall_center(pos, direction),
            if step.x == 0 { size } else { DEPTH },
            if step.y == 0 { size } else { DEPTH },
            if step.z == 0 { size } else { DEPTH },
        )
    }

    /// Vanilla `recalculateBoundingBox`.
    fn recalculate_bounding_box(&self) {
        let (pos, facing) = (self.attachment.pos(), self.get_facing());
        apply_bounding_box(
            &self.entity,
            wall_center(pos, facing),
            Self::create_bounding_box(pos, facing, self.has_framed_map()),
        );
    }

    /// Vanilla `hasFramedMap`.
    #[must_use]
    pub fn has_framed_map(&self) -> bool {
        self.item_stack
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get_data_component::<MapIdImpl>()
            .is_some()
    }

    #[must_use]
    pub fn get_item(&self) -> ItemStack {
        self.item_stack
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// The part of vanilla `setItem` that holds for loading too: no sound, no neighbours.
    fn store_item(&self, mut item_stack: ItemStack) {
        if !item_stack.is_empty() {
            item_stack.item_count = 1;
        }
        let item_serializer = ItemStackSerializer::from(item_stack.clone());
        *self
            .item_stack
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = item_stack;
        self.entity
            .set_synced_data(item_frame::ITEM, item_serializer);
        self.recalculate_bounding_box();
    }

    /// Vanilla `setItem`.
    pub fn set_item(&self, item_stack: ItemStack, update_neighbours: bool) {
        let play_sound = !item_stack.is_empty();
        self.store_item(item_stack);
        if play_sound {
            self.play_entity_sound(self.get_add_item_sound(), 1.0, 1.0);
        }
        if update_neighbours {
            self.update_output_signal();
        }
    }

    /// Vanilla `Level.updateNeighbourForOutputSignal(pos, Blocks.AIR)`.
    fn update_output_signal(&self) {
        self.entity
            .world
            .load()
            .update_neighbour_for_output_signal(&self.attachment.pos(), &Block::AIR);
    }

    #[must_use]
    pub fn get_rotation(&self) -> u8 {
        self.rotation.load(Ordering::Relaxed) % NUM_ROTATIONS
    }

    /// Vanilla `setRotation`.
    pub fn set_rotation(&self, rotation: u8, update_neighbours: bool) {
        let rot = rotation % NUM_ROTATIONS;
        self.rotation.store(rot, Ordering::Relaxed);
        self.entity
            .set_synced_data(item_frame::ROTATION, i32::from(rot));
        if update_neighbours {
            self.update_output_signal();
        }
    }

    #[must_use]
    pub fn get_drop_chance(&self) -> f32 {
        self.item_drop_chance.load()
    }

    pub fn set_drop_chance(&self, chance: f32) {
        self.item_drop_chance.store(chance);
    }

    #[must_use]
    pub fn is_fixed(&self) -> bool {
        self.fixed.load(Ordering::Relaxed)
    }

    pub fn set_fixed(&self, fixed: bool) {
        self.fixed.store(fixed, Ordering::Relaxed);
    }

    #[must_use]
    pub fn is_invisible(&self) -> bool {
        self.entity.invisible.load(Ordering::Relaxed)
    }

    pub fn set_invisible(&self, invisible: bool) {
        self.entity.set_invisible(invisible);
    }

    #[must_use]
    pub fn get_frame_item_stack(&self) -> ItemStack {
        if self.is_glow() {
            ItemStack::new(1, &Item::GLOW_ITEM_FRAME)
        } else {
            ItemStack::new(1, &Item::ITEM_FRAME)
        }
    }

    #[must_use]
    pub fn get_frame_item_stack_with_data(&self) -> ItemStack {
        let mut stack = self.get_frame_item_stack();
        copy_custom_name(&self.entity, &mut stack);
        stack
    }

    #[must_use]
    pub fn get_pick_result(&self) -> ItemStack {
        let framed_stack = self.get_item();
        if framed_stack.is_empty() {
            self.get_frame_item_stack_with_data()
        } else {
            framed_stack
        }
    }

    /// The comparator signal this frame produces.
    ///
    /// Vanilla: `getItem().isEmpty() ? 0 : getRotation() % 8 + 1`.
    #[must_use]
    pub fn get_analog_output(&self) -> u8 {
        if self
            .item_stack
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_empty()
        {
            0
        } else {
            self.get_rotation() + 1
        }
    }

    /// Vanilla private `dropItem(level, causedBy, withFrame)`.
    pub fn drop_item(&self, caused_by: Option<&dyn EntityBase>, with_frame: bool) {
        if self.is_fixed() {
            return;
        }

        let item_stack = self.get_item();
        self.set_item(ItemStack::EMPTY.clone(), true);

        let entity_drops = self
            .entity
            .world
            .load()
            .level_info
            .load()
            .game_rules
            .entity_drops;
        if !entity_drops || is_creative_player(caused_by) {
            return;
        }

        let facing = self.get_facing();
        if with_frame {
            spawn_hanging_drop(&self.entity, facing, self.get_frame_item_stack_with_data());
        }
        if !item_stack.is_empty() && rand::random::<f32>() < self.item_drop_chance.load() {
            spawn_hanging_drop(&self.entity, facing, item_stack);
        }
    }
}

impl BlockAttached for ItemFrameEntity {
    fn attachment(&self) -> &BlockAttachment {
        &self.attachment
    }

    /// Vanilla `ItemFrame.survives`.
    fn survives(&self) -> bool {
        if self.is_fixed() {
            return true;
        }
        let world = self.entity.world.load();
        let (pos, facing) = (self.attachment.pos(), self.get_facing());
        let pop_box = Self::create_bounding_box(pos, facing, false);
        if has_level_collision(&world, pop_box) {
            return false;
        }
        let (block, state) = world.get_block_and_state(&pos.offset(facing.opposite().to_offset()));
        (state.is_solid() || (facing.is_horizontal() && is_diode(block)))
            && can_coexist(
                &world,
                Some(self.entity.entity_id),
                self.entity.entity_type,
                facing,
                &pop_box,
                true,
            )
    }

    /// Vanilla public `dropItem(level, causedBy)`.
    fn drop_attached_item(&self, caused_by: Option<&dyn EntityBase>) {
        self.play_entity_sound(self.get_break_sound(), 1.0, 1.0);
        self.drop_item(caused_by, true);
        emit_game_event(&self.entity, GameEvent::BlockChange, caused_by);
    }
}

impl EntityBase for ItemFrameEntity {
    fn write_custom_nbt(&self, nbt: &mut NbtCompound) {
        self.attachment.write_nbt(nbt);
        let item = self.get_item();
        if !item.is_empty() {
            let mut item_compound = NbtCompound::new();
            item.write_item_stack(&mut item_compound);
            nbt.put_compound("Item", item_compound);
        }
        nbt.put_byte("ItemRotation", self.get_rotation() as i8);
        nbt.put_float("ItemDropChance", self.item_drop_chance.load());
        nbt.put_byte("Facing", self.get_facing().to_index() as i8);
        nbt.put_bool("Invisible", self.is_invisible());
        nbt.put_bool("Fixed", self.is_fixed());
    }

    /// Vanilla `readAdditionalSaveData`: a missing item empties the frame, a missing facing is
    /// down.
    fn read_custom_nbt(&self, nbt: &NbtCompound) {
        if !self.attachment.read_nbt(nbt, self.entity.block_pos.load()) {
            // Saves from before `block_pos` was stored: the center is always in that block.
            self.attachment.set_pos(self.entity.block_pos.load());
        }
        self.store_item(
            nbt.get_compound("Item")
                .and_then(ItemStack::read_item_stack)
                .unwrap_or_else(|| ItemStack::EMPTY.clone()),
        );
        self.set_rotation(nbt.get_byte("ItemRotation").unwrap_or(0) as u8, false);
        self.item_drop_chance
            .store(nbt.get_float("ItemDropChance").unwrap_or(1.0));
        self.set_facing(
            BlockDirection::from_index(nbt.get_byte("Facing").unwrap_or(0) as u8 % 6)
                .unwrap_or(BlockDirection::Down),
        );
        self.entity
            .set_invisible(nbt.get_bool("Invisible").unwrap_or(false));
        self.set_fixed(nbt.get_bool("Fixed").unwrap_or(false));
    }

    fn get_entity(&self) -> &Entity {
        &self.entity
    }

    fn get_living_entity(&self) -> Option<&LivingEntity> {
        None
    }

    fn tick(&self, caller: &dyn EntityBase, _server: &Server) {
        self.tick_attached(caller);
    }

    fn init_data_tracker(&self) {
        self.recalculate_bounding_box();
    }

    fn send_java_spawn_packet(&self, client: &JavaClient) {
        send_spawn_at_block(self, self.attachment.pos(), client);
    }

    /// Vanilla `ItemFrame.interact`.
    fn interact_with_hand(
        &self,
        player: &Arc<Player>,
        item_stack: &mut ItemStack,
        _hand: pumpkin_util::Hand,
    ) -> Option<bool> {
        if self.is_fixed() {
            return Some(false);
        }

        if !self.get_item().is_empty() {
            self.play_entity_sound(self.get_rotate_item_sound(), 1.0, 1.0);
            self.set_rotation(self.get_rotation() + 1, true);
            emit_game_event(&self.entity, GameEvent::BlockChange, Some(&**player));
            return Some(true);
        }

        if item_stack.is_empty() || self.entity.is_removed() {
            return Some(false);
        }
        self.set_item(item_stack.clone(), true);
        emit_game_event(&self.entity, GameEvent::BlockChange, Some(&**player));
        item_stack.decrement_unless_creative(player.gamemode.load(), 1);
        Some(true)
    }

    /// Vanilla `ItemFrame.hurtServer`.
    fn damage_with_context(
        &self,
        _caller: &dyn EntityBase,
        _amount: f32,
        damage_type: DamageType,
        _position: Option<Vector3<f64>>,
        _source: Option<&dyn EntityBase>,
        cause: Option<&dyn EntityBase>,
    ) -> bool {
        if self.is_fixed() {
            // Vanilla `canHurtWhenFixed`.
            return (damage_type.has_tag(&tag::DamageType::MINECRAFT_BYPASSES_INVULNERABILITY)
                || is_creative_player(cause))
                && self.hurt_attached(damage_type, cause);
        }
        if self.entity.is_invulnerable_to(&damage_type, cause) {
            return false;
        }
        // Vanilla `shouldDamageDropItem`: the first hit only takes the item out.
        if !damage_type.has_tag(&tag::DamageType::MINECRAFT_IS_EXPLOSION)
            && !self.get_item().is_empty()
        {
            self.drop_item(cause, false);
            emit_game_event(&self.entity, GameEvent::BlockChange, cause);
            self.play_entity_sound(self.get_remove_item_sound(), 1.0, 1.0);
            return true;
        }
        self.hurt_attached(damage_type, cause)
    }

    fn kill(&self, _caller: &dyn EntityBase) {
        self.kill_attached(None);
    }

    fn can_hit(&self) -> bool {
        self.entity.is_alive()
    }

    /// Vanilla `ItemFrame.move`: a fixed frame stays.
    fn move_entity(&self, _caller: &dyn EntityBase, motion: Vector3<f64>) {
        if !self.is_fixed() {
            self.moved_off_block(motion);
        }
    }

    /// Vanilla `ItemFrame.push`: a fixed frame stays.
    fn push_from_explosion(&self, impulse: Vector3<f64>) {
        if !self.is_fixed() {
            self.moved_off_block(impulse);
        }
    }

    fn ignore_explosion(&self, affects_blocklike_entities: bool, source_in_water: bool) -> bool {
        attached_ignore_explosion(affects_blocklike_entities, source_in_water)
    }

    fn skip_attack_interaction(&self, player: &Player) -> bool {
        self.skip_attack_on_attached(player)
    }

    /// Vanilla `BlockAttachedEntity.thunderHit` does nothing.
    fn on_lightning_strike(
        &self,
        _caller: &dyn EntityBase,
        _lightning: &crate::entity::lightning::LightningBoltEntity,
    ) {
    }

    fn cast_any(&self) -> &dyn std::any::Any {
        self
    }
}
