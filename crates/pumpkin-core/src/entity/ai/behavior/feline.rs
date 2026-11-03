//! Goals cats and ocelots share

use std::sync::Weak;

use pumpkin_data::entity::{EntityPose, EntityType};
use pumpkin_data::item::Item;

use crate::entity::ai::behavior::fleeing::Flee;
use crate::entity::ai::goal::breed::BreedGoal;
use crate::entity::ai::goal::goal_selector::GoalSelector;
use crate::entity::ai::goal::leap_at_target::LeapAtTargetGoal;
use crate::entity::ai::goal::look_at_entity::LookAtEntityGoal;
use crate::entity::ai::goal::ocelot_attack::OcelotAttackGoal;
use crate::entity::ai::goal::revenge::MobFilter;
use crate::entity::ai::goal::tempt::TemptGoal;
use crate::entity::ai::goal::wander_around::WanderAroundGoal;
use crate::entity::ai::target_match::TargetMatch;
use crate::entity::mob::Mob;

pub const TEMPT_ITEMS: &[&Item] = &[&Item::COD, &Item::SALMON];
/// Vanilla `Cat.TEMPT_SPEED_MOD`.
const TEMPT_SPEED: f64 = 0.6;
/// Vanilla `Cat.SPRINT_SPEED_MOD`.
const SPRINT_SPEED: f64 = 1.33;

#[must_use]
pub fn tempt_goal() -> TemptGoal {
    TemptGoal::new(TEMPT_SPEED, TEMPT_ITEMS, true)
}

/// Vanilla `Cat.customServerAiStep` and `Ocelot.customServerAiStep`: it creeps at the speed it
/// is tempted at and sprints at the speed it flees at.
pub fn update_pose_by_speed(mob: &dyn Mob) {
    let entity = mob.get_entity();
    let speed = {
        let steered = mob.get_mob_entity().steered();
        let control = steered.move_control();
        control.has_wanted().then(|| control.get_speed_modifier())
    };
    let creeping = speed.is_some_and(|speed| (speed - TEMPT_SPEED).abs() < 1.0E-6);
    let sprinting = speed.is_some_and(|speed| (speed - SPRINT_SPEED).abs() < 1.0E-6);
    entity.set_pose(if creeping {
        EntityPose::Crouching
    } else {
        EntityPose::Standing
    });
    entity.set_sprinting(sprinting);
}

/// Shy of players while `gate` holds: untamed cats, wary ocelots.
#[must_use]
pub const fn flee_players(priority: u8, gate: MobFilter) -> Flee {
    Flee::new(
        priority,
        TargetMatch::Type(&EntityType::PLAYER),
        16.0,
        0.8,
        1.33,
    )
    .as_long_as(gate)
}

/// Leap, pounce, breed, stroll and look at players, on consecutive priorities from `first`.
pub fn apply_tail(goals: &GoalSelector, mob: Weak<dyn Mob>, first: u8) {
    goals.add_goal(first, Box::new(LeapAtTargetGoal::new(0.3)));
    goals.add_goal(first + 1, Box::new(OcelotAttackGoal::new()));
    goals.add_goal(first + 2, BreedGoal::new(0.8));
    goals.add_goal(
        first + 3,
        Box::new(WanderAroundGoal::water_avoiding_with_probability(
            0.8, 1.0E-5,
        )),
    );
    goals.add_goal(
        first + 4,
        LookAtEntityGoal::with_default(mob, &EntityType::PLAYER, 10.0),
    );
}
