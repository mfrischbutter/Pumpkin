use crate::block::registry::BlockActionResult;
use crate::entity::decoration::block_attached::emit_game_event;
use crate::entity::decoration::leash_knot::{
    LeashKnotEntity, can_have_leash_attached_to, leashed_to_holder,
};
use crate::entity::player::Player;
use crate::item::{ItemBehaviour, ItemMetadata};
use crate::server::Server;
use pumpkin_data::Block;
use pumpkin_data::BlockDirection;
use pumpkin_data::game_event::GameEvent;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::Sound;
use pumpkin_data::tag::Taggable;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;

pub struct LeadItem;

impl ItemMetadata for LeadItem {
    fn ids() -> Box<[u16]> {
        [Item::LEAD.id].into()
    }
}

impl ItemBehaviour for LeadItem {
    /// Vanilla `LeadItem.useOn` and `bindPlayerMobs`.
    fn use_on_block(
        &self,
        _item: &mut ItemStack,
        player: &Player,
        location: BlockPos,
        _face: BlockDirection,
        _cursor_pos: Vector3<f32>,
        block: &Block,
        _server: &Server,
    ) -> BlockActionResult {
        if !block.has_tag(&pumpkin_data::tag::Block::MINECRAFT_FENCES) {
            return BlockActionResult::Pass;
        }

        let world = player.world();
        let entities_to_leash =
            leashed_to_holder(&world, location.to_centered_f64(), player.entity_id());
        if entities_to_leash.is_empty() {
            return BlockActionResult::Pass;
        }

        let existing_knot = LeashKnotEntity::get_knot(&world, location);
        let knot = existing_knot
            .clone()
            .unwrap_or_else(|| LeashKnotEntity::create_knot(&world, location));

        let mut any_leashed = false;
        for leashable in entities_to_leash {
            if can_have_leash_attached_to(leashable.get_entity(), knot.get_entity()) {
                leashable.get_entity().leash_to(knot.clone());
                any_leashed = true;
            }
        }

        if any_leashed {
            knot.play_entity_sound(Sound::ItemLeadTied, 1.0, 1.0);
            emit_game_event(knot.get_entity(), GameEvent::BlockAttach, Some(player));
            BlockActionResult::SuccessServer
        } else {
            if existing_knot.is_none() {
                knot.get_entity().remove();
            }
            BlockActionResult::Pass
        }
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}
