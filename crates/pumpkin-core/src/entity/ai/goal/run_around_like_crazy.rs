use pumpkin_data::entity::EntityStatus;
use pumpkin_protocol::bedrock::server::actor_event::ActorEventID;
use pumpkin_util::math::vector3::Vector3;
use rand::RngExt;

use crate::entity::ai::goal::{Controls, Goal, to_goal_ticks};
use crate::entity::ai::pathfinder::NavigatorGoal;
use crate::entity::ai::util::default_random_pos;
use crate::entity::mob::{Mob, saddle};
use crate::entity::passive::abstract_horse::{self as horse_base, AbstractHorse};

/// Vanilla `RunAroundLikeCrazyGoal`: a wild member of the horse family with a rider bucks
/// around; now and then it either takes the rider as owner, more likely the calmer it got, or
/// throws them off.
pub struct RunAroundLikeCrazyGoal {
    speed: f64,
    target: Option<Vector3<f64>>,
}

impl RunAroundLikeCrazyGoal {
    #[must_use]
    pub const fn new(speed: f64) -> Self {
        Self {
            speed,
            target: None,
        }
    }
}

impl Goal for RunAroundLikeCrazyGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        let Some(horse) = mob.as_abstract_horse() else {
            return false;
        };
        if horse.is_mob_controlled()
            || horse_base::is_tamed(horse)
            || !mob.get_entity().has_passengers()
        {
            return false;
        }
        self.target = default_random_pos::get_pos(mob, 5, 4);
        self.target.is_some()
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        mob.as_abstract_horse()
            .is_some_and(|horse| !horse_base::is_tamed(horse))
            && !mob.is_navigator_idle()
            && mob.get_entity().has_passengers()
    }

    fn start(&mut self, mob: &dyn Mob) {
        if let Some(target) = self.target {
            let pos = mob.get_entity().pos.load();
            mob.get_mob_entity()
                .steered()
                .navigator()
                .set_progress(NavigatorGoal::new(pos, target, self.speed));
        }
    }

    fn tick(&mut self, mob: &dyn Mob) {
        let Some(horse) = mob.as_abstract_horse() else {
            return;
        };
        if horse_base::is_tamed(horse) || mob.get_random().random_range(0..to_goal_ticks(50)) != 0 {
            return;
        }
        let entity = mob.get_entity();
        if !entity.has_passengers() {
            return;
        }
        if let Some(rider) = saddle::first_passenger_player(mob) {
            let max = horse.max_temper();
            if max > 0 && mob.get_random().random_range(0..max) < horse_base::temper(horse) {
                horse_base::tame_with_name(horse, &rider);
                return;
            }
            horse_base::modify_temper(horse, 5);
        }
        eject_passengers(horse);
        horse_base::make_mad(horse);
        entity.world.load().send_entity_status(
            entity,
            EntityStatus::TamingFailed,
            ActorEventID::TamingFailed,
        );
    }

    fn controls(&self) -> Controls {
        Controls::MOVE
    }
}

fn eject_passengers(horse: &dyn AbstractHorse) {
    let entity = horse.get_entity();
    let passengers: Vec<i32> = entity
        .passengers
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .iter()
        .map(|passenger| passenger.get_entity().entity_id)
        .collect();
    for id in passengers {
        entity.remove_passenger(id);
    }
}
