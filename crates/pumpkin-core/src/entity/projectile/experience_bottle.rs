use std::sync::RwLock;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::entity::experience_orb::ExperienceOrbEntity;
use crate::entity::projectile::owner::ProjectileOwner;
use crate::entity::projectile::thrown_item::{read_thrown_item, write_thrown_item};
use crate::entity::projectile::{ProjectileHit, ThrownItemEntity};
use crate::entity::{Entity, EntityBase};
use crate::item::potion::BASE_POTION_COLOR;
use crate::server::Server;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::world::WorldEvent;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_protocol::codec::item_stack_seralizer::ItemStackSerializer;
use pumpkin_util::math::position::BlockPos;
use rand::RngExt;

const GRAVITY: f64 = 0.07;

pub struct ExperienceBottleEntity {
    pub thrown: ThrownItemEntity,
    pub item_stack: RwLock<ItemStack>,
}

impl ExperienceBottleEntity {
    pub fn new(entity: Entity) -> Self {
        Self {
            thrown: ThrownItemEntity {
                entity,
                owner: ProjectileOwner::none(),
                collides_with_projectiles: false,
                has_hit: AtomicBool::new(false),
                gravity: GRAVITY,
            },
            item_stack: RwLock::new(ItemStack::new(1, &Item::EXPERIENCE_BOTTLE)),
        }
    }

    pub fn new_shot(entity: Entity, shooter: &Entity) -> Self {
        Self {
            thrown: ThrownItemEntity::new(entity, shooter, GRAVITY),
            item_stack: RwLock::new(ItemStack::new(1, &Item::EXPERIENCE_BOTTLE)),
        }
    }

    pub fn set_item_stack(&self, item_stack: ItemStack) {
        let mut write = self
            .item_stack
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *write = item_stack;
    }
}

impl EntityBase for ExperienceBottleEntity {
    fn get_projectile_owner(&self) -> Option<&ProjectileOwner> {
        Some(&self.thrown.owner)
    }

    fn get_projectile_item(&self) -> Option<ItemStack> {
        Some(
            self.item_stack
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone(),
        )
    }

    fn init_data_tracker(&self) {
        let entity = self.get_entity();
        let stack = self
            .item_stack
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        entity.set_synced_data(
            pumpkin_data::tracked_data::experience_bottle::ITEM_STACK,
            ItemStackSerializer::from(stack.clone()),
        );
    }

    fn tick(&self, caller: &dyn EntityBase, server: &Server) {
        self.thrown.process_throwable_tick(caller, server);
    }

    fn get_entity(&self) -> &Entity {
        self.thrown.get_entity()
    }

    fn get_living_entity(&self) -> Option<&crate::entity::living::LivingEntity> {
        None
    }

    fn write_custom_nbt(&self, nbt: &mut NbtCompound) {
        write_thrown_item(&self.item_stack, nbt);
    }

    fn read_custom_nbt(&self, nbt: &NbtCompound) {
        read_thrown_item(&self.item_stack, nbt);
    }

    /// Vanilla `ThrownExperienceBottle.onHit`: the orbs fly off the face it hit, or back
    /// along its flight from an entity.
    fn on_hit(&self, hit: ProjectileHit) {
        let entity = self.get_entity();
        let world = entity.world.load_full();
        let hit_pos = hit.hit_pos();
        let block_pos = BlockPos::floored_v(hit_pos);

        world.sync_world_event(
            WorldEvent::ParticlesSpellPotionSplash,
            block_pos,
            BASE_POTION_COLOR,
        );
        if !entity.silent.load(Ordering::Relaxed) {
            world.sync_world_event(WorldEvent::SoundSpellPotionSplash, block_pos, 0);
        }

        let mut rng = rand::rng();
        let xp_count = 3 + rng.random_range(0..5) + rng.random_range(0..5);
        let direction = hit.face().map_or_else(
            || entity.velocity.load().multiply(-1.0, -1.0, -1.0),
            |face| face.to_offset().to_f64(),
        );
        ExperienceOrbEntity::spawn_with_direction(&world, hit_pos, direction, xp_count);
    }

    fn cast_any(&self) -> &dyn std::any::Any {
        self
    }
}
