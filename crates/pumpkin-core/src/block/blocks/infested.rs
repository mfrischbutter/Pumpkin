use std::sync::Arc;

use pumpkin_data::{
    Block, BlockId, BlockState, BlockStateId, Enchantment, Rotation,
    block_properties::{Axis, PaleOakWoodLikeProperties},
    entity::{EntityStatus, EntityType},
};
use pumpkin_util::{GameMode, math::vector3::Vector3};

use crate::{
    block::{BlockBehaviour, BlockMetadata, BrokenArgs, OnPlaceArgs},
    entity::r#type::from_type,
    world::World,
};
use pumpkin_protocol::bedrock::server::actor_event::ActorEventID;
use pumpkin_util::math::position::BlockPos;

pub struct InfestedBlock;

/// Vanilla `InfestedBlock` hosts: each block a silverfish can hide in, with its infested twin.
const HOSTS: [(&Block, &Block); 7] = [
    (&Block::STONE, &Block::INFESTED_STONE),
    (&Block::COBBLESTONE, &Block::INFESTED_COBBLESTONE),
    (&Block::STONE_BRICKS, &Block::INFESTED_STONE_BRICKS),
    (
        &Block::MOSSY_STONE_BRICKS,
        &Block::INFESTED_MOSSY_STONE_BRICKS,
    ),
    (
        &Block::CRACKED_STONE_BRICKS,
        &Block::INFESTED_CRACKED_STONE_BRICKS,
    ),
    (
        &Block::CHISELED_STONE_BRICKS,
        &Block::INFESTED_CHISELED_STONE_BRICKS,
    ),
    (&Block::DEEPSLATE, &Block::INFESTED_DEEPSLATE),
];

/// Vanilla `InfestedBlock.isCompatibleHostBlock`.
#[must_use]
pub fn is_compatible_host_block(state_id: BlockStateId) -> bool {
    let block = Block::from_state_id(state_id);
    HOSTS.iter().any(|(host, _)| *host == block)
}

/// Vanilla `InfestedBlock.infestedStateByHost`, keeping the deepslate's axis.
#[must_use]
pub fn infested_state_by_host(state_id: BlockStateId) -> Option<BlockStateId> {
    let block = Block::from_state_id(state_id);
    let (_, infested) = HOSTS.iter().find(|(host, _)| *host == block)?;
    Some(swap_keeping_axis(state_id, infested))
}

/// Vanilla `InfestedBlock.hostStateByInfested`, keeping the deepslate's axis.
#[must_use]
pub fn host_state_by_infested(state_id: BlockStateId) -> Option<BlockStateId> {
    let block = Block::from_state_id(state_id);
    let (host, _) = HOSTS.iter().find(|(_, infested)| *infested == block)?;
    Some(swap_keeping_axis(state_id, host))
}

fn swap_keeping_axis(state_id: BlockStateId, to: &Block) -> BlockStateId {
    if to == &Block::DEEPSLATE || to == &Block::INFESTED_DEEPSLATE {
        PaleOakWoodLikeProperties::from_state_id(state_id).to_state_id(to)
    } else {
        to.default_state.id
    }
}

/// Vanilla `InfestedBlock.spawnInfestation`: a silverfish leaves the block, with the spawn
/// animation.
pub fn spawn_infestation(world: &Arc<World>, position: &BlockPos) {
    let pos = Vector3::new(
        f64::from(position.0.x) + 0.5,
        f64::from(position.0.y),
        f64::from(position.0.z) + 0.5,
    );
    let silverfish = from_type(&EntityType::SILVERFISH, pos, world, uuid::Uuid::new_v4());
    silverfish.get_entity().set_rotation(0.0, 0.0);
    world.spawn_entity(silverfish.clone());
    world.send_entity_status(
        silverfish.get_entity(),
        EntityStatus::SilverfishMergeAnim,
        ActorEventID::SilverfishMergeAnimation,
    );
}

impl BlockMetadata for InfestedBlock {
    fn ids() -> Box<[BlockId]> {
        [
            BlockId::INFESTED_STONE,
            BlockId::INFESTED_COBBLESTONE,
            BlockId::INFESTED_STONE_BRICKS,
            BlockId::INFESTED_MOSSY_STONE_BRICKS,
            BlockId::INFESTED_CRACKED_STONE_BRICKS,
            BlockId::INFESTED_CHISELED_STONE_BRICKS,
            BlockId::INFESTED_DEEPSLATE,
        ]
        .into()
    }
}

impl BlockBehaviour for InfestedBlock {
    fn on_place(&self, args: OnPlaceArgs<'_>) -> BlockStateId {
        if args.block == &Block::INFESTED_DEEPSLATE {
            let mut props = PaleOakWoodLikeProperties::default(args.block);
            props.axis = args.direction.to_axis();
            props.to_state_id(args.block)
        } else {
            args.block.default_state.id
        }
    }

    fn rotate(
        &self,
        block: &Block,
        state_id: BlockStateId,
        rotation: Rotation,
    ) -> &'static BlockState {
        if block == &Block::INFESTED_DEEPSLATE {
            let mut props = PaleOakWoodLikeProperties::from_state_id(state_id);
            if matches!(
                rotation,
                Rotation::Clockwise90 | Rotation::CounterClockwise90
            ) {
                props.axis = match props.axis {
                    Axis::X => Axis::Z,
                    Axis::Z => Axis::X,
                    Axis::Y => Axis::Y,
                };
            }
            BlockState::from_id(props.to_state_id(block))
        } else {
            BlockState::from_id(state_id)
        }
    }

    fn broken(&self, args: BrokenArgs<'_>) {
        // TODO: ugly fix, use onStacksDropped
        if args.player.gamemode.load() == GameMode::Creative
            || !args.world.level_info.load().game_rules.block_drops
            || args
                .player
                .inventory()
                .held_item()
                .get_enchantment_level(&Enchantment::SILK_TOUCH)
                > 0
        {
            return;
        }
        spawn_infestation(args.world, args.position);
    }
}

#[cfg(test)]
mod tests {
    use pumpkin_util::math::vector2::Vector2;
    use pumpkin_world::{chunk::ChunkData, world::BlockFlags};

    use super::*;
    use crate::entity::ai::test_support::world;
    use crate::world::explosion::{BlockInteraction, Explosion};

    const POS: BlockPos = BlockPos(Vector3::new(0, 64, 0));

    fn world_with_infested_stone(folder: &tempfile::TempDir) -> Arc<World> {
        let world = world(folder);
        world
            .level
            .loaded_chunks
            .insert(Vector2::new(0, 0), ChunkData::empty_sync(0, 0));
        world.set_block_state(
            &POS,
            Block::INFESTED_STONE.default_state.id,
            BlockFlags::SKIP_BLOCK_ADDED_CALLBACK | BlockFlags::UPDATE_KNOWN_SHAPE,
        );
        world
    }

    fn blast(world: &Arc<World>, interaction: BlockInteraction) {
        Explosion::new(4.0, Vector3::new(0.5, 64.5, 0.5), interaction).explode(world);
    }

    fn silverfish_positions(world: &World) -> Vec<Vector3<f64>> {
        world
            .entities
            .load()
            .iter()
            .filter(|entity| entity.get_entity().entity_type == &EntityType::SILVERFISH)
            .map(|entity| entity.get_entity().pos.load())
            .collect()
    }

    /// Vanilla `BlockBehaviour.onExplosionHit` calls `spawnAfterBreak` whatever the loot decay.
    #[tokio::test]
    async fn a_blast_releases_a_silverfish() -> Result<(), Box<dyn std::error::Error>> {
        for interaction in [
            BlockInteraction::Destroy,
            BlockInteraction::DestroyWithDecay,
        ] {
            let folder = tempfile::tempdir()?;
            let world = world_with_infested_stone(&folder);

            blast(&world, interaction);

            assert!(world.get_block_state(&POS).is_air());
            assert_eq!(silverfish_positions(&world), [Vector3::new(0.5, 64.0, 0.5)]);
        }
        Ok(())
    }

    /// Vanilla `InfestedBlock.spawnAfterBreak` is gated on `GameRules.BLOCK_DROPS`; the block
    /// still breaks.
    #[tokio::test]
    async fn block_drops_off_holds_the_silverfish_back() -> Result<(), Box<dyn std::error::Error>> {
        let folder = tempfile::tempdir()?;
        let world = world_with_infested_stone(&folder);
        let mut level_info = (**world.level_info.load()).clone();
        level_info.game_rules.block_drops = false;
        world.level_info.store(Arc::new(level_info));

        blast(&world, BlockInteraction::Destroy);

        assert!(world.get_block_state(&POS).is_air());
        assert!(silverfish_positions(&world).is_empty());
        Ok(())
    }

    /// `TRIGGER_BLOCK` skips the break in `onExplosionHit`, so a wind charge leaves it infested.
    #[tokio::test]
    async fn a_trigger_blast_leaves_the_block_infested() -> Result<(), Box<dyn std::error::Error>> {
        let folder = tempfile::tempdir()?;
        let world = world_with_infested_stone(&folder);

        blast(&world, BlockInteraction::TriggerBlock);

        assert_eq!(world.get_block(&POS), &Block::INFESTED_STONE);
        assert!(silverfish_positions(&world).is_empty());
        Ok(())
    }
}
