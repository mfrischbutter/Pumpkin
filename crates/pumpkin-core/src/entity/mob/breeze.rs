use std::sync::Arc;
use std::sync::atomic::Ordering;

use pumpkin_data::damage::DamageType;
use pumpkin_data::entity::EntityType;
use pumpkin_data::environment_attribute::Activity;
use pumpkin_data::sound::Sound;

use super::breeze_ai;
use crate::entity::ai::brain::memory::{MemoryModuleId, PackedMemories, types};
use crate::entity::ai::brain::sensing::SensorType;
use crate::entity::ai::brain::{Brain, BrainProvider};
use crate::entity::ai::pathfinder::node::PathType;
use crate::entity::{
    Entity, EntityBase,
    mob::{Mob, MobEntity},
};

/// Vanilla `Breeze.xpReward`.
const XP_REWARD: u32 = 10;
/// Vanilla `Breeze.getFiringYPosition`, above its middle.
const FIRING_HEIGHT: f64 = 0.3;

const MEMORIES: [MemoryModuleId; 1] = [types::NEAREST_ATTACKABLE.id()];

/// The trial chambers' whirl of wind, which leaps around its target and shoots wind charges.
///
/// Wiki: <https://minecraft.wiki/w/Breeze>
pub struct BreezeEntity {
    pub mob_entity: MobEntity,
}

impl BreezeEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let mut mob_entity = MobEntity::new(entity);
        let navigator = mob_entity
            .navigator
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        navigator.set_pathfinding_malus(PathType::DangerTrapdoor, -1.0);
        navigator.set_pathfinding_malus(PathType::DamageFire, -1.0);
        Arc::new(Self { mob_entity })
    }

    /// Vanilla `getFiringYPosition`.
    #[must_use]
    pub fn firing_y(&self) -> f64 {
        let entity = self.get_entity();
        entity.pos.load().y + f64::from(entity.height()) / 2.0 + FIRING_HEIGHT
    }
}

impl Mob for BreezeEntity {
    /// Vanilla `Breeze.makeBrain`: it starts in the fight activity.
    fn make_brain(&self, packed: &PackedMemories) -> Brain {
        let mut brain = BrainProvider {
            memory_types: &MEMORIES,
            sensor_types: &[
                SensorType::NearestLivingEntities,
                SensorType::HurtBy,
                SensorType::NearestPlayers,
                SensorType::BreezeAttackEntity,
            ],
            activities: breeze_ai::activities,
        }
        .make_brain(self, packed);
        brain.set_default_activity(Activity::Fight);
        brain.use_default_activity();
        brain
    }

    /// Vanilla `Breeze.customServerAiStep`.
    fn custom_server_ai_step(&self, _caller: &dyn EntityBase) {
        self.mob_entity.tick_brain(self);
        let target = self.mob_entity.with_brain(self, |brain| {
            breeze_ai::update_activity(brain);
            brain.get(types::ATTACK_TARGET).cloned()
        });
        // Vanilla `getTarget` reads the brain.
        self.mob_entity.set_target(target);
    }

    /// Vanilla `Breeze.canAttack`: only players and iron golems.
    fn can_attack(&self, target: &dyn EntityBase) -> bool {
        let kind = target.get_entity().entity_type;
        (kind == &EntityType::PLAYER || kind == &EntityType::IRON_GOLEM)
            && self.get_mob_entity().living_entity.can_attack(target)
    }

    /// Vanilla `Breeze.isInvulnerableTo`: breezes do not hurt each other.
    fn mob_is_invulnerable_to(
        &self,
        damage_type: &DamageType,
        source: Option<&dyn EntityBase>,
        cause: Option<&dyn EntityBase>,
    ) -> bool {
        cause
            .or(source)
            .is_some_and(|by| by.get_entity().entity_type == &EntityType::BREEZE)
            || self.get_entity().is_invulnerable_to(damage_type, cause)
    }

    /// Vanilla `Breeze.playAmbientSound` plays on the client only, as does the whirl.
    fn play_ambient_sound(&self) {}

    /// Vanilla `Breeze.getAmbientSound`.
    fn get_ambient_sound(&self) -> Option<Sound> {
        Some(if self.get_entity().on_ground.load(Ordering::Relaxed) {
            Sound::EntityBreezeIdleGround
        } else {
            Sound::EntityBreezeIdleAir
        })
    }

    /// Vanilla `Breeze.getMaxHeadYRot`.
    fn get_max_head_rotation(&self) -> f32 {
        30.0
    }

    fn get_base_experience_reward(&self) -> u32 {
        XP_REWARD
    }

    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }
}
