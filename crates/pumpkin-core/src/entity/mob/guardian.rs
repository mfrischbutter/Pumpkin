use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};

use pumpkin_data::entity::EntityType;
use pumpkin_data::fluid::Fluid;
use pumpkin_data::{damage::DamageType, sound::Sound};
use pumpkin_util::math::position::BlockPos;
use rand::RngExt;

use crate::entity::custom_sound::CustomSound;
use crate::entity::{
    Entity, EntityBase,
    ai::{
        control::guardian_move_control::GuardianMoveControl,
        goal::{
            active_target::ActiveTargetGoal, look_around::RandomLookAroundGoal,
            look_at_entity::LookAtEntityGoal, wander_around::WanderAroundGoal,
        },
        pathfinder::{Navigator, node::PathType},
    },
    mob::{Mob, MobEntity},
};

const WATER_ACCELERATION: f64 = 0.1;
const WATER_FRICTION: f64 = 0.9;
const SINK_SPEED: f64 = 0.005;
const LAND_HOP_HORIZONTAL: f32 = 0.4;
const LAND_HOP_VERTICAL: f64 = 0.5;
/// `Guardian.getMaxHeadXRot`, also the elder's.
pub const MAX_HEAD_X_ROT: f32 = 180.0;

/// Vanilla `Guardian`'s water navigation (water malus 0) and move control, shared with the
/// elder guardian. Returns the synced `DATA_ID_MOVING` flag the move control keeps.
pub fn initialize_guardian_movement(mob: &mut MobEntity) -> Arc<AtomicBool> {
    let moving = Arc::new(AtomicBool::new(false));
    let navigator = mob
        .navigator
        .get_mut()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    *navigator = Navigator::water_bound(false);
    navigator.set_pathfinding_malus(PathType::Water, 0.0);
    *mob.move_control
        .get_mut()
        .unwrap_or_else(std::sync::PoisonError::into_inner) =
        Box::new(GuardianMoveControl::new(moving.clone()));
    moving
}

/// Vanilla `Guardian.travelInWater`: no gravity, and a slow sink while idle without a target.
pub fn guardian_travel_in_water(mob: &dyn Mob, caller: &dyn EntityBase, moving: bool) {
    let mob_entity = mob.get_mob_entity();
    let living = &mob_entity.living_entity;
    let entity = &living.entity;
    entity.update_velocity_from_input(living.movement_input.load(), WATER_ACCELERATION);
    living.make_move(caller);
    let mut velocity = entity.velocity.load() * WATER_FRICTION;
    if !moving && mob_entity.get_target().is_none() {
        velocity.y -= SINK_SPEED;
    }
    entity.velocity.store(velocity);
}

/// The server part of vanilla `Guardian.aiStep`: full air in water, a random hop on land.
pub fn guardian_ai_step(mob: &dyn Mob) {
    let living = &mob.get_mob_entity().living_entity;
    let entity = &living.entity;
    if !living.is_alive() {
        return;
    }
    if entity.is_in_water() {
        entity.set_air_supply(300);
    } else if entity.on_ground.load(Ordering::Relaxed) {
        let mut rng = mob.get_random();
        let x = (rng.random::<f32>() * 2.0 - 1.0) * LAND_HOP_HORIZONTAL;
        let z = (rng.random::<f32>() * 2.0 - 1.0) * LAND_HOP_HORIZONTAL;
        entity.velocity.store(entity.velocity.load().add_raw(
            f64::from(x),
            LAND_HOP_VERTICAL,
            f64::from(z),
        ));
        entity.yaw.store(rng.random::<f32>() * 360.0);
        entity.on_ground.store(false, Ordering::Relaxed);
        entity.velocity_dirty.store(true, Ordering::Relaxed);
    }
}

/// Vanilla `Guardian.getWalkTargetValue`: water first, else dark spots like any monster.
pub fn guardian_walk_target_value(mob: &dyn Mob, pos: &BlockPos) -> f32 {
    let world = mob.get_entity().world.load();
    let light_cost = world.get_light_level_dependent_magic_value(pos) - 0.5;
    if world
        .get_fluid_and_fluid_state(pos)
        .0
        .matches_type(&Fluid::WATER)
    {
        10.0 + light_cost
    } else {
        -light_cost
    }
}

pub struct GuardianEntity {
    pub mob_entity: MobEntity,
    moving: Arc<AtomicBool>,
}

impl GuardianEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let mut mob_entity = MobEntity::new(entity);
        let moving = initialize_guardian_movement(&mut mob_entity);
        let guardian = Self { mob_entity, moving };
        let mob_arc = Arc::new(guardian);
        let mob_weak: Weak<dyn Mob> = {
            let mob_arc: Arc<dyn Mob> = mob_arc.clone();
            Arc::downgrade(&mob_arc)
        };

        {
            let goal_selector = &mob_arc.mob_entity.goals_selector;

            goal_selector.add_goal(4, Box::new(WanderAroundGoal::new(1.0)));
            goal_selector.add_goal(
                5,
                LookAtEntityGoal::with_default(mob_weak.clone(), &EntityType::PLAYER, 8.0),
            );
            goal_selector.add_goal(6, Box::new(RandomLookAroundGoal::default()));

            let target_selector = &mob_arc.mob_entity.target_selector;
            target_selector.add_goal(
                1,
                ActiveTargetGoal::with_default(&mob_arc.mob_entity, &EntityType::PLAYER, true),
            );
            target_selector.add_goal(
                2,
                ActiveTargetGoal::with_default(&mob_arc.mob_entity, &EntityType::SQUID, true),
            );
            target_selector.add_goal(
                2,
                ActiveTargetGoal::with_default(&mob_arc.mob_entity, &EntityType::GLOW_SQUID, true),
            );
            target_selector.add_goal(
                3,
                ActiveTargetGoal::with_default(&mob_arc.mob_entity, &EntityType::AXOLOTL, true),
            );
        };

        mob_arc
    }
}

impl CustomSound for GuardianEntity {
    fn hurt_sound(&self) -> Option<Sound> {
        let entity = self.get_entity();
        let is_water = entity.touching_water.load(Ordering::Relaxed);
        Some(if is_water {
            Sound::EntityGuardianHurt
        } else {
            Sound::EntityGuardianHurtLand
        })
    }

    fn death_sound(&self) -> Option<Sound> {
        let entity = self.get_entity();
        let is_water = entity.touching_water.load(Ordering::Relaxed);
        Some(if is_water {
            Sound::EntityGuardianDeath
        } else {
            Sound::EntityGuardianDeathLand
        })
    }
}

impl Mob for GuardianEntity {
    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    fn get_max_look_pitch_change(&self) -> f32 {
        MAX_HEAD_X_ROT
    }

    fn before_mob_ai_step(&self) {
        guardian_ai_step(self);
    }

    fn travel_in_water(&self, caller: &dyn EntityBase, _gravity: f64, _falling: bool) {
        guardian_travel_in_water(self, caller, self.moving.load(Ordering::Relaxed));
    }

    fn get_walk_target_value(&self, pos: &BlockPos) -> f32 {
        guardian_walk_target_value(self, pos)
    }

    fn on_damage(&self, _damage_type: DamageType, source: Option<&dyn EntityBase>) {
        if let Some(src) = source
            && let Some(living) = src.get_living_entity()
        {
            let _ = living.damage(src, 2.0, DamageType::THORNS);
        }
    }
}
