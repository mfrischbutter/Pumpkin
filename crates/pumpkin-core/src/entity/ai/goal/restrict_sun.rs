use super::{Controls, Goal};
use crate::entity::mob::Mob;
use crate::entity::mob::sun_burn;

/// Vanilla `RestrictSunGoal`: while the sun is up, an unprotected mob's paths stop before
/// they would leave the shade. Only ground navigations avoid the sun; the others ignore it.
#[derive(Default)]
pub struct RestrictSunGoal;

impl RestrictSunGoal {
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    fn set_avoid_sun(mob: &dyn Mob, avoid_sun: bool) {
        let steered = mob.get_mob_entity().steered();
        steered.navigator().set_avoid_sun(avoid_sun);
    }
}

impl Goal for RestrictSunGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        mob.get_entity().world.load().is_bright_outside() && !sun_burn::is_protected(mob)
    }

    fn start(&mut self, mob: &dyn Mob) {
        Self::set_avoid_sun(mob, true);
    }

    fn stop(&mut self, mob: &dyn Mob) {
        Self::set_avoid_sun(mob, false);
    }

    fn controls(&self) -> Controls {
        Controls::empty()
    }
}
