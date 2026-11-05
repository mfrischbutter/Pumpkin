use crate::entity::projectile::owner::ProjectileOwner;
use std::sync::RwLock;
use std::sync::atomic::AtomicBool;

use crate::entity::area_effect_cloud::AreaEffectCloudEntity;
use crate::entity::projectile::ProjectileHit;
use crate::entity::projectile::thrown_item::{read_thrown_item, write_thrown_item};
use crate::entity::projectile::thrown_potion;
use crate::{
    entity::{Entity, EntityBase, projectile::ThrownItemEntity},
    server::Server,
};
use pumpkin_data::entity::EntityType;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::math::position::BlockPos;

const GRAVITY: f64 = 0.05;

pub struct LingeringPotionEntity {
    pub thrown: ThrownItemEntity,
    pub item_stack: RwLock<ItemStack>,
}

impl LingeringPotionEntity {
    pub fn new(entity: Entity) -> Self {
        let thrown = ThrownItemEntity {
            entity,
            owner: ProjectileOwner::none(),
            collides_with_projectiles: false,
            has_hit: AtomicBool::new(false),
            gravity: GRAVITY,
        };

        Self {
            thrown,
            item_stack: RwLock::new(ItemStack::new(
                1,
                &pumpkin_data::item::Item::LINGERING_POTION,
            )),
        }
    }

    pub fn new_shot(entity: Entity, shooter: &Entity) -> Self {
        let thrown = ThrownItemEntity::new(entity, shooter, GRAVITY);
        Self {
            thrown,
            item_stack: RwLock::new(ItemStack::new(
                1,
                &pumpkin_data::item::Item::LINGERING_POTION,
            )),
        }
    }

    pub fn set_item_stack(&self, item_stack: ItemStack) {
        let mut write = self
            .item_stack
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *write = item_stack;
    }

    /// Vanilla `ThrownLingeringPotion.onHitAsPotion`; the plugin event can cancel the cloud.
    fn on_hit_as_potion(&self, stack: &ItemStack, hit: &ProjectileHit) {
        let entity = self.get_entity();
        let world = entity.world.load_full();

        if let Some(server) = world.server.upgrade() {
            let mut event = crate::plugin::api::events::entity::lingering_potion_splash::LingeringPotionSplashEvent::new(
                entity.entity_id,
                BlockPos::floored_v(hit.hit_pos()),
                stack.item.registry_key.to_string(),
            );
            server.plugin_manager.fire_blocking(&server, &mut event);
            if event.cancelled {
                return;
            }
        }

        let position = match hit {
            ProjectileHit::Entity { entity: target, .. } => target.get_entity().pos.load(),
            ProjectileHit::Block { hit_pos, .. } => *hit_pos,
        };
        let owner = self
            .thrown
            .owner
            .get(&world)
            .filter(|owner| owner.get_living_entity().is_some());
        let cloud = AreaEffectCloudEntity::from_lingering_potion(
            Entity::new(world.clone(), position, &EntityType::AREA_EFFECT_CLOUD),
            stack,
            owner.as_deref().map(EntityBase::get_entity),
        );
        world.spawn_entity(cloud);
    }
}

impl EntityBase for LingeringPotionEntity {
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

        // Sync the item stack so the client renders the correct potion type
        entity.set_synced_data(
            pumpkin_data::tracked_data::lingering_potion::ITEM_STACK,
            pumpkin_protocol::codec::item_stack_seralizer::ItemStackSerializer::from(stack.clone()),
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

    fn on_hit(&self, hit: ProjectileHit) {
        let stack = self
            .item_stack
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        thrown_potion::on_hit(self, &stack, &hit, |_effects| {
            self.on_hit_as_potion(&stack, &hit);
        });
    }
}
