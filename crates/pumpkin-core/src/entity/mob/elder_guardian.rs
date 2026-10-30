use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};

use pumpkin_data::damage::DamageType;
use pumpkin_data::effect::StatusEffect;
use pumpkin_data::entity::EntityType;
use pumpkin_data::potion::Effect;
use pumpkin_protocol::java::client::play::{CGameEvent, GameEvent};

use pumpkin_util::math::position::BlockPos;

use crate::entity::ai::behavior::hunting;
use crate::entity::mob::guardian::GUARDIAN_HUNTS;
use crate::entity::{
    Entity, EntityBase,
    ai::goal::{
        look_around::RandomLookAroundGoal, look_at_entity::LookAtEntityGoal,
        wander_around::WanderAroundGoal,
    },
    mob::{
        Mob, MobEntity,
        guardian::{
            MAX_HEAD_X_ROT, guardian_ai_step, guardian_travel_in_water, guardian_walk_target_value,
            initialize_guardian_movement,
        },
    },
};

const EFFECT_INTERVAL: i32 = 1200;

pub struct ElderGuardianEntity {
    pub mob_entity: MobEntity,
    moving: Arc<AtomicBool>,
}

impl ElderGuardianEntity {
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
            hunting::apply(target_selector, &mob_arc.mob_entity, GUARDIAN_HUNTS);
        };

        mob_arc
    }
}

impl Mob for ElderGuardianEntity {
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

    fn custom_server_ai_step(&self, _caller: &dyn EntityBase) {
        let entity = &self.mob_entity.living_entity.entity;
        if !entity.is_alive() {
            return;
        }

        if (entity.tick_count.load(Ordering::Relaxed) + entity.entity_id) % EFFECT_INTERVAL == 0 {
            let pos = entity.pos.load();
            let world = entity.world.load();
            let players = world.get_nearby_players(pos, 50.0);
            let packet = CGameEvent::new(GameEvent::PlayElderGuardianMobAppearance, 1.0);

            for player in players {
                player.try_send_client_packet(&packet);
                player.living_entity.add_effect(Effect {
                    effect_type: &StatusEffect::MINING_FATIGUE,
                    duration: 6000,
                    amplifier: 2,
                    ambient: false,
                    show_particles: true,
                    show_icon: true,
                    blend: false,
                });
            }
        }
    }

    fn on_damage(&self, _damage_type: DamageType, source: Option<&dyn EntityBase>) {
        if let Some(src) = source
            && let Some(living) = src.get_living_entity()
        {
            let _ = living.damage(src, 2.0, DamageType::THORNS);
        }
    }
}
