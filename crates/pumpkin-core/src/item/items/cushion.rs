use crate::block::registry::BlockActionResult;
use std::sync::Arc;

use crate::entity::Entity;
use crate::entity::decoration::cushion::CushionEntity;
use crate::entity::player::Player;
use crate::item::items::spawn_egg::apply_entity_variant;
use crate::item::{ItemBehaviour, ItemMetadata};
use crate::server::Server;
use pumpkin_data::entity::EntityType;
use pumpkin_data::game_event::GameEvent;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::tag::{self, Taggable};
use pumpkin_data::{Block, BlockDirection};
use pumpkin_util::math::boundingbox::{BoundingBox, EntityDimensions};
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_world::clip::intersects_aabb_with_hit;

/// Vanilla `CushionItem.COLLISION_SHAPE_RAYCAST_EPSILON`.
const COLLISION_SHAPE_RAYCAST_EPSILON: f64 = 0.001;

pub struct CushionItem;

/// Vanilla `CushionItem.recalculateContextForSpecialCollisionShapes`: on a block whose collision
/// is taller than its outline, like a fence, the click moves to the collision shape.
fn recalculate_hit(
    player: &Player,
    location: BlockPos,
    block: &Block,
    face: BlockDirection,
    click: Vector3<f64>,
) -> (BlockDirection, Vector3<f64>) {
    if !block.has_tag(&tag::Block::MINECRAFT_CUSHION_USES_COLLISION_SHAPE) {
        return (face, click);
    }
    let from = player.get_entity().get_eye_pos();
    let ray = click.sub(&from).normalize();
    let to = click.add(&(ray * COLLISION_SHAPE_RAYCAST_EPSILON));
    let state = player.world().get_block_state(&location);
    state
        .get_block_collision_shapes_at(&location)
        .filter_map(|shape| {
            let shape = shape.at_pos(location);
            intersects_aabb_with_hit(from, to, shape.min, shape.max)
        })
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map_or((face, click), |(_, hit_face, hit_pos, _)| (hit_face, hit_pos))
}

impl ItemMetadata for CushionItem {
    fn ids() -> Box<[u16]> {
        [
            Item::WHITE_CUSHION.id,
            Item::ORANGE_CUSHION.id,
            Item::MAGENTA_CUSHION.id,
            Item::LIGHT_BLUE_CUSHION.id,
            Item::YELLOW_CUSHION.id,
            Item::LIME_CUSHION.id,
            Item::PINK_CUSHION.id,
            Item::GRAY_CUSHION.id,
            Item::LIGHT_GRAY_CUSHION.id,
            Item::CYAN_CUSHION.id,
            Item::PURPLE_CUSHION.id,
            Item::BLUE_CUSHION.id,
            Item::BROWN_CUSHION.id,
            Item::GREEN_CUSHION.id,
            Item::RED_CUSHION.id,
            Item::BLACK_CUSHION.id,
        ]
        .into()
    }
}

impl ItemBehaviour for CushionItem {
    fn use_on_block(
        &self,
        item: &mut ItemStack,
        player: &Player,
        location: BlockPos,
        face: BlockDirection,
        cursor_pos: Vector3<f32>,
        block: &Block,
        _server: &Server,
    ) -> BlockActionResult {
        let click = Vector3::new(
            f64::from(location.0.x) + f64::from(cursor_pos.x),
            f64::from(location.0.y) + f64::from(cursor_pos.y),
            f64::from(location.0.z) + f64::from(cursor_pos.z),
        );
        let (face, click) = recalculate_hit(player, location, block, face, click);
        if face != BlockDirection::Up {
            return BlockActionResult::Fail;
        }

        let world = player.world();
        let target_pos = if world.get_block_state(&location).replaceable() {
            location
        } else {
            location.offset(face.to_offset())
        };
        let bottom_center = Vector3::new(
            f64::from(target_pos.0.x) + 0.5,
            click.y,
            f64::from(target_pos.0.z) + 0.5,
        );

        let [width, height] = EntityType::CUSHION.dimension;
        let bounding_box = BoundingBox::new_from_pos(
            bottom_center.x,
            bottom_center.y,
            bottom_center.z,
            &EntityDimensions::new(width, height, height),
        );

        if !CushionEntity::can_be_placed_at(&world, &bounding_box)
            || world
                .get_entities_at_box(&bounding_box)
                .iter()
                .any(|e| e.get_entity().entity_type == &EntityType::CUSHION)
        {
            return BlockActionResult::Fail;
        }

        let (player_yaw, _) = player.rotation();
        // Vanilla `Direction.fromYRot(...).toYRot()`.
        let rotation = ((f64::from(player_yaw) / 90.0 + 0.5).floor() as i32 & 3) * 90;
        let entity = Entity::new(world.clone(), bottom_center, &EntityType::CUSHION);
        entity.set_rotation(rotation as f32, 0.0);

        let pos = entity.pos.load();
        world.play_sound_fine(
            Sound::EntityCushionPlace,
            SoundCategory::Blocks,
            &pos,
            0.75,
            0.8,
        );

        let cushion = Arc::new(CushionEntity::new(
            entity,
            CushionEntity::color_from_stack(item),
        ));
        // Vanilla `EntityType.createDefaultStackConfig`: the custom name and `entity_data`.
        apply_entity_variant(item, &*cushion, Some(player));
        if world.spawn_entity(cushion.clone()) {
            cushion.destroy_if_in_fire();
        }
        world.emit_game_event(GameEvent::EntityPlace.name(), pos);
        item.decrement_unless_creative(player.gamemode.load(), 1);
        BlockActionResult::Success
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}
