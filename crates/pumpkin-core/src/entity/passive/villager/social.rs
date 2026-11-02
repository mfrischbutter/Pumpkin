//! What villagers do with each other: gossip and share goods, and have children.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use pumpkin_data::entity::{EntityStatus, EntityType};
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::poi_type::PoiType;
use pumpkin_protocol::bedrock::server::actor_event::ActorEventID;
use pumpkin_util::math::position::BlockPos;
use pumpkin_world::poi::Occupancy;
use rand::RngExt;
use uuid::Uuid;

use super::data::{BREEDING_FOOD_THRESHOLD, VillagerData, VillagerProfession};
use super::{GossipType, VillagerEntity};
use crate::entity::ageable::AgeableMob;
use crate::entity::ai::brain::BrainTick;
use crate::entity::ai::brain::behavior::poi::{SCAN_RANGE, global_pos};
use crate::entity::ai::brain::behavior::{Behavior, utils};
use crate::entity::ai::brain::memory::{MemoryModuleId, MemoryStatus, types};
use crate::entity::mob::Mob;
use crate::entity::{Entity, EntityBase};
use crate::world::World;

/// Vanilla `Villager.GOSSIP_COOLDOWN`.
const GOSSIP_COOLDOWN: i64 = 1200;
/// Vanilla `Villager.MAX_GOSSIP_TOPICS`.
const MAX_GOSSIP_TOPICS: usize = 10;
/// Vanilla `GossipContainer.DISCARD_THRESHOLD`.
const DISCARD_THRESHOLD: i32 = 2;
/// Vanilla `Villager.HOW_MANY_VILLAGERS_NEED_TO_AGREE_TO_SPAWN_A_GOLEM`.
const VILLAGERS_TO_AGREE_ON_A_GOLEM: usize = 5;
/// Vanilla `VillagerGoalPackages.INTERACT_DIST_SQR`.
const INTERACT_DIST_SQR: f64 = 5.0;

fn as_villager(mob: &dyn Mob) -> Option<&VillagerEntity> {
    mob.cast_any().downcast_ref::<VillagerEntity>()
}

/// Vanilla `BehaviorUtils.lockGazeAndWalkToEachOther`.
fn lock_gaze_and_walk_to_each_other(
    tick: &mut BrainTick<'_>,
    me: Arc<dyn EntityBase>,
    target: &VillagerEntity,
    target_entity: Arc<dyn EntityBase>,
    speed_modifier: f32,
    close_enough_dist: i32,
) {
    utils::set_walk_and_look_target_memories_to_entity(
        tick.state,
        target_entity,
        speed_modifier,
        close_enough_dist,
    );
    target.with_other_brain(|brain| {
        utils::set_walk_and_look_target_memories_to_entity(
            brain,
            me,
            speed_modifier,
            close_enough_dist,
        );
    });
}

impl VillagerEntity {
    /// Vanilla `GossipContainer.selectGossipsForTransfer` and `transferFrom`, from `source`.
    fn take_gossips_from(&self, source: &Self) -> usize {
        let entries: Vec<(Uuid, GossipType, i32)> = source
            .gossips
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .flat_map(|(target, values)| {
                values
                    .iter()
                    .map(|(gossip, value)| (*target, *gossip, *value))
            })
            .collect();
        if entries.is_empty() {
            return 0;
        }
        let weights: Vec<i32> = entries
            .iter()
            .map(|(_, gossip, value)| (value * gossip.weight()).abs())
            .collect();
        let total: i32 = weights.iter().sum();
        if total <= 0 {
            return 0;
        }
        let mut rng = self.get_random();
        let mut chosen = vec![false; entries.len()];
        for _ in 0..MAX_GOSSIP_TOPICS {
            let mut choice = rng.random_range(0..total);
            for (index, weight) in weights.iter().enumerate() {
                if choice < *weight {
                    chosen[index] = true;
                    break;
                }
                choice -= weight;
            }
        }
        let mut gossips = self
            .gossips
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut transferred = 0;
        for (index, (target, gossip, value)) in entries.into_iter().enumerate() {
            if !chosen[index] {
                continue;
            }
            transferred += 1;
            let decayed = value - gossip.decay_per_transfer();
            if decayed >= DISCARD_THRESHOLD {
                let entry = gossips
                    .entry(target)
                    .or_default()
                    .entry(gossip)
                    .or_default();
                *entry = (*entry).max(decayed);
            }
        }
        transferred
    }

    /// Vanilla `Villager.gossip`, from this villager's brain tick.
    fn gossip(&self, tick: &mut BrainTick<'_>, target: &Self) {
        let time = tick.time;
        let recent = |villager: &Self| {
            let last = villager.last_gossip_share_time.load(Ordering::Relaxed);
            time >= last && time < last + GOSSIP_COOLDOWN
        };
        if recent(self) || recent(target) {
            return;
        }
        let new_gossip = self.take_gossips_from(target);
        self.last_gossip_share_time.store(time, Ordering::Relaxed);
        target.last_gossip_share_time.store(time, Ordering::Relaxed);
        self.spawn_golem_if_needed(tick.state, tick.world, time, VILLAGERS_TO_AGREE_ON_A_GOLEM);
        if new_gossip > 0 {
            self.notify_trading_player_offers_updated();
        }
    }

    /// Vanilla `Villager.onReputationEventFrom`, `VILLAGER_KILLED`.
    pub(super) fn witness_murder(&self, killer: Uuid) {
        let mut gossips = self
            .gossips
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let value = gossips
            .entry(killer)
            .or_default()
            .entry(GossipType::MajorNegative)
            .or_default();
        *value = (*value + 25).min(GossipType::MajorNegative.max_value());
    }

    /// Vanilla `Villager.hasExcessFood`.
    fn has_excess_food(&self) -> bool {
        self.count_food_points_in_inventory() >= 24
    }

    /// Vanilla `Villager.wantsMoreFood`.
    fn wants_more_food(&self) -> bool {
        self.count_food_points_in_inventory() < BREEDING_FOOD_THRESHOLD
    }

    /// Vanilla `TradeWithVillager.throwHalfStack`: half a big stack the filter accepts.
    fn throw_half_stack(&self, keep: impl Fn(&ItemStack) -> bool, target: &Entity) {
        let to_throw = {
            let mut inventory = self
                .inventory
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            inventory.iter_mut().find_map(|stack| {
                if stack.is_empty() || !keep(stack) {
                    return None;
                }
                let count = if stack.item_count > stack.get_max_stack_size() / 2 {
                    stack.item_count / 2
                } else if stack.item_count > 24 {
                    stack.item_count - 24
                } else {
                    return None;
                };
                Some(stack.split(count))
            })
        };
        if let Some(stack) = to_throw {
            utils::throw_item(self.get_entity(), stack, target.pos.load());
        }
    }

    /// Vanilla `Villager.canBreed`.
    pub(super) fn can_breed(&self) -> bool {
        self.food_level.load(Ordering::Relaxed) + self.count_food_points_in_inventory()
            >= BREEDING_FOOD_THRESHOLD
            && !self.mob_entity.living_entity.is_sleeping()
            && self.get_entity().age.load(Ordering::Relaxed) == 0
    }

    /// Vanilla `Villager.eatAndDigestFood`.
    fn eat_and_digest_food(&self) {
        self.eat_until_full();
        self.food_level
            .fetch_sub(BREEDING_FOOD_THRESHOLD, Ordering::Relaxed);
    }

    /// Vanilla `Villager.getBreedOffspring`.
    fn breed_offspring(&self, partner: &Self) -> Arc<Self> {
        let entity = self.get_entity();
        let child = Self::new(Entity::new(
            entity.world.load_full(),
            entity.pos.load(),
            &EntityType::VILLAGER,
        ));
        let mine = self.villager_type();
        let roll: f64 = self.get_random().random();
        let child_type = if roll < 0.5 {
            VillagerData::type_for_biome(entity.world.load().get_biome(&entity.block_pos.load()))
        } else if roll < 0.75 {
            mine
        } else {
            partner.villager_type()
        };
        child.set_villager_data(VillagerData::new(child_type, VillagerProfession::None, 1));
        child
    }
}

/// Which goods go from one villager to the next (`figureOutWhatIAmWillingToTrade`).
fn willing_to_trade(me: &VillagerEntity, target: &VillagerEntity) -> Vec<&'static Item> {
    let mine = me.profession().requested_items();
    target
        .profession()
        .requested_items()
        .iter()
        .copied()
        .filter(|item| !mine.contains(item))
        .collect()
}

fn interaction_villager(tick: &BrainTick<'_>) -> Option<Arc<dyn EntityBase>> {
    tick.state
        .get(types::INTERACTION_TARGET)
        .filter(|target| {
            target.get_entity().is_alive()
                && target.get_entity().entity_type == &EntityType::VILLAGER
                && utils::can_see(&tick.visibility(), target.as_ref())
        })
        .cloned()
}

/// Vanilla `TradeWithVillager`.
#[derive(Default)]
pub(super) struct TradeWithVillager {
    trades: Vec<&'static Item>,
}

const TRADE_CONDITIONS: [(MemoryModuleId, MemoryStatus); 2] = [
    (types::INTERACTION_TARGET.id(), MemoryStatus::ValuePresent),
    (
        types::NEAREST_VISIBLE_LIVING_ENTITIES.id(),
        MemoryStatus::ValuePresent,
    ),
];

impl Behavior for TradeWithVillager {
    fn entry_conditions(&self) -> &[(MemoryModuleId, MemoryStatus)] {
        &TRADE_CONDITIONS
    }

    fn check_extra_start_conditions(&mut self, tick: &mut BrainTick<'_>) -> bool {
        interaction_villager(tick).is_some()
    }

    fn can_still_use(&mut self, tick: &mut BrainTick<'_>) -> bool {
        interaction_villager(tick).is_some()
    }

    fn start(&mut self, tick: &mut BrainTick<'_>) {
        let (Some(me), Some(target_entity)) = (self_entity(tick), interaction_villager(tick))
        else {
            return;
        };
        let Some(target) = target_entity.cast_any().downcast_ref::<VillagerEntity>() else {
            return;
        };
        lock_gaze_and_walk_to_each_other(tick, me, target, target_entity.clone(), 0.5, 2);
        if let Some(body) = as_villager(tick.mob) {
            self.trades = willing_to_trade(body, target);
        }
    }

    fn tick(&mut self, tick: &mut BrainTick<'_>) {
        let (Some(me), Some(target_entity)) = (self_entity(tick), interaction_villager(tick))
        else {
            return;
        };
        let Some(target) = target_entity.cast_any().downcast_ref::<VillagerEntity>() else {
            return;
        };
        let Some(body) = as_villager(tick.mob) else {
            return;
        };
        let distance = body
            .get_entity()
            .pos
            .load()
            .squared_distance_to_vec(&target.get_entity().pos.load());
        if distance > INTERACT_DIST_SQR {
            return;
        }
        lock_gaze_and_walk_to_each_other(tick, me, target, target_entity.clone(), 0.5, 2);
        body.gossip(tick, target);
        let is_farmer = body.profession() == VillagerProfession::Farmer;
        let target_body = target.get_entity();
        if body.has_excess_food() && (is_farmer || target.wants_more_food()) {
            body.throw_half_stack(super::is_villager_food, target_body);
        }
        if is_farmer
            && body.count_item(&Item::WHEAT)
                > i32::from(ItemStack::new(1, &Item::WHEAT).get_max_stack_size()) / 2
        {
            body.throw_half_stack(|stack| stack.item == &Item::WHEAT, target_body);
        }
        if !self.trades.is_empty() {
            let trades = &self.trades;
            body.throw_half_stack(|stack| trades.contains(&stack.item), target_body);
        }
    }

    fn stop(&mut self, tick: &mut BrainTick<'_>) {
        tick.state.erase(types::INTERACTION_TARGET.id());
    }

    fn debug_name(&self) -> &'static str {
        "TradeWithVillager"
    }
}

fn self_entity(tick: &BrainTick<'_>) -> Option<Arc<dyn EntityBase>> {
    let entity = tick.mob.get_entity();
    tick.world.get_entity_by_id(entity.entity_id)
}

/// Vanilla `VillagerMakeLove`.
#[derive(Default)]
pub(super) struct VillagerMakeLove {
    birth_timestamp: i64,
}

const MAKE_LOVE_CONDITIONS: [(MemoryModuleId, MemoryStatus); 2] = [
    (types::BREED_TARGET.id(), MemoryStatus::ValuePresent),
    (
        types::NEAREST_VISIBLE_LIVING_ENTITIES.id(),
        MemoryStatus::ValuePresent,
    ),
];

fn breed_partner(tick: &BrainTick<'_>) -> Option<Arc<dyn EntityBase>> {
    let body = as_villager(tick.mob)?;
    let target = tick.state.get(types::BREED_TARGET)?;
    let villager = target.cast_any().downcast_ref::<VillagerEntity>()?;
    (target.get_entity().is_alive()
        && utils::can_see(&tick.visibility(), target.as_ref())
        && body.can_breed()
        && villager.can_breed())
    .then(|| target.clone())
}

impl Behavior for VillagerMakeLove {
    fn entry_conditions(&self) -> &[(MemoryModuleId, MemoryStatus)] {
        &MAKE_LOVE_CONDITIONS
    }

    fn min_duration(&self) -> i32 {
        350
    }

    fn max_duration(&self) -> i32 {
        350
    }

    fn check_extra_start_conditions(&mut self, tick: &mut BrainTick<'_>) -> bool {
        breed_partner(tick).is_some()
    }

    fn can_still_use(&mut self, tick: &mut BrainTick<'_>) -> bool {
        tick.time <= self.birth_timestamp && breed_partner(tick).is_some()
    }

    fn start(&mut self, tick: &mut BrainTick<'_>) {
        let (Some(me), Some(partner_entity)) = (self_entity(tick), breed_partner(tick)) else {
            return;
        };
        let Some(partner) = partner_entity.cast_any().downcast_ref::<VillagerEntity>() else {
            return;
        };
        lock_gaze_and_walk_to_each_other(tick, me, partner, partner_entity.clone(), 0.5, 2);
        for entity in [tick.mob.get_entity(), partner.get_entity()] {
            tick.world.send_entity_status(
                entity,
                EntityStatus::InLoveHearts,
                ActorEventID::InLoveHearts,
            );
        }
        self.birth_timestamp = tick.time + tick.mob.get_random().random_range(275..325);
    }

    fn tick(&mut self, tick: &mut BrainTick<'_>) {
        let (Some(me), Some(partner_entity)) = (self_entity(tick), breed_partner(tick)) else {
            return;
        };
        let Some(partner) = partner_entity.cast_any().downcast_ref::<VillagerEntity>() else {
            return;
        };
        let Some(body) = as_villager(tick.mob) else {
            return;
        };
        let distance = body
            .get_entity()
            .pos
            .load()
            .squared_distance_to_vec(&partner.get_entity().pos.load());
        if distance > INTERACT_DIST_SQR {
            return;
        }
        lock_gaze_and_walk_to_each_other(tick, me, partner, partner_entity.clone(), 0.5, 2);
        if tick.time >= self.birth_timestamp {
            body.eat_and_digest_food();
            partner.eat_and_digest_food();
            try_to_give_birth(tick, body, partner);
        } else if tick.mob.get_random().random_range(0..35) == 0 {
            for entity in [partner.get_entity(), body.get_entity()] {
                tick.world.send_entity_status(
                    entity,
                    EntityStatus::LoveHearts,
                    ActorEventID::LoveHearts,
                );
            }
        }
    }

    fn stop(&mut self, tick: &mut BrainTick<'_>) {
        tick.state.erase(types::BREED_TARGET.id());
    }

    fn debug_name(&self) -> &'static str {
        "VillagerMakeLove"
    }
}

/// Vanilla `VillagerMakeLove.takeVacantBed`: the closest free bed the villager can reach.
fn take_vacant_bed(world: &World, body: &VillagerEntity) -> Option<BlockPos> {
    let origin = body.get_entity().block_pos.load();
    let mut beds = world.pois_in_range(
        origin,
        SCAN_RANGE,
        |poi| poi == &PoiType::HOME,
        Occupancy::HasSpace,
    );
    beds.sort_by_key(|(pos, _)| {
        let delta = pos.0 - origin.0;
        i64::from(delta.x).pow(2) + i64::from(delta.y).pow(2) + i64::from(delta.z).pow(2)
    });
    let steered = body.mob_entity.steered();
    let owner = &steered.mob_entity().living_entity;
    let bed = beds.into_iter().find_map(|(pos, kind)| {
        steered
            .navigator()
            .create_path(owner, pos.0.to_f64(), kind.valid_range as i32)
            .is_some_and(|path| path.can_reach())
            .then_some(pos)
    })?;
    world.take_poi(bed).then_some(bed)
}

/// Vanilla `VillagerMakeLove.tryToGiveBirth` and `breed`.
fn try_to_give_birth(tick: &BrainTick<'_>, body: &VillagerEntity, partner: &VillagerEntity) {
    let Some(bed) = take_vacant_bed(tick.world, body) else {
        for entity in [partner.get_entity(), body.get_entity()] {
            tick.world.send_entity_status(
                entity,
                EntityStatus::VillagerAngry,
                ActorEventID::VillagerAngry,
            );
        }
        return;
    };
    let child = body.breed_offspring(partner);
    body.set_age(6000);
    partner.set_age(6000);
    child.set_age(-24000);
    child.get_entity().set_pos(body.get_entity().pos.load());
    let child_entity: Arc<dyn EntityBase> = child.clone();
    if tick.world.spawn_entity(child_entity) {
        tick.world.send_entity_status(
            child.get_entity(),
            EntityStatus::LoveHearts,
            ActorEventID::LoveHearts,
        );
        let home = global_pos(tick.world, bed);
        child
            .mob_entity
            .with_brain(child.as_ref(), |brain| brain.set(types::HOME, home));
    } else {
        tick.world.release_poi(bed);
    }
}
