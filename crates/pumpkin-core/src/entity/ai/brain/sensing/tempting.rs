use std::sync::Arc;

use pumpkin_data::attributes::Attributes;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::tag::{self, Taggable};

use super::super::BrainTick;
use super::super::memory::{MemoryModuleId, types};
use super::Sensor;
use crate::entity::EntityBase;
use crate::entity::player::Player;

const REQUIRES: [MemoryModuleId; 1] = [types::TEMPTING_PLAYER.id()];

/// Vanilla `TemptingSensor.forAnimal` (`FOOD_TEMPTATIONS`): the nearest player within tempt
/// range, seen or not, holding the animal's food and not riding it.
pub struct TemptingSensor;

impl Sensor for TemptingSensor {
    fn requires(&self) -> &'static [MemoryModuleId] {
        &REQUIRES
    }

    fn do_tick(&mut self, tick: &mut BrainTick<'_>) {
        let Some(animal) = tick.mob.as_animal() else {
            tick.state.erase(types::TEMPTING_PLAYER.id());
            return;
        };
        set_tempting_player(tick, |stack| animal.is_food(stack));
    }
}

/// Vanilla `TemptingSensor` of `FROG_TEMPTATIONS`, for the tadpole, which is no animal.
pub struct FrogTemptingSensor;

impl Sensor for FrogTemptingSensor {
    fn requires(&self) -> &'static [MemoryModuleId] {
        &REQUIRES
    }

    fn do_tick(&mut self, tick: &mut BrainTick<'_>) {
        set_tempting_player(tick, |stack| {
            stack.item.has_tag(&tag::Item::MINECRAFT_FROG_FOOD)
        });
    }
}

fn set_tempting_player(tick: &mut BrainTick<'_>, is_food: impl Fn(&ItemStack) -> bool) {
    let living = &tick.mob.get_mob_entity().living_entity;
    let entity = &living.entity;
    let position = entity.pos.load();
    let range = living.get_attribute_value(&Attributes::TEMPT_RANGE);
    let distance =
        |player: &Arc<Player>| position.squared_distance_to_vec(&player.get_entity().pos.load());
    let riders: Vec<i32> = entity
        .passengers
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .iter()
        .map(|passenger| passenger.get_entity().entity_id)
        .collect();
    let holding = |player: &Arc<Player>| {
        [pumpkin_util::Hand::Right, pumpkin_util::Hand::Left]
            .into_iter()
            .any(|hand| is_food(&player.inventory.get_stack_in_hand(hand)))
    };
    let nearest = tick
        .world
        .players
        .load()
        .iter()
        .filter(|player| {
            !player.is_spectator()
                && player.get_entity().is_alive()
                && distance(player) <= range * range
                && !riders.contains(&player.entity_id())
                && holding(player)
        })
        .min_by(|a, b| distance(a).total_cmp(&distance(b)))
        .cloned();
    tick.state.set_optional(types::TEMPTING_PLAYER, nearest);
}
