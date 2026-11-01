use std::sync::atomic::Ordering;

use pumpkin_data::Block;
use pumpkin_data::data_component_impl::EquipmentSlot;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use rand::RngExt;

use super::{PandaEntity, TOTAL_UNHAPPY_TIME, as_panda, data, edible_items};
use crate::entity::EntityBase;
use crate::entity::ageable::AgeableMob;
use crate::entity::ai::control::move_control::MoveControl;
use crate::entity::ai::control::{Control, MoveControlTrait};
use crate::entity::ai::goal::breed::BreedGoal;
use crate::entity::ai::goal::escape_danger::EscapeDangerGoal;
use crate::entity::ai::goal::look_at_entity::LookAtEntityGoal;
use crate::entity::ai::goal::{Controls, Goal, GoalReach, to_goal_ticks};
use crate::entity::ai::pathfinder::NavigatorGoal;
use crate::entity::mob::Mob;

/// Vanilla `Panda.PandaMoveControl`: it only moves while it can act.
#[derive(Default)]
pub(super) struct PandaMoveControl {
    inner: MoveControl,
}

impl Control for PandaMoveControl {}

impl MoveControlTrait for PandaMoveControl {
    fn tick(&mut self, mob: &dyn Mob) {
        if as_panda(mob).is_none_or(PandaEntity::can_perform_action) {
            self.inner.tick(mob);
        }
    }

    fn set_wanted_position(&mut self, x: f64, y: f64, z: f64, speed_modifier: f64) {
        self.inner.set_wanted_position(x, y, z, speed_modifier);
    }

    fn strafe(&mut self, forwards: f32, right: f32) {
        self.inner.strafe(forwards, right);
    }

    fn has_wanted(&self) -> bool {
        self.inner.has_wanted()
    }

    fn get_speed_modifier(&self) -> f64 {
        self.inner.get_speed_modifier()
    }

    fn get_wanted_position(&self) -> Vector3<f64> {
        self.inner.get_wanted_position()
    }
}

/// Vanilla `Panda.PandaPanicGoal`: sitting down ends the panic.
pub(super) struct PandaPanicGoal(pub(super) Box<EscapeDangerGoal>);

impl Goal for PandaPanicGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        self.0.can_start(mob)
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        if as_panda(mob).is_some_and(PandaEntity::is_sitting) {
            mob.get_mob_entity().steered().navigator().stop();
            return false;
        }
        self.0.should_continue(mob)
    }

    fn start(&mut self, mob: &dyn Mob) {
        self.0.start(mob);
    }

    fn stop(&mut self, mob: &dyn Mob) {
        self.0.stop(mob);
    }

    fn controls(&self) -> Controls {
        self.0.controls()
    }

    fn tick_reach(&self, mob: &dyn Mob, running: bool) -> Option<GoalReach> {
        self.0.tick_reach(mob, running)
    }
}

/// Vanilla `Panda.PandaBreedGoal`: without bamboo within 8 blocks it sulks instead.
pub(super) struct PandaBreedGoal {
    pub(super) inner: Box<BreedGoal>,
    pub(super) unhappy_cooldown: i32,
}

impl PandaBreedGoal {
    /// Vanilla `canFindBamboo`: bamboo up to 2 above the feet, 7 blocks around.
    fn can_find_bamboo(mob: &dyn Mob) -> bool {
        let entity = mob.get_entity();
        let world = entity.world.load();
        let base = entity.block_pos.load();
        (0..3).any(|dy| {
            (-7..=7).any(|dx: i32| {
                (-7..=7).any(|dz: i32| {
                    world.get_block(&BlockPos::new(base.0.x + dx, base.0.y + dy, base.0.z + dz))
                        == &Block::BAMBOO
                })
            })
        })
    }
}

impl Goal for PandaBreedGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        let Some(panda) = as_panda(mob) else {
            return false;
        };
        if !self.inner.can_start(mob) || panda.unhappy_counter.load(Ordering::Relaxed) != 0 {
            return false;
        }
        if Self::can_find_bamboo(mob) {
            return true;
        }
        let tick = panda.get_entity().tick_count.load(Ordering::Relaxed);
        if self.unhappy_cooldown <= tick {
            panda.set_counter(
                &panda.unhappy_counter,
                data::UNHAPPY_COUNTER,
                TOTAL_UNHAPPY_TIME,
            );
            self.unhappy_cooldown = tick + 600;
        }
        false
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        self.inner.should_continue(mob)
    }

    fn start(&mut self, mob: &dyn Mob) {
        self.inner.start(mob);
    }

    fn stop(&mut self, mob: &dyn Mob) {
        self.inner.stop(mob);
    }

    fn tick(&mut self, mob: &dyn Mob) {
        self.inner.tick(mob);
    }

    fn should_run_every_tick(&self) -> bool {
        self.inner.should_run_every_tick()
    }

    fn controls(&self) -> Controls {
        self.inner.controls()
    }

    fn tick_reach(&self, mob: &dyn Mob, running: bool) -> Option<GoalReach> {
        self.inner.tick_reach(mob, running)
    }
}

/// Vanilla `Panda.PandaLookAtPlayerGoal`: only while it can act.
pub(super) struct PandaLookAtPlayerGoal(pub(super) Box<LookAtEntityGoal>);

impl Goal for PandaLookAtPlayerGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        self.0.can_start(mob) && as_panda(mob).is_some_and(PandaEntity::can_perform_action)
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        self.0.should_continue(mob)
    }

    fn start(&mut self, mob: &dyn Mob) {
        self.0.start(mob);
    }

    fn stop(&mut self, mob: &dyn Mob) {
        self.0.stop(mob);
    }

    fn tick(&mut self, mob: &dyn Mob) {
        self.0.tick(mob);
    }

    fn should_run_every_tick(&self) -> bool {
        self.0.should_run_every_tick()
    }

    fn controls(&self) -> Controls {
        self.0.controls()
    }

    fn tick_reach(&self, mob: &dyn Mob, running: bool) -> Option<GoalReach> {
        self.0.tick_reach(mob, running)
    }
}

/// Vanilla `Panda.PandaSitGoal`: with food in its paw, or edible items within 6 blocks, it
/// fetches them and sits down to eat.
pub(super) struct PandaSitGoal {
    pub(super) cooldown: i32,
}

impl Goal for PandaSitGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        let Some(panda) = as_panda(mob) else {
            return false;
        };
        if self.cooldown > panda.get_entity().tick_count.load(Ordering::Relaxed)
            || panda.is_baby()
            || panda.get_entity().is_in_water()
            || !panda.can_perform_action()
            || panda.unhappy_counter.load(Ordering::Relaxed) > 0
        {
            return false;
        }
        !panda.held_item().is_empty() || !edible_items(mob, 6.0).is_empty()
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        let Some(panda) = as_panda(mob) else {
            return false;
        };
        let mut rng = mob.get_random();
        !panda.get_entity().is_in_water()
            && (panda.is_lazy() || rng.random_range(0..to_goal_ticks(600)) != 1)
            && rng.random_range(0..to_goal_ticks(2000)) != 1
    }

    fn start(&mut self, mob: &dyn Mob) {
        let Some(panda) = as_panda(mob) else {
            return;
        };
        if panda.held_item().is_empty() {
            if let Some(item) = edible_items(mob, 8.0).first() {
                let pos = mob.get_entity().pos.load();
                mob.get_mob_entity()
                    .steered()
                    .navigator()
                    .set_progress(NavigatorGoal::new(pos, item.get_entity().pos.load(), 1.2));
            }
        } else {
            panda.try_to_sit();
        }
        self.cooldown = 0;
    }

    fn stop(&mut self, mob: &dyn Mob) {
        let Some(panda) = as_panda(mob) else {
            return;
        };
        let held = panda.held_item();
        if !held.is_empty() {
            panda.get_entity().spawn_at_location(held);
            panda
                .mob_entity
                .set_item_slot(&EquipmentSlot::MAIN_HAND, ItemStack::EMPTY.clone());
            let wait_seconds = {
                let mut rng = mob.get_random();
                if panda.is_lazy() {
                    rng.random_range(0..50) + 10
                } else {
                    rng.random_range(0..150) + 10
                }
            };
            self.cooldown =
                panda.get_entity().tick_count.load(Ordering::Relaxed) + wait_seconds * 20;
        }
        panda.set_sitting(false);
    }

    fn tick(&mut self, mob: &dyn Mob) {
        if let Some(panda) = as_panda(mob)
            && !panda.is_sitting()
            && !panda.held_item().is_empty()
        {
            panda.try_to_sit();
        }
    }

    fn controls(&self) -> Controls {
        Controls::MOVE
    }
}

/// Vanilla `Panda.PandaLieOnBackGoal`: lazy pandas roll onto their back now and then.
pub(super) struct PandaLieOnBackGoal {
    pub(super) cooldown: i32,
}

impl Goal for PandaLieOnBackGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        as_panda(mob).is_some_and(|panda| {
            self.cooldown < panda.get_entity().tick_count.load(Ordering::Relaxed)
                && panda.is_lazy()
                && panda.can_perform_action()
                && mob.get_random().random_range(0..to_goal_ticks(400)) == 1
        })
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        let Some(panda) = as_panda(mob) else {
            return false;
        };
        let mut rng = mob.get_random();
        !panda.get_entity().is_in_water()
            && (panda.is_lazy() || rng.random_range(0..to_goal_ticks(600)) != 1)
            && rng.random_range(0..to_goal_ticks(2000)) != 1
    }

    fn start(&mut self, mob: &dyn Mob) {
        if let Some(panda) = as_panda(mob) {
            panda.set_on_back(true);
        }
        self.cooldown = 0;
    }

    fn stop(&mut self, mob: &dyn Mob) {
        if let Some(panda) = as_panda(mob) {
            panda.set_on_back(false);
            self.cooldown = panda.get_entity().tick_count.load(Ordering::Relaxed) + 200;
        }
    }
}

/// Vanilla `Panda.PandaSneezeGoal`: babies sneeze, weak ones more often.
pub(super) struct PandaSneezeGoal;

impl Goal for PandaSneezeGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        let Some(panda) = as_panda(mob) else {
            return false;
        };
        if !panda.is_baby() || !panda.can_perform_action() {
            return false;
        }
        let mut rng = mob.get_random();
        (panda.is_weak() && rng.random_range(0..to_goal_ticks(500)) == 1)
            || rng.random_range(0..to_goal_ticks(6000)) == 1
    }

    fn should_continue(&mut self, _mob: &dyn Mob) -> bool {
        false
    }

    fn start(&mut self, mob: &dyn Mob) {
        if let Some(panda) = as_panda(mob) {
            panda.set_sneezing(true);
        }
    }
}

/// Vanilla `Panda.PandaRollGoal`: babies and playful pandas somersault, always before a drop.
pub(super) struct PandaRollGoal;

impl Goal for PandaRollGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        let Some(panda) = as_panda(mob) else {
            return false;
        };
        let entity = panda.get_entity();
        if !(panda.is_baby() || panda.is_playful())
            || !entity.on_ground.load(Ordering::Relaxed)
            || !panda.can_perform_action()
        {
            return false;
        }
        let angle = entity.yaw.load().to_radians();
        let (x_dir, z_dir) = (-angle.sin(), angle.cos());
        let step = |dir: f32| {
            if dir.abs() > 0.5 {
                dir.signum() as i32
            } else {
                0
            }
        };
        let pos = entity.block_pos.load();
        let ahead = BlockPos::new(pos.0.x + step(x_dir), pos.0.y - 1, pos.0.z + step(z_dir));
        if entity.world.load().get_block_state(&ahead).is_air() {
            return true;
        }
        let mut rng = mob.get_random();
        (panda.is_playful() && rng.random_range(0..to_goal_ticks(60)) == 1)
            || rng.random_range(0..to_goal_ticks(500)) == 1
    }

    fn should_continue(&mut self, _mob: &dyn Mob) -> bool {
        false
    }

    fn start(&mut self, mob: &dyn Mob) {
        if let Some(panda) = as_panda(mob) {
            panda.set_rolling(true);
        }
    }

    fn can_stop(&self) -> bool {
        false
    }

    fn controls(&self) -> Controls {
        Controls::MOVE | Controls::LOOK | Controls::JUMP
    }
}
