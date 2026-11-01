//! Saddles as vanilla keeps them since 1.21.5: an item in the mob's saddle slot, sent to
//! clients with the rest of its equipment, and the rider who steers it.

use std::sync::Arc;

use pumpkin_data::data_component_impl::EquipmentSlot;
use pumpkin_data::game_event::GameEvent;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_nbt::compound::NbtCompound;

use crate::entity::mob::{Mob, MobEntity};
use crate::entity::player::Player;

/// Vanilla `Mob.isSaddled`: a saddle in the saddle slot.
pub fn has_saddle(mob: &MobEntity) -> bool {
    !mob.living_entity
        .entity_equipment
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(&EquipmentSlot::SADDLE)
        .is_empty()
}

/// Vanilla `Equippable.equipOnTarget` for a saddle: a mob that can wear one and has none takes
/// one saddle from `stack`, with its equip sound. `true` when it was put on.
pub fn try_equip_saddle(mob: &dyn Mob, player: &Player, stack: &mut ItemStack) -> bool {
    let entity = mob.get_entity();
    if stack.item != &Item::SADDLE
        || !entity.is_alive()
        || !mob.can_be_saddled()
        || mob.is_saddled()
    {
        return false;
    }
    mob.get_mob_entity()
        .set_item_slot_and_drop_when_killed(&EquipmentSlot::SADDLE, stack.copy_with_count(1));
    let world = entity.world.load();
    let pos = entity.pos.load();
    if !entity.is_silent() {
        world.play_sound(mob.saddle_sound(), mob.mob_sound_source(), &pos);
    }
    world.emit_game_event(GameEvent::Equip.name(), pos);
    stack.decrement_unless_creative(player.gamemode.load(), 1);
    true
}

/// Saves from before 1.21.5 (and older Pumpkin saves) kept the saddle as a `Saddle` flag.
pub fn read_legacy_saddle(mob: &dyn Mob, nbt: &NbtCompound) {
    let saddled = nbt
        .get_bool("Saddle")
        .or_else(|| nbt.get_byte("Saddle").map(|flag| flag == 1));
    if saddled == Some(true) && !mob.is_saddled() {
        mob.get_mob_entity().set_item_slot_and_drop_when_killed(
            &EquipmentSlot::SADDLE,
            ItemStack::new(1, &Item::SADDLE),
        );
    }
}

/// The first passenger, when it is a player: vanilla `getFirstPassenger() instanceof Player`.
pub fn first_passenger_player(mob: &dyn Mob) -> Option<Arc<Player>> {
    let entity = mob.get_entity();
    let first = entity
        .passengers
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .first()
        .cloned()?;
    first.get_player()?;
    entity
        .world
        .load()
        .get_player_by_id(first.get_entity().entity_id)
}

/// Vanilla `Player.isHolding(item)`: in either hand.
pub fn is_holding(player: &Player, item: &Item) -> bool {
    [pumpkin_util::Hand::Right, pumpkin_util::Hand::Left]
        .into_iter()
        .any(|hand| player.inventory.get_stack_in_hand(hand).item == item)
}

/// The usual `tickRidden` turn: the mount faces where its rider looks, its pitch scaled.
pub fn face_rider(mob: &dyn Mob, controller: &Player, pitch_scale: f32) {
    let (yaw, pitch) = controller.rotation();
    let entity = mob.get_entity();
    entity.set_rotation(yaw, pitch * pitch_scale);
    entity.body_yaw.store(yaw);
    entity.head_yaw.store(yaw);
}
