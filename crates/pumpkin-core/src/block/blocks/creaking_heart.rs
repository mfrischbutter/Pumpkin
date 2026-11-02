use std::sync::Arc;

use pumpkin_data::BlockStateId;
use pumpkin_data::block_properties::{Axis, CreakingHeartLikeProperties, CreakingHeartState};
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_macros::pumpkin_block;
use pumpkin_util::GameMode;
use pumpkin_world::world::BlockFlags;
use rand::RngExt;

use crate::block::entities::creaking_heart::{
    CreakingHeartBlockEntity, creaking_active, has_required_logs, with_heart,
};
use crate::block::{
    BlockBehaviour, BrokenArgs, GetComparatorOutputArgs, OnNeighborUpdateArgs, OnPlaceArgs,
    OnStateReplacedArgs, PlacedArgs,
};
use crate::entity::experience_orb::ExperienceOrbEntity;
use crate::world::World;

/// Vanilla `tryAwardExperience`.
const NATURAL_HEART_XP: std::ops::RangeInclusive<u32> = 20..=24;

#[pumpkin_block("minecraft:creaking_heart")]
pub struct CreakingHeartBlock;

impl CreakingHeartBlock {
    /// Vanilla `updateState`: an uprooted heart between two logs wakes up or dozes off.
    fn root_if_possible(world: &Arc<World>, position: &pumpkin_util::math::position::BlockPos) {
        let state_id = world.get_block_state_id(position);
        let mut props = CreakingHeartLikeProperties::from_state_id(state_id);
        if props.creaking_heart_state != CreakingHeartState::Uprooted
            || !has_required_logs(world, position, props.axis)
        {
            return;
        }
        props.creaking_heart_state = Self::rooted_state(world);
        world.set_block_state(
            position,
            props.to_state_id(state_id.to_block()),
            BlockFlags::NOTIFY_ALL,
        );
    }

    fn rooted_state(world: &World) -> CreakingHeartState {
        if creaking_active(world) {
            CreakingHeartState::Awake
        } else {
            CreakingHeartState::Dormant
        }
    }
}

impl BlockBehaviour for CreakingHeartBlock {
    fn on_place(&self, args: OnPlaceArgs<'_>) -> BlockStateId {
        let mut props = CreakingHeartLikeProperties::from_state_id(args.block.default_state.id);
        props.axis = match args.direction {
            pumpkin_data::BlockDirection::North | pumpkin_data::BlockDirection::South => Axis::Z,
            pumpkin_data::BlockDirection::East | pumpkin_data::BlockDirection::West => Axis::X,
            pumpkin_data::BlockDirection::Up | pumpkin_data::BlockDirection::Down => Axis::Y,
        };
        props.creaking_heart_state = CreakingHeartState::Uprooted;
        props.to_state_id(args.block)
    }

    fn placed(&self, args: PlacedArgs<'_>) {
        let entity = CreakingHeartBlockEntity::new(*args.position);
        args.world.add_block_entity(Arc::new(entity));
        Self::root_if_possible(args.world, args.position);
    }

    /// Vanilla `updateShape` and `tick`: only the rooting; the block entity uproots it.
    fn on_neighbor_update(&self, args: OnNeighborUpdateArgs<'_>) {
        Self::root_if_possible(args.world, args.position);
    }

    /// Vanilla `preRemoveSideEffects`.
    fn on_state_replaced(&self, args: OnStateReplacedArgs<'_>) {
        if args.old_state_id.to_block() == args.block
            && args.world.get_block_state_id(args.position).to_block() != args.block
        {
            with_heart(args.world, args.position, |heart| {
                heart.remove_protector(args.world);
            });
        }
    }

    /// The experience of vanilla `playerWillDestroy`, for a natural heart.
    fn broken(&self, args: BrokenArgs<'_>) {
        args.world.play_sound(
            Sound::BlockCreakingHeartBreak,
            SoundCategory::Blocks,
            &args.position.to_f64(),
        );
        let gamemode = args.player.gamemode.load();
        if CreakingHeartLikeProperties::from_state_id(args.state.id).natural
            && gamemode != GameMode::Creative
            && gamemode != GameMode::Spectator
        {
            ExperienceOrbEntity::spawn(
                args.world,
                args.position.to_centered_f64(),
                rand::rng().random_range(NATURAL_HEART_XP),
            );
        }
    }

    /// Vanilla `getAnalogOutputSignal`.
    fn get_comparator_output(&self, args: GetComparatorOutputArgs<'_>) -> Option<u8> {
        let props = CreakingHeartLikeProperties::from_state_id(args.state.id);
        if props.creaking_heart_state == CreakingHeartState::Uprooted {
            return Some(0);
        }
        with_heart(args.world, args.position, |heart| {
            heart.output_signal() as u8
        })
    }
}
