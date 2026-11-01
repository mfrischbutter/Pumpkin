use std::sync::Arc;

use pumpkin_data::effect::StatusEffect;
use pumpkin_data::game_event::GameEvent;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::tracked_data;
use rand::RngExt;

use crate::entity::EntityBase;
use crate::entity::mob::Mob;
use crate::entity::mob::skeleton::{
    INCREASED_ATTACK_INTERVAL, INCREASED_HARD_ATTACK_INTERVAL, SkeletonEntityBase, SkeletonKind,
};
use crate::entity::player::Player;
use crate::world::loot::{LootContextParameters, generate_loot_from_handle};

/// Vanilla `Bogged`: slower shots whose arrows poison for 5 seconds, and mushrooms to shear.
pub static BOGGED: SkeletonKind = SkeletonKind {
    step_sound: Sound::EntityBoggedStep,
    ambient_sound: Sound::EntityBoggedAmbient,
    attack_interval: INCREASED_ATTACK_INTERVAL,
    hard_attack_interval: INCREASED_HARD_ATTACK_INTERVAL,
    arrow_effect: Some((&StatusEffect::POISON, 100)),
    extra_hunts: &[],
};

/// Vanilla `Bogged.mobInteract`: shears take the mushrooms of a bogged that has them.
/// Returns whether the interaction was used.
pub(super) fn try_shear(
    bogged: &SkeletonEntityBase,
    player: &Arc<Player>,
    item_stack: &mut ItemStack,
) -> bool {
    if item_stack.get_item() != &Item::SHEARS || bogged.is_sheared() {
        return false;
    }
    let entity = bogged.get_entity();
    let world = entity.world.load();
    if let Some(server) = world.server.upgrade() {
        let mut event =
            crate::plugin::api::events::player::player_shear_entity::PlayerShearEntityEvent {
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
    shear(bogged, SoundCategory::Players, item_stack);
    world.emit_game_event(GameEvent::Shear.name(), entity.pos.load());
    let _ = player.damage_item_stack(item_stack, 1);
    true
}

/// Vanilla `Bogged.shear`: the shear sound, the `shearing/bogged` mushrooms dropped at head
/// height, and the bogged is sheared.
fn shear(bogged: &SkeletonEntityBase, category: SoundCategory, tool: &ItemStack) {
    let entity = bogged.get_entity();
    let world = entity.world.load_full();
    let pos = entity.pos.load();
    world.play_sound(Sound::EntityBoggedShear, category, &pos);
    if let Some(table) = world.get_loot_table("minecraft:shearing/bogged") {
        let params = LootContextParameters {
            world: Some(&world),
            position: Some(pos),
            this_entity: Some(bogged),
            tool: Some(tool.clone()),
            ..Default::default()
        };
        let height = f64::from(entity.entity_dimension.load().height);
        for stack in generate_loot_from_handle(&table, bogged.get_random().random(), &params) {
            world.drop_stack_at(pos.add_raw(0.0, height, 0.0), stack);
        }
    }
    bogged.set_sheared(true);
}

/// Sends `Bogged.DATA_SHEARED` to tracking clients.
pub(super) fn sync_sheared(bogged: &SkeletonEntityBase) {
    bogged
        .get_entity()
        .set_synced_data(tracked_data::bogged::DATA_SHEARED, bogged.is_sheared());
}
