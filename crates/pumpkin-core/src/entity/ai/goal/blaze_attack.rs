use std::sync::{Arc, Weak};

use pumpkin_data::attributes::Attributes;
use pumpkin_data::entity::EntityType;
use pumpkin_data::world::WorldEvent;
use pumpkin_util::math::vector3::Vector3;

use crate::entity::ai::util::RandomExt;
use crate::entity::{
    Entity,
    ai::goal::{Controls, Goal},
    living::LivingEntity,
    mob::{Mob, blaze::BlazeEntity},
    projectile::small_fireball::SmallFireballEntity,
};

const MELEE_RANGE_SQUARED: f64 = 4.0;
const MELEE_INTERVAL: i32 = 20;
const CHARGE_TIME: i32 = 60;
const SHOT_INTERVAL: i32 = 6;
/// Steps 2 to 4 each shoot one fireball.
const LAST_SHOT_STEP: i32 = 4;
const VOLLEY_COOLDOWN: i32 = 100;
const CHASE_UNSEEN_TICKS: i32 = 5;
const SPREAD: f64 = 2.297;

/// Vanilla `Blaze.BlazeAttackGoal`: melee up close, otherwise charge and shoot a volley of three
/// small fireballs.
pub struct BlazeAttackGoal {
    blaze: Weak<BlazeEntity>,
    attack_step: i32,
    attack_time: i32,
    last_seen: i32,
}

impl BlazeAttackGoal {
    #[must_use]
    pub const fn new(blaze: Weak<BlazeEntity>) -> Self {
        Self {
            blaze,
            attack_step: 0,
            attack_time: 0,
            last_seen: 0,
        }
    }

    fn set_charged(&self, charged: bool) {
        if let Some(blaze) = self.blaze.upgrade() {
            blaze.set_charged(charged);
        }
    }
}

impl Goal for BlazeAttackGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        let Some(target) = mob.get_mob_entity().get_target() else {
            return false;
        };
        target
            .get_living_entity()
            .is_some_and(LivingEntity::is_alive)
            && mob.can_attack(target.as_ref())
    }

    fn start(&mut self, _mob: &dyn Mob) {
        self.attack_step = 0;
    }

    fn stop(&mut self, _mob: &dyn Mob) {
        self.set_charged(false);
        self.last_seen = 0;
    }

    fn should_run_every_tick(&self) -> bool {
        true
    }

    fn tick(&mut self, mob: &dyn Mob) {
        self.attack_time -= 1;

        let mob_entity = mob.get_mob_entity();
        let Some(target) = mob_entity.get_target() else {
            return;
        };

        let has_line_of_sight = mob.has_line_of_sight(target.get_entity());
        if has_line_of_sight {
            self.last_seen = 0;
        } else {
            self.last_seen += 1;
        }

        let entity = mob.get_entity();
        let blaze_pos = entity.pos.load();
        let target_entity = target.get_entity();
        let target_pos = target_entity.pos.load();
        let distance_sq = blaze_pos.squared_distance_to_vec(&target_pos);
        let follow_distance = mob_entity
            .living_entity
            .get_attribute_value(&Attributes::FOLLOW_RANGE);

        if distance_sq < MELEE_RANGE_SQUARED {
            if !has_line_of_sight {
                return;
            }

            if self.attack_time <= 0 {
                self.attack_time = MELEE_INTERVAL;
                mob.do_hurt_target(target.as_ref());
            }

            mob_entity.steered().move_control().set_wanted_position(
                target_pos.x,
                target_pos.y,
                target_pos.z,
                1.0,
            );
        } else if distance_sq < follow_distance * follow_distance && has_line_of_sight {
            let xd = target_pos.x - blaze_pos.x;
            let yd = (target_pos.y + f64::from(target_entity.entity_dimension.load().height) * 0.5)
                - (blaze_pos.y + f64::from(entity.entity_dimension.load().height) * 0.5);
            let zd = target_pos.z - blaze_pos.z;

            if self.attack_time <= 0 {
                self.attack_step += 1;
                if self.attack_step == 1 {
                    self.attack_time = CHARGE_TIME;
                    self.set_charged(true);
                } else if self.attack_step <= LAST_SHOT_STEP {
                    self.attack_time = SHOT_INTERVAL;
                } else {
                    self.attack_time = VOLLEY_COOLDOWN;
                    self.attack_step = 0;
                    self.set_charged(false);
                }

                if self.attack_step > 1 {
                    shoot_fireball(mob, Vector3::new(xd, yd, zd), distance_sq);
                }
            }

            mob_entity
                .look_control
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .look_at_entity_with_range(&target, 10.0, 10.0);
        } else if self.last_seen < CHASE_UNSEEN_TICKS {
            mob_entity.steered().move_control().set_wanted_position(
                target_pos.x,
                target_pos.y,
                target_pos.z,
                1.0,
            );
        }
    }

    fn controls(&self) -> Controls {
        Controls::MOVE | Controls::LOOK
    }
}

/// One fireball towards `offset`, spread sideways more the farther the target is.
fn shoot_fireball(mob: &dyn Mob, offset: Vector3<f64>, distance_sq: f64) {
    let blaze = mob.get_entity();
    let world = blaze.world.load_full();
    let spread = SPREAD * distance_sq.sqrt().sqrt() * 0.5;

    if !blaze.is_silent() {
        world.sync_world_event(WorldEvent::SoundBlazeFireball, blaze.block_pos.load(), 0);
    }

    let direction = {
        let mut rng = mob.get_random();
        let x = rng.triangle(offset.x, spread);
        let z = rng.triangle(offset.z, spread);
        Vector3::new(x, offset.y, z)
    };

    let blaze_pos = blaze.pos.load();
    let spawn_pos = Vector3::new(
        blaze_pos.x,
        blaze_pos.y + f64::from(blaze.entity_dimension.load().height) * 0.5 + 0.5,
        blaze_pos.z,
    );
    let base_entity = Entity::new(world.clone(), spawn_pos, &EntityType::SMALL_FIREBALL);

    let fireball = SmallFireballEntity::new_shot(base_entity, blaze, direction);
    // `ThrownItemEntity::new` puts the projectile at the shooter's eye, re-anchor it.
    fireball.thrown.entity.pos.store(spawn_pos);
    world.spawn_entity(Arc::new(fireball));
}
