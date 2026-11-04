//! Vanilla `FireworkRocketEntity`.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::RwLock;

use pumpkin_data::damage::DamageType;
use pumpkin_data::data_component_impl::FireworksImpl;
use pumpkin_data::entity::EntityStatus;
use pumpkin_data::game_event::GameEvent;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_protocol::bedrock::server::actor_event::ActorEventID;
use pumpkin_protocol::codec::item_stack_seralizer::ItemStackSerializer;
use pumpkin_protocol::codec::optional_int::OptionalInt;
use pumpkin_util::Hand;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_util::random::{RandomGenerator, RandomImpl, get_seed, xoroshiro128::Xoroshiro};
use pumpkin_world::clip::{ClipBlock, ClipContext, ClipFluid};

use super::hit_result::{self, MoveHit};
use crate::entity::projectile::ProjectileHit;
use crate::entity::projectile::owner::ProjectileOwner;
use crate::plugin::api::events::player::player_elytra_boost::PlayerElytraBoostEvent;
use crate::world::game_event::GameEventContext;
use crate::{
    entity::{Entity, EntityBase, projectile::ThrownItemEntity},
    server::Server,
    world::World,
};

const GRAVITY: f64 = 0.0;
/// The radius of vanilla `dealExplosionDamage`.
const EXPLOSION_RADIUS: f64 = 5.0;

pub struct FireworkRocketEntity {
    pub entity: ThrownItemEntity,
    life: AtomicU32,
    life_time: AtomicU32,
    item_stack: RwLock<ItemStack>,
    attached_to_entity: Option<i32>,
    shot_at_angle: AtomicBool,
    has_been_shot: AtomicBool,
}

impl FireworkRocketEntity {
    /// Vanilla's base constructor: a drift, a slow start up, and the lifetime from the item.
    fn with_owner(entity: Entity, owner: Option<&Entity>, stack: ItemStack) -> Self {
        let mut random = RandomGenerator::Xoroshiro(Xoroshiro::from_seed(get_seed()));
        let flight_count = 1 + stack
            .get_data_component::<FireworksImpl>()
            .map_or(0, |fireworks| fireworks.flight_duration.max(0) as u32);
        let life_time = 10 * flight_count
            + random.next_bounded_i32(6) as u32
            + random.next_bounded_i32(7) as u32;
        entity.velocity.store(Vector3::new(
            random.next_triangular(0.0, 0.002_297),
            0.05,
            random.next_triangular(0.0, 0.002_297),
        ));
        Self {
            entity: ThrownItemEntity {
                entity,
                owner: ProjectileOwner::new(owner),
                collides_with_projectiles: false,
                has_hit: AtomicBool::new(false),
                gravity: GRAVITY,
            },
            life: 0.into(),
            life_time: life_time.into(),
            item_stack: RwLock::new(stack),
            attached_to_entity: None,
            shot_at_angle: AtomicBool::new(false),
            has_been_shot: AtomicBool::new(false),
        }
    }

    pub fn new(entity: Entity) -> Self {
        Self::new_with_stack(entity, ItemStack::new(1, &Item::FIREWORK_ROCKET))
    }

    pub fn new_with_stack(entity: Entity, stack: ItemStack) -> Self {
        Self::with_owner(entity, None, stack)
    }

    pub fn new_shot(entity: Entity, shooter: &Entity) -> Self {
        Self::new_shot_with_stack(entity, shooter, ItemStack::new(1, &Item::FIREWORK_ROCKET))
    }

    /// Vanilla's `FireworkRocketEntity(level, sourceItemStack, stuckTo)`: the elytra boost.
    pub fn new_shot_with_stack(entity: Entity, shooter: &Entity, stack: ItemStack) -> Self {
        let mut rocket = Self::with_owner(entity, Some(shooter), stack);
        rocket.attached_to_entity = Some(shooter.entity_id);
        rocket
    }

    /// Vanilla's `FireworkRocketEntity(level, sourceItemStack, owner, x, y, z, true)`.
    pub fn new_shot_at_angle(entity: Entity, shooter: &Entity, stack: ItemStack) -> Self {
        let rocket = Self::with_owner(entity, Some(shooter), stack);
        rocket.shot_at_angle.store(true, Ordering::Relaxed);
        rocket
    }

    pub fn set_shot_at_angle(&self, shot_at_angle: bool) {
        self.shot_at_angle.store(shot_at_angle, Ordering::Relaxed);
    }

    /// The number of explosions, vanilla `getExplosions().size()`.
    fn explosion_count(&self) -> usize {
        self.item_stack
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get_data_component::<FireworksImpl>()
            .map_or(0, |fireworks| fireworks.explosions.len())
    }

    /// Vanilla `explode`.
    pub fn explode_and_remove(&self, world: &World) {
        let entity = self.get_entity();
        if let Some(server) = world.server.upgrade() {
            let mut event =
                crate::plugin::api::events::entity::firework_explode::FireworkExplodeEvent {
                    entity_id: entity.entity_id,
                    cancelled: false,
                };
            server.plugin_manager.fire_blocking(&server, &mut event);
            if event.cancelled {
                return;
            }
        }
        world.send_entity_status(
            entity,
            EntityStatus::FireworksExplode,
            ActorEventID::FireworksExplode,
        );
        let owner = self.entity.owner.get(world);
        world.emit_game_event_with_context(
            GameEvent::Explode.name(),
            entity.pos.load(),
            &GameEventContext {
                source_entity: owner.as_deref(),
            },
        );
        self.deal_explosion_damage(world);

        entity.remove();
    }

    /// Vanilla `dealExplosionDamage`.
    fn deal_explosion_damage(&self, world: &World) {
        let explosion_count = self.explosion_count();
        if explosion_count == 0 {
            return;
        }

        let damage_amount = 5.0 + (explosion_count as f32) * 2.0;
        let entity = self.get_entity();
        let owner = self.entity.owner.get(world);
        let rocket_pos = entity.pos.load();

        if let Some(attached_id) = self.attached_to_entity
            && let Some(attached) = world.get_entity_by_id(attached_id)
        {
            attached.damage_with_context(
                attached.as_ref(),
                damage_amount,
                DamageType::FIREWORKS,
                Some(rocket_pos),
                Some(self),
                owner.as_deref(),
            );
        }

        let search_box = entity.bounding_box.load().expand_all(EXPLOSION_RADIUS);
        for target in world.get_all_at_box(&search_box) {
            let target_entity = target.get_entity();
            if Some(target_entity.entity_id) == self.attached_to_entity
                || target.get_living_entity().is_none()
            {
                continue;
            }
            let distance = rocket_pos
                .squared_distance_to_vec(&target_entity.pos.load())
                .sqrt();
            if distance > EXPLOSION_RADIUS || !Self::can_see(world, rocket_pos, target_entity) {
                continue;
            }
            let falloff = ((EXPLOSION_RADIUS - distance) / EXPLOSION_RADIUS).sqrt();
            let damage = damage_amount * falloff as f32;
            target.damage_with_context(
                target.as_ref(),
                damage,
                DamageType::FIREWORKS,
                Some(rocket_pos),
                Some(self),
                owner.as_deref(),
            );
        }
    }

    /// The sight check of vanilla `dealExplosionDamage`: the feet or the middle of the body.
    fn can_see(world: &World, from: Vector3<f64>, target: &Entity) -> bool {
        let feet = target.pos.load();
        let height = f64::from(target.entity_dimension.load().height);
        [0.0, 0.5].into_iter().any(|scale| {
            world
                .clip(&ClipContext::new(
                    from,
                    feet.add_raw(0.0, height * scale, 0.0),
                    ClipBlock::Collider,
                    ClipFluid::None,
                ))
                .is_miss
        })
    }

    /// The attached branch of vanilla's tick: the boost of an elytra flight. A player's boost
    /// is simulated on its client too, so its movement is not sent back.
    fn tick_attached(&self, attached_id: i32, world: &World) {
        let Some(attached) = world.get_entity_by_id(attached_id) else {
            return;
        };
        let entity = self.get_entity();
        let shooter = attached.get_entity();
        let mut hand_angle = Vector3::new(0.0, 0.0, 0.0);
        if shooter.is_fall_flying() {
            if !self.is_boost_cancelled(world) {
                let look = shooter.rotation().to_f64();
                let movement = shooter.velocity.load();
                shooter
                    .velocity
                    .store(movement + (look * 0.1 + (look * 1.5 - movement) * 0.5));
            }
            hand_angle = Self::hand_holding_rocket_angle(&*attached);
        }
        entity.set_pos(shooter.pos.load().add(&hand_angle));
        entity.velocity.store(shooter.velocity.load());
    }

    fn is_boost_cancelled(&self, world: &World) -> bool {
        let Some(player) = self.entity.owner.get_player(world) else {
            return false;
        };
        let Some(server) = world.server.upgrade() else {
            return false;
        };
        let mut event = PlayerElytraBoostEvent {
            player,
            firework_id: self.get_entity().entity_id,
            cancelled: false,
        };
        server.plugin_manager.fire_blocking(&server, &mut event);
        event.cancelled
    }

    /// Vanilla `Entity.getHandHoldingItemAngle`: half a block to the side of the rocket's hand.
    fn hand_holding_rocket_angle(attached: &dyn EntityBase) -> Vector3<f64> {
        let Some(player) = attached.get_player() else {
            return Vector3::new(0.0, 0.0, 0.0);
        };
        let inventory = player.inventory();
        let only_in_off_hand = inventory.off_hand_item().item.id == Item::FIREWORK_ROCKET.id
            && inventory.held_item().item.id != Item::FIREWORK_ROCKET.id;
        let main_hand_is_right = player.config.load().main_hand == Hand::Right;
        let side = if main_hand_is_right == only_in_off_hand {
            -80.0
        } else {
            80.0
        };
        Vector3::from_yaw_pitch(attached.get_entity().yaw.load() + side, 0.0)
            .multiply(0.5, 0.5, 0.5)
    }

    /// The free flight branch of vanilla's tick. The movement stays what it was whatever the
    /// move ran into.
    fn tick_free_flight(&self, caller: &dyn EntityBase) -> MoveHit {
        let entity = self.get_entity();
        if !self.shot_at_angle.load(Ordering::Relaxed) {
            let acceleration = if entity.horizontal_collision.load(Ordering::Relaxed) {
                1.0
            } else {
                1.15
            };
            let velocity = entity.velocity.load();
            entity.velocity.store(
                velocity
                    .multiply(acceleration, 1.0, acceleration)
                    .add_raw(0.0, 0.04, 0.0),
            );
        }
        let movement = entity.velocity.load();
        let hit = self.hit_on_move();
        entity.move_entity(caller, movement);
        entity.tick_block_collisions(caller);
        entity.velocity.store(movement);
        hit
    }

    fn hit_on_move(&self) -> MoveHit {
        let entity = self.get_entity();
        hit_result::hit_result_on_move_vector(entity, &|target| {
            hit_result::can_be_hit(target)
                && self
                    .entity
                    .owner
                    .can_hit(&entity.world.load(), target.get_entity())
        })
    }

    /// Vanilla `onHitBlock`: a rocket without explosions goes on.
    fn on_hit_block(&self, pos: BlockPos, hit_pos: Vector3<f64>) {
        let world = self.get_entity().world.load();
        let (block, state) = world.get_block_and_state(&pos);
        let Some(server) = world.server.upgrade() else {
            return;
        };
        world
            .block_registry
            .on_entity_collision(block, &world, self, &pos, state, &server);
        if self.explosion_count() > 0 {
            self.explode_and_remove(&world);
        }
        world
            .block_registry
            .on_projectile_hit(block, &world, self, &pos, state, &hit_pos, &server);
    }
}

impl EntityBase for FireworkRocketEntity {
    fn get_projectile_owner(&self) -> Option<&ProjectileOwner> {
        Some(&self.entity.owner)
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
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        entity.set_synced_data(
            pumpkin_data::tracked_data::firework_rocket::ID_FIREWORKS_ITEM,
            ItemStackSerializer::from(stack),
        );
        if let Some(attached_id) = self.attached_to_entity {
            entity.set_synced_data(
                pumpkin_data::tracked_data::firework_rocket::ATTACHED_TO_TARGET,
                OptionalInt(Some(attached_id)),
            );
        }
        if self.shot_at_angle.load(Ordering::Relaxed) {
            entity.set_synced_data(
                pumpkin_data::tracked_data::firework_rocket::SHOT_AT_ANGLE,
                true,
            );
        }
    }

    fn write_custom_nbt(&self, nbt: &mut NbtCompound) {
        nbt.put_bool("HasBeenShot", self.has_been_shot.load(Ordering::Relaxed));
        nbt.put_int("Life", self.life.load(Ordering::Relaxed) as i32);
        nbt.put_int("LifeTime", self.life_time.load(Ordering::Relaxed) as i32);
        let mut item = NbtCompound::new();
        self.item_stack
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .write_item_stack(&mut item);
        nbt.put_compound("FireworksItem", item);
        nbt.put_bool("ShotAtAngle", self.shot_at_angle.load(Ordering::Relaxed));
    }

    fn read_custom_nbt(&self, nbt: &NbtCompound) {
        if let Some(shot) = nbt.get_bool("HasBeenShot") {
            self.has_been_shot.store(shot, Ordering::Relaxed);
        }
        if let Some(life) = nbt.get_int("Life") {
            self.life.store(life.max(0) as u32, Ordering::Relaxed);
        }
        if let Some(life_time) = nbt.get_int("LifeTime") {
            self.life_time
                .store(life_time.max(0) as u32, Ordering::Relaxed);
        }
        if let Some(stack) = nbt
            .get_compound("FireworksItem")
            .and_then(ItemStack::read_item_stack)
        {
            *self
                .item_stack
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = stack;
        }
        if let Some(shot_at_angle) = nbt.get_bool("ShotAtAngle") {
            self.set_shot_at_angle(shot_at_angle);
        }
        self.init_data_tracker();
    }

    /// Vanilla `FireworkRocketEntity.tick`.
    fn tick(&self, caller: &dyn EntityBase, server: &Server) {
        let entity = self.get_entity();
        let world = entity.world.load();
        hit_result::emit_shoot_event(caller, &self.has_been_shot);
        self.entity.owner.check_left_owner(entity);
        entity.tick(caller, server);

        let hit = if let Some(attached_id) = self.attached_to_entity {
            self.tick_attached(attached_id, &world);
            self.hit_on_move()
        } else {
            self.tick_free_flight(caller)
        };
        if entity.is_alive()
            && !entity.no_physics.load(Ordering::Relaxed)
            && let MoveHit::Hit(hit) = hit
        {
            hit_result::hit_target_or_deflect_self(caller, hit);
            entity.velocity_dirty.store(true, Ordering::Relaxed);
        }

        hit_result::update_rotation(entity);
        let life = self.life.fetch_add(1, Ordering::Relaxed);
        if life == 0 && !entity.silent.load(Ordering::Relaxed) {
            world.play_sound_fine(
                Sound::EntityFireworkRocketLaunch,
                SoundCategory::Ambient,
                &entity.pos.load(),
                3.0,
                1.0,
            );
        }
        if life + 1 > self.life_time.load(Ordering::Relaxed) {
            self.explode_and_remove(&world);
        }
    }

    fn get_entity(&self) -> &crate::entity::Entity {
        &self.entity.entity
    }

    fn get_living_entity(&self) -> Option<&crate::entity::living::LivingEntity> {
        None
    }

    fn cast_any(&self) -> &dyn std::any::Any {
        self
    }

    /// Vanilla `onHitEntity` and `onHitBlock`.
    fn on_hit(&self, hit: ProjectileHit) {
        match hit {
            ProjectileHit::Entity { .. } => {
                let world = self.get_entity().world.load();
                self.explode_and_remove(&world);
            }
            ProjectileHit::Block { pos, hit_pos, .. } => self.on_hit_block(pos, hit_pos),
        }
    }
}
