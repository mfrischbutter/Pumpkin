use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};

use pumpkin_data::BlockDirection;
use pumpkin_data::entity::EntityStatus;
use pumpkin_protocol::bedrock::server::actor_event::ActorEventID;
use pumpkin_util::math::position::BlockPos;
use pumpkin_world::world::BlockFlags;
use rand::RngExt;

use super::{Controls, Goal, to_goal_ticks, wander_around::WanderAroundGoal};
use crate::block::blocks::infested;
use crate::entity::mob::Mob;

/// Vanilla `Silverfish.SilverfishWakeUpFriendsGoal`: a hurt silverfish wakes the silverfish
/// hiding in infested blocks around it, one block more for every coin flip that comes up tails.
pub struct SilverfishWakeUpFriendsGoal {
    look_for_friends: Arc<AtomicI32>,
}

impl SilverfishWakeUpFriendsGoal {
    /// `look_for_friends` is shared with the silverfish, which arms it when hurt.
    #[must_use]
    pub const fn new(look_for_friends: Arc<AtomicI32>) -> Self {
        Self { look_for_friends }
    }

    /// Vanilla `notifyHurt`.
    pub fn notify_hurt(look_for_friends: &AtomicI32) {
        let _ = look_for_friends.compare_exchange(
            0,
            to_goal_ticks(20),
            Ordering::Relaxed,
            Ordering::Relaxed,
        );
    }

    /// The vanilla search order: 0, 1, -1, 2, -2, ... up to `reach` each way.
    fn offsets(reach: i32) -> impl Iterator<Item = i32> {
        std::iter::successors(Some(0), |offset| Some(i32::from(*offset <= 0) - offset))
            .take_while(move |offset| (-reach..=reach).contains(offset))
    }

    fn wake_friends(mob: &dyn Mob) {
        let entity = mob.get_entity();
        let world = entity.world.load_full();
        let base = entity.block_pos.load();
        let game_rules = world.level_info.load().game_rules.clone();
        for y in Self::offsets(5) {
            for x in Self::offsets(10) {
                for z in Self::offsets(10) {
                    let pos = base.offset(pumpkin_util::math::vector3::Vector3::new(x, y, z));
                    let state_id = world.get_block_state_id(&pos);
                    let Some(host) = infested::host_state_by_infested(state_id) else {
                        continue;
                    };
                    if game_rules.mob_griefing {
                        world.break_block(&pos, None, BlockFlags::NOTIFY_ALL);
                        if game_rules.block_drops {
                            infested::spawn_infestation(&world, &pos);
                        }
                    } else {
                        world.set_block_state(&pos, host, BlockFlags::NOTIFY_ALL);
                    }
                    if mob.get_random().random::<bool>() {
                        return;
                    }
                }
            }
        }
    }
}

impl Goal for SilverfishWakeUpFriendsGoal {
    fn can_start(&mut self, _mob: &dyn Mob) -> bool {
        self.look_for_friends.load(Ordering::Relaxed) > 0
    }

    fn tick(&mut self, mob: &dyn Mob) {
        if self.look_for_friends.fetch_sub(1, Ordering::Relaxed) - 1 <= 0 {
            Self::wake_friends(mob);
        }
    }

    fn controls(&self) -> Controls {
        Controls::empty()
    }
}

/// Vanilla `Silverfish.SilverfishMergeWithStoneGoal`: an idle silverfish strolls, and now and
/// then hides in the block next to it when that block can host it.
pub struct SilverfishMergeWithStoneGoal {
    stroll: WanderAroundGoal,
    merge_pos: Option<BlockPos>,
}

impl Default for SilverfishMergeWithStoneGoal {
    fn default() -> Self {
        Self {
            stroll: WanderAroundGoal::with_interval(1.0, 10),
            merge_pos: None,
        }
    }
}

impl SilverfishMergeWithStoneGoal {
    /// The block in `direction` from the silverfish's middle.
    fn neighbour(mob: &dyn Mob, direction: BlockDirection) -> BlockPos {
        let pos = mob.get_entity().pos.load();
        BlockPos::floored(pos.x, pos.y + 0.5, pos.z).offset(direction.to_offset())
    }
}

impl Goal for SilverfishMergeWithStoneGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        if mob.get_mob_entity().get_target().is_some() || !mob.is_navigator_idle() {
            return false;
        }
        let world = mob.get_entity().world.load();
        if world.level_info.load().game_rules.mob_griefing
            && mob.get_random().random_range(0..to_goal_ticks(10)) == 0
        {
            let index = mob.get_random().random_range(0..6);
            let direction = BlockDirection::all()[index];
            let pos = Self::neighbour(mob, direction);
            if infested::is_compatible_host_block(world.get_block_state_id(&pos)) {
                self.merge_pos = Some(pos);
                return true;
            }
        }
        self.merge_pos = None;
        self.stroll.can_start(mob)
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        self.merge_pos.is_none() && self.stroll.should_continue(mob)
    }

    fn start(&mut self, mob: &dyn Mob) {
        let Some(pos) = self.merge_pos else {
            self.stroll.start(mob);
            return;
        };
        let entity = mob.get_entity();
        let world = entity.world.load_full();
        if let Some(infested) = infested::infested_state_by_host(world.get_block_state_id(&pos)) {
            world.set_block_state(&pos, infested, BlockFlags::NOTIFY_ALL);
            world.send_entity_status(
                entity,
                EntityStatus::SilverfishMergeAnim,
                ActorEventID::SilverfishMergeAnimation,
            );
            entity.remove();
        }
    }

    fn stop(&mut self, mob: &dyn Mob) {
        self.stroll.stop(mob);
    }

    fn tick(&mut self, mob: &dyn Mob) {
        self.stroll.tick(mob);
    }

    fn controls(&self) -> Controls {
        Controls::MOVE
    }
}
