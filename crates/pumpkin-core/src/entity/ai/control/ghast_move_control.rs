use pumpkin_data::attributes::Attributes;
use pumpkin_data::tag::{self, Taggable};
use pumpkin_util::math::boundingbox::BoundingBox;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use rand::RngExt;
use rustc_hash::FxHashSet;

use super::{
    Control, MoveControlTrait,
    move_control::{MoveControl, Operation},
};
use crate::entity::mob::Mob;
use crate::world::World;

/// Vanilla `FLYING_SPEED * 5.0 / 3.0`: the push of one float step per flying speed.
const FLOAT_PUSH_SCALE: f64 = 5.0 / 3.0;
/// Vanilla `AABB.collidedAlongVector` shrinks the moving box by this, so touching is no hit.
const COLLISION_MARGIN: f64 = 1.0E-7;

/// Vanilla `Ghast.GhastMoveControl`, flying in pushes towards the wanted position.
///
/// The ghast's flavour: not careful and never stopped. Every few ticks it pushes the mob
/// towards the wanted position and gives up once blocks are in the way.
#[derive(Default)]
pub struct GhastMoveControl {
    inner: MoveControl,
    float_duration: i32,
    /// Vanilla `careful`: keeps a block away from everything and out of what it avoids.
    careful: bool,
    /// Vanilla `shouldBeStopped`.
    should_be_stopped: Option<fn(&dyn Mob) -> bool>,
}

impl GhastMoveControl {
    /// The happy ghast's flavour.
    #[must_use]
    pub fn careful(should_be_stopped: fn(&dyn Mob) -> bool) -> Self {
        Self {
            careful: true,
            should_be_stopped: Some(should_be_stopped),
            ..Self::default()
        }
    }
}

/// Vanilla `Mob.stopInPlace` for a mob without a path.
fn stop_in_place(mob: &dyn Mob) {
    let living = &mob.get_mob_entity().living_entity;
    living.movement_input.store(Vector3::default());
    living.entity.velocity.store(Vector3::default());
    living.fall_distance.store(0.0);
}

impl Control for GhastMoveControl {}

impl MoveControlTrait for GhastMoveControl {
    fn tick(&mut self, mob: &dyn Mob) {
        if self.should_be_stopped.is_some_and(|stopped| stopped(mob)) {
            self.inner.operation = Operation::Wait;
            stop_in_place(mob);
        }
        if self.inner.operation != Operation::MoveTo {
            return;
        }
        let duration = self.float_duration;
        self.float_duration -= 1;
        if duration > 0 {
            return;
        }
        self.float_duration += mob.get_random().random_range(0..5) + 2;

        let living = &mob.get_mob_entity().living_entity;
        let entity = &living.entity;
        let pos = entity.pos.load();
        let travel = Vector3::new(
            self.inner.wanted_x - pos.x,
            self.inner.wanted_y - pos.y,
            self.inner.wanted_z - pos.z,
        );
        if can_reach(mob, travel, self.careful) {
            let push = living.get_attribute_value(&Attributes::FLYING_SPEED) * FLOAT_PUSH_SCALE;
            entity
                .velocity
                .store(entity.velocity.load() + travel.normalize() * push);
        } else {
            self.inner.operation = Operation::Wait;
        }
    }

    fn set_wanted_position(&mut self, x: f64, y: f64, z: f64, speed_modifier: f64) {
        self.inner.set_wanted_position(x, y, z, speed_modifier);
    }

    fn strafe(&mut self, forwards: f32, right: f32) {
        self.inner.strafe(forwards, right);
    }

    fn set_wait(&mut self) {
        self.inner.operation = Operation::Wait;
    }

    fn has_wanted(&self) -> bool {
        self.inner.has_wanted()
    }

    fn get_speed_modifier(&self) -> f64 {
        self.inner.get_speed_modifier()
    }

    fn get_wanted_position(&self) -> Vector3<f64> {
        Vector3::new(
            self.inner.wanted_x,
            self.inner.wanted_y,
            self.inner.wanted_z,
        )
    }
}

/// Vanilla `GhastMoveControl.canReach`.
///
/// The mob's box can move along `travel` without running into a block shape it does not
/// already overlap.
/// Vanilla visits the blocks along the way with `forEachBlockIntersectedBetween`; this walks
/// the way in steps of at most one block, which covers the same blocks.
fn can_reach(mob: &dyn Mob, travel: Vector3<f64>, careful: bool) -> bool {
    let entity = mob.get_entity();
    let world = entity.world.load();
    let aabb = entity.bounding_box.load();
    if careful && !destination_is_clear(&world, aabb.shift(travel)) {
        return false;
    }
    let (in_water, in_lava) = (
        entity.is_in_water(),
        entity
            .touching_lava
            .load(std::sync::atomic::Ordering::SeqCst),
    );
    let from = Vector3::new(
        f64::midpoint(aabb.min.x, aabb.max.x),
        f64::midpoint(aabb.min.y, aabb.max.y),
        f64::midpoint(aabb.min.z, aabb.max.z),
    );
    let half_size = Vector3::new(
        (aabb.max.x - aabb.min.x) * 0.5 - COLLISION_MARGIN,
        (aabb.max.y - aabb.min.y) * 0.5 - COLLISION_MARGIN,
        (aabb.max.z - aabb.min.z) * 0.5 - COLLISION_MARGIN,
    );
    let steps = travel.length().ceil().max(1.0) as i32;
    let step = travel * (1.0 / f64::from(steps));
    let mut visited = FxHashSet::default();
    let mut segment = aabb;
    for _ in 0..steps {
        let swept = segment.stretch(step);
        for pos in BlockPos::iterate(swept.min_block_pos(), swept.max_block_pos()) {
            if !visited.insert(pos) || aabb.intersects(&BoundingBox::from_block(&pos)) {
                continue;
            }
            if careful && avoids_on_the_way(&world, pos, in_water, in_lava) {
                return false;
            }
            if blocks_travel(&world, pos, from, travel, half_size) {
                return false;
            }
        }
        segment = segment.shift(step);
    }
    true
}

/// The careful loop of vanilla `canReach`: nothing in the way a block around the destination.
fn destination_is_clear(world: &World, destination: BoundingBox) -> bool {
    let around = destination.expand(1.0, 1.0, 1.0);
    BlockPos::iterate(around.min_block_pos(), around.max_block_pos()).all(|pos| {
        let state = world.get_block_state(&pos);
        state.is_air()
            || (state.get_block_collision_shapes().next().is_none()
                && !avoids_on_the_way(world, pos, false, false))
    })
}

/// The careful part of vanilla `blockTraversalPossible`: blocks it stays away from, and fluids it
/// is not in already.
fn avoids_on_the_way(world: &World, pos: BlockPos, in_water: bool, in_lava: bool) -> bool {
    if world
        .get_block(&pos)
        .has_tag(&tag::Block::MINECRAFT_HAPPY_GHAST_AVOIDS)
    {
        return true;
    }
    let fluid = world.get_fluid(&pos);
    (fluid.has_tag(&tag::Fluid::MINECRAFT_WATER) && !in_water)
        || (fluid.has_tag(&tag::Fluid::MINECRAFT_LAVA) && !in_lava)
}

/// Vanilla `blockTraversalPossible` negated, for a mob that is not careful.
///
/// A collision shape of the block at `pos` is in the way of the box centred on `from` moving
/// along `travel`.
fn blocks_travel(
    world: &World,
    pos: BlockPos,
    from: Vector3<f64>,
    travel: Vector3<f64>,
    half_size: Vector3<f64>,
) -> bool {
    let state = world.get_block_state(&pos);
    if state.is_air() {
        return false;
    }
    let (x, y, z) = (half_size.x, half_size.y, half_size.z);
    for shape in state.get_block_collision_shapes_at(&pos) {
        let inflated = shape.at_pos(pos).expand(x, y, z);
        if inflated.ray_intersection(from, travel).is_some() {
            return true;
        }
    }
    false
}
