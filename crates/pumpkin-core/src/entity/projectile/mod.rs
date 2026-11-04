use super::{Entity, EntityBase, living::LivingEntity};
use crate::plugin::api::events::entity::projectile_hit::ProjectileHitEvent;
use crate::world::World;
use crate::world::game_event::GameEventContext;
use pumpkin_data::BlockDirection;
use pumpkin_data::entity::EntityType;
use pumpkin_data::game_event::GameEvent;
use pumpkin_protocol::java::client::play::CEntityVelocity;
use pumpkin_util::math::boundingbox::BoundingBox;
use pumpkin_util::math::{position::BlockPos, vector3::Vector3};
use std::{
    sync::Arc,
    sync::atomic::{AtomicBool, Ordering},
};
pub mod arrow;
pub mod dragon_fireball;
pub mod egg;
pub mod ender_pearl;
pub mod evoker_fangs;
pub mod experience_bottle;
pub mod eye_of_ender;
pub mod fireball;
pub mod firework_rocket;
pub mod fishing_bobber;
pub mod hurting;
pub mod lingering_potion;
pub mod llama_spit;
pub mod owner;
pub mod projectile_item;
pub mod shulker_bullet;
pub mod small_fireball;
pub mod snowball;
pub mod splash_potion;
pub mod trident;
pub mod wind_charge;
pub mod wither_skull;

use pumpkin_data::item_stack::ItemStack;

#[must_use]
pub fn is_projectile(entity_type: &EntityType) -> bool {
    *entity_type == EntityType::ARROW
        || *entity_type == EntityType::TRIDENT
        || *entity_type == EntityType::EGG
        || *entity_type == EntityType::EXPERIENCE_BOTTLE
        || *entity_type == EntityType::SNOWBALL
        || *entity_type == EntityType::FIREWORK_ROCKET
        || *entity_type == EntityType::WIND_CHARGE
        || *entity_type == EntityType::SPLASH_POTION
        || *entity_type == EntityType::LINGERING_POTION
        || *entity_type == EntityType::ENDER_PEARL
        || *entity_type == EntityType::SHULKER_BULLET
        || *entity_type == EntityType::FIREBALL
        || *entity_type == EntityType::SMALL_FIREBALL
        || *entity_type == EntityType::FISHING_BOBBER
        || *entity_type == EntityType::WITHER_SKULL
        || *entity_type == EntityType::LLAMA_SPIT
}

/// Helper to apply projectile spawned enchantment effects matching vanilla `Projectile::applyOnProjectileSpawned`.
pub fn apply_on_projectile_spawned(
    projectile_entity: &Entity,
    pickup_item_stack: &ItemStack,
    weapon: Option<&ItemStack>,
    arrow: Option<&arrow::ArrowEntity>,
) {
    crate::enchantment::EnchantmentHelper::on_projectile_spawned(
        pickup_item_stack,
        projectile_entity,
        arrow,
    );
    if let Some(weapon) = weapon
        && weapon.item_count > 0
        && weapon.item.id != pickup_item_stack.item.id
    {
        crate::enchantment::EnchantmentHelper::on_projectile_spawned(
            weapon,
            projectile_entity,
            arrow,
        );
    }
}

pub struct ThrownItemEntity {
    pub entity: Entity,
    pub owner: owner::ProjectileOwner,
    pub collides_with_projectiles: bool,
    pub has_hit: AtomicBool,
    pub gravity: f64,
}

impl ThrownItemEntity {
    pub fn new(entity: Entity, owner: &Entity, gravity: f64) -> Self {
        let mut owner_pos = owner.pos.load();
        owner_pos.y += owner.get_eye_height() - 0.1;
        entity.pos.store(owner_pos);
        Self {
            entity,
            owner: owner::ProjectileOwner::new(Some(owner)),
            collides_with_projectiles: false,
            has_hit: AtomicBool::new(false),
            gravity,
        }
    }

    pub fn set_velocity_from(&self, pitch: f32, yaw: f32, roll: f32, speed: f32, divergence: f32) {
        let yaw_rad = yaw.to_radians();
        let pitch_rad = pitch.to_radians();
        let roll_rad = (pitch + roll).to_radians();

        let x = -yaw_rad.sin() * pitch_rad.cos();
        let y = -roll_rad.sin();
        let z = yaw_rad.cos() * pitch_rad.cos();

        self.set_velocity(
            f64::from(x),
            f64::from(y),
            f64::from(z),
            f64::from(speed),
            f64::from(divergence),
        );
    }

    pub fn set_velocity(&self, x: f64, y: f64, z: f64, power: f64, uncertainty: f64) {
        fn next_triangular(mode: f64, deviation: f64) -> f64 {
            deviation.mul_add(rand::random::<f64>() - rand::random::<f64>(), mode)
        }
        let velocity = Vector3::new(x, y, z)
            .normalize()
            .add_raw(
                next_triangular(0.0, 0.017_227_5 * uncertainty),
                next_triangular(0.0, 0.017_227_5 * uncertainty),
                next_triangular(0.0, 0.017_227_5 * uncertainty),
            )
            .multiply(power, power, power);

        self.entity.velocity.store(velocity);
        let len = velocity.horizontal_length();
        self.entity.set_rotation(
            velocity.x.atan2(velocity.z) as f32 * 57.295_776,
            velocity.y.atan2(len) as f32 * 57.295_776,
        );
    }
}

impl ThrownItemEntity {
    /// Process a tick for projectile movement and collisions
    pub fn process_tick(&self, caller: &dyn EntityBase) {
        let entity = self.get_entity();

        // Apply gravity and inertia
        let mut velocity = entity.velocity.load();
        velocity.y -= self.get_gravity();

        let inertia = if entity.touching_water.load(Ordering::Relaxed) {
            0.8
        } else {
            0.99
        };
        entity
            .velocity
            .store(velocity.multiply(inertia, inertia, inertia));

        self.move_and_collide(caller);
    }

    /// Vanilla `AbstractHurtingProjectile.tick`: speeds up along its heading instead of falling.
    pub fn process_hurting_tick(&self, caller: &dyn EntityBase, power: f64) {
        let entity = self.get_entity();

        let velocity = entity.velocity.load();
        let inertia = if entity.touching_water.load(Ordering::Relaxed) {
            fireball::WATER_INERTIA
        } else {
            fireball::AIR_INERTIA
        };
        let push = velocity.normalize().multiply(power, power, power);
        let velocity = velocity.add(&push).multiply(inertia, inertia, inertia);
        entity.velocity.store(velocity);

        self.move_and_collide(caller);
    }

    /// Moves by the velocity and handles the first block or entity hit on the way: fires the
    /// hit event, runs `on_hit` of `caller` and removes the projectile.
    pub(crate) fn move_and_collide(&self, caller: &dyn EntityBase) {
        let entity = self.get_entity();

        self.tick_projectile_state();
        entity.update_last_pos();

        let velocity = entity.velocity.load();
        let start_pos = entity.pos.load();

        // Update position
        entity.set_pos(start_pos.add(&velocity));
        self.broadcast_velocity();

        // Handle hit or continue
        if let Some(h) = self.find_hit(caller, start_pos, velocity) {
            self.hit_and_remove(caller, h);
        }
    }

    /// Vanilla `Projectile.tick`: the game event of the shot once, then `checkLeftOwner`.
    pub(crate) fn tick_projectile_state(&self) {
        let entity = self.get_entity();
        if self.owner.mark_shot() {
            let world = entity.world.load();
            let owner = self.owner.get(&world);
            world.emit_game_event_with_context(
                GameEvent::ProjectileShoot.name(),
                entity.pos.load(),
                &GameEventContext {
                    source_entity: owner.as_deref(),
                },
            );
        }
        self.owner.check_left_owner(entity);
    }

    /// Sends the velocity to clients.
    pub(crate) fn broadcast_velocity(&self) {
        let entity = self.get_entity();
        let packet = CEntityVelocity::new(entity.entity_id.into(), entity.velocity.load());
        entity
            .world
            .load()
            .broadcast_to_chunk(entity.chunk_pos.load(), &packet);
    }

    /// Vanilla `ProjectileUtil.getHitResultOnMoveVector`: the first block or entity on the way.
    pub(crate) fn find_hit(
        &self,
        caller: &dyn EntityBase,
        start_pos: Vector3<f64>,
        delta: Vector3<f64>,
    ) -> Option<ProjectileHit> {
        let entity = self.get_entity();
        let world = entity.world.load();
        let new_pos = start_pos.add(&delta);

        // Calculate search box for collisions
        let search_box = BoundingBox::new(
            Vector3::new(
                start_pos.x.min(new_pos.x),
                start_pos.y.min(new_pos.y),
                start_pos.z.min(new_pos.z),
            ),
            Vector3::new(
                start_pos.x.max(new_pos.x),
                start_pos.y.max(new_pos.y),
                start_pos.z.max(new_pos.z),
            ),
        )
        .expand(0.3, 0.3, 0.3);

        let mut closest_t = 1.0f64;
        let mut hit = None;

        // Block collisions
        let (block_cols, block_positions) = world.get_block_collisions(search_box, caller);
        for (idx, bb) in block_cols.iter().enumerate() {
            if let Some(t) = calculate_ray_intersection(&start_pos, &delta, bb)
                && t < closest_t
            {
                closest_t = t;
                // Map back to block pos
                for (len, pos) in &block_positions {
                    if idx < *len {
                        let hit_pos = start_pos.add(&delta.multiply(t, t, t));
                        hit = Some(ProjectileHit::Block {
                            pos: *pos,
                            face: get_hit_face(hit_pos, bb),
                            hit_pos,
                            normal: delta.normalize().multiply(-1.0, -1.0, -1.0),
                        });
                        break;
                    }
                }
            }
        }

        // Entity collisions
        let candidates = world
            .get_all_at_box(&search_box)
            .into_iter()
            .flat_map(|entity| crate::entity::boss::ender_dragon::projectile_targets(&entity));
        let margin = compute_margin(entity);
        for cand in candidates {
            if self.should_skip_collision(&world, entity, &cand)
                || !caller.projectile_can_hit(cand.as_ref())
            {
                continue;
            }

            let ebb = cand
                .get_entity()
                .bounding_box
                .load()
                .expand(margin, margin, margin);
            // Vanilla `AABB.clip` finds no way into a box from inside it.
            if contains(&ebb, &start_pos) {
                continue;
            }
            if let Some(t) = calculate_ray_intersection(&start_pos, &delta, &ebb)
                && t < closest_t
            {
                closest_t = t;
                let hit_pos = start_pos.add(&delta.multiply(t, t, t));
                hit = Some(ProjectileHit::Entity {
                    entity: cand.clone(),
                    hit_pos,
                    normal: delta.normalize().multiply(-1.0, -1.0, -1.0),
                });
            }
        }

        hit
    }

    /// Handles a hit that ends the projectile.
    pub(crate) fn hit_and_remove(&self, caller: &dyn EntityBase, hit: ProjectileHit) {
        // Ensure hit is only processed once per projectile
        if self.has_hit.load(Ordering::SeqCst) {
            return;
        }
        if self.hit_target_or_deflect_self(caller, hit) {
            self.has_hit.store(true, Ordering::SeqCst);
            self.get_entity().remove();
        }
    }

    /// Vanilla `Projectile.hitTargetOrDeflectSelf` and `onHit`. Returns whether `onHit` ran:
    /// not when the target deflected the projectile or a plugin cancelled the hit.
    pub(crate) fn hit_target_or_deflect_self(
        &self,
        caller: &dyn EntityBase,
        hit: ProjectileHit,
    ) -> bool {
        if deflected_by_target(caller, &hit) || !fire_projectile_hit_event(caller, &hit) {
            return false;
        }
        let world = self.get_entity().world.load();
        if let ProjectileHit::Block { pos, hit_pos, .. } = &hit {
            let block = world.get_block(pos);
            let state = world.get_block_state(pos);
            if let Some(server) = world.server.upgrade() {
                world
                    .block_registry
                    .on_projectile_hit(block, &world, caller, pos, state, hit_pos, &server);
            }
        }
        hurting::redirect_hit_projectile(caller, &hit);
        let landed_at = hit.hit_pos();
        caller.on_hit(hit);
        world.emit_game_event_with_context(
            GameEvent::ProjectileLand.name(),
            landed_at,
            &GameEventContext {
                source_entity: Some(caller),
            },
        );
        true
    }

    /// Returns if collision should be skipped (e.g. owner or projectile vs projectile)
    fn should_skip_collision(
        &self,
        world: &World,
        self_ent: &Entity,
        other: &Arc<dyn EntityBase>,
    ) -> bool {
        let other_ent = other.get_entity();
        if other_ent.entity_id == self_ent.entity_id {
            return true;
        }

        if other.is_spectator() {
            return true;
        }

        if !self.owner.can_hit(world, other_ent) {
            return true;
        }

        // Its own dragon's parts: they are no owner to the check above.
        if let Some(part) = other
            .cast_any()
            .downcast_ref::<crate::entity::boss::ender_dragon::part::EnderDragonPart>()
            && self
                .owner
                .get(world)
                .is_some_and(|owner| owner.get_entity().entity_uuid == part.dragon_uuid)
        {
            return true;
        }

        if !other.can_be_hit_by_projectile() {
            return true;
        }

        // Projectiles should pass through lingering clouds
        if *other_ent.entity_type == EntityType::AREA_EFFECT_CLOUD {
            return true;
        }

        // Projectile vs projectile logic
        if !self.collides_with_projectiles && !hurting::is_pickable(other.as_ref()) {
            return true;
        }

        false
    }

    const fn get_entity(&self) -> &Entity {
        &self.entity
    }

    #[allow(dead_code, clippy::unused_self)]
    const fn get_living_entity(&self) -> Option<&LivingEntity> {
        None
    }
    const fn get_gravity(&self) -> f64 {
        self.gravity
    }
}

/// Fires [`ProjectileHitEvent`] for a projectile about to process `hit`.
///
/// Returns `false` when a plugin cancelled it; the caller then skips the hit and the projectile
/// flies on.
#[must_use]
pub fn fire_projectile_hit_event(projectile: &dyn EntityBase, hit: &ProjectileHit) -> bool {
    let entity = projectile.get_entity();
    let world = entity.world.load_full();
    let Some(server) = world.server.upgrade() else {
        return true;
    };
    if !server.plugin_manager.has_handlers::<ProjectileHitEvent>() {
        return true;
    }

    let hit_entity_id = match hit {
        ProjectileHit::Block { .. } => None,
        ProjectileHit::Entity { entity: target, .. } => Some(target.get_entity().entity_id),
    };
    let mut event = ProjectileHitEvent::new(
        entity.entity_id,
        hit.hit_pos(),
        hit_entity_id,
        projectile.get_projectile_item(),
        world,
    );
    server.plugin_manager.fire_blocking(&server, &mut event);
    !event.cancelled
}

/// Ray intersection algorithm for AABBs, returning a t value
fn calculate_ray_intersection(
    start: &Vector3<f64>,
    dir: &Vector3<f64>,
    bb: &BoundingBox,
) -> Option<f64> {
    let mut t_min = 0.0f64;
    let mut t_max = 1.0f64;

    let b_min = [bb.min.x, bb.min.y, bb.min.z];
    let b_max = [bb.max.x, bb.max.y, bb.max.z];
    let s = [start.x, start.y, start.z];
    let d = [dir.x, dir.y, dir.z];

    for i in 0..3 {
        if d[i].abs() < 1e-9 {
            if s[i] < b_min[i] || s[i] > b_max[i] {
                return None;
            }
        } else {
            let t1 = (b_min[i] - s[i]) / d[i];
            let t2 = (b_max[i] - s[i]) / d[i];
            t_min = t_min.max(t1.min(t2));
            t_max = t_max.min(t1.max(t2));
        }
    }

    (t_min <= t_max && (0.0..=1.0).contains(&t_min)).then_some(t_min)
}

/// Vanilla `ProjectileUtil.computeMargin`: a young projectile hits only what it really touches.
fn compute_margin(entity: &Entity) -> f64 {
    let age = entity.tick_count.load(Ordering::Relaxed) - 2;
    f64::from((age as f32 / 20.0).clamp(0.0, 0.3))
}

/// Vanilla `AABB.contains`.
fn contains(bb: &BoundingBox, point: &Vector3<f64>) -> bool {
    (bb.min.x..bb.max.x).contains(&point.x)
        && (bb.min.y..bb.max.y).contains(&point.y)
        && (bb.min.z..bb.max.z).contains(&point.z)
}

/// Get the face of the collision box that was hit
fn get_hit_face(hit_pos: Vector3<f64>, bb: &BoundingBox) -> BlockDirection {
    let eps = 1.0e-4;

    if hit_pos.x <= bb.min.x + eps {
        BlockDirection::West
    } else if hit_pos.x >= bb.max.x - eps {
        BlockDirection::East
    } else if hit_pos.y <= bb.min.y + eps {
        BlockDirection::Down
    } else if hit_pos.y >= bb.max.y - eps {
        BlockDirection::Up
    } else if hit_pos.z <= bb.min.z + eps {
        BlockDirection::North
    } else {
        BlockDirection::South
    }
}

pub enum ProjectileHit {
    Block {
        pos: BlockPos,
        face: BlockDirection,
        hit_pos: Vector3<f64>,
        normal: Vector3<f64>,
    },
    Entity {
        entity: Arc<dyn EntityBase>,
        hit_pos: Vector3<f64>,
        normal: Vector3<f64>,
    },
}

impl ProjectileHit {
    /// Returns the exact impact coordinates regardless of what was hit.
    #[must_use]
    pub const fn hit_pos(&self) -> Vector3<f64> {
        match self {
            Self::Block { hit_pos, .. } | Self::Entity { hit_pos, .. } => *hit_pos,
        }
    }

    /// Returns the surface normal of the impact.
    #[must_use]
    pub const fn normal(&self) -> Vector3<f64> {
        match self {
            Self::Block { normal, .. } | Self::Entity { normal, .. } => *normal,
        }
    }

    /// Safely returns the face hit if it was a block, otherwise None.
    #[must_use]
    pub const fn face(&self) -> Option<BlockDirection> {
        match self {
            Self::Block { face, .. } => Some(*face),
            Self::Entity { .. } => None,
        }
    }
}

/// Vanilla `Projectile.hitTargetOrDeflectSelf` with `Breeze.deflection`: what hits a mob that
/// deflects projectiles turns around, a wind charge goes through to its target.
pub(crate) fn deflected_by_target(projectile: &dyn EntityBase, hit: &ProjectileHit) -> bool {
    use pumpkin_data::tag::{self, Taggable};
    let ProjectileHit::Entity { entity: target, .. } = hit else {
        return false;
    };
    let kind = projectile.get_entity().entity_type;
    let target = target.get_entity();
    if kind == &EntityType::WIND_CHARGE
        || kind == &EntityType::BREEZE_WIND_CHARGE
        || !target
            .entity_type
            .has_tag(&tag::EntityType::MINECRAFT_DEFLECTS_PROJECTILES)
    {
        return false;
    }
    // Vanilla `lastDeflectedBy`: once turned around it flies off through the box it is still in.
    let projectile_entity = projectile.get_entity();
    let to_target = target.pos.load() - projectile_entity.pos.load();
    let velocity = projectile_entity.velocity.load();
    if velocity.x * to_target.x + velocity.y * to_target.y + velocity.z * to_target.z <= 0.0 {
        return true;
    }
    target.world.load().play_sound(
        pumpkin_data::sound::Sound::EntityBreezeDeflect,
        pumpkin_data::sound::SoundCategory::Hostile,
        &target.pos.load(),
    );
    super::projectile_deflection::ProjectileDeflectionType::Simple.deflect(projectile, None);
    projectile.on_deflection(false);
    true
}
