use crate::entity::projectile::owner::ProjectileOwner;
use std::sync::RwLock;
use std::sync::atomic::AtomicBool;

use crate::entity::projectile::ProjectileHit;
use crate::entity::projectile::thrown_item::{hurt_thrown, read_thrown_item, write_thrown_item};
use crate::{
    entity::{Entity, EntityBase, projectile::ThrownItemEntity},
    server::Server,
};
use pumpkin_data::entity::{EntityStatus, EntityType};
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_protocol::bedrock::server::actor_event::ActorEventID;
use pumpkin_protocol::codec::item_stack_seralizer::ItemStackSerializer;

const GRAVITY: f64 = 0.03;
/// Vanilla `Snowball.onHitEntity`: blazes take this, everything else a hit of zero.
const BLAZE_DAMAGE: f32 = 3.0;

pub struct SnowballEntity {
    pub thrown: ThrownItemEntity,
    pub item_stack: RwLock<ItemStack>,
}

impl SnowballEntity {
    pub fn new(entity: Entity) -> Self {
        // Initialize without owner
        let thrown = ThrownItemEntity {
            entity,
            owner: ProjectileOwner::none(),
            collides_with_projectiles: false,
            has_hit: AtomicBool::new(false),
            gravity: GRAVITY,
        };

        Self {
            thrown,
            item_stack: RwLock::new(ItemStack::new(1, &Item::SNOWBALL)),
        }
    }

    pub fn new_shot(entity: Entity, shooter: &Entity) -> Self {
        let thrown = ThrownItemEntity::new(entity, shooter, GRAVITY);
        Self {
            thrown,
            item_stack: RwLock::new(ItemStack::new(1, &Item::SNOWBALL)),
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

impl EntityBase for SnowballEntity {
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
            pumpkin_data::tracked_data::snowball::ITEM_STACK,
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
    fn cast_any(&self) -> &dyn std::any::Any {
        self
    }

    fn write_custom_nbt(&self, nbt: &mut NbtCompound) {
        write_thrown_item(&self.item_stack, nbt);
    }

    fn read_custom_nbt(&self, nbt: &NbtCompound) {
        read_thrown_item(&self.item_stack, nbt);
    }

    /// Vanilla `Snowball.onHitEntity` and `onHit`.
    fn on_hit(&self, hit: ProjectileHit) {
        if let ProjectileHit::Entity { entity: target, .. } = &hit {
            let is_blaze = target.get_entity().entity_type.id == EntityType::BLAZE.id;
            hurt_thrown(self, target, if is_blaze { BLAZE_DAMAGE } else { 0.0 });
        }

        let world = self.get_entity().world.load();
        world.send_entity_status(self.get_entity(), EntityStatus::Death, ActorEventID::Death);
    }
}
