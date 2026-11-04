use std::sync::atomic::AtomicBool;

use pumpkin_data::entity::EntityType;
use pumpkin_data::world::WorldEvent;
use pumpkin_util::math::boundingbox::BoundingBox;
use pumpkin_util::math::vector3::Vector3;

use crate::entity::area_effect_cloud::AreaEffectCloudEntity;
use crate::entity::projectile::owner::ProjectileOwner;
use crate::entity::{
    Entity, EntityBase,
    projectile::{ProjectileHit, ThrownItemEntity, fireball::INITIAL_ACCELERATION_POWER},
};
use crate::server::Server;

const SPLASH_RANGE: f64 = 4.0;
const CLOUD_RADIUS: f32 = 3.0;
const CLOUD_DURATION: i32 = 600;
const CLOUD_FINAL_RADIUS: f32 = 7.0;

/// Vanilla `DragonFireball`: bursts into a growing cloud of dragon's breath where it lands.
pub struct DragonFireballEntity {
    thrown: ThrownItemEntity,
}

impl DragonFireballEntity {
    #[must_use]
    pub const fn new(entity: Entity) -> Self {
        Self {
            thrown: ThrownItemEntity {
                entity,
                owner: ProjectileOwner::none(),
                collides_with_projectiles: false,
                has_hit: AtomicBool::new(false),
                gravity: 0.0,
            },
        }
    }

    /// A fireball of `shooter`'s, leaving `start` along `direction`.
    #[must_use]
    pub fn new_shot(
        entity: Entity,
        shooter: &Entity,
        start: Vector3<f64>,
        direction: Vector3<f64>,
    ) -> Self {
        let thrown = ThrownItemEntity::new(entity, shooter, 0.0);
        thrown.entity.pos.store(start);
        thrown.entity.velocity.store(direction.normalize().multiply(
            INITIAL_ACCELERATION_POWER,
            INITIAL_ACCELERATION_POWER,
            INITIAL_ACCELERATION_POWER,
        ));
        Self { thrown }
    }

    /// Where the cloud goes: on a living thing within splash range, else where the ball burst.
    fn cloud_position(&self, at: Vector3<f64>) -> Vector3<f64> {
        let entity = self.get_entity();
        let bounds =
            entity
                .bounding_box
                .load()
                .expand(SPLASH_RANGE, SPLASH_RANGE / 2.0, SPLASH_RANGE);
        let world = entity.world.load();
        let nearby = world.get_all_at_box(&BoundingBox::new(bounds.min, bounds.max));
        nearby
            .iter()
            .filter(|other| other.get_living_entity().is_some())
            .map(|other| other.get_entity().pos.load())
            .find(|pos| {
                let (x, y, z) = (pos.x - at.x, pos.y - at.y, pos.z - at.z);
                x * x + y * y + z * z < SPLASH_RANGE * SPLASH_RANGE
            })
            .unwrap_or(at)
    }
}

impl EntityBase for DragonFireballEntity {
    fn get_projectile_owner(&self) -> Option<&ProjectileOwner> {
        Some(&self.thrown.owner)
    }

    fn tick(&self, caller: &dyn EntityBase, _server: &Server) {
        self.thrown
            .process_hurting_tick(caller, INITIAL_ACCELERATION_POWER);
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

    /// Vanilla `DragonFireball.onHit`.
    fn on_hit(&self, hit: ProjectileHit) {
        let entity = self.get_entity();
        let world = entity.world.load_full();
        // Vanilla: flying into its own shooter does nothing.
        if let ProjectileHit::Entity { entity: hit, .. } = &hit
            && self
                .thrown
                .owner
                .get(&world)
                .is_some_and(|owner| owner.get_entity().entity_id == hit.get_entity().entity_id)
        {
            return;
        }
        let at = entity.pos.load();
        let position = self.cloud_position(at);
        let owner = self
            .thrown
            .owner
            .get(&world)
            .filter(|owner| owner.get_living_entity().is_some());
        let cloud = AreaEffectCloudEntity::dragon_breath(
            Entity::new(world.clone(), position, &EntityType::AREA_EFFECT_CLOUD),
            CLOUD_RADIUS,
            CLOUD_DURATION,
            (CLOUD_FINAL_RADIUS - CLOUD_RADIUS) / CLOUD_DURATION as f32,
            1,
            owner.as_deref().map(EntityBase::get_entity),
        );
        world.sync_world_event(
            WorldEvent::ParticlesDragonFireballSplash,
            entity.block_pos.load(),
            1,
        );
        world.spawn_entity(cloud);
    }
}
