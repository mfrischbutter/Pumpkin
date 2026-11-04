use crate::entity::projectile::owner::ProjectileOwner;
use std::sync::RwLock;
use std::sync::atomic::AtomicBool;

use crate::entity::projectile::ProjectileHit;
use crate::entity::projectile::thrown_item::{hurt_thrown, read_thrown_item, write_thrown_item};
use crate::plugin::player::egg_throw::PlayerEggThrowEvent;
use crate::{
    entity::{Entity, EntityBase, projectile::ThrownItemEntity, r#type::from_type},
    server::Server,
};
use pumpkin_data::entity::{EntityStatus, EntityType};
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_protocol::bedrock::server::actor_event::ActorEventID;
use pumpkin_protocol::codec::item_stack_seralizer::ItemStackSerializer;
use pumpkin_util::math::boundingbox::EntityDimensions;
use rand::RngExt;
use uuid::Uuid;

const MAX_EGG_HATCH_EVENT_SPAWNS: usize = 16;
const GRAVITY: f64 = 0.03;
/// Vanilla `ThrownEgg.ZERO_SIZED_DIMENSIONS`.
const ZERO_SIZED_DIMENSIONS: EntityDimensions = EntityDimensions::new(0.0, 0.0, 0.0);
/// The age of a hatched chick.
const BABY_AGE: i32 = -24000;

pub struct EggEntity {
    pub thrown: ThrownItemEntity,
    pub item_stack: RwLock<ItemStack>,
}

impl EggEntity {
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
            item_stack: RwLock::new(ItemStack::new(1, &Item::EGG)),
        }
    }

    pub fn new_shot(entity: Entity, shooter: &Entity) -> Self {
        let thrown = ThrownItemEntity::new(entity, shooter, GRAVITY);

        Self {
            thrown,
            item_stack: RwLock::new(ItemStack::new(1, &Item::EGG)),
        }
    }

    /// Set the item stack shown by this thrown egg
    pub fn set_item_stack(&self, item_stack: ItemStack) {
        let mut write = self
            .item_stack
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *write = item_stack;
    }

    /// The hatching of vanilla `ThrownEgg.onHit`, which the plugin event can change.
    fn hatch(&self, hit: &ProjectileHit) {
        let entity = self.get_entity();
        let world = entity.world.load_full();

        // One in eight hatches, and one of those in 32 hatches four.
        let mut rng = rand::rng();
        let mut to_spawn = if rng.random_range(0..8) != 0 {
            0usize
        } else if rng.random_range(0..32) == 0 {
            4
        } else {
            1
        };
        let mut hatching = to_spawn > 0;
        let mut hatching_type: &'static EntityType = &EntityType::CHICKEN;

        let variant_name = {
            let stack = self
                .item_stack
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            stack
                .get_data_component::<pumpkin_data::data_component_impl::ChickenVariantImpl>()
                .map(|comp| comp.value.clone())
        };

        if let Some(player) = self.thrown.owner.get_player(&world)
            && let Some(server) = world.server.upgrade()
        {
            let mut event = PlayerEggThrowEvent::new(
                player,
                entity.entity_uuid,
                hatching,
                to_spawn as u8,
                hatching_type,
            );
            server.plugin_manager.fire_blocking(&server, &mut event);
            if event.cancelled {
                hatching = false;
            } else {
                hatching = event.hatching;
                to_spawn = (event.num_hatches as usize).min(MAX_EGG_HATCH_EVENT_SPAWNS);
                hatching_type = event.hatching_type;
            }
        }

        if !hatching {
            return;
        }
        let yaw = entity.yaw.load();
        for _ in 0..to_spawn {
            let mob = from_type(hatching_type, hit.hit_pos(), &world, Uuid::new_v4());
            let new_entity = mob.get_entity();
            new_entity.set_rotation(yaw, 0.0);
            new_entity.set_age(BABY_AGE);
            if let Some(name) = &variant_name {
                mob.set_variant_name(name);
            }
            // No room for it where the egg broke: vanilla stops hatching.
            if !new_entity.fudge_position_for(mob.as_ref(), ZERO_SIZED_DIMENSIONS) {
                break;
            }
            world.spawn_entity(mob);
        }
    }
}

impl EntityBase for EggEntity {
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

        // Sync the item stack so the client renders the correct color/variant
        entity.set_synced_data(
            pumpkin_data::tracked_data::egg::ITEM_STACK,
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

    /// Vanilla `ThrownEgg.onHitEntity` and `onHit`.
    fn on_hit(&self, hit: ProjectileHit) {
        if let ProjectileHit::Entity { entity: target, .. } = &hit {
            hurt_thrown(self, target, 0.0);
        }
        self.hatch(&hit);

        let world = self.get_entity().world.load();
        world.send_entity_status(self.get_entity(), EntityStatus::Death, ActorEventID::Death);
    }

    fn cast_any(&self) -> &dyn std::any::Any {
        self
    }
}
