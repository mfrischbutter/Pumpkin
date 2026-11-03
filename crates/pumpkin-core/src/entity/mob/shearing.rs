//! Shears taking equipment off a mob: vanilla `Entity.interact` with
//! `Mob.attemptToShearEquipment` and `Mob.shearItem`.

use std::sync::Arc;

use pumpkin_data::data_component_impl::{EquipmentSlot, EquippableImpl};
use pumpkin_data::game_event::GameEvent;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_util::GameMode;

use crate::entity::EntityBase;
use crate::entity::living::prevents_armor_change;
use crate::entity::mob::Mob;
use crate::entity::player::Player;
use crate::entity::player::advancement::trigger::AdvancementTrigger;
use crate::plugin::api::events::player::player_shear_entity::PlayerShearEntityEvent;

/// Vanilla `EquipmentSlot.VALUES`: the first shearable item in this order comes off.
const SHEARING_ORDER: [EquipmentSlot; 8] = [
    EquipmentSlot::MAIN_HAND,
    EquipmentSlot::OFF_HAND,
    EquipmentSlot::FEET,
    EquipmentSlot::LEGS,
    EquipmentSlot::CHEST,
    EquipmentSlot::HEAD,
    EquipmentSlot::BODY,
    EquipmentSlot::SADDLE,
];

/// The check `Entity.interact` makes before the mob's own interaction. `true` when an item came
/// off.
pub fn try_shear_equipment(mob: &dyn Mob, player: &Arc<Player>, shears: &mut ItemStack) -> bool {
    shears.get_item() == &Item::SHEARS
        && mob.can_shear_equipment(player)
        && !player.get_entity().is_sneaking()
        && attempt_to_shear_equipment(mob, player, shears)
}

/// Vanilla `Mob.attemptToShearEquipment`.
fn attempt_to_shear_equipment(mob: &dyn Mob, player: &Arc<Player>, shears: &mut ItemStack) -> bool {
    let mob_entity = mob.get_mob_entity();
    let living = &mob_entity.living_entity;
    let creative = player.gamemode.load() == GameMode::Creative;
    let sheared = {
        let equipment = living
            .entity_equipment
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        SHEARING_ORDER.into_iter().find_map(|slot| {
            let stack = equipment.equipment.get(&slot)?;
            let shearing_sound = stack
                .get_data_component::<EquippableImpl>()
                .filter(|equippable| equippable.can_be_sheared)?
                .shearing_sound
                .clone();
            (creative || !prevents_armor_change(stack))
                .then(|| (slot, stack.clone(), shearing_sound))
        })
    };
    let Some((slot, stack, shearing_sound)) = sheared else {
        return false;
    };

    let entity = &living.entity;
    let world = entity.world.load();
    if let Some(server) = world.server.upgrade() {
        let mut event = PlayerShearEntityEvent {
            player: player.clone(),
            entity_id: entity.entity_id,
            hand: 0,
            cancelled: false,
        };
        server.plugin_manager.fire_blocking(&server, &mut event);
        if event.cancelled {
            return false;
        }
    }

    let _ = player.damage_item_stack(shears, 1);
    mob_entity.set_item_slot(&slot, ItemStack::EMPTY.clone());
    let pos = entity.pos.load();
    world.emit_game_event(GameEvent::Unequip.name(), pos);
    world.emit_game_event(GameEvent::Shear.name(), pos);
    player.trigger_advancement(AdvancementTrigger::PlayerShearedEquipment {
        entity_type: format!("minecraft:{}", entity.entity_type.resource_name),
        item_id: format!("minecraft:{}", stack.item.registry_key),
    });
    entity.spawn_at_location(stack);
    if !entity.is_silent() {
        world.play_sound_event(&shearing_sound, mob.mob_sound_source(), &pos);
    }
    true
}
