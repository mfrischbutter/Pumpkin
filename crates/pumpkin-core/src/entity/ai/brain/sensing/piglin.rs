use std::sync::Arc;

use pumpkin_data::block_properties::CampfireLikeProperties;
use pumpkin_data::entity::EntityType;
use pumpkin_data::tag::{self, Taggable};
use pumpkin_data::{Block, BlockState};
use pumpkin_util::math::position::{BlockPos, OutwardIterator};

use super::super::BrainTick;
use super::super::memory::{MemoryModuleId, types};
use super::Sensor;
use crate::entity::EntityBase;
use crate::entity::mob::hoglin::HoglinEntity;
use crate::entity::mob::hoglin_ai::{is_adult_mob, is_baby_mob};
use crate::entity::mob::piglin::PiglinEntity;
use crate::entity::mob::piglin_ai::PiglinAi;
use crate::entity::mob::piglin_anger::is_adult_piglin;
use crate::entity::player::Player;

const REPELLENT_RANGE_HORIZONTAL: i32 = 8;
const REPELLENT_RANGE_VERTICAL: i32 = 4;

const PIGLIN_REQUIRES: [MemoryModuleId; 11] = [
    types::NEAREST_VISIBLE_LIVING_ENTITIES.id(),
    types::NEAREST_LIVING_ENTITIES.id(),
    types::NEAREST_VISIBLE_NEMESIS.id(),
    types::NEAREST_TARGETABLE_PLAYER_NOT_WEARING_GOLD.id(),
    types::NEAREST_PLAYER_HOLDING_WANTED_ITEM.id(),
    types::NEAREST_VISIBLE_HUNTABLE_HOGLIN.id(),
    types::NEAREST_VISIBLE_BABY_HOGLIN.id(),
    types::NEAREST_VISIBLE_ADULT_PIGLINS.id(),
    types::NEARBY_ADULT_PIGLINS.id(),
    types::VISIBLE_ADULT_PIGLIN_COUNT.id(),
    types::VISIBLE_ADULT_HOGLIN_COUNT.id(),
];

const BRUTE_REQUIRES: [MemoryModuleId; 3] = [
    types::NEAREST_VISIBLE_LIVING_ENTITIES.id(),
    types::NEAREST_VISIBLE_NEMESIS.id(),
    types::NEARBY_ADULT_PIGLINS.id(),
];

fn is_wither_kind(entity: &dyn EntityBase) -> bool {
    let kind = entity.get_entity().entity_type;
    kind == &EntityType::WITHER_SKELETON || kind == &EntityType::WITHER
}

fn nearby_adult_piglins(tick: &BrainTick<'_>) -> Vec<Arc<dyn EntityBase>> {
    tick.state
        .get(types::NEAREST_LIVING_ENTITIES)
        .map_or_else(Vec::new, |nearby| {
            nearby
                .iter()
                .filter(|entity| is_adult_piglin(entity.as_ref()))
                .cloned()
                .collect()
        })
}

fn visible_entities(tick: &BrainTick<'_>) -> Vec<Arc<dyn EntityBase>> {
    let ctx = tick.visibility();
    tick.state
        .get(types::NEAREST_VISIBLE_LIVING_ENTITIES)
        .map_or_else(Vec::new, |visible| {
            visible.find_all(&ctx, |_| true).cloned().collect()
        })
}

fn is_valid_repellent(block: &Block, state: &BlockState) -> bool {
    block.has_tag(&tag::Block::MINECRAFT_PIGLIN_REPELLENTS)
        && (block.id != Block::SOUL_CAMPFIRE.id
            || CampfireLikeProperties::from_state_id(state.id).lit)
}

/// What the scan finds, vanilla's locals in `PiglinSpecificSensor.doTick`.
#[derive(Default)]
struct Seen {
    nemesis: Option<Arc<dyn EntityBase>>,
    huntable_hoglin: Option<Arc<dyn EntityBase>>,
    baby_hoglin: Option<Arc<dyn EntityBase>>,
    found_baby_piglin: bool,
    zombified: Option<Arc<dyn EntityBase>>,
    player_not_wearing_gold: Option<Arc<Player>>,
    player_holding_wanted_item: Option<Arc<Player>>,
    adult_hoglins: i32,
    adult_piglins: Vec<Arc<dyn EntityBase>>,
}

impl Seen {
    fn see(&mut self, tick: &BrainTick<'_>, entity: &Arc<dyn EntityBase>) {
        let kind = entity.get_entity().entity_type;
        if let Some(hoglin) = entity.cast_any().downcast_ref::<HoglinEntity>() {
            self.see_hoglin(entity, hoglin);
        } else if kind == &EntityType::PIGLIN_BRUTE {
            self.adult_piglins.push(entity.clone());
        } else if let Some(piglin) = entity.cast_any().downcast_ref::<PiglinEntity>() {
            if piglin.is_baby() && !self.found_baby_piglin {
                self.found_baby_piglin = true;
            } else if piglin.is_adult() {
                self.adult_piglins.push(entity.clone());
            }
        } else if let Some(player) = entity.get_player() {
            self.see_player(tick, entity, player);
        } else if self.nemesis.is_none() && is_wither_kind(entity.as_ref()) {
            self.nemesis = Some(entity.clone());
        } else if self.zombified.is_none() && PiglinAi::is_zombified(kind) {
            self.zombified = Some(entity.clone());
        }
    }

    fn see_hoglin(&mut self, entity: &Arc<dyn EntityBase>, hoglin: &HoglinEntity) {
        if is_baby_mob(hoglin) && self.baby_hoglin.is_none() {
            self.baby_hoglin = Some(entity.clone());
        } else if is_adult_mob(hoglin) {
            self.adult_hoglins += 1;
            if self.huntable_hoglin.is_none() && hoglin.can_be_hunted() {
                self.huntable_hoglin = Some(entity.clone());
            }
        }
    }

    fn see_player(&mut self, tick: &BrainTick<'_>, entity: &Arc<dyn EntityBase>, player: &Player) {
        let arc = || tick.world.get_player_by_uuid(player.gameprofile.id);
        if self.player_not_wearing_gold.is_none()
            && !PiglinAi::is_wearing_safe_armor(&player.living_entity)
            && tick.mob.can_attack(entity.as_ref())
        {
            self.player_not_wearing_gold = arc();
        }
        if self.player_holding_wanted_item.is_none()
            && !player.is_spectator()
            && PiglinAi::is_player_holding_loved_item(player)
        {
            self.player_holding_wanted_item = arc();
        }
    }
}

/// Vanilla `PiglinSpecificSensor`.
pub struct PiglinSpecificSensor;

impl Sensor for PiglinSpecificSensor {
    fn requires(&self) -> &'static [MemoryModuleId] {
        &PIGLIN_REQUIRES
    }

    fn do_tick(&mut self, tick: &mut BrainTick<'_>) {
        let center = tick.mob.get_entity().block_pos.load();
        let repellent = OutwardIterator::new(
            center,
            REPELLENT_RANGE_HORIZONTAL,
            REPELLENT_RANGE_VERTICAL,
            REPELLENT_RANGE_HORIZONTAL,
        )
        .find(|pos: &BlockPos| {
            let (block, state) = tick.world.get_block_and_state(pos);
            is_valid_repellent(block, state)
        });
        tick.state.set_optional(types::NEAREST_REPELLENT, repellent);

        let mut seen = Seen::default();
        for entity in visible_entities(tick) {
            seen.see(tick, &entity);
        }
        let adults = nearby_adult_piglins(tick);
        let state = &mut *tick.state;
        state.set_optional(types::NEAREST_VISIBLE_NEMESIS, seen.nemesis);
        state.set_optional(types::NEAREST_VISIBLE_HUNTABLE_HOGLIN, seen.huntable_hoglin);
        state.set_optional(types::NEAREST_VISIBLE_BABY_HOGLIN, seen.baby_hoglin);
        state.set_optional(types::NEAREST_VISIBLE_ZOMBIFIED, seen.zombified);
        state.set_optional(
            types::NEAREST_TARGETABLE_PLAYER_NOT_WEARING_GOLD,
            seen.player_not_wearing_gold,
        );
        state.set_optional(
            types::NEAREST_PLAYER_HOLDING_WANTED_ITEM,
            seen.player_holding_wanted_item,
        );
        state.set(types::NEARBY_ADULT_PIGLINS, adults);
        state.set(
            types::VISIBLE_ADULT_PIGLIN_COUNT,
            seen.adult_piglins.len() as i32,
        );
        state.set(types::NEAREST_VISIBLE_ADULT_PIGLINS, seen.adult_piglins);
        state.set(types::VISIBLE_ADULT_HOGLIN_COUNT, seen.adult_hoglins);
    }
}

/// Vanilla `PiglinBruteSpecificSensor`.
pub struct PiglinBruteSpecificSensor;

impl Sensor for PiglinBruteSpecificSensor {
    fn requires(&self) -> &'static [MemoryModuleId] {
        &BRUTE_REQUIRES
    }

    fn do_tick(&mut self, tick: &mut BrainTick<'_>) {
        let nemesis = visible_entities(tick)
            .into_iter()
            .find(|entity| is_wither_kind(entity.as_ref()));
        let adults = nearby_adult_piglins(tick);
        tick.state
            .set_optional(types::NEAREST_VISIBLE_NEMESIS, nemesis);
        tick.state.set(types::NEARBY_ADULT_PIGLINS, adults);
    }
}
