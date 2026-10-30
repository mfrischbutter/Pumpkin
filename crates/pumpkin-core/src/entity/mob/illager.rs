//! Vanilla `AbstractIllager`: what every illager shares.

use pumpkin_data::entity::EntityType;
use pumpkin_data::tag::{self, Taggable};

use crate::entity::EntityBase;
use crate::entity::ai::target_match::TargetMatch;
use crate::entity::living::LivingEntity;
use crate::entity::mob::Mob;

/// Vanilla `AbstractIllager.canAttack`: never a baby villager.
#[must_use]
pub fn can_attack(mob: &dyn Mob, target: &dyn EntityBase) -> bool {
    let target_type = target.get_entity().entity_type;
    if TargetMatch::VILLAGER_LIKE.matches(target_type)
        && target
            .get_living_entity()
            .is_some_and(LivingEntity::is_baby)
    {
        return false;
    }
    // Vanilla's `super.canAttack` goes through `Mob`.
    target_type != &EntityType::GHAST && mob.get_mob_entity().living_entity.can_attack(target)
}

/// Vanilla `AbstractIllager.considersEntityAsAlly`: its team, or any illager friend while
/// neither is in a team.
#[must_use]
pub fn considers_entity_as_ally(mob: &dyn Mob, other: &dyn EntityBase) -> bool {
    mob.is_on_same_team(other)
        || (other
            .get_entity()
            .entity_type
            .has_tag(&tag::EntityType::MINECRAFT_ILLAGER_FRIENDS)
            && mob.get_team_name().is_none()
            && other.get_team_name().is_none())
}
