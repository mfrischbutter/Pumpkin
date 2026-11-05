use std::sync::{Arc, atomic::Ordering};

use crate::{
    entity::{Entity, EntityBase, living::LivingEntity},
    net::{bedrock::BedrockClient, java::JavaClient},
    server::Server,
};

/// Vanilla `Marker`: never sent to clients (its tracking range is 0) and without state of its
/// own. The `data` tag every entity has lives on [`Entity`].
pub struct MarkerEntity {
    pub entity: Entity,
}

impl MarkerEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        entity.no_physics.store(true, Ordering::Relaxed);
        Arc::new(Self { entity })
    }
}

impl EntityBase for MarkerEntity {
    fn tick(&self, _caller: &dyn EntityBase, _server: &Server) {}

    fn init_data_tracker(&self) {}

    fn get_entity(&self) -> &Entity {
        &self.entity
    }

    fn get_living_entity(&self) -> Option<&LivingEntity> {
        None
    }

    fn cast_any(&self) -> &dyn std::any::Any {
        self
    }

    fn is_pushed_by_fluids(&self) -> bool {
        false
    }

    fn is_immune_to_explosion(&self) -> bool {
        true
    }

    fn send_java_spawn_packet(&self, _client: &JavaClient) {}

    fn send_bedrock_spawn_packet(&self, _client: &BedrockClient) {}
}
