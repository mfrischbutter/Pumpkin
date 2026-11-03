//! Vanilla `SulfurCube`'s goals and its look control.

use std::sync::Arc;
use std::sync::atomic::Ordering::Relaxed;

use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::tag::{self, Taggable};
use pumpkin_util::math::vector3::Vector3;
use pumpkin_util::math::wrap_degrees_90;

use super::SulfurCubeEntity;
use super::body::is_swallowable_item;
use crate::entity::ageable::AgeableMob;
use crate::entity::ai::control::look_control::LookControl;
use crate::entity::ai::control::{Control, LookControlTrait};
use crate::entity::ai::goal::tempt::{TemptGoal, TemptSteering};
use crate::entity::ai::goal::{Controls, Goal};
use crate::entity::item::ItemEntity;
use crate::entity::mob::Mob;
use crate::entity::mob::cube_mob::{self, ATTACK_TURN_SPEED};
use crate::entity::player::Player;
use crate::entity::{Entity, EntityBase};

/// Vanilla `SulfurCube.addBehaviourGoals`: the speed its tempt goal hops with.
const TEMPT_SPEED_MODIFIER: f64 = 1.0;
/// Vanilla `SulfurCube.addBehaviourGoals`: where its tempt goal stops.
const TEMPT_STOP_DISTANCE: f64 = 1.0;
/// Vanilla `SulfurCubeSearchForItemsGoal.canUse`: how far around it looks for items.
const ITEM_SEARCH_RANGE: f64 = 8.0;

fn sulfur_cube(mob: &dyn Mob) -> Option<&SulfurCubeEntity> {
    mob.cast_any().downcast_ref::<SulfurCubeEntity>()
}

/// Vanilla `SulfurCube.addBehaviourGoals`: food tempts a baby, a swallowable block any other.
fn tempted_by(mob: &dyn Mob, stack: &ItemStack) -> bool {
    sulfur_cube(mob).is_some_and(|cube| {
        if cube.is_baby() {
            stack.item.has_tag(&tag::Item::MINECRAFT_SULFUR_CUBE_FOOD)
        } else {
            is_swallowable_item(stack)
        }
    })
}

/// Vanilla `SulfurCubeTemptGoal.stopNavigation`.
fn stop_navigation(mob: &dyn Mob) {
    if let Some(cube) = mob.as_cube_mob() {
        cube.cube_state().set_wanted_movement(0.0);
    }
}

/// Vanilla `SulfurCubeTemptGoal.navigateTowards`.
fn navigate_towards(mob: &dyn Mob, player: &Player) {
    let entity = mob.get_entity();
    cube_mob::look_at(entity, player, ATTACK_TURN_SPEED);
    if let Some(cube) = mob.as_cube_mob() {
        cube.cube_state().set_direction(entity.yaw.load(), true);
    }
}

/// Vanilla `SulfurCubeTemptGoal`.
pub(super) fn tempt_goal() -> TemptGoal {
    TemptGoal::for_non_pathfinders(TEMPT_SPEED_MODIFIER, tempted_by, false, TEMPT_STOP_DISTANCE)
        .with_controls(Controls::LOOK)
        .steered_by(TemptSteering {
            stop: stop_navigation,
            navigate_towards,
        })
}

/// Vanilla `SulfurCube.ALLOWED_ITEMS`.
fn is_allowed_item(item: &ItemEntity) -> bool {
    !item.has_pickup_delay()
        && item.get_entity().is_alive()
        && is_swallowable_item(
            &item
                .get_item_stack()
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        )
}

fn nearest_allowed_item(entity: &Entity) -> Option<Arc<dyn EntityBase>> {
    let pos = entity.pos.load();
    let area = entity.bounding_box.load().expand(
        ITEM_SEARCH_RANGE,
        ITEM_SEARCH_RANGE,
        ITEM_SEARCH_RANGE,
    );
    entity
        .world
        .load()
        .get_entities_at_box(&area)
        .into_iter()
        .filter(|candidate| candidate.get_item_entity().is_some_and(is_allowed_item))
        .min_by(|a, b| {
            let distance_a = a.get_entity().pos.load().squared_distance_to_vec(&pos);
            let distance_b = b.get_entity().pos.load().squared_distance_to_vec(&pos);
            distance_a.total_cmp(&distance_b)
        })
}

/// Vanilla `SulfurCubeSearchForItemsGoal`: hops at the nearest swallowable item.
#[derive(Default)]
pub struct SulfurCubeSearchForItemsGoal {
    target_item: Option<Arc<dyn EntityBase>>,
}

impl Goal for SulfurCubeSearchForItemsGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        let Some(cube) = sulfur_cube(mob) else {
            return false;
        };
        if cube.is_baby() || cube.pickup_timer.load(Relaxed) > 0 {
            return false;
        }
        self.target_item = nearest_allowed_item(mob.get_entity());
        self.target_item.is_some()
    }

    fn tick(&mut self, mob: &dyn Mob) {
        let entity = mob.get_entity();
        if let Some(target) = &self.target_item {
            cube_mob::look_at(entity, target.as_ref(), ATTACK_TURN_SPEED);
        }
        if let Some(cube) = mob.as_cube_mob() {
            cube.cube_state().set_direction(entity.yaw.load(), true);
        }
    }

    fn controls(&self) -> Controls {
        Controls::LOOK
    }
}

/// Vanilla `SulfurCubeLookControl`: with a block it snaps to the closest side, without it looks
/// around like any mob.
#[derive(Default)]
pub struct SulfurCubeLookControl {
    inner: LookControl,
}

impl Control for SulfurCubeLookControl {}

impl LookControlTrait for SulfurCubeLookControl {
    fn tick(&mut self, mob: &dyn Mob) {
        if !sulfur_cube(mob).is_some_and(SulfurCubeEntity::has_body_item) {
            self.inner.tick(mob);
            return;
        }
        let entity = mob.get_entity();
        let yaw = entity.yaw.load();
        let closest_side = yaw - wrap_degrees_90(yaw);
        entity.yaw.store(closest_side);
        entity.head_yaw.store(closest_side);
    }

    fn look_at_with_range(
        &mut self,
        x: f64,
        y: f64,
        z: f64,
        max_yaw_change: f32,
        max_pitch_change: f32,
    ) {
        self.inner
            .look_at_with_range(x, y, z, max_yaw_change, max_pitch_change);
    }

    fn wanted_position(&self) -> Vector3<f64> {
        LookControlTrait::wanted_position(&self.inner)
    }

    fn is_looking_at_target(&self) -> bool {
        LookControlTrait::is_looking_at_target(&self.inner)
    }
}
