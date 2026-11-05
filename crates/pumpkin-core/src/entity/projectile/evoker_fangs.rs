use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};

use pumpkin_data::damage::DamageType;
use pumpkin_data::entity::EntityStatus;
use pumpkin_protocol::bedrock::server::ActorEventID;

use crate::{
    entity::{Entity, EntityBase, projectile::owner::EntityReference},
    server::Server,
    world::World,
};

/// Vanilla `EvokerFangs.tick`: the warmup counter's value when the fangs bite.
const BITE_WARMUP: i32 = -8;
/// Vanilla `EvokerFangs.dealDamageTo` damage.
const BITE_DAMAGE: f32 = 6.0;
/// Vanilla `lifeTicks`: `ATTACK_DURATION` plus `LIFE_OFFSET`.
const LIFE_TICKS: i32 = 22;

pub struct EvokerFangsEntity {
    pub entity: Entity,
    pub warmup_delay_ticks: AtomicI32,
    pub life_ticks: AtomicI32,
    pub owner: EntityReference,
    pub sent_spike_event: AtomicBool,
}

impl EvokerFangsEntity {
    #[must_use]
    pub fn new(entity: Entity, warmup_ticks: u32, yaw: f32, owner: Option<&Entity>) -> Self {
        entity.set_rotation(yaw, 0.0);
        Self {
            entity,
            warmup_delay_ticks: AtomicI32::new(warmup_ticks as i32),
            life_ticks: AtomicI32::new(LIFE_TICKS),
            owner: EntityReference::new(owner),
            sent_spike_event: AtomicBool::new(false),
        }
    }

    /// Vanilla `EvokerFangs.tick`: every living thing in reach is bitten once.
    fn bite(&self, world: &World) {
        let bounds = self.entity.bounding_box.load().expand(0.2, 0.0, 0.2);
        let owner = self
            .owner
            .get(world)
            .filter(|owner| owner.get_living_entity().is_some());
        for target in world.get_all_at_box(&bounds) {
            self.deal_damage_to(target.as_ref(), owner.as_deref());
        }
    }

    /// Vanilla `EvokerFangs.dealDamageTo`: never its owner or what the owner is allied to.
    fn deal_damage_to(&self, target: &dyn EntityBase, owner: Option<&dyn EntityBase>) {
        let target_entity = target.get_entity();
        let Some(living) = target.get_living_entity() else {
            return;
        };
        if !living.is_alive()
            || target_entity.invulnerable.load(Ordering::Relaxed)
            || self.owner.matches(target_entity)
        {
            return;
        }
        if let Some(owner) = owner {
            if owner.is_allied_to(target) {
                return;
            }
            let _ = target.damage_with_context(
                target,
                BITE_DAMAGE,
                DamageType::INDIRECT_MAGIC,
                Some(self.entity.pos.load()),
                Some(self),
                Some(owner),
            );
        } else {
            let _ = target.damage_with_context(
                target,
                BITE_DAMAGE,
                DamageType::MAGIC,
                None,
                None,
                None,
            );
        }
    }
}

impl EntityBase for EvokerFangsEntity {
    fn write_custom_nbt(&self, nbt: &mut pumpkin_nbt::compound::NbtCompound) {
        nbt.put_int("Warmup", self.warmup_delay_ticks.load(Ordering::Relaxed));
        self.owner.write_nbt(nbt, "Owner");
    }

    fn read_custom_nbt(&self, nbt: &pumpkin_nbt::compound::NbtCompound) {
        if let Some(warmup) = nbt.get_int("Warmup") {
            self.warmup_delay_ticks.store(warmup, Ordering::Relaxed);
        }
        self.owner.read_nbt(nbt, "Owner");
    }

    fn remove_custom_nbt(&self, removed: &[Box<str>]) {
        if removed.iter().any(|key| &**key == "Owner") {
            self.owner.clear();
        }
    }

    /// Vanilla `EvokerFangs.tick`. The attack sound is the client's, from the entity event.
    fn tick(&self, caller: &dyn EntityBase, server: &Server) {
        let entity = &self.entity;
        entity.tick(caller, server);
        if entity.is_removed() {
            return;
        }
        let world = entity.world.load();

        let warmup = self.warmup_delay_ticks.fetch_sub(1, Ordering::Relaxed) - 1;
        if warmup < 0 {
            if warmup == BITE_WARMUP {
                self.bite(&world);
            }

            if !self.sent_spike_event.swap(true, Ordering::SeqCst) {
                world.send_entity_status(
                    entity,
                    EntityStatus::StartAttacking,
                    ActorEventID::StartAttacking,
                );
            }

            let life = self.life_ticks.fetch_sub(1, Ordering::Relaxed) - 1;
            if life < 0 {
                entity.remove();
            }
        }
    }

    fn get_entity(&self) -> &Entity {
        &self.entity
    }

    fn get_living_entity(&self) -> Option<&crate::entity::living::LivingEntity> {
        None
    }

    fn cast_any(&self) -> &dyn std::any::Any {
        self
    }
}
