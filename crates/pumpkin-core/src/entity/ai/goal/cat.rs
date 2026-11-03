//! Vanilla `CatLieOnBlockGoal`, `CatSitOnBlockGoal` and `Cat.CatRelaxOnOwnerGoal`.

use std::sync::Arc;

use pumpkin_data::Block;
use pumpkin_data::HorizontalFacingExt;
use pumpkin_data::block_properties::{BedPart, FurnaceLikeProperties, WhiteBedLikeProperties};
use pumpkin_data::tag::{self, Taggable};
use pumpkin_util::math::position::BlockPos;

use super::move_to_target_pos::{MoveToTargetPos, MoveToTargetPosGoal};
use super::{Controls, Goal, ParentHandle, to_goal_ticks};
use crate::block::entities::chest::ChestBlockEntity;
use crate::entity::EntityBase;
use crate::entity::ai::pathfinder::NavigatorGoal;
use crate::entity::mob::Mob;
use crate::entity::passive::cat::CatEntity;
use crate::entity::passive::tamable::TamableAnimal;
use crate::entity::player::Player;
use crate::world::World;

const LIE_SPEED: f64 = 1.1;
const LIE_RANGE: i32 = 8;
const LIE_VERTICAL_RANGE: i32 = 6;
const LIE_VERTICAL_START: i32 = -2;
const LIE_NEXT_START_TICKS: i32 = 40;
const SIT_SPEED: f64 = 0.8;
const SIT_RANGE: i32 = 8;
const RELAX_SPEED: f64 = 1.1;
const RELAX_RANGE_SQ: f64 = 100.0;
const ON_BED_DISTANCE_SQ: f64 = 2.5;
const LOOK_TICKS_ON_BED: i32 = 16;

fn as_cat(mob: &dyn Mob) -> Option<&CatEntity> {
    mob.cast_any().downcast_ref::<CatEntity>()
}

fn is_free_above(world: &World, pos: BlockPos) -> bool {
    world.get_block_state(&pos.up()).is_air()
}

struct LieTarget;

impl MoveToTargetPos for LieTarget {
    fn is_target_pos(&self, world: Arc<World>, pos: BlockPos) -> bool {
        is_free_above(&world, pos)
            && world
                .get_block(&pos)
                .has_tag(&tag::Block::MINECRAFT_CATS_CAN_LIE_ON)
    }
}

/// Vanilla `CatLieOnBlockGoal`: a tame cat lies down on a bed or something as soft.
pub struct CatLieOnBlockGoal {
    inner: MoveToTargetPosGoal<LieTarget>,
    _target: Box<LieTarget>,
}

impl CatLieOnBlockGoal {
    #[must_use]
    pub fn new() -> Self {
        let target = Box::new(LieTarget);
        // SAFETY: the target has its own heap allocation owned by this goal, so it stays put
        // and outlives the handle, and it is only ever read through shared references.
        let handle: ParentHandle<LieTarget> = unsafe { ParentHandle::new(&target) };
        let mut inner = MoveToTargetPosGoal::new(handle, LIE_SPEED, LIE_RANGE, LIE_VERTICAL_RANGE);
        inner.lowest_y = LIE_VERTICAL_START;
        Self {
            inner,
            _target: target,
        }
    }
}

impl Default for CatLieOnBlockGoal {
    fn default() -> Self {
        Self::new()
    }
}

impl Goal for CatLieOnBlockGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        let Some(cat) = as_cat(mob) else {
            return false;
        };
        if !cat.is_tame() || cat.is_ordered_to_sit() || cat.is_lying() {
            return false;
        }
        let was_waiting = self.inner.cooldown > 0;
        let found = self.inner.can_start(mob);
        // Vanilla `nextStartTick`: 40 ticks instead of the default wait.
        if !was_waiting {
            self.inner.cooldown = to_goal_ticks(LIE_NEXT_START_TICKS);
        }
        found
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        self.inner.should_continue(mob)
    }

    fn start(&mut self, mob: &dyn Mob) {
        self.inner.start(mob);
        if let Some(cat) = as_cat(mob) {
            cat.set_in_sitting_pose(false);
        }
    }

    fn stop(&mut self, mob: &dyn Mob) {
        self.inner.stop(mob);
        if let Some(cat) = as_cat(mob) {
            cat.set_lying(false);
        }
    }

    fn tick(&mut self, mob: &dyn Mob) {
        self.inner.tick(mob);
        let Some(cat) = as_cat(mob) else {
            return;
        };
        cat.set_in_sitting_pose(false);
        let reached = self.inner.is_reached_target();
        if cat.is_lying() != reached {
            cat.set_lying(reached);
        }
    }

    fn should_run_every_tick(&self) -> bool {
        true
    }

    fn controls(&self) -> Controls {
        Controls::JUMP | Controls::MOVE
    }
}

struct SitTarget;

impl MoveToTargetPos for SitTarget {
    fn is_target_pos(&self, world: Arc<World>, pos: BlockPos) -> bool {
        if !is_free_above(&world, pos) {
            return false;
        }
        let (block, state) = world.get_block_and_state_id(&pos);
        if !block.has_tag(&tag::Block::MINECRAFT_CATS_CAN_SIT_ON) {
            return false;
        }
        if block == &Block::CHEST {
            return world.get_block_entity(&pos).is_none_or(|entity| {
                entity
                    .as_any()
                    .downcast_ref::<ChestBlockEntity>()
                    .is_none_or(|chest| chest.viewer_count() < 1)
            });
        }
        if block == &Block::FURNACE {
            return FurnaceLikeProperties::from_state_id(state).lit;
        }
        if block.has_tag(&tag::Block::MINECRAFT_BEDS) {
            return WhiteBedLikeProperties::from_state_id(state).part != BedPart::Head;
        }
        true
    }
}

/// Vanilla `CatSitOnBlockGoal`: a tame cat sits on a chest, a lit furnace or the foot of a bed.
pub struct CatSitOnBlockGoal {
    inner: MoveToTargetPosGoal<SitTarget>,
    _target: Box<SitTarget>,
}

impl CatSitOnBlockGoal {
    #[must_use]
    pub fn new() -> Self {
        let target = Box::new(SitTarget);
        // SAFETY: the target has its own heap allocation owned by this goal, so it stays put
        // and outlives the handle, and it is only ever read through shared references.
        let handle: ParentHandle<SitTarget> = unsafe { ParentHandle::new(&target) };
        Self {
            inner: MoveToTargetPosGoal::with_default(handle, SIT_SPEED, SIT_RANGE),
            _target: target,
        }
    }
}

impl Default for CatSitOnBlockGoal {
    fn default() -> Self {
        Self::new()
    }
}

impl Goal for CatSitOnBlockGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        as_cat(mob).is_some_and(|cat| cat.is_tame() && !cat.is_ordered_to_sit())
            && self.inner.can_start(mob)
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        self.inner.should_continue(mob)
    }

    fn start(&mut self, mob: &dyn Mob) {
        self.inner.start(mob);
        if let Some(cat) = as_cat(mob) {
            cat.set_in_sitting_pose(false);
        }
    }

    fn stop(&mut self, mob: &dyn Mob) {
        self.inner.stop(mob);
        if let Some(cat) = as_cat(mob) {
            cat.set_in_sitting_pose(false);
        }
    }

    fn tick(&mut self, mob: &dyn Mob) {
        self.inner.tick(mob);
        if let Some(cat) = as_cat(mob) {
            cat.set_in_sitting_pose(self.inner.is_reached_target());
        }
    }

    fn should_run_every_tick(&self) -> bool {
        true
    }

    fn controls(&self) -> Controls {
        Controls::JUMP | Controls::MOVE
    }
}

/// Vanilla `Cat.CatRelaxOnOwnerGoal`: it curls up on the bed of its sleeping owner.
#[derive(Default)]
pub struct CatRelaxOnOwnerGoal {
    owner: Option<Arc<Player>>,
    goal_pos: Option<BlockPos>,
    on_bed_ticks: i32,
}

impl CatRelaxOnOwnerGoal {
    fn space_is_occupied(cat: &CatEntity, pos: BlockPos) -> bool {
        let world = cat.get_entity().world.load();
        let center = pos.to_centered_f64();
        world.entities.load().iter().any(|entity| {
            let other = entity.get_entity();
            other.entity_id != cat.get_entity().entity_id
                && entity
                    .cast_any()
                    .downcast_ref::<CatEntity>()
                    .is_some_and(|other_cat| other_cat.is_lying() || other_cat.is_relax_state_one())
                && other.pos.load().squared_distance_to_vec(&center) < 4.0
        })
    }

    fn walk_to_goal(cat: &CatEntity, pos: BlockPos) {
        let steered = cat.mob_entity.steered();
        let mut navigator = steered.navigator();
        navigator.set_progress(NavigatorGoal {
            current_progress: cat.get_entity().pos.load(),
            destination: pos.to_centered_f64(),
            speed: RELAX_SPEED,
        });
    }

    /// Vanilla `giveMorningGift`: something from the night is left next to where it lay.
    fn give_morning_gift(cat: &CatEntity) {
        let entity = cat.get_entity();
        let world = entity.world.load_full();
        let pos = entity.pos.load();
        for stack in crate::entity::passive::cat::morning_gift(cat) {
            let yaw = entity.body_yaw.load().to_radians();
            let at = pumpkin_util::math::vector3::Vector3::new(
                pos.x.floor() - f64::from(yaw.sin()),
                pos.y.floor(),
                pos.z.floor() + f64::from(yaw.cos()),
            );
            world.drop_stack_at(at, stack);
        }
    }
}

impl Goal for CatRelaxOnOwnerGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        let Some(cat) = as_cat(mob) else {
            return false;
        };
        if !cat.is_tame() || cat.is_ordered_to_sit() {
            return false;
        }
        let world = cat.get_entity().world.load();
        let Some(owner) = cat
            .get_owner()
            .and_then(|uuid| world.get_player_by_uuid(uuid))
        else {
            return false;
        };
        if !owner.is_sleeping()
            || cat
                .get_entity()
                .pos
                .load()
                .squared_distance_to_vec(&owner.get_entity().pos.load())
                > RELAX_RANGE_SQ
        {
            return false;
        }
        let owner_pos = owner.get_entity().block_pos.load();
        let (block, state) = world.get_block_and_state_id(&owner_pos);
        if !block.has_tag(&tag::Block::MINECRAFT_CATS_CAN_LIE_ON) {
            return false;
        }
        let goal = if block.has_tag(&tag::Block::MINECRAFT_BEDS) {
            let facing = WhiteBedLikeProperties::from_state_id(state).facing;
            owner_pos.offset(facing.opposite().to_block_direction().to_offset())
        } else {
            owner_pos
        };
        self.owner = Some(owner);
        self.goal_pos = Some(goal);
        !Self::space_is_occupied(cat, goal)
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        as_cat(mob).is_some_and(|cat| {
            cat.is_tame()
                && !cat.is_ordered_to_sit()
                && self.owner.as_ref().is_some_and(|owner| owner.is_sleeping())
                && self
                    .goal_pos
                    .is_some_and(|goal| !Self::space_is_occupied(cat, goal))
        })
    }

    fn start(&mut self, mob: &dyn Mob) {
        if let (Some(cat), Some(goal)) = (as_cat(mob), self.goal_pos) {
            cat.set_in_sitting_pose(false);
            Self::walk_to_goal(cat, goal);
        }
    }

    fn stop(&mut self, mob: &dyn Mob) {
        let Some(cat) = as_cat(mob) else {
            return;
        };
        cat.set_lying(false);
        // Vanilla's chance of the `cat_waking_up_gift_chance` attribute, 0.7 in the overworld.
        if self.on_bed_ticks >= 100 && rand::random::<f32>() < 0.7 {
            Self::give_morning_gift(cat);
        }
        self.on_bed_ticks = 0;
        cat.set_relax_state_one(false);
        cat.mob_entity.steered().navigator().stop();
    }

    fn tick(&mut self, mob: &dyn Mob) {
        let (Some(cat), Some(owner), Some(goal)) = (as_cat(mob), self.owner.clone(), self.goal_pos)
        else {
            return;
        };
        cat.set_in_sitting_pose(false);
        Self::walk_to_goal(cat, goal);
        let owner_pos = owner.get_entity().pos.load();
        if cat
            .get_entity()
            .pos
            .load()
            .squared_distance_to_vec(&owner_pos)
            < ON_BED_DISTANCE_SQ
        {
            self.on_bed_ticks += 1;
            if self.on_bed_ticks > to_goal_ticks(LOOK_TICKS_ON_BED) {
                cat.set_lying(true);
                cat.set_relax_state_one(false);
            } else {
                cat.mob_entity
                    .look_control
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .look_at_entity_with_range(&(owner as Arc<dyn EntityBase>), 45.0, 45.0);
                cat.set_relax_state_one(true);
            }
        } else {
            cat.set_lying(false);
        }
    }

    fn should_run_every_tick(&self) -> bool {
        true
    }

    fn controls(&self) -> Controls {
        Controls::empty()
    }
}
