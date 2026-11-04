use crate::entity::projectile::owner::ProjectileOwner;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, RwLock};

use super::ender_pearl_ticket::PearlTickets;
use crate::entity::living::LivingEntity;
use crate::entity::player::Player;
use crate::entity::projectile::ProjectileHit;
use crate::entity::projectile::thrown_item::{hurt_thrown, read_thrown_item, write_thrown_item};
use crate::world::World;
use crate::{
    entity::{
        Entity, EntityBase, EntityType, mob::endermite::EndermiteEntity,
        projectile::ThrownItemEntity,
    },
    server::Server,
};
use pumpkin_data::damage::DamageType;
use pumpkin_data::entity::EntityPose;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_protocol::codec::item_stack_seralizer::ItemStackSerializer;
use pumpkin_util::math::vector2::Vector2;
use pumpkin_util::math::vector3::Vector3;

const GRAVITY: f64 = 0.03;
const ENDERMITE_SPAWN_CHANCE: f32 = 0.05;
/// Vanilla `ThrownEnderpearl.onHit`: what the landing costs a player.
const TELEPORT_DAMAGE: f32 = 5.0;

pub struct EnderPearlEntity {
    pub thrown: ThrownItemEntity,
    pub item_stack: RwLock<ItemStack>,
    tickets: Mutex<PearlTickets>,
}

impl EnderPearlEntity {
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
            item_stack: RwLock::new(ItemStack::new(1, &Item::ENDER_PEARL)),
            tickets: Mutex::new(PearlTickets::default()),
        }
    }

    pub fn new_shot(entity: Entity, shooter: &Entity) -> Self {
        let thrown = ThrownItemEntity::new(entity, shooter, GRAVITY);
        Self {
            thrown,
            item_stack: RwLock::new(ItemStack::new(1, &Item::ENDER_PEARL)),
            tickets: Mutex::new(PearlTickets::default()),
        }
    }

    pub fn set_item_stack(&self, item_stack: ItemStack) {
        let mut write = self
            .item_stack
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *write = item_stack;
    }

    /// Vanilla `ServerPlayer.placeEnderPearlTicket` for a pearl loaded with its owner.
    pub(crate) fn place_ticket(&self) {
        let entity = self.get_entity();
        self.tickets
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .tick(
                &entity.world.load().level,
                entity.chunk_pos.load(),
                true,
            );
    }

    /// The ticket step of vanilla `ThrownEnderpearl.tick`, with
    /// `ServerPlayer.registerAndUpdateEnderPearlTicket`.
    fn update_ticket(&self, owner: &Player, previous_chunk: Vector2<i32>) {
        let entity = self.get_entity();
        let world = entity.world.load_full();
        let chunk = entity.chunk_pos.load();
        let placed = self
            .tickets
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .tick(&world.level, chunk, chunk != previous_chunk);
        if placed && let Some(pearl) = world.get_entity_by_id(entity.entity_id) {
            owner.register_ender_pearl(&pearl);
        }
    }

    /// The teleport of vanilla `ThrownEnderpearl.onHit`: mobs just move, a player can bring
    /// an endermite along and pays for the trip.
    fn teleport_owner(&self, owner: &dyn EntityBase, world: &std::sync::Arc<World>) {
        let entity = self.get_entity();
        let owner_entity = owner.get_entity();
        let teleport_pos = entity.last_pos.load();
        let is_player = owner.get_player().is_some();

        if is_player {
            if rand::random::<f32>() < ENDERMITE_SPAWN_CHANCE && world.should_spawn_monsters() {
                let endermite = Entity::new(
                    world.clone(),
                    owner_entity.pos.load(),
                    &EntityType::ENDERMITE,
                );
                endermite.set_rotation(owner_entity.yaw.load(), owner_entity.pitch.load());
                world.spawn_entity(EndermiteEntity::new(endermite));
            }
            if entity.portal_cooldown.load(Ordering::Relaxed) > 0 {
                owner_entity
                    .portal_cooldown
                    .store(owner_entity.default_portal_cooldown(), Ordering::Relaxed);
            }
        }

        // `TeleportTransition` without `asPassenger` leaves the vehicle.
        if let Some(vehicle) = owner_entity.get_vehicle() {
            vehicle
                .get_entity()
                .remove_passenger_before_teleport(owner_entity.entity_id);
        }
        owner.teleport(
            teleport_pos,
            Some(owner_entity.yaw.load()),
            Some(owner_entity.pitch.load()),
            world.clone(),
        );
        if let Some(living) = owner.get_living_entity() {
            living.fall_distance.store(0.0);
            living.current_impulse.reset();
        }
        if is_player {
            owner.damage(owner, TELEPORT_DAMAGE, DamageType::ENDER_PEARL);
        } else {
            owner_entity.set_velocity(Vector3::new(0.0, 0.0, 0.0));
        }

        world.play_sound(
            Sound::EntityPlayerTeleport,
            SoundCategory::Players,
            &teleport_pos,
        );
    }
}

/// Vanilla `ThrownEnderpearl.isAllowedToTeleportOwner`.
///
/// Only players can change worlds on this engine: `Entity::teleport` ignores the world, so a
/// mob owner in another dimension stays where it is.
fn is_allowed_to_teleport_owner(owner: &dyn EntityBase, world: &World) -> bool {
    let owner_entity = owner.get_entity();
    let alive = owner
        .get_living_entity()
        .map_or_else(|| owner_entity.is_alive(), LivingEntity::is_alive);
    if owner_entity.world.load().dimension == world.dimension {
        alive && owner_entity.pose.load() != EntityPose::Sleeping
    } else {
        alive && owner.get_player().is_some()
    }
}

impl EntityBase for EnderPearlEntity {
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
            pumpkin_data::tracked_data::ender_pearl::ITEM_STACK,
            ItemStackSerializer::from(stack.clone()),
        );
    }

    /// Vanilla `ThrownEnderpearl.tick`: the pearl of a dead thrower vanishes, the pearl of a
    /// player keeps its chunks loaded.
    fn tick(&self, caller: &dyn EntityBase, server: &Server) {
        let entity = self.get_entity();
        let owner = self.thrown.owner.get_player(&entity.world.load());
        let previous_chunk = entity.chunk_pos.load();
        let vanishes = owner.as_ref().is_some_and(|player| {
            !player.living_entity.is_alive()
                && player
                    .world()
                    .level_info
                    .load()
                    .game_rules
                    .ender_pearls_vanish_on_death
        });
        if vanishes {
            entity.remove();
        } else {
            self.thrown.process_throwable_tick(caller, server);
        }

        if entity.is_alive()
            && let Some(owner) = owner
        {
            self.update_ticket(&owner, previous_chunk);
        }
    }

    fn get_entity(&self) -> &Entity {
        self.thrown.get_entity()
    }

    fn get_living_entity(&self) -> Option<&LivingEntity> {
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

    /// Vanilla `ThrownEnderpearl.onHitEntity` and `onHit`. The 32 portal particles of `onHit`
    /// go through `Level.addParticle`, which does nothing on a server: each client draws them
    /// when its own copy of the pearl hits.
    fn on_hit(&self, hit: ProjectileHit) {
        let world = self.get_entity().world.load_full();
        if let ProjectileHit::Entity { entity: target, .. } = &hit {
            hurt_thrown(self, target, 0.0);
        }
        if let Some(owner) = self.get_owner_entity()
            && is_allowed_to_teleport_owner(owner.as_ref(), &world)
        {
            self.teleport_owner(owner.as_ref(), &world);
        }
    }
}
