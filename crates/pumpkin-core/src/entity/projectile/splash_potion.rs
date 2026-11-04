use crate::entity::projectile::owner::ProjectileOwner;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, RwLock};

use crate::entity::living::EffectSource;
use crate::entity::projectile::ProjectileHit;
use crate::entity::projectile::thrown_item::{read_thrown_item, write_thrown_item};
use crate::entity::projectile::thrown_potion::{
    self, SPLASH_HEIGHT, SPLASH_RANGE, SPLASH_RANGE_SQ, box_distance_sqr, compute_margin,
    living_in_box,
};
use crate::item::potion::{PotionContents, PotionEffect};
use crate::{
    entity::{Entity, EntityBase, projectile::ThrownItemEntity},
    server::Server,
};
use pumpkin_data::item_stack::ItemStack;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::math::position::BlockPos;

const GRAVITY: f64 = 0.05;

pub struct SplashPotionEntity {
    pub thrown: ThrownItemEntity,
    pub item_stack: RwLock<ItemStack>,
}

impl SplashPotionEntity {
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
            item_stack: RwLock::new(ItemStack::new(1, &pumpkin_data::item::Item::SPLASH_POTION)),
        }
    }

    pub fn new_shot(entity: Entity, shooter: &Entity) -> Self {
        let thrown = ThrownItemEntity::new(entity, shooter, GRAVITY);
        Self {
            thrown,
            item_stack: RwLock::new(ItemStack::new(1, &pumpkin_data::item::Item::SPLASH_POTION)),
        }
    }

    pub fn set_item_stack(&self, item_stack: ItemStack) {
        let mut write = self
            .item_stack
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *write = item_stack;
    }

    /// Vanilla `ThrownSplashPotion.onHitAsPotion`; the plugin event can cancel the effects or
    /// narrow who gets them.
    fn on_hit_as_potion(&self, stack: &ItemStack, effects: &[PotionEffect], hit: &ProjectileHit) {
        let entity = self.get_entity();
        let world = entity.world.load_full();
        let potion_box = entity.bounding_box.load();
        let margin = compute_margin(entity);
        let area = potion_box.expand(SPLASH_RANGE, SPLASH_HEIGHT, SPLASH_RANGE);

        let mut affected: Vec<(Arc<dyn EntityBase>, f64)> = living_in_box(&world, &area)
            .into_iter()
            .filter_map(|target| {
                if !target.get_living_entity()?.is_affected_by_potions() {
                    return None;
                }
                let target_box = target.get_entity().bounding_box.load().expand_all(margin);
                let dist = box_distance_sqr(&potion_box, &target_box);
                let scale = 1.0 - dist.sqrt() / SPLASH_RANGE;
                (dist < SPLASH_RANGE_SQ).then_some((target, scale))
            })
            .collect();

        if let Some(server) = world.server.upgrade() {
            let mut event =
                crate::plugin::api::events::entity::potion_splash::PotionSplashEvent::new(
                    entity.entity_id,
                    BlockPos::floored_v(hit.hit_pos()),
                    stack.item.registry_key.to_string(),
                    affected
                        .iter()
                        .map(|(target, _)| target.get_entity().entity_id)
                        .collect(),
                );
            server.plugin_manager.fire_blocking(&server, &mut event);
            if event.cancelled {
                return;
            }
            affected.retain(|(target, _)| {
                event
                    .affected_entities
                    .contains(&target.get_entity().entity_id)
            });
        }

        let duration_scale = PotionContents::duration_scale(stack);
        let owner = self.thrown.owner.get(&world);
        for (target, scale) in affected {
            if let Some(living) = target.get_living_entity() {
                PotionContents::apply_splash_effects(
                    living,
                    effects,
                    scale,
                    duration_scale,
                    EffectSource {
                        direct: self,
                        owner: owner.as_deref(),
                    },
                );
            }
        }
    }
}

impl EntityBase for SplashPotionEntity {
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

        // Sync the item stack
        entity.set_synced_data(
            pumpkin_data::tracked_data::splash_potion::ITEM_STACK,
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
        thrown_potion::on_hit(self, &stack, &hit, |effects| {
            self.on_hit_as_potion(&stack, effects, &hit);
        });
    }
}
