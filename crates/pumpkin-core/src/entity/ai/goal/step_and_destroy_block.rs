use super::{Controls, Goal, to_goal_ticks};
use crate::entity::ai::goal::ParentHandle;
use crate::entity::ai::goal::move_to_target_pos::{MoveToTargetPos, MoveToTargetPosGoal};
use crate::entity::mob::Mob;
use crate::world::World;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::particle::Particle;
use pumpkin_data::{Block, BlockStateId};
use pumpkin_protocol::java::client::play::ParticleOptions;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_util::random::{RandomImpl, get_seed, xoroshiro128::Xoroshiro};
use pumpkin_world::world::BlockFlags;
use rand::RngExt;
use std::sync::Arc;

const MAX_COOLDOWN: i32 = 20;
const PARTICLE_SPEED: f32 = 0.15;

pub struct StepAndDestroyBlockGoal<S: Stepping, M: MoveToTargetPos> {
    pub stepping: ParentHandle<S>,
    pub move_to_target_pos_goal: MoveToTargetPosGoal<M>,
    target_block: &'static Block,
    counter: i32,
}

impl<S: Stepping, M: MoveToTargetPos> StepAndDestroyBlockGoal<S, M> {
    // if stepping is set to None then will use itself for stepping
    #[must_use]
    pub fn new(
        stepping: ParentHandle<S>,
        move_to_target_pos: ParentHandle<M>,
        target_block: &'static Block,
        speed: f64,
        max_y_difference: i32,
    ) -> Self {
        let move_to_target_pos_goal =
            MoveToTargetPosGoal::new(move_to_target_pos, speed, 24, max_y_difference);

        Self {
            stepping,
            move_to_target_pos_goal,
            target_block,
            counter: 0,
        }
    }

    #[must_use]
    pub fn with_default(target_block: &'static Block, speed: f64, max_y_difference: i32) -> Self {
        Self::new(
            ParentHandle::none(),
            ParentHandle::none(),
            target_block,
            speed,
            max_y_difference,
        )
    }
    fn tweak_to_proper_pos(&self, pos: BlockPos, world: &World) -> Option<BlockPos> {
        if world.get_block(&pos).id == self.target_block.id {
            Some(pos)
        } else {
            let block_pos = [
                pos.down(),
                pos.west(),
                pos.east(),
                pos.north(),
                pos.south(),
                pos.down().down(),
            ];

            for pos in block_pos {
                if world.get_block(&pos).id == self.target_block.id {
                    return Some(pos);
                }
            }
            None
        }
    }
}

/// Vanilla `RemoveBlockGoal.tick` sends egg item particles whatever block is removed.
fn spawn_egg_particles(world: &World, mob: &dyn Mob, pos: BlockPos) {
    let mut random = mob.get_random();
    let mut spread = || (random.random::<f32>() - 0.5) * 0.08;
    let offset = Vector3::new(spread(), spread(), spread());
    let center = pos.to_f64();
    world.spawn_particle_with_options(
        Vector3::new(center.x, center.y + 0.7, center.z),
        offset,
        PARTICLE_SPEED,
        3,
        &ParticleOptions::Item(ItemStack::new(1, &Item::EGG)),
    );
}

/// The 20 single poof particles of vanilla `RemoveBlockGoal.tick` once the block is gone.
fn spawn_poof_particles(world: &World, pos: BlockPos) {
    let mut random = Xoroshiro::from_seed(get_seed());
    let center = pos.to_f64();
    for _ in 0..20 {
        let offset = Vector3::new(
            (random.next_gaussian() * 0.02) as f32,
            (random.next_gaussian() * 0.02) as f32,
            (random.next_gaussian() * 0.02) as f32,
        );
        world.spawn_particles(Particle::Poof, center, 1, offset, PARTICLE_SPEED);
    }
}

pub trait Stepping: Send + Sync {
    fn tick_stepping(&self, _world: Arc<World>, _block_pos: BlockPos) {}

    fn on_destroy_block(&self, _world: Arc<World>, _block_pos: BlockPos) {}
}

impl<S: Stepping + Send + Sync, M: MoveToTargetPos + Send + Sync> Goal
    for StepAndDestroyBlockGoal<S, M>
{
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        let world = mob.get_entity().world.load();
        let level_info = world.level_info.load();
        if !level_info.game_rules.mob_griefing {
            false
        } else if self.move_to_target_pos_goal.cooldown > 0 {
            self.move_to_target_pos_goal.cooldown -= 1;
            false
        } else if self.move_to_target_pos_goal.find_target_pos(mob) {
            self.move_to_target_pos_goal.cooldown = to_goal_ticks(MAX_COOLDOWN);
            true
        } else {
            self.move_to_target_pos_goal.cooldown = MoveToTargetPosGoal::<M>::get_interval(mob);
            false
        }
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        self.move_to_target_pos_goal.should_continue(mob)
    }

    fn start(&mut self, mob: &dyn Mob) {
        self.move_to_target_pos_goal.start(mob);
        self.counter = 0;
    }

    fn stop(&mut self, mob: &dyn Mob) {
        // Assuming fall_distance is AtomicF32/f32
        mob.get_mob_entity().living_entity.fall_distance.store(1.0);
    }

    fn tick(&mut self, mob: &dyn Mob) {
        self.move_to_target_pos_goal.tick(mob);
        let mob_entity = mob.get_mob_entity();
        let world = mob.get_entity().world.load_full();
        let block_pos = mob.get_entity().block_pos.load();

        let Some(tweak_pos) = self.tweak_to_proper_pos(block_pos, &world) else {
            return;
        };
        if !self.move_to_target_pos_goal.reached {
            return;
        }
        let counter = self.counter;

        if counter > 0 {
            let velocity = mob_entity.living_entity.entity.velocity.load();
            mob_entity
                .living_entity
                .entity
                .set_velocity(Vector3::new(velocity.x, 0.3, velocity.z));
            spawn_egg_particles(&world, mob, tweak_pos);
        }

        if counter % 2 == 0 {
            let velocity = mob_entity.living_entity.entity.velocity.load();
            mob_entity
                .living_entity
                .entity
                .set_velocity(Vector3::new(velocity.x, -0.3, velocity.z));
            if counter % 6 == 0 {
                if let Some(stepping) = self.stepping.get() {
                    stepping.tick_stepping(world.clone(), self.move_to_target_pos_goal.target_pos);
                } else {
                    self.tick_stepping(world.clone(), self.move_to_target_pos_goal.target_pos);
                }
            }
        }

        if counter > 60 {
            // Replace with air, no drops.
            world.set_block_state(&tweak_pos, BlockStateId::AIR, BlockFlags::NOTIFY_ALL);
            spawn_poof_particles(&world, tweak_pos);
            if let Some(stepping) = self.stepping.get() {
                stepping.on_destroy_block(world, tweak_pos);
            } else {
                self.on_destroy_block(world, tweak_pos);
            }
        }

        self.counter += 1;
    }

    fn should_run_every_tick(&self) -> bool {
        self.move_to_target_pos_goal.should_run_every_tick()
    }

    fn controls(&self) -> Controls {
        self.move_to_target_pos_goal.controls()
    }
}

impl<S: Stepping + Send + Sync, M: MoveToTargetPos + Send + Sync> MoveToTargetPos
    for StepAndDestroyBlockGoal<S, M>
{
    fn is_target_pos(&self, world: Arc<World>, block_pos: BlockPos) -> bool {
        world.get_block(&block_pos).id == self.target_block.id
            && world.get_block_state(&block_pos.up()).is_air()
            && world.get_block_state(&block_pos.up_height(2)).is_air()
    }
}

impl<S: Stepping + Send + Sync, M: MoveToTargetPos + Send + Sync> Stepping
    for StepAndDestroyBlockGoal<S, M>
{
    fn tick_stepping(&self, _world: Arc<World>, _block_pos: BlockPos) {}

    fn on_destroy_block(&self, _world: Arc<World>, _block_pos: BlockPos) {}
}
