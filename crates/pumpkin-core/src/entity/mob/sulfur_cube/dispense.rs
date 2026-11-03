//! What a dispenser does to the sulfur cube in front of it.

use std::sync::Arc;

use pumpkin_data::item_stack::ItemStack;
use pumpkin_util::math::boundingbox::BoundingBox;
use pumpkin_util::math::position::BlockPos;

use super::SulfurCubeEntity;
use crate::entity::EntityBase;
use crate::world::World;

fn cubes_at(world: &Arc<World>, pos: &BlockPos) -> impl Iterator<Item = Arc<dyn EntityBase>> {
    world
        .get_entities_at_box(&BoundingBox::from_block(pos))
        .into_iter()
        .filter(|entity| entity.cast_any().is::<SulfurCubeEntity>())
}

impl SulfurCubeEntity {
    /// Vanilla `SulfurCubeBlockDispenseItemBehavior.dispenseBlock`.
    pub fn dispense_block(world: &Arc<World>, pos: &BlockPos, dispensed: &mut ItemStack) -> bool {
        for entity in cubes_at(world, pos) {
            if let Some(cube) = entity.cast_any().downcast_ref::<Self>()
                && cube.equip_item(dispensed)
            {
                dispensed.decrement(1);
                return true;
            }
        }
        false
    }

    /// Vanilla `FlintAndSteelDispenseItemBehavior.tryIgniteExplosiveEntities`.
    pub fn ignite_explosive_at(world: &Arc<World>, pos: &BlockPos) -> bool {
        for entity in cubes_at(world, pos) {
            if let Some(cube) = entity.cast_any().downcast_ref::<Self>()
                && cube.can_explode()
            {
                cube.prime_time(false);
                return true;
            }
        }
        false
    }
}
