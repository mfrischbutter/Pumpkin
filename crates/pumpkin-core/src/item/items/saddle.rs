use std::any::Any;
use std::sync::Arc;

use crate::entity::EntityBase;
use crate::entity::player::Player;
use crate::item::{ItemBehaviour, ItemMetadata};
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;

pub struct SaddleItem;

impl ItemMetadata for SaddleItem {
    fn ids() -> Box<[u16]> {
        Box::new([Item::SADDLE.id])
    }
}

impl ItemBehaviour for SaddleItem {
    fn use_on_entity(&self, item: &mut ItemStack, player: &Player, entity: Arc<dyn EntityBase>) {
        if let Some(mob) = entity.get_mob() {
            crate::entity::mob::saddle::try_equip_saddle(mob, player, item);
        }
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}
