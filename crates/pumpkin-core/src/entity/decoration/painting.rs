use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use crate::entity::decoration::block_attached::{
    BlockAttached, BlockAttachment, DEPTH, apply_bounding_box, attached_ignore_explosion,
    can_coexist, copy_custom_name, has_level_collision, is_creative_player, is_supported,
    send_spawn_at_block, spawn_hanging_drop, sync_direction, wall_center,
};
use crate::entity::player::Player;
use crate::entity::{Entity, EntityBase, living::LivingEntity};
use crate::net::java::JavaClient;
use crate::server::Server;
use crate::world::World;
use pumpkin_data::BlockDirection;
use pumpkin_data::damage::DamageType;
use pumpkin_data::entity::EntityType;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::painting_variant::PaintingVariant;
use pumpkin_data::sound::Sound;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_protocol::java::client::play::Metadata;
use pumpkin_util::math::boundingbox::BoundingBox;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_util::version::JavaMinecraftVersion;

/// The world stores a painting facing as a horizontal value: 0 south, 1 west,
/// 2 north, 3 east (`Direction.get2DDataValue`). The entity data, and the spawn
/// packet built from it, use the 3D index instead: north 2, south 3, west 4,
/// east 5. Sending the raw horizontal value makes the client read 0 as down,
/// fail the horizontal check in `HangingEntity.setDirection` and disconnect.
const fn facing_from_horizontal(value: u8) -> BlockDirection {
    match value & 3 {
        1 => BlockDirection::West,
        2 => BlockDirection::North,
        3 => BlockDirection::East,
        _ => BlockDirection::South,
    }
}

/// Vanilla reads the field with `getByte`, which yields 0 (south) when it is
/// absent, so a partial entity file keeps a horizontal default.
fn facing_from_nbt(nbt: &NbtCompound) -> BlockDirection {
    facing_from_horizontal(nbt.get_byte("facing").unwrap_or(0) as u8)
}

/// Vanilla `Direction.get2DDataValue`.
pub(super) const fn facing_to_horizontal(direction: BlockDirection) -> u8 {
    match direction {
        BlockDirection::West => 1,
        BlockDirection::North => 2,
        BlockDirection::East => 3,
        _ => 0,
    }
}

/// Vanilla `offsetForPaintingSize`.
const fn offset_for_painting_size(size: u32) -> f64 {
    if size.is_multiple_of(2) { 0.5 } else { 0.0 }
}

pub struct PaintingEntity {
    pub entity: Entity,
    attachment: BlockAttachment,
    variant_id: AtomicU32,
}

impl PaintingEntity {
    #[must_use]
    pub fn new(entity: Entity) -> Self {
        Self::new_with_variant(entity, PaintingVariant::Alban)
    }

    #[must_use]
    pub fn new_with_variant(entity: Entity, variant: PaintingVariant) -> Self {
        // Vanilla `HangingEntity.DEFAULT_DIRECTION`; the client refuses a vertical one.
        sync_direction(&entity, BlockDirection::South);
        let pos = entity.block_pos.load();
        Self {
            entity,
            attachment: BlockAttachment::new(pos),
            variant_id: AtomicU32::new(variant.id()),
        }
    }

    /// Vanilla `Painting(Level, BlockPos, Direction, Holder)`, attached in the block `pos`.
    #[must_use]
    pub fn create(
        world: &Arc<World>,
        pos: BlockPos,
        direction: BlockDirection,
        variant: PaintingVariant,
    ) -> Self {
        let center = Self::calculate_center(pos, direction, variant.width(), variant.height());
        let entity = Entity::new(world.clone(), center, &EntityType::PAINTING);
        let painting = Self::new_with_variant(entity, variant);
        painting.attachment.set_pos(pos);
        painting.set_direction(direction);
        painting
    }

    #[must_use]
    pub fn variant(&self) -> PaintingVariant {
        let id = self.variant_id.load(Ordering::Relaxed);
        PaintingVariant::all()
            .get(id as usize)
            .copied()
            .unwrap_or(PaintingVariant::Alban)
    }

    pub fn set_variant(&self, variant: PaintingVariant) {
        self.variant_id.store(variant.id(), Ordering::Relaxed);
        self.sync_variant();
        self.recalculate_bounding_box();
    }

    pub fn sync_variant(&self) {
        self.entity.set_synced_data(
            pumpkin_data::tracked_data::painting::DATA_PAINTING_VARIANT_ID,
            VarInt(self.variant_id.load(Ordering::Relaxed) as i32),
        );
    }

    /// Vanilla `HangingEntity.getDirection`.
    #[must_use]
    pub fn direction(&self) -> BlockDirection {
        BlockDirection::from_index(self.entity.data.load(Ordering::Relaxed) as u8)
            .unwrap_or(BlockDirection::South)
    }

    /// Vanilla `HangingEntity.setDirection`.
    pub fn set_direction(&self, direction: BlockDirection) {
        sync_direction(&self.entity, direction);
        self.entity
            .set_rotation(f32::from(facing_to_horizontal(direction)) * 90.0, 0.0);
        self.recalculate_bounding_box();
    }

    /// Vanilla `recalculateBoundingBox`.
    fn recalculate_bounding_box(&self) {
        let variant = self.variant();
        let (width, height) = (variant.width(), variant.height());
        let (pos, direction) = (self.attachment.pos(), self.direction());
        apply_bounding_box(
            &self.entity,
            Self::calculate_center(pos, direction, width, height),
            Self::calculate_bounding_box(pos, direction, width, height),
        );
    }

    /// The center of vanilla `calculateBoundingBox` for a painting attached in the block `pos`.
    #[must_use]
    pub fn calculate_center(
        pos: BlockPos,
        direction: BlockDirection,
        width: u32,
        height: u32,
    ) -> Vector3<f64> {
        let attached_to_wall = wall_center(pos, direction);
        let horizontal_offset = offset_for_painting_size(width);
        let left = direction.rotate_counter_clockwise().to_offset().to_f64();
        Vector3::new(
            attached_to_wall.x + left.x * horizontal_offset,
            attached_to_wall.y + offset_for_painting_size(height),
            attached_to_wall.z + left.z * horizontal_offset,
        )
    }

    /// Vanilla `calculateBoundingBox`.
    #[must_use]
    pub fn calculate_bounding_box(
        pos: BlockPos,
        direction: BlockDirection,
        width: u32,
        height: u32,
    ) -> BoundingBox {
        let step = direction.to_offset();
        let x_size = if step.x == 0 { f64::from(width) } else { DEPTH };
        let z_size = if step.z == 0 { f64::from(width) } else { DEPTH };
        BoundingBox::of_size(
            Self::calculate_center(pos, direction, width, height),
            x_size,
            f64::from(height),
            z_size,
        )
    }

    /// Vanilla `HangingEntity.survives` for a painting of `variant` in the block `pos`.
    fn variant_survives(
        world: &World,
        own: Option<i32>,
        pos: BlockPos,
        direction: BlockDirection,
        variant: PaintingVariant,
    ) -> bool {
        let bounds =
            Self::calculate_bounding_box(pos, direction, variant.width(), variant.height());
        !has_level_collision(world, bounds)
            && is_supported(world, bounds, direction)
            && can_coexist(world, own, &EntityType::PAINTING, direction, &bounds, false)
    }

    /// Vanilla `Painting.create`: of the placeable variants that survive in the block `pos`, a
    /// random one with the largest area.
    #[must_use]
    pub fn choose_variant(
        world: &World,
        pos: BlockPos,
        direction: BlockDirection,
    ) -> Option<PaintingVariant> {
        use rand::seq::IndexedRandom;

        let mut fitting_variants = Vec::new();
        let mut max_area = 0;

        for &variant in PaintingVariant::all_placeable() {
            if Self::variant_survives(world, None, pos, direction, variant) {
                let area = variant.width() * variant.height();
                if area > max_area {
                    max_area = area;
                    fitting_variants.clear();
                    fitting_variants.push(variant);
                } else if area == max_area {
                    fitting_variants.push(variant);
                }
            }
        }

        let mut rng = rand::rng();
        fitting_variants.choose(&mut rng).copied()
    }

    /// The attached block of a save from before `block_pos` was stored: undoes the size offsets
    /// of [`Self::calculate_center`] on the saved position.
    fn block_pos_from_center(&self) -> BlockPos {
        let center = self.entity.pos.load();
        let variant = self.variant();
        let horizontal_offset = offset_for_painting_size(variant.width());
        let left = self
            .direction()
            .rotate_counter_clockwise()
            .to_offset()
            .to_f64();
        BlockPos::floored(
            center.x - left.x * horizontal_offset,
            center.y - offset_for_painting_size(variant.height()),
            center.z - left.z * horizontal_offset,
        )
    }
}

impl BlockAttached for PaintingEntity {
    fn attachment(&self) -> &BlockAttachment {
        &self.attachment
    }

    fn survives(&self) -> bool {
        Self::variant_survives(
            &self.entity.world.load(),
            Some(self.entity.entity_id),
            self.attachment.pos(),
            self.direction(),
            self.variant(),
        )
    }

    /// Vanilla `Painting.dropItem`.
    fn drop_attached_item(&self, caused_by: Option<&dyn EntityBase>) {
        let entity_drops = self
            .entity
            .world
            .load()
            .level_info
            .load()
            .game_rules
            .entity_drops;
        if !entity_drops {
            return;
        }
        self.play_entity_sound(Sound::EntityPaintingBreak, 1.0, 1.0);
        if !is_creative_player(caused_by) {
            let mut stack = ItemStack::new(1, &Item::PAINTING);
            copy_custom_name(&self.entity, &mut stack);
            spawn_hanging_drop(&self.entity, self.direction(), stack);
        }
    }
}

impl EntityBase for PaintingEntity {
    fn write_custom_nbt(&self, nbt: &mut NbtCompound) {
        nbt.put_byte("facing", facing_to_horizontal(self.direction()) as i8);
        self.attachment.write_nbt(nbt);
        let variant_id = self.variant_id.load(Ordering::Relaxed);
        let variant_str =
            self.entity
                .resolve_dynamic_entry_id("minecraft:painting_variant", variant_id, |_id| {
                    self.variant().asset_id().to_string()
                });
        nbt.put_string("variant", variant_str);
    }

    fn read_custom_nbt(&self, nbt: &NbtCompound) {
        let facing = facing_from_nbt(nbt);
        sync_direction(&self.entity, facing);
        if let Some(variant_str) = nbt
            .get_string("variant")
            .or_else(|| nbt.get_string("Motive"))
            && let Some(raw_id) =
                self.entity
                    .resolve_dynamic_raw_id("minecraft:painting_variant", variant_str, |n| {
                        PaintingVariant::from_name(n).map(|v| v.id())
                    })
        {
            self.variant_id.store(raw_id, Ordering::Relaxed);
            self.sync_variant();
        }
        if !self.attachment.read_nbt(nbt, self.entity.block_pos.load()) {
            self.attachment.set_pos(self.block_pos_from_center());
        }
        self.set_direction(facing);
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
        self.sync_variant();
        self.recalculate_bounding_box();
    }

    fn java_spawn_metadata(&self, version: JavaMinecraftVersion) -> Option<Box<[u8]>> {
        let mut metadata = Vec::new();
        Metadata::new(
            pumpkin_data::tracked_data::painting::DATA_PAINTING_VARIANT_ID,
            VarInt(self.variant_id.load(Ordering::Relaxed) as i32),
        )
        .write(&mut metadata, &version)
        .ok()?;
        metadata.push(255);
        Some(metadata.into_boxed_slice())
    }

    fn send_java_spawn_packet(&self, client: &JavaClient) {
        send_spawn_at_block(self, self.attachment.pos(), client);
    }

    fn set_variant_name(&self, name: &str) {
        if let Some(variant) = PaintingVariant::from_name(name) {
            self.set_variant(variant);
        } else if let Some(server) = self.entity.world.load().server.upgrade()
            && let Some(raw_id) = server
                .datapack_manager
                .get_dynamic_registry_raw_id("minecraft:painting_variant", name)
        {
            self.variant_id.store(raw_id, Ordering::Relaxed);
            self.sync_variant();
            self.recalculate_bounding_box();
        }
    }

    fn damage_with_context(
        &self,
        _caller: &dyn EntityBase,
        _amount: f32,
        damage_type: DamageType,
        _position: Option<Vector3<f64>>,
        _source: Option<&dyn EntityBase>,
        cause: Option<&dyn EntityBase>,
    ) -> bool {
        self.hurt_attached(damage_type, cause)
    }

    fn kill(&self, _caller: &dyn EntityBase) {
        self.kill_attached(None);
    }

    fn can_hit(&self) -> bool {
        self.entity.is_alive()
    }

    fn move_entity(&self, _caller: &dyn EntityBase, motion: Vector3<f64>) {
        self.moved_off_block(motion);
    }

    fn push_from_explosion(&self, impulse: Vector3<f64>) {
        self.moved_off_block(impulse);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn horizontal_and_3d_facings_round_trip() {
        for horizontal in 0u8..4 {
            let direction = facing_from_horizontal(horizontal);
            assert!(direction.is_horizontal(), "{direction:?} is vertical");
            assert_eq!(facing_to_horizontal(direction), horizontal);
        }
    }

    #[test]
    fn south_two_is_not_down_zero() {
        // The old bug: south (0 in the world file) was sent as index 0, which the
        // client reads as down and rejects.
        assert_eq!(facing_from_horizontal(0), BlockDirection::South);
        assert_eq!(facing_from_horizontal(0).to_index(), 3);
    }

    #[test]
    fn missing_facing_defaults_to_south() {
        let nbt = NbtCompound::new();
        assert_eq!(facing_from_nbt(&nbt), BlockDirection::South);
        assert_eq!(facing_from_nbt(&nbt).to_index(), 3);

        let mut with_facing = NbtCompound::new();
        with_facing.put_byte("facing", 1);
        assert_eq!(facing_from_nbt(&with_facing), BlockDirection::West);
    }

    #[test]
    fn out_of_range_values_wrap_into_horizontal_directions() {
        assert!(facing_from_horizontal(4).is_horizontal());
        assert!(facing_from_horizontal(255).is_horizontal());
    }

    #[test]
    fn calculate_center_works() {
        // Attached in the block north of a wall at (10, 64, 20).
        let location = BlockPos(Vector3::new(10, 64, 19));

        // 1x1 painting facing North
        let pos_1x1 = PaintingEntity::calculate_center(location, BlockDirection::North, 1, 1);
        assert_eq!(pos_1x1.x, 10.5);
        assert_eq!(pos_1x1.y, 64.5);
        assert!((pos_1x1.z - 19.96875).abs() < 1e-6);

        // 2x1 painting facing North: width is 2, shifts along counter-clockwise (West = -X)
        let pos_2x1 = PaintingEntity::calculate_center(location, BlockDirection::North, 2, 1);
        assert_eq!(pos_2x1.x, 10.0);
        assert_eq!(pos_2x1.y, 64.5);
        assert!((pos_2x1.z - 19.96875).abs() < 1e-6);

        // 2x2 painting facing North: height is 2, shifts +0.5 along Y
        let pos_2x2 = PaintingEntity::calculate_center(location, BlockDirection::North, 2, 2);
        assert_eq!(pos_2x2.x, 10.0);
        assert_eq!(pos_2x2.y, 65.0);
        assert!((pos_2x2.z - 19.96875).abs() < 1e-6);

        // 4x4 painting facing North
        let pos_4x4 = PaintingEntity::calculate_center(location, BlockDirection::North, 4, 4);
        assert_eq!(pos_4x4.x, 10.0);
        assert_eq!(pos_4x4.y, 65.0);
        assert!((pos_4x4.z - 19.96875).abs() < 1e-6);

        // 1x1 painting facing South, attached in the block south of the same wall
        let south_of_wall = BlockPos(Vector3::new(10, 64, 21));
        let pos_south =
            PaintingEntity::calculate_center(south_of_wall, BlockDirection::South, 1, 1);
        assert_eq!(pos_south.x, 10.5);
        assert_eq!(pos_south.y, 64.5);
        assert!((pos_south.z - 21.03125).abs() < 1e-6);
    }

    #[test]
    fn bounding_box_is_thin_on_the_facing_axis() {
        let pos = BlockPos(Vector3::new(10, 64, 19));
        let bounds = PaintingEntity::calculate_bounding_box(pos, BlockDirection::North, 4, 3);
        assert!((bounds.max.x - bounds.min.x - 4.0).abs() < 1e-9);
        assert!((bounds.max.y - bounds.min.y - 3.0).abs() < 1e-9);
        assert!((bounds.max.z - bounds.min.z - DEPTH).abs() < 1e-9);
    }

    #[test]
    fn variant_resolution() {
        assert_eq!(
            PaintingVariant::from_name("minecraft:kebab"),
            Some(PaintingVariant::Kebab)
        );
        assert_eq!(
            PaintingVariant::from_name("kebab"),
            Some(PaintingVariant::Kebab)
        );
        assert_eq!(PaintingVariant::from_name("invalid"), None);
        assert_eq!(PaintingVariant::Kebab.asset_id(), "minecraft:kebab");
    }
}
