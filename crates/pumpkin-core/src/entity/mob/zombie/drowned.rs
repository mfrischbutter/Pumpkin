use pumpkin_data::BlockState;
use pumpkin_data::sound::Sound;
use pumpkin_util::math::position::BlockPos;
use std::sync::Arc;

use crate::entity::ai::behavior::hunting::{self, Hunt};
use crate::entity::ai::goal::swim::SwimGoal;
use crate::entity::ai::goal::wander_around::WanderAroundGoal;
use crate::entity::ai::goal::zombie_attack::ZombieAttackGoal;
use crate::entity::ai::target_match::TargetMatch;
use crate::entity::living::LivingEntity;
use crate::entity::mob::zombie::ZombieEntityBase;
use crate::entity::{
    Entity, EntityBase,
    mob::{Mob, MobEntity},
};
use crate::world::World;
use pumpkin_data::entity::EntityType;
use pumpkin_nbt::compound::NbtCompound;

const DROWNED_HUNTS: &[Hunt] = &[
    Hunt::new(2, TargetMatch::Type(&EntityType::PLAYER), true).matching(ok_target),
    Hunt::new(3, TargetMatch::VILLAGER_LIKE, false),
    Hunt::new(3, TargetMatch::Type(&EntityType::IRON_GOLEM), true),
    Hunt::new(3, TargetMatch::Type(&EntityType::AXOLOTL), true),
    hunting::baby_turtles(5, true),
];

/// Vanilla `okTarget`: players only at night or in water.
fn ok_target(_hunter: &dyn Mob, target: &LivingEntity, world: &World) -> bool {
    !world.is_bright_outside() || target.entity.is_in_water()
}

pub struct DrownedEntity {
    entity: Arc<ZombieEntityBase>,
}

impl DrownedEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        Self::with_can_break_doors(entity, false)
    }

    #[must_use]
    pub fn with_can_break_doors(entity: Entity, can_break_doors: bool) -> Arc<Self> {
        let entity = ZombieEntityBase::without_behaviour_goals(entity);
        Self::add_behaviour_goals(&entity.mob_entity);
        entity.set_can_break_doors(can_break_doors, &*entity);
        let zombie = Self { entity };
        Arc::new(zombie)
    }

    // TODO: vanilla `Drowned.addBehaviourGoals`; until then the drowned keeps the goals the
    // zombies had before they got their vanilla ones.
    fn add_behaviour_goals(mob: &MobEntity) {
        let goal_selector = &mob.goals_selector;
        goal_selector.add_goal(0, Box::new(SwimGoal::default()));
        goal_selector.add_goal(2, ZombieAttackGoal::new(1.0, false));
        goal_selector.add_goal(7, Box::new(WanderAroundGoal::new(1.0)));

        let target_selector = &mob.target_selector;
        target_selector.add_goal(
            1,
            Box::new(
                ZombieEntityBase::revenge_goal()
                    .ignoring(|entity_type| entity_type == &EntityType::DROWNED),
            ),
        );
        hunting::apply(target_selector, mob, DROWNED_HUNTS);
    }
}

impl Mob for DrownedEntity {
    fn mob_play_step_sound(&self, _pos: &BlockPos, _state: &BlockState) {
        self.play_entity_sound(Sound::EntityDrownedStep, 0.15, 1.0);
    }

    fn get_mob_entity(&self) -> &MobEntity {
        &self.entity.mob_entity
    }

    /// Vanilla `Drowned.checkSpawnObstruction`: only entities obstruct, the water does not.
    fn check_spawn_obstruction(&self, world: &crate::world::World) -> bool {
        world.is_unobstructed(&self.entity.mob_entity.living_entity.entity)
    }

    fn spawn_as_baby(&self) -> bool {
        self.entity.spawn_as_baby()
    }

    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        self.entity.mob_write_nbt(nbt);
    }

    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        self.entity.mob_read_nbt(nbt);
    }
}
