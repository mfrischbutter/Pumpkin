//! What a trader llama adds to the llama (vanilla `TraderLlama`): it stands by its wandering
//! trader, panics faster, hunts zombies and illagers, and disappears when its time is up.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use pumpkin_data::entity::EntityType;

use super::llama::LlamaEntity;
use crate::entity::ai::behavior::hunting::{self, Hunt};
use crate::entity::ai::goal::escape_danger::EscapeDangerGoal;
use crate::entity::ai::goal::track_target::TrackTargetGoal;
use crate::entity::ai::goal::{Controls, Goal};
use crate::entity::ai::target_match::TargetMatch;
use crate::entity::ai::target_predicate::TargetPredicate;
use crate::entity::mob::{Mob, MobEntity};
use crate::entity::{Entity, EntityBase};
use crate::plugin::api::events::entity::entity_target::TargetReason;

/// Vanilla `TraderLlama.DEFAULT_DESPAWN_DELAY`.
pub const DEFAULT_DESPAWN_DELAY: i32 = 47999;

const TRADER_LLAMA_HUNTS: &[Hunt] = &[
    Hunt::new(2, TargetMatch::ZOMBIE_LIKE, true).matching(|_hunter, target, _world| {
        target.entity.entity_type != &EntityType::ZOMBIFIED_PIGLIN
    }),
    Hunt::new(2, TargetMatch::ILLAGER, true),
];

/// Vanilla `TraderLlama.registerGoals`, after the llama's.
pub fn register_goals(mob_entity: &MobEntity) {
    mob_entity
        .goals_selector
        .add_goal(1, EscapeDangerGoal::new(2.0));
    let targets = &mob_entity.target_selector;
    targets.add_goal(1, Box::new(TraderLlamaDefendWanderingTraderGoal::new()));
    hunting::apply(targets, mob_entity, TRADER_LLAMA_HUNTS);
}

/// The wandering trader leading it.
#[must_use]
pub fn leashing_trader(entity: &Entity) -> Option<Arc<dyn EntityBase>> {
    entity
        .get_leash_holder()
        .filter(|holder| holder.get_entity().entity_type == &EntityType::WANDERING_TRADER)
}

fn has_exactly_one_player_passenger(entity: &Entity) -> bool {
    let passengers = entity
        .passengers
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    passengers.len() == 1 && passengers[0].get_entity().entity_type == &EntityType::PLAYER
}

/// Vanilla `TraderLlama.canDespawn`.
fn can_despawn(llama: &LlamaEntity) -> bool {
    let entity = llama.get_entity();
    !llama.is_tame()
        && (!entity.is_leashed() || leashing_trader(entity).is_some())
        && !has_exactly_one_player_passenger(entity)
        && !crate::entity::ageable::AgeableMob::is_age_locked(llama)
        && !llama
            .mob_entity
            .persistence_required
            .load(Ordering::Relaxed)
}

/// Vanilla `TraderLlama.maybeDespawn`: mirrors the leashing trader's remaining delay.
pub fn maybe_despawn(llama: &LlamaEntity) {
    if !can_despawn(llama) {
        return;
    }
    let entity = llama.get_entity();
    let trader_delay = leashing_trader(entity)
        .and_then(|trader| trader.get_mob().and_then(Mob::get_despawn_delay));
    let despawn_delay = trader_delay.map_or_else(
        || llama.despawn_delay.load(Ordering::Relaxed) - 1,
        |delay| delay - 1,
    );
    llama.despawn_delay.store(despawn_delay, Ordering::Relaxed);
    if despawn_delay <= 0 {
        entity.remove_leash();
        entity.remove();
    }
}

/// Vanilla `TraderLlama.TraderLlamaDefendWanderingTraderGoal`.
struct TraderLlamaDefendWanderingTraderGoal {
    track_target_goal: TrackTargetGoal,
    target: Option<Arc<dyn EntityBase>>,
    timestamp: i32,
    target_predicate: TargetPredicate,
}

impl TraderLlamaDefendWanderingTraderGoal {
    fn new() -> Self {
        Self {
            track_target_goal: TrackTargetGoal::with_default(false),
            target: None,
            timestamp: 0,
            target_predicate: TargetPredicate::create_attackable(),
        }
    }
}

impl Goal for TraderLlamaDefendWanderingTraderGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        let Some(trader) = leashing_trader(mob.get_entity()) else {
            return false;
        };
        let Some(trader_living) = trader.get_living_entity() else {
            return false;
        };
        let timestamp = trader_living.get_last_hurt_by_mob_timestamp();
        let Some(attacker) = trader_living.get_last_hurt_by_mob() else {
            return false;
        };
        if timestamp == self.timestamp
            || !self.track_target_goal.can_track(
                mob,
                Some(attacker.as_ref()),
                &self.target_predicate,
            )
        {
            return false;
        }

        self.target = Some(attacker);
        true
    }

    fn start(&mut self, mob: &dyn Mob) {
        mob.set_mob_target(self.target.clone(), TargetReason::TargetAttackedOwner);
        if let Some(trader) = leashing_trader(mob.get_entity())
            && let Some(trader_living) = trader.get_living_entity()
        {
            self.timestamp = trader_living.get_last_hurt_by_mob_timestamp();
        }
        self.track_target_goal.set_target_mob(self.target.clone());
        self.track_target_goal.start(mob);
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        self.track_target_goal.should_continue(mob)
    }

    fn stop(&mut self, mob: &dyn Mob) {
        self.target = None;
        self.track_target_goal.stop(mob);
    }

    fn controls(&self) -> Controls {
        self.track_target_goal.controls()
    }
}
