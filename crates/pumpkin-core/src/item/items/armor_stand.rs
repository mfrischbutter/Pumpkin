use crate::block::registry::BlockActionResult;

use crate::entity::decoration::block_attached::emit_game_event;
use crate::entity::mob::spawn::create_at;
use crate::entity::player::Player;
use crate::item::items::spawn_egg::apply_entity_variant;
use crate::item::{ItemBehaviour, ItemMetadata};
use crate::server::Server;
use pumpkin_data::entity::EntityType;
use pumpkin_data::game_event::GameEvent;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::{Block, BlockDirection};
use pumpkin_util::math::boundingbox::BoundingBox;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_util::math::wrap_degrees;

pub struct ArmorStandItem;

impl ItemMetadata for ArmorStandItem {
    fn ids() -> Box<[u16]> {
        [Item::ARMOR_STAND.id].into()
    }
}

impl ItemBehaviour for ArmorStandItem {
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
        if face == BlockDirection::Down {
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
            f64::from(target_pos.0.y),
            f64::from(target_pos.0.z) + 0.5,
        );

        let armor_stand_dimensions = EntityType::ARMOR_STAND.dimension;
        let width = f64::from(armor_stand_dimensions[0]);
        let height = f64::from(armor_stand_dimensions[1]);

        let bounding_box = BoundingBox::new(
            Vector3::new(
                bottom_center.x - width / 2.0,
                bottom_center.y,
                bottom_center.z - width / 2.0,
            ),
            Vector3::new(
                bottom_center.x + width / 2.0,
                bottom_center.y + height,
                bottom_center.z + width / 2.0,
            ),
        );

        if world.is_space_empty(bounding_box)
            && world.get_entities_at_box(&bounding_box).is_empty()
            && world.get_players_at_box(&bounding_box).is_empty()
        {
            // Vanilla `EntityType.create` with `tryMoveDown` and `movedUp`: the stand drops
            // onto what is below the block, like a slab.
            let armor_stand = create_at(&EntityType::ARMOR_STAND, &world, &target_pos, true, true);
            apply_entity_variant(item, armor_stand.as_ref(), Some(player));

            let (player_yaw, _) = player.rotation();
            let rotation = ((wrap_degrees(player_yaw - 180.0) + 22.5) / 45.0).floor() * 45.0;
            let entity = armor_stand.get_entity();
            entity.set_rotation(rotation, 0.0);
            // Vanilla `ArmorStand.setYBodyRot`: body and head always face the stand's way.
            entity.head_yaw.store(rotation);
            entity.body_yaw.store(rotation);

            world.spawn_entity(armor_stand.clone());
            world.play_sound_fine(
                Sound::EntityArmorStandPlace,
                SoundCategory::Blocks,
                &armor_stand.get_entity().pos.load(),
                0.75,
                0.8,
            );
            emit_game_event(
                armor_stand.get_entity(),
                GameEvent::EntityPlace,
                Some(player),
            );
            item.decrement_unless_creative(player.gamemode.load(), 1);
            BlockActionResult::Success
        } else {
            BlockActionResult::Fail
        }
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}
