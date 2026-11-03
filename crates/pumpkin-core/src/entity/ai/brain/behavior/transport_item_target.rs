use std::sync::Arc;

use pumpkin_data::{Block, BlockDirection, BlockState};
use pumpkin_inventory::Inventory;
use pumpkin_util::math::boundingbox::BoundingBox;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector2::Vector2;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_world::clip::{ClipBlock, ClipContext, ClipFluid};

use super::super::BrainTick;
use super::super::memory::types;
use super::poi;
use crate::block::blocks::chest_container;
use crate::block::entities::BlockEntity;
use crate::entity::ai::pathfinder::path::Path;
use crate::entity::mob::Mob;
use crate::world::World;

/// Vanilla `CLOSE_ENOUGH_TO_START_INTERACTING_WITH_TARGET_DISTANCE`.
const CLOSE_ENOUGH_TO_START_INTERACTING_WITH_TARGET_DISTANCE: f64 = 0.5;
/// Vanilla `CLOSE_ENOUGH_TO_START_INTERACTING_WITH_TARGET_PATH_END_DISTANCE`.
const CLOSE_ENOUGH_TO_START_INTERACTING_WITH_TARGET_PATH_END_DISTANCE: f64 = 1.0;
/// The vertical inflation in vanilla `isWithinTargetDistance`.
const TARGET_VERTICAL_REACH: f64 = 0.5;

/// Vanilla `TransportItemsBetweenContainers.TransportItemTarget`.
///
/// Only chests are ever picked, so the container comes from `chest_container::chest_container` and
/// vanilla's branch for other containers has no counterpart.
pub struct TransportItemTarget {
    pub pos: BlockPos,
    pub container: Arc<dyn Inventory>,
    pub block_entity: Arc<dyn BlockEntity>,
    pub block: &'static Block,
    pub state: &'static BlockState,
}

impl TransportItemTarget {
    /// Vanilla `tryCreatePossibleTarget`: `None` for a blocked chest.
    pub(super) fn try_create_possible_target(
        block_entity: Arc<dyn BlockEntity>,
        world: &World,
    ) -> Option<Self> {
        let pos = block_entity.get_position();
        let (block, state) = world.get_block_and_state(&pos);
        let container = chest_container::chest_container(world, &pos)?;
        Some(Self {
            pos,
            container,
            block_entity,
            block,
            state,
        })
    }

    /// Vanilla `VoxelShape.bounds` of the collision shape, local to the block.
    fn collision_bounds(&self) -> Option<BoundingBox> {
        self.state.get_block_collision_shapes().reduce(|a, b| {
            BoundingBox::new(
                Vector3::new(
                    a.min.x.min(b.min.x),
                    a.min.y.min(b.min.y),
                    a.min.z.min(b.min.z),
                ),
                Vector3::new(
                    a.max.x.max(b.max.x),
                    a.max.y.max(b.max.y),
                    a.max.z.max(b.max.z),
                ),
            )
        })
    }

    /// The other half of a double chest, for vanilla `getConnectedTargets`.
    pub(super) fn connected_target(&self, world: &World) -> Option<Self> {
        let pos = chest_container::connected_chest_pos(&self.pos, self.state.id)?;
        Self::try_create_possible_target(world.get_block_entity(&pos)?, world)
    }
}

/// The chests in the loaded chunks around `center`: vanilla's `getChunkNow` loop, copied out so
/// no map guard is held while the candidates read the world.
pub(super) fn chests_around(
    world: &World,
    center: Vector2<i32>,
    radius: i32,
) -> Vec<Arc<dyn BlockEntity>> {
    let mut found = Vec::new();
    for x in center.x - radius..=center.x + radius {
        for z in center.y - radius..=center.y + radius {
            if let Some(chunk) = world.block_entities.get(&Vector2::new(x, z)) {
                found.extend(
                    chunk
                        .values()
                        .filter(|entity| {
                            chest_container::chest_opener_count(entity.as_ref()).is_some()
                        })
                        .cloned(),
                );
            }
        }
    }
    found
}

/// Vanilla `AABB.contains` with the corner of the chest's block.
pub(super) fn is_in_search_area(area: &BoundingBox, pos: BlockPos) -> bool {
    let (x, y, z) = (f64::from(pos.0.x), f64::from(pos.0.y), f64::from(pos.0.z));
    x >= area.min.x
        && x < area.max.x
        && y >= area.min.y
        && y < area.max.y
        && z >= area.min.z
        && z < area.max.z
}

/// Vanilla `isPositionAlreadyVisited`.
pub(super) fn is_position_already_visited(
    tick: &BrainTick<'_>,
    target: &TransportItemTarget,
) -> bool {
    let visited = tick.state.get(types::VISITED_BLOCK_POSITIONS);
    let unreachable = tick.state.get(types::UNREACHABLE_TRANSPORT_BLOCK_POSITIONS);
    let seen = |pos: BlockPos| {
        let pos = poi::global_pos(tick.world, pos);
        visited.is_some_and(|positions| positions.contains(&pos))
            || unreachable.is_some_and(|positions| positions.contains(&pos))
    };
    seen(target.pos)
        || target
            .connected_target(tick.world)
            .is_some_and(|connected| seen(connected.pos))
}

/// Vanilla `targetHasNotChanged`.
pub(super) fn target_has_not_changed(world: &World, target: &TransportItemTarget) -> bool {
    world
        .get_block_entity(&target.pos)
        .is_some_and(|block_entity| Arc::ptr_eq(&block_entity, &target.block_entity))
}

/// Vanilla `hasValidTravellingPath`.
pub(super) fn has_valid_travelling_path(
    target: &TransportItemTarget,
    tick: &BrainTick<'_>,
) -> bool {
    let steered = tick.mob.get_mob_entity().steered();
    let path = {
        let owner = &steered.mob_entity().living_entity;
        let mut navigator = steered.navigator();
        let current = navigator.get_path().cloned();
        current.or_else(|| navigator.create_path(owner, target.pos.0.to_f64(), 0))
    };
    let from = get_position_to_reach_target_from(path.as_ref(), tick.mob);
    let can_reach_target =
        is_within_target_distance(get_interaction_range(tick.mob), target, tick.mob, from);
    let has_not_yet_created_path_to_target = path.is_none() && !can_reach_target;
    has_not_yet_created_path_to_target
        || (can_reach_target && can_see_any_target_side(target, tick.world, from))
}

/// Vanilla `getPositionToReachTargetFrom`.
fn get_position_to_reach_target_from(path: Option<&Path>, mob: &dyn Mob) -> Vector3<f64> {
    let bottom_center = path.and_then(Path::get_end_node).map_or_else(
        || mob.get_entity().pos.load(),
        |node| {
            Vector3::new(
                f64::from(node.pos.0.x) + 0.5,
                f64::from(node.pos.0.y),
                f64::from(node.pos.0.z) + 0.5,
            )
        },
    );
    set_middle_y_position(mob, bottom_center)
}

/// Vanilla `setMiddleYPosition`.
fn set_middle_y_position(mob: &dyn Mob, pos: Vector3<f64>) -> Vector3<f64> {
    let bounding_box = mob.get_entity().bounding_box.load();
    pos.add_raw(0.0, (bounding_box.max.y - bounding_box.min.y) / 2.0, 0.0)
}

/// Vanilla `getCenterPos`.
pub(super) fn center_pos(mob: &dyn Mob) -> Vector3<f64> {
    set_middle_y_position(mob, mob.get_entity().pos.load())
}

/// Vanilla `getInteractionRange` with `hasFinishedPath`.
pub(super) fn get_interaction_range(mob: &dyn Mob) -> f64 {
    let has_finished_path = mob
        .get_mob_entity()
        .steered()
        .navigator()
        .get_path()
        .is_some_and(Path::is_done);
    if has_finished_path {
        CLOSE_ENOUGH_TO_START_INTERACTING_WITH_TARGET_PATH_END_DISTANCE
    } else {
        CLOSE_ENOUGH_TO_START_INTERACTING_WITH_TARGET_DISTANCE
    }
}

/// Vanilla `isWithinTargetDistance`.
pub(super) fn is_within_target_distance(
    distance: f64,
    target: &TransportItemTarget,
    mob: &dyn Mob,
    from: Vector3<f64>,
) -> bool {
    let bounding_box = mob.get_entity().bounding_box.load();
    let moved_bounding_box = BoundingBox::of_size(
        from,
        bounding_box.max.x - bounding_box.min.x,
        bounding_box.max.y - bounding_box.min.y,
        bounding_box.max.z - bounding_box.min.z,
    );
    target.collision_bounds().is_some_and(|bounds| {
        bounds
            .expand(distance, TARGET_VERTICAL_REACH, distance)
            .at_pos(target.pos)
            .intersects(&moved_bounding_box)
    })
}

/// Vanilla `canSeeAnyTargetSide`: a collider ray from `from` to the middle of each face.
fn can_see_any_target_side(
    target: &TransportItemTarget,
    world: &World,
    from: Vector3<f64>,
) -> bool {
    let center = target.pos.to_centered_f64();
    BlockDirection::all().into_iter().any(|direction| {
        let step = direction.to_offset();
        let side = center.add_raw(
            0.5 * f64::from(step.x),
            0.5 * f64::from(step.y),
            0.5 * f64::from(step.z),
        );
        let hit = world.clip(&ClipContext::new(from, side, ClipBlock::Collider, ClipFluid::None));
        !hit.is_miss && hit.block_pos == target.pos
    })
}
