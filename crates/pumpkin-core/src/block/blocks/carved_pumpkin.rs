use std::sync::Arc;

use pumpkin_data::{
    Block, BlockDirection, BlockId, BlockStateId,
    block_properties::WallTorchLikeProperties,
    entity::EntityType,
    tag::{self, Taggable},
    world::WorldEvent,
};
use pumpkin_util::math::position::BlockPos;
use pumpkin_world::world::BlockFlags;

use crate::{
    block::{
        BlockBehaviour, BlockMetadata, OnPlaceArgs, PlacedArgs, PlayerPlacedArgs,
        blocks::{chests::CopperChestBlock, weathering_copper::WeatherState},
    },
    entity::{
        Entity, EntityBase,
        passive::{
            copper_golem::CopperGolemEntity, iron_golem::IronGolemEntity,
            snow_golem::SnowGolemEntity,
        },
    },
    world::World,
};

/// Vanilla `CopperChestBlock.COPPER_TO_COPPER_CHEST_MAPPING` with the state of vanilla
/// `CarvedPumpkinBlock.getWeatherStateFromPattern`, for the `#minecraft:copper` blocks.
const COPPER_GOLEM_BASES: [(&Block, &Block, WeatherState); 8] = [
    (
        &Block::COPPER_BLOCK,
        &Block::COPPER_CHEST,
        WeatherState::Unaffected,
    ),
    (
        &Block::EXPOSED_COPPER,
        &Block::EXPOSED_COPPER_CHEST,
        WeatherState::Exposed,
    ),
    (
        &Block::WEATHERED_COPPER,
        &Block::WEATHERED_COPPER_CHEST,
        WeatherState::Weathered,
    ),
    (
        &Block::OXIDIZED_COPPER,
        &Block::OXIDIZED_COPPER_CHEST,
        WeatherState::Oxidized,
    ),
    (
        &Block::WAXED_COPPER_BLOCK,
        &Block::WAXED_COPPER_CHEST,
        WeatherState::Unaffected,
    ),
    (
        &Block::WAXED_EXPOSED_COPPER,
        &Block::WAXED_EXPOSED_COPPER_CHEST,
        WeatherState::Exposed,
    ),
    (
        &Block::WAXED_WEATHERED_COPPER,
        &Block::WAXED_WEATHERED_COPPER_CHEST,
        WeatherState::Weathered,
    ),
    (
        &Block::WAXED_OXIDIZED_COPPER,
        &Block::WAXED_OXIDIZED_COPPER_CHEST,
        WeatherState::Oxidized,
    ),
];

pub struct GolemPattern {
    entity_type: &'static EntityType,
    blocks: Vec<BlockPos>,
    base: BlockPos,
}

// Mojang uses some BlockPattern magic, way too complex tbh
#[must_use]
pub fn find_golem_pattern(world: &Arc<World>, pos: &BlockPos) -> Option<GolemPattern> {
    let down_pos = pos.down();
    let upper = world.get_block(&down_pos);
    let lower = world.get_block(&down_pos.down());

    if upper == &Block::SNOW_BLOCK && lower == &Block::SNOW_BLOCK {
        return Some(GolemPattern {
            entity_type: &EntityType::SNOW_GOLEM,
            blocks: vec![*pos, down_pos, down_pos.down()],
            base: down_pos.down(),
        });
    }

    // Vanilla's pattern also matches a pumpkin beside the copper; Pumpkin's patterns look down.
    if upper.has_tag(&tag::Block::MINECRAFT_COPPER) {
        return Some(GolemPattern {
            entity_type: &EntityType::COPPER_GOLEM,
            blocks: vec![*pos, down_pos],
            base: *pos,
        });
    }

    if upper != &Block::IRON_BLOCK || lower != &Block::IRON_BLOCK {
        return None;
    }

    for dir in [BlockDirection::North, BlockDirection::West] {
        let arm1 = down_pos.offset(dir.to_offset());
        let arm2 = down_pos.offset(dir.opposite().to_offset());
        let above_arm1 = arm1.up();
        let above_arm2 = arm2.up();
        let below_arm1 = arm1.down();
        let below_arm2 = arm2.down();

        if world.get_block(&arm1) == &Block::IRON_BLOCK
            && world.get_block(&arm2) == &Block::IRON_BLOCK
            && world.get_block_state(&above_arm1).is_air()
            && world.get_block_state(&above_arm2).is_air()
            && world.get_block_state(&below_arm1).is_air()
            && world.get_block_state(&below_arm2).is_air()
        {
            return Some(GolemPattern {
                entity_type: &EntityType::IRON_GOLEM,
                blocks: vec![*pos, down_pos, down_pos.down(), arm1, arm2],
                base: down_pos.down(),
            });
        }
    }

    None
}

fn spawn_golem(world: &Arc<World>, pattern: &GolemPattern) {
    let copper_block_pos = pattern.base.down();
    let copper_block = world.get_block(&copper_block_pos);
    let pumpkin_state = world.get_block_state_id(&pattern.base);

    for &pos in &pattern.blocks {
        let state_id = world.get_block_state_id(&pos);
        world.set_block_state(
            &pos,
            Block::AIR.default_state.id,
            BlockFlags::NOTIFY_LISTENERS,
        );
        world.sync_world_event(
            WorldEvent::ParticlesDestroyBlock,
            pos,
            i32::from(state_id.as_u16()),
        );
    }

    let mut spawn_pos = pattern.base.to_centered_f64();
    spawn_pos.y = f64::from(pattern.base.0.y) + 0.05;
    let entity = Entity::new(world.clone(), spawn_pos, pattern.entity_type);
    if pattern.entity_type == &EntityType::COPPER_GOLEM {
        let golem = CopperGolemEntity::new(entity);
        world.spawn_entity(golem.clone());
        update_pattern_blocks(world, pattern);
        replace_copper_block_with_chest(
            world,
            &golem,
            copper_block,
            copper_block_pos,
            pumpkin_state,
        );
        return;
    }
    let golem: Arc<dyn EntityBase> = if pattern.entity_type == &EntityType::SNOW_GOLEM {
        SnowGolemEntity::new(entity)
    } else {
        IronGolemEntity::new(entity)
    };
    world.spawn_entity(golem);
    update_pattern_blocks(world, pattern);
}

fn update_pattern_blocks(world: &Arc<World>, pattern: &GolemPattern) {
    for &pos in &pattern.blocks {
        world.update_neighbors(&pos, None);
    }
}

/// Vanilla `CarvedPumpkinBlock.replaceCopperBlockWithChest` and `CopperGolem.spawn`.
fn replace_copper_block_with_chest(
    world: &Arc<World>,
    golem: &CopperGolemEntity,
    copper_block: &Block,
    copper_block_pos: BlockPos,
    pumpkin_state: BlockStateId,
) {
    let (chest, weather_state) = COPPER_GOLEM_BASES
        .iter()
        .find(|(copper, _, _)| *copper == copper_block)
        .map_or(
            (&Block::COPPER_CHEST, WeatherState::Unaffected),
            |(_, chest, weather_state)| (*chest, *weather_state),
        );
    let facing = WallTorchLikeProperties::from_state_id(pumpkin_state).facing;
    world.set_block_state(
        &copper_block_pos,
        CopperChestBlock::get_from_copper_block(world, chest, facing, &copper_block_pos),
        BlockFlags::NOTIFY_LISTENERS,
    );
    golem.spawn(weather_state);
}

pub struct CarvedPumpkinBlock;

impl BlockMetadata for CarvedPumpkinBlock {
    fn ids() -> Box<[BlockId]> {
        [BlockId::JACK_O_LANTERN, BlockId::CARVED_PUMPKIN].into()
    }
}

impl BlockBehaviour for CarvedPumpkinBlock {
    fn on_place(&self, args: OnPlaceArgs<'_>) -> BlockStateId {
        let mut props = WallTorchLikeProperties::default(args.block);
        props.facing = args
            .player
            .living_entity
            .entity
            .get_horizontal_facing()
            .opposite();
        props.to_state_id(args.block)
    }

    fn placed(&self, args: PlacedArgs<'_>) {
        if Block::from_state_id(args.old_state_id) == args.block {
            return;
        }
        if let Some(pattern) = find_golem_pattern(args.world, args.position) {
            spawn_golem(args.world, &pattern);
        }
    }

    fn player_placed(&self, args: PlayerPlacedArgs<'_>) {
        if let Some(pattern) = find_golem_pattern(args.world, args.position)
            && pattern.entity_type == &EntityType::IRON_GOLEM
        {
            args.player.trigger_advancement(
                crate::entity::player::advancement::trigger::AdvancementTrigger::SummonedEntity {
                    entity_type: "minecraft:iron_golem".to_string(),
                },
            );
        }
    }
}
