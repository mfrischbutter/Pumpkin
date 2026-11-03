use super::{Controls, Goal};
use crate::entity::ai::pathfinder::NavigatorGoal;
use crate::entity::mob::Mob;
use crate::entity::mob::sun_burn;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use rand::RngExt;

pub struct FleeSunGoal {
    goal_control: Controls,
    speed_modifier: f64,
    wanted_x: f64,
    wanted_y: f64,
    wanted_z: f64,
}

impl FleeSunGoal {
    #[must_use]
    pub const fn new(speed_modifier: f64) -> Self {
        Self {
            goal_control: Controls::MOVE,
            speed_modifier,
            wanted_x: 0.0,
            wanted_y: 0.0,
            wanted_z: 0.0,
        }
    }

    /// Vanilla `FleeSunGoal.getHidePos`: ten tries for a spot out of the sky the mob likes
    /// to walk to.
    fn find_hide_pos(mob: &dyn Mob) -> Option<Vector3<f64>> {
        let mob_pos = mob.get_entity().block_pos.load();
        let world = mob.get_entity().world.load();

        for _ in 0..10 {
            let check_pos = {
                let mut rng = mob.get_random();
                BlockPos::new(
                    mob_pos.0.x + rng.random_range(0..20) - 10,
                    mob_pos.0.y + rng.random_range(0..6) - 3,
                    mob_pos.0.z + rng.random_range(0..20) - 10,
                )
            };
            if !world.can_see_sky(&check_pos)
                && mob.get_walk_target_value(&check_pos, &**world) < 0.0
            {
                return Some(Vector3::new(
                    f64::from(check_pos.0.x) + 0.5,
                    f64::from(check_pos.0.y),
                    f64::from(check_pos.0.z) + 0.5,
                ));
            }
        }

        None
    }

    /// Vanilla `setWantedPos`: `true` when there is somewhere to go.
    pub fn set_wanted_pos(&mut self, mob: &dyn Mob) -> bool {
        if let Some(pos) = Self::find_hide_pos(mob) {
            self.wanted_x = pos.x;
            self.wanted_y = pos.y;
            self.wanted_z = pos.z;
            true
        } else {
            false
        }
    }
}

impl Goal for FleeSunGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        // Vanilla `FleeSunGoal.canUse`.
        let entity = &mob.get_mob_entity().living_entity.entity;
        if mob.get_mob_entity().get_target().is_some() {
            return false;
        }
        let world = entity.world.load();
        if !world.is_bright_outside() || !entity.is_on_fire() {
            return false;
        }
        if !world.can_see_sky(&entity.block_pos.load()) || sun_burn::is_protected(mob) {
            return false;
        }

        self.set_wanted_pos(mob)
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        let is_idle = mob.is_navigator_idle();
        !is_idle
    }

    fn start(&mut self, mob: &dyn Mob) {
        let mob_pos = mob.get_entity().pos.load();
        let steered = mob.get_mob_entity().steered();
        let mut navigator = steered.navigator();
        navigator.set_progress(NavigatorGoal::new(
            mob_pos,
            Vector3::new(self.wanted_x, self.wanted_y, self.wanted_z),
            self.speed_modifier,
        ));
    }

    fn controls(&self) -> Controls {
        self.goal_control
    }
}
