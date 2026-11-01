use std::sync::Arc;

use pumpkin_data::tag::Taggable;
use pumpkin_data::{Block, BlockState, entity::EntityType};
use pumpkin_util::math::{
    boundingbox::{BoundingBox, EntityDimensions},
    position::BlockPos,
    vector3::Vector3,
};
use rand::RngExt;
use uuid::Uuid;

use crate::entity::{Entity, EntityBase, mob::Mob};
use crate::world::World;
use crate::world::level_accessor::LevelAccessor;

/// Vanilla `SpawnUtil.Strategy`: which floor a spawn position may rest on.
#[derive(Clone, Copy)]
pub enum SpawnStrategy {
    OnTopOfCollider,
    /// The village golem's floor: a solid block or powder snow under air or liquid, none of
    /// the blocks the legacy rule lists.
    LegacyIronGolem,
}

impl SpawnStrategy {
    fn can_spawn_on(self, state: &BlockState, above_state: &BlockState) -> bool {
        match self {
            Self::OnTopOfCollider => {
                above_state.get_block_collision_shapes().next().is_none() && is_face_full_up(state)
            }
            Self::LegacyIronGolem => {
                let block = Block::from_state_id(state.id);
                !is_legacy_iron_golem_excluded(block)
                    && (above_state.is_air() || above_state.is_liquid())
                    && (state.is_solid() || block.id == Block::POWDER_SNOW.id)
            }
        }
    }
}

/// The blocks vanilla `Strategy.LEGACY_IRON_GOLEM` refuses as the golem's floor: cobwebs,
/// cacti, every glass block and pane, leaves, conduits, ice, TNT and the light blocks.
fn is_legacy_iron_golem_excluded(block: &Block) -> bool {
    block.id == Block::COBWEB.id
        || block.id == Block::CACTUS.id
        || block.id == Block::GLASS_PANE.id
        || is_stained_glass_pane(block)
        || block.has_tag(&pumpkin_data::tag::Block::MINECRAFT_IMPERMEABLE)
        || block.has_tag(&pumpkin_data::tag::Block::MINECRAFT_LEAVES)
        || block.id == Block::CONDUIT.id
        || block.id == Block::ICE.id
        || block.id == Block::TNT.id
        || block.id == Block::GLOWSTONE.id
        || block.id == Block::BEACON.id
        || block.id == Block::SEA_LANTERN.id
        || block.id == Block::FROSTED_ICE.id
}

/// Vanilla `StainedGlassPaneBlock`.
fn is_stained_glass_pane(block: &Block) -> bool {
    matches!(
        block.id,
        id if id == Block::WHITE_STAINED_GLASS_PANE.id
            || id == Block::ORANGE_STAINED_GLASS_PANE.id
            || id == Block::MAGENTA_STAINED_GLASS_PANE.id
            || id == Block::LIGHT_BLUE_STAINED_GLASS_PANE.id
            || id == Block::YELLOW_STAINED_GLASS_PANE.id
            || id == Block::LIME_STAINED_GLASS_PANE.id
            || id == Block::PINK_STAINED_GLASS_PANE.id
            || id == Block::GRAY_STAINED_GLASS_PANE.id
            || id == Block::LIGHT_GRAY_STAINED_GLASS_PANE.id
            || id == Block::CYAN_STAINED_GLASS_PANE.id
            || id == Block::PURPLE_STAINED_GLASS_PANE.id
            || id == Block::BLUE_STAINED_GLASS_PANE.id
            || id == Block::BROWN_STAINED_GLASS_PANE.id
            || id == Block::GREEN_STAINED_GLASS_PANE.id
            || id == Block::RED_STAINED_GLASS_PANE.id
            || id == Block::BLACK_STAINED_GLASS_PANE.id
    )
}

/// Vanilla `Block.isFaceFull(getCollisionShape(...), Direction.UP)`, what `entityCanStandOn` checks.
pub(crate) fn is_face_full_up(state: &BlockState) -> bool {
    state.is_full_cube()
        || state.get_block_collision_shapes().any(|shape| {
            shape.max.y >= 1.0
                && shape.min.x <= 0.0
                && shape.min.z <= 0.0
                && shape.max.x >= 1.0
                && shape.max.z >= 1.0
        })
}

/// Vanilla `SpawnUtil.trySpawnMob`: a mob at a spot the strategy accepts within the range
/// around `start`, passing its own spawn rules and obstruction test.
#[expect(clippy::too_many_arguments)]
pub fn try_spawn_mob<T: Mob + 'static>(
    entity_type: &'static EntityType,
    create: fn(Entity) -> Arc<T>,
    world: &Arc<World>,
    start: &BlockPos,
    spawn_attempts: i32,
    spawn_range_xz: i32,
    spawn_range_y: i32,
    strategy: SpawnStrategy,
    check_collisions: bool,
) -> Option<Arc<T>> {
    for _ in 0..spawn_attempts {
        let (dx, dz) = {
            let mut random = rand::rng();
            (
                random.random_range(-spawn_range_xz..=spawn_range_xz),
                random.random_range(-spawn_range_xz..=spawn_range_xz),
            )
        };
        let search_pos = start.add(dx, spawn_range_y, dz);
        let in_border = world
            .worldborder
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains_block(search_pos.0.x, search_pos.0.z);
        if !in_border {
            continue;
        }
        let Some(spawn_pos) =
            move_to_possible_spawn_position(world, spawn_range_y, search_pos, strategy)
        else {
            continue;
        };

        let position = Vector3::new(
            f64::from(spawn_pos.0.x) + 0.5,
            f64::from(spawn_pos.0.y),
            f64::from(spawn_pos.0.z) + 0.5,
        );
        if check_collisions {
            let dimensions = EntityDimensions::new(
                entity_type.dimension[0],
                entity_type.dimension[1],
                entity_type.eye_height,
            );
            let bounding_box =
                BoundingBox::new_from_pos(position.x, position.y, position.z, &dimensions);
            if !world.no_collision(&bounding_box) {
                continue;
            }
        }

        let mob = create(Entity::from_uuid(
            Uuid::new_v4(),
            world.clone(),
            position,
            entity_type,
        ));
        let level: &World = world;
        if !mob
            .get_path_aware_entity()
            .is_none_or(|mob| mob.can_spawn(level))
            || !mob.check_spawn_obstruction(level)
        {
            continue;
        }
        world.spawn_entity(mob.clone() as Arc<dyn EntityBase>);
        mob.play_ambient_sound();
        return Some(mob);
    }

    None
}

fn move_to_possible_spawn_position(
    world: &World,
    spawn_range_y: i32,
    mut search_pos: BlockPos,
    strategy: SpawnStrategy,
) -> Option<BlockPos> {
    let mut above_state = world.get_block_state(&search_pos);

    for _ in -spawn_range_y..=spawn_range_y {
        search_pos = search_pos.down();
        let current_state = world.get_block_state(&search_pos);
        if strategy.can_spawn_on(current_state, above_state) {
            return Some(search_pos.up());
        }
        above_state = current_state;
    }

    None
}
