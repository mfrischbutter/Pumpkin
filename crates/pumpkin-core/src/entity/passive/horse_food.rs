//! Feeding the horse family: vanilla `AbstractHorse.handleEating`, its table and `fedFood`.

use std::sync::atomic::Ordering;

use pumpkin_data::entity::EntityStatus;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_protocol::bedrock::server::actor_event::ActorEventID;
use rand::RngExt;

use crate::entity::passive::abstract_horse::{self as base, AbstractHorse};
use crate::entity::player::Player;

/// What one food does: a row of vanilla `handleEating`.
#[derive(Clone, Copy, Default)]
pub struct FoodEffect {
    pub heal: f32,
    pub age_up: i32,
    pub temper: i32,
    /// A tame adult falls in love.
    pub love: bool,
}

/// Vanilla `handleEating`: heals, ages up and calms a horse, the right food makes a tame adult
/// fall in love. `true` when it ate.
fn handle_eating(horse: &dyn AbstractHorse, player: &Player, stack: &ItemStack) -> bool {
    let FoodEffect {
        heal,
        age_up,
        temper,
        love,
    } = horse.food_effect(stack.item);
    let mob_entity = horse.get_mob_entity();
    let living = &mob_entity.living_entity;
    let entity = &living.entity;
    let mut used = false;
    if love
        && base::is_tamed(horse)
        && entity.age.load(Ordering::Relaxed) == 0
        && !mob_entity.is_in_love()
    {
        used = true;
        if let Some(ticks) = horse.approve_love(Some(player.entity_id()), 600) {
            mob_entity.set_love_ticks(ticks, Some(player.gameprofile.id));
            entity.world.load().send_entity_status(
                entity,
                EntityStatus::InLoveHearts,
                ActorEventID::InLoveHearts,
            );
        }
    }
    if living.health.load() < living.get_max_health() && heal > 0.0 {
        living.heal(heal);
        used = true;
    }
    if horse.is_baby() && age_up > 0 && !horse.is_age_locked() {
        horse.age_up(age_up, false);
        used = true;
    }
    if temper > 0 && (used || !base::is_tamed(horse)) && base::temper(horse) < horse.max_temper() {
        base::modify_temper(horse, temper);
        used = true;
    }
    if used {
        eating(horse);
    }
    used
}

/// Vanilla `AbstractHorse.handleEating`'s table.
pub(super) fn horse_food_effect(item: &Item) -> FoodEffect {
    let (heal, age_up, temper) = match item {
        i if i == &Item::WHEAT => (2.0, 20, 3),
        i if i == &Item::SUGAR => (1.0, 30, 3),
        i if i == &Item::HAY_BLOCK => (20.0, 180, 0),
        i if i == &Item::APPLE || i == &Item::CARROT => (3.0, 60, 3),
        i if i == &Item::RED_MUSHROOM => (3.0, 0, 3),
        i if i == &Item::GOLDEN_CARROT => (4.0, 60, 5),
        i if i == &Item::GOLDEN_APPLE || i == &Item::ENCHANTED_GOLDEN_APPLE => (10.0, 240, 10),
        _ => (0.0, 0, 0),
    };
    FoodEffect {
        heal,
        age_up,
        temper,
        love: item == &Item::GOLDEN_CARROT
            || item == &Item::GOLDEN_APPLE
            || item == &Item::ENCHANTED_GOLDEN_APPLE,
    }
}

/// Vanilla `eating`: the mouth opens and the eating sound plays.
fn eating(horse: &dyn AbstractHorse) {
    if horse.opens_mouth_to_eat() {
        base::open_mouth(horse);
    }
    if let Some(sound) = horse.horse_eating_sound() {
        let pitch = {
            let mut rng = horse.get_random();
            (rng.random::<f32>() - rng.random::<f32>()).mul_add(0.2, 1.0)
        };
        horse.play_entity_sound(sound, 1.0, pitch);
    }
    if horse.opens_mouth_to_eat() {
        let entity = horse.get_entity();
        entity.world.load().emit_game_event(
            pumpkin_data::game_event::GameEvent::Eat.name(),
            entity.pos.load(),
        );
    }
}

/// Vanilla `fedFood`.
pub(super) fn fed_food(horse: &dyn AbstractHorse, player: &Player, stack: &mut ItemStack) -> bool {
    let ate = handle_eating(horse, player, stack);
    if ate {
        stack.decrement_unless_creative(player.gamemode.load(), 1);
    }
    ate
}
