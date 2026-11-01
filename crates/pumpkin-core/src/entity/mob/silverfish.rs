use crate::world::level_accessor::LevelAccessor;
use std::sync::Arc;
use std::sync::atomic::AtomicI32;

use pumpkin_data::BlockState;
use pumpkin_data::damage::DamageType;
use pumpkin_data::entity::EntityType;
use pumpkin_data::sound::Sound;
use pumpkin_data::tag::{self, Taggable};
use pumpkin_util::math::position::BlockPos;

use crate::block::blocks::infested;

use crate::entity::ai::behavior::hunting::{self, Hunt};
use crate::entity::ai::target_match::TargetMatch;
use crate::entity::{
    Entity, EntityBase,
    ai::goal::{
        climb_on_top_of_powder_snow::ClimbOnTopOfPowderSnowGoal,
        melee_attack::MeleeAttackGoal,
        revenge::RevengeGoal,
        silverfish::{SilverfishMergeWithStoneGoal, SilverfishWakeUpFriendsGoal},
        swim::SwimGoal,
    },
    mob::{Mob, MobEntity},
};

const SILVERFISH_HUNTS: &[Hunt] = &[Hunt::new(2, TargetMatch::Type(&EntityType::PLAYER), true)];

pub struct SilverfishEntity {
    pub entity: Arc<MobEntity>,
    /// Shared with its `SilverfishWakeUpFriendsGoal`.
    look_for_friends: Arc<AtomicI32>,
}

impl SilverfishEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let entity = Arc::new(MobEntity::new(entity));
        let look_for_friends = Arc::new(AtomicI32::new(0));
        let silverfish = Self {
            entity,
            look_for_friends: look_for_friends.clone(),
        };
        let mob_arc = Arc::new(silverfish);

        {
            let goal_selector = &mob_arc.entity.goals_selector;
            let target_selector = &mob_arc.entity.target_selector;

            // Vanilla `Silverfish.registerGoals`.
            goal_selector.add_goal(1, Box::new(SwimGoal::default()));
            goal_selector.add_goal(1, Box::new(ClimbOnTopOfPowderSnowGoal));
            goal_selector.add_goal(
                3,
                Box::new(SilverfishWakeUpFriendsGoal::new(look_for_friends)),
            );
            goal_selector.add_goal(4, Box::new(MeleeAttackGoal::new(1.0, false)));
            goal_selector.add_goal(5, Box::new(SilverfishMergeWithStoneGoal::default()));

            target_selector.add_goal(1, Box::new(RevengeGoal::new(true).alerting_others()));
            hunting::apply(target_selector, &mob_arc.entity, SILVERFISH_HUNTS);
        };

        mob_arc
    }
}

impl Mob for SilverfishEntity {
    fn mob_play_step_sound(&self, _pos: &BlockPos, _state: &BlockState) {
        self.play_entity_sound(Sound::EntitySilverfishStep, 0.15, 1.0);
    }

    fn get_mob_entity(&self) -> &MobEntity {
        &self.entity
    }

    fn get_ambient_sound(&self) -> Option<Sound> {
        Some(Sound::EntitySilverfishAmbient)
    }

    fn mob_tick(&self, _caller: &dyn EntityBase) {
        let entity = &self.entity.living_entity.entity;
        if !entity.is_alive() {
            return;
        }

        let yaw = entity.yaw.load();
        entity.body_yaw.store(yaw);
        entity.head_yaw.store(yaw);
    }

    /// Vanilla `Silverfish.hurtServer`: an attacker, or damage that always does, wakes friends.
    fn on_damage(&self, damage_type: DamageType, source: Option<&dyn EntityBase>) {
        if source.is_some()
            || damage_type.has_tag(&tag::DamageType::MINECRAFT_ALWAYS_TRIGGERS_SILVERFISH)
        {
            SilverfishWakeUpFriendsGoal::notify_hurt(&self.look_for_friends);
        }
    }

    /// Vanilla `Silverfish.getWalkTargetValue`: best on blocks it can hide in.
    fn get_walk_target_value(&self, pos: &BlockPos, level: &dyn LevelAccessor) -> f32 {
        if infested::is_compatible_host_block(level.get_block_state_id(&pos.down())) {
            10.0
        } else {
            -level.get_pathfinding_cost_from_light_levels(pos)
        }
    }
}
