use std::any::Any;
use std::sync::Arc;

use crate::block::registry::BlockActionResult;
use crate::entity::EntityBase;
use crate::entity::decoration::block_attached::{BlockAttached, emit_game_event};
use crate::entity::decoration::item_frame::ItemFrameEntity;
use crate::entity::decoration::painting::PaintingEntity;
use crate::entity::player::Player;
use crate::item::items::spawn_egg::apply_entity_variant;
use crate::item::{ItemBehaviour, ItemMetadata};
use crate::server::Server;
use crate::world::World;
use pumpkin_data::data_component_impl::PaintingVariantImpl;
use pumpkin_data::entity::EntityType;
use pumpkin_data::game_event::GameEvent;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::Sound;
use pumpkin_data::{Block, BlockDirection};
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;

pub struct HangingEntityItem;

impl ItemMetadata for HangingEntityItem {
    fn ids() -> Box<[u16]> {
        [
            Item::PAINTING.id,
            Item::ITEM_FRAME.id,
            Item::GLOW_ITEM_FRAME.id,
        ]
        .into()
    }
}

/// The end of vanilla `HangingEntityItem.useOn`: a candidate that survives enters the world.
fn place_if_survives<T: BlockAttached>(
    entity: T,
    place_sound: Sound,
    world: &Arc<World>,
    item: &mut ItemStack,
    player: &Player,
) -> BlockActionResult {
    if !entity.survives() {
        return BlockActionResult::Consume;
    }
    entity.play_entity_sound(place_sound, 1.0, 1.0);
    emit_game_event(entity.get_entity(), GameEvent::EntityPlace, Some(player));
    world.spawn_entity(Arc::new(entity));
    item.decrement_unless_creative(player.gamemode.load(), 1);
    BlockActionResult::Success
}

impl ItemBehaviour for HangingEntityItem {
    /// Vanilla `HangingEntityItem.useOn` and the `mayPlace` of `ItemFrameItem`.
    fn use_on_block(
        &self,
        item: &mut ItemStack,
        player: &Player,
        location: BlockPos,
        face: BlockDirection,
        _cursor_pos: Vector3<f32>,
        _block: &Block,
        _server: &Server,
    ) -> BlockActionResult {
        let world = player.world();
        let target_pos = location.offset(face.to_offset());

        if item.item.id == Item::PAINTING.id {
            if !face.is_horizontal() {
                return BlockActionResult::Fail;
            }
            let Some(variant) = PaintingEntity::choose_variant(&world, target_pos, face) else {
                return BlockActionResult::Consume;
            };
            let painting = PaintingEntity::create(&world, target_pos, face, variant);
            // Vanilla `EntityType.createDefaultStackConfig`: the item's components come last,
            // and a variant they name still has to survive on this wall.
            if let Some(component) = item.get_data_component::<PaintingVariantImpl>() {
                painting.set_variant_name(&component.value);
            }
            apply_entity_variant(item, &painting, Some(player));
            return place_if_survives(painting, Sound::EntityPaintingPlace, &world, item, player);
        }

        if !world.is_in_build_limit(target_pos) {
            return BlockActionResult::Fail;
        }
        let entity_type = if item.item.id == Item::GLOW_ITEM_FRAME.id {
            &EntityType::GLOW_ITEM_FRAME
        } else {
            &EntityType::ITEM_FRAME
        };
        let frame = ItemFrameEntity::create(&world, entity_type, target_pos, face);
        apply_entity_variant(item, &frame, Some(player));
        let place_sound = frame.get_place_sound();
        place_if_survives(frame, place_sound, &world, item, player)
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}
