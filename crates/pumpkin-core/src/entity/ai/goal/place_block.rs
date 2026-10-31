use std::sync::Arc;

use super::{Goal, to_goal_ticks};
use crate::entity::EntityBase;
use crate::entity::mob::Mob;
use crate::entity::mob::enderman::EndermanEntity;
use crate::world::World;
use crate::world::game_event::GameEventContext;
use pumpkin_data::game_event::GameEvent;
use pumpkin_data::{Block, BlockStateId};
use pumpkin_util::math::boundingbox::BoundingBox;
use pumpkin_util::math::position::BlockPos;
use pumpkin_world::world::BlockFlags;
use rand::RngExt;

/// Vanilla `EndermanLeaveBlockGoal`: puts the carried block down nearby.
pub struct PlaceBlockGoal {
    enderman: Arc<EndermanEntity>,
}

impl PlaceBlockGoal {
    pub const fn new(enderman: Arc<EndermanEntity>) -> Self {
        Self { enderman }
    }

    /// Vanilla `canPlaceBlock`: air above a full block, where the block survives.
    ///
    /// Bedrock below and entities other than the enderman itself are in the way.
    fn can_place_block(&self, world: &Arc<World>, pos: &BlockPos, carried: BlockStateId) -> bool {
        if !world.get_block_state(pos).is_air() {
            return false;
        }
        let (below_block, below_state) = world.get_block_and_state(&pos.down());
        if below_state.is_air() || below_block == &Block::BEDROCK || !below_state.is_full_cube() {
            return false;
        }
        if !world.block_registry.can_place_at(
            None,
            Some(&**world),
            &**world,
            None,
            Block::from_state_id(carried),
            carried.to_state(),
            pos,
            None,
            None,
        ) {
            return false;
        }

        let cell = BoundingBox::from_block(pos);
        let enderman_id = self.enderman.get_entity().entity_id;
        world
            .get_entities_at_box(&cell)
            .iter()
            .all(|entity| entity.get_entity().entity_id == enderman_id)
            && world
                .get_players_at_box(&cell)
                .iter()
                .all(|player| player.is_spectator())
    }
}

impl Goal for PlaceBlockGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        if self.enderman.get_carried_block().is_none() {
            return false;
        }

        let entity = &mob.get_mob_entity().living_entity.entity;
        let world = entity.world.load();
        if !world.level_info.load().game_rules.mob_griefing {
            return false;
        }

        if mob.get_random().random_range(0..to_goal_ticks(2000)) != 0 {
            return false;
        }

        true
    }

    fn tick(&mut self, mob: &dyn Mob) {
        let Some(carried) = self.enderman.get_carried_block() else {
            return;
        };

        let entity = &mob.get_mob_entity().living_entity.entity;
        let pos = entity.pos.load();

        let (bx, by, bz) = {
            let mut rng = mob.get_random();
            (
                (pos.x - 1.0 + rng.random::<f64>() * 2.0).floor() as i32,
                (pos.y + rng.random::<f64>() * 2.0).floor() as i32,
                (pos.z - 1.0 + rng.random::<f64>() * 2.0).floor() as i32,
            )
        };

        let world = entity.world.load_full();
        let target_pos = BlockPos::new(bx, by, bz);
        let carried = world.update_from_neighbor_shapes(carried, &target_pos);
        if !self.can_place_block(&world, &target_pos, carried) {
            return;
        }

        world.set_block_state(&target_pos, carried, BlockFlags::NOTIFY_ALL);
        let source: &dyn EntityBase = mob;
        world.emit_game_event_with_context(
            GameEvent::BlockPlace.name(),
            target_pos.to_centered_f64(),
            &GameEventContext {
                source_entity: Some(source),
            },
        );
        self.enderman.set_carried_block(None);
    }
}
