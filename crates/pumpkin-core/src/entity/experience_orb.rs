use core::f32;
use std::sync::{
    Arc, Mutex, PoisonError, Weak,
    atomic::{AtomicI32, Ordering},
};

use pumpkin_data::{
    damage::DamageType, entity::EntityType, fluid::Fluid,
    tracked_data::experience_orb::VALUE as DATA_VALUE,
};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_util::math::{
    boundingbox::BoundingBox,
    vector3::{Axis, Vector3},
};

use crate::{
    block::blocks::bubble_column::SURFACE_UPWARD_ACCELERATION,
    server::Server,
    world::{
        World,
        entity_islands::{self, AuditedReach},
    },
};

use super::{Entity, EntityBase, living::LivingEntity, player::Player};
use rand::RngExt;

/// Vanilla `ExperienceOrb`.
pub struct ExperienceOrbEntity {
    entity: Entity,
    /// Vanilla `DATA_VALUE`: the experience of one orb, and the size clients draw.
    value: AtomicI32,
    /// Vanilla `count`: how many orbs of this value this one stands for after merging.
    count: AtomicI32,
    health: AtomicI32,
    orb_age: AtomicI32,
    /// Vanilla `followingPlayer`.
    following_player: Mutex<Option<Weak<Player>>>,
}

impl ExperienceOrbEntity {
    const AIR_DRAG: f32 = 0.98;
    const DEFAULT_HEALTH: i32 = 5;
    /// Vanilla `LIFETIME`.
    const LIFETIME: i32 = 6000;
    /// Vanilla `ENTITY_SCAN_PERIOD`.
    const ENTITY_SCAN_PERIOD: i32 = 20;
    /// Vanilla `MAX_FOLLOW_DIST`.
    const MAX_FOLLOW_DIST: f64 = 8.0;
    /// Vanilla `ORB_GROUPS_PER_AREA`.
    const ORB_GROUPS_PER_AREA: i32 = 40;
    /// Vanilla `ORB_MERGE_DISTANCE`.
    const ORB_MERGE_DISTANCE: f64 = 0.5;
    /// The largest pull of `followNearbyPlayer` per tick.
    const MAX_FOLLOW_ACCELERATION: f64 = 0.1;
    /// The speed of the bounce out of lava in vanilla `tick`.
    const LAVA_BOUNCE_SPEED: f32 = 0.2;
    /// Vanilla `setUnderwaterMovement`.
    const UNDERWATER_DRAG: f32 = 0.99;
    const UNDERWATER_RISE: f32 = 5.0e-4;
    const UNDERWATER_MAX_RISE: f32 = 0.06;

    pub const fn new(entity: Entity, value: u32) -> Self {
        Self {
            entity,
            value: AtomicI32::new(value as i32),
            count: AtomicI32::new(1),
            health: AtomicI32::new(Self::DEFAULT_HEALTH),
            orb_age: AtomicI32::new(0),
            following_player: Mutex::new(None),
        }
    }

    /// Vanilla `getValue`.
    #[must_use]
    pub fn value(&self) -> i32 {
        self.value.load(Ordering::Relaxed)
    }

    /// Vanilla `setValue`.
    fn set_value(&self, value: i32) {
        self.value.store(value, Ordering::Relaxed);
        self.entity.set_synced_data(DATA_VALUE, VarInt(value));
    }

    pub fn spawn(world: &Arc<World>, position: Vector3<f64>, amount: u32) {
        Self::spawn_with_direction(world, position, Vector3::new(0.0, 0.0, 0.0), amount);
    }

    fn new_with_direction(
        world: &Arc<World>,
        position: Vector3<f64>,
        direction: Vector3<f64>,
        amount: u32,
    ) -> Self {
        let entity = Entity::new(world.clone(), position, &EntityType::EXPERIENCE_ORB);
        let orb = Self::new(entity, amount);
        orb.entity.yaw.store(rand::random::<f32>() * 360.0);
        let mut velocity = Vector3::new(
            (rand::random::<f64>() * 0.2 - 0.1) * 2.0,
            rand::random::<f64>() * 0.2 * 2.0,
            (rand::random::<f64>() * 0.2 - 0.1) * 2.0,
        );
        if direction.length_squared() > 0.0 && direction.dot(&velocity) < 0.0 {
            velocity = velocity.multiply(-1.0, -1.0, -1.0);
        }
        let size = orb.entity.bounding_box.load().get_average_side_length();
        let offset = direction.normalize() * (size * 0.5);
        orb.entity.set_pos(position.add(&offset));
        orb.entity.set_velocity(velocity);
        if !world.is_space_empty(orb.entity.bounding_box.load()) {
            orb.unstuck_if_possible(size);
        }
        orb
    }

    fn unstuck_if_possible(&self, max_distance: f64) {
        let bounding_box = self.entity.bounding_box.load();
        let size = bounding_box.max - bounding_box.min;
        let center = (bounding_box.min + bounding_box.max) * 0.5;
        let half_distance = Vector3::new(max_distance, max_distance, max_distance) * 0.5;
        let allowed_centers = BoundingBox {
            min: center - half_distance,
            max: center + half_distance,
        };
        let world = self.entity.world.load();
        let (mut collisions, _) =
            world.get_block_collisions(allowed_centers.expand(size.x, size.y, size.z), self);
        {
            let border = world
                .worldborder
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let half = border.new_diameter * 0.5;
            let epsilon = f64::from(1.0e-5f32);
            collisions.retain(|shape| {
                shape.min.x >= border.center_x - half
                    && shape.max.x - epsilon < border.center_x + half
                    && shape.min.z >= border.center_z - half
                    && shape.max.z - epsilon < border.center_z + half
            });
        };
        let expanded_collisions = collisions
            .into_iter()
            .map(|shape| shape.expand(size.x * 0.5, size.y * 0.5, size.z * 0.5));
        if let Some(position) = closest_free_center(allowed_centers, center, expanded_collisions) {
            self.entity
                .set_pos(position.add_raw(0.0, -size.y * 0.5, 0.0));
        }
    }

    pub fn spawn_with_direction(
        world: &Arc<World>,
        position: Vector3<f64>,
        direction: Vector3<f64>,
        amount: u32,
    ) {
        let mut amount = amount;
        while amount > 0 {
            let i = Self::round_to_orb_size(amount);
            amount -= i;
            if !Self::try_merge_to_existing(world, position, i as i32) {
                let orb = Arc::new(Self::new_with_direction(world, position, direction, i));
                world.spawn_entity(orb);
            }
        }
    }

    /// Vanilla `tryMergeToExisting`: a new orb only adds to the count of one nearby.
    fn try_merge_to_existing(world: &World, position: Vector3<f64>, value: i32) -> bool {
        let area = BoundingBox::of_size(position, 1.0, 1.0, 1.0);
        let id = rand::rng().random_range(0..Self::ORB_GROUPS_PER_AREA);
        for candidate in world.get_entities_at_box(&area) {
            if let Some(orb) = candidate.cast_any().downcast_ref::<Self>()
                && orb.can_merge(id, value)
            {
                orb.count.fetch_add(1, Ordering::Relaxed);
                orb.orb_age.store(0, Ordering::Relaxed);
                return true;
            }
        }
        false
    }

    /// Vanilla `canMerge(orb, id, value)`, on the orb that would be merged.
    fn can_merge(&self, id: i32, value: i32) -> bool {
        !self.entity.is_removed()
            && self.entity.entity_id.wrapping_sub(id) % Self::ORB_GROUPS_PER_AREA == 0
            && self.value() == value
    }

    /// Vanilla `scanForMerges`.
    fn scan_for_merges(&self, world: &World) {
        let area = self
            .entity
            .bounding_box
            .load()
            .expand_all(Self::ORB_MERGE_DISTANCE);
        for candidate in world.get_entities_at_box(&area) {
            if let Some(orb) = candidate.cast_any().downcast_ref::<Self>()
                && orb.entity.entity_id != self.entity.entity_id
                && orb.can_merge(self.entity.entity_id, self.value())
            {
                // Vanilla `merge`.
                self.count
                    .fetch_add(orb.count.load(Ordering::Relaxed), Ordering::Relaxed);
                self.orb_age
                    .fetch_min(orb.orb_age.load(Ordering::Relaxed), Ordering::Relaxed);
                orb.entity.remove();
            }
        }
    }

    /// Vanilla `followNearbyPlayer`. False when no player is being followed.
    fn follow_nearby_player(&self, world: &World) -> bool {
        let entity = &self.entity;
        let pos = entity.pos.load();
        let mut following_player = self
            .following_player
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let mut following = following_player.as_ref().and_then(Weak::upgrade);
        if following.as_ref().is_none_or(|player| {
            player.is_spectator()
                || player.position().squared_distance_to_vec(&pos)
                    > Self::MAX_FOLLOW_DIST * Self::MAX_FOLLOW_DIST
        }) {
            following = world
                .get_nearest_player(pos, Self::MAX_FOLLOW_DIST, |player| !player.is_spectator())
                .filter(|player| !player.living_entity.is_dead_or_dying());
            *following_player = following.as_ref().map(Arc::downgrade);
        }
        drop(following_player);

        let Some(player) = following else {
            return false;
        };
        let target = player.position();
        let delta = Vector3::new(
            target.x - pos.x,
            target.y + player.get_entity().get_eye_height() / 2.0 - pos.y,
            target.z - pos.z,
        );
        let power = 1.0 - delta.length() / Self::MAX_FOLLOW_DIST;
        let pull = delta.normalize() * (power * power * Self::MAX_FOLLOW_ACCELERATION);
        entity.velocity.store(entity.velocity.load() + pull);
        true
    }

    /// The steps of vanilla `tick` before `followNearbyPlayer` that set the velocity.
    fn update_velocity(&self, world: &World, colliding: bool) {
        let entity = &self.entity;
        if entity.is_submerged_in_water() {
            // Vanilla `setUnderwaterMovement`.
            let velocity = entity.velocity.load();
            let drag = f64::from(Self::UNDERWATER_DRAG);
            entity.velocity.store(Vector3::new(
                velocity.x * drag,
                (velocity.y + f64::from(Self::UNDERWATER_RISE))
                    .min(f64::from(Self::UNDERWATER_MAX_RISE)),
                velocity.z * drag,
            ));
        } else if !colliding {
            let mut velocity = entity.velocity.load();
            velocity.y -= self.get_gravity();
            entity.velocity.store(velocity);
        }

        if world
            .get_fluid(&entity.block_pos.load())
            .matches_type(&Fluid::LAVA)
        {
            let bounce = || {
                f64::from((rand::random::<f32>() - rand::random::<f32>()) * Self::LAVA_BOUNCE_SPEED)
            };
            entity.velocity.store(Vector3::new(
                bounce(),
                f64::from(Self::LAVA_BOUNCE_SPEED),
                bounce(),
            ));
        }
    }

    const fn round_to_orb_size(value: u32) -> u32 {
        if value >= 2477 {
            2477
        } else if value >= 1237 {
            1237
        } else if value >= 617 {
            617
        } else if value >= 307 {
            307
        } else if value >= 149 {
            149
        } else if value >= 73 {
            73
        } else if value >= 37 {
            37
        } else if value >= 17 {
            17
        } else if value >= 7 {
            7
        } else if value >= 3 {
            3
        } else {
            1
        }
    }
}

fn closest_free_center(
    allowed_centers: BoundingBox,
    center: Vector3<f64>,
    collisions: impl Iterator<Item = BoundingBox>,
) -> Option<Vector3<f64>> {
    // Subtract expanded collision boxes, matching vanilla's ONLY_FIRST shape operation.
    let mut free_boxes = vec![allowed_centers];
    for collision in collisions {
        let mut remaining = Vec::new();
        for mut free in free_boxes {
            if !free.intersects(&collision) {
                remaining.push(free);
                continue;
            }
            for axis in [Axis::X, Axis::Y, Axis::Z] {
                if free.min.get_axis(axis) < collision.min.get_axis(axis) {
                    let mut piece = free;
                    piece.max.set_axis(axis, collision.min.get_axis(axis));
                    remaining.push(piece);
                    free.min.set_axis(axis, collision.min.get_axis(axis));
                }
                if free.max.get_axis(axis) > collision.max.get_axis(axis) {
                    let mut piece = free;
                    piece.min.set_axis(axis, collision.max.get_axis(axis));
                    remaining.push(piece);
                    free.max.set_axis(axis, collision.max.get_axis(axis));
                }
            }
        }
        free_boxes = remaining;
    }
    free_boxes
        .into_iter()
        .map(|free| {
            Vector3::new(
                center.x.clamp(free.min.x, free.max.x),
                center.y.clamp(free.min.y, free.max.y),
                center.z.clamp(free.min.z, free.max.z),
            )
        })
        .min_by(|a, b| {
            a.squared_distance_to_vec(&center)
                .total_cmp(&b.squared_distance_to_vec(&center))
        })
}

impl EntityBase for ExperienceOrbEntity {
    /// Vanilla `ExperienceOrb.doWaterSplashEffect` does nothing.
    fn do_water_splash_effect(&self, _caller: &dyn EntityBase) {}

    fn init_data_tracker(&self) {
        self.entity
            .set_synced_data(DATA_VALUE, VarInt(self.value()));
    }

    fn audited_reach(&self) -> Option<AuditedReach> {
        let entity = &self.entity;
        let world = entity.world.load();
        // What the tick can add to the velocity before moving.
        let pushes = super::max_fluid_push_speed(world.dimension.fast_lava)
            + super::MAX_PUSH_OUT_SPEED
            + Self::MAX_FOLLOW_ACCELERATION
            + f64::from(Self::LAVA_BOUNCE_SPEED);
        let velocity = entity.velocity.load();
        let mut reach = AuditedReach {
            bounding_box: entity.bounding_box.load(),
            motion: (
                velocity.x.hypot(velocity.z) + pushes,
                velocity.y.abs()
                    + self.get_gravity()
                    + SURFACE_UPWARD_ACCELERATION
                    + f64::from(Self::UNDERWATER_RISE)
                    + pushes,
            ),
            entities: Self::ORB_MERGE_DISTANCE,
            players: Self::MAX_FOLLOW_DIST,
            block_reads: 1.0,
            route: None,
            changes_blocks: false,
            global: false,
            links: entity_islands::links_of(self),
        };
        reach.scan_contact_blocks(&world);
        Some(reach)
    }

    /// Vanilla `addAdditionalSaveData`.
    fn write_custom_nbt(&self, nbt: &mut NbtCompound) {
        nbt.put_short("Health", self.health.load(Ordering::Relaxed) as i16);
        nbt.put_short("Age", self.orb_age.load(Ordering::Relaxed) as i16);
        nbt.put_short("Value", self.value() as i16);
        nbt.put_int("Count", self.count.load(Ordering::Relaxed));
    }

    /// Vanilla `readAdditionalSaveData`. Missing keys keep their value, a fresh orb holds the
    /// defaults; a `Count` below 1 is invalid in vanilla's codec and reads as 1.
    fn read_custom_nbt(&self, nbt: &NbtCompound) {
        if let Some(health) = nbt.get_numeric_short("Health") {
            self.health.store(i32::from(health), Ordering::Relaxed);
        }
        if let Some(age) = nbt.get_numeric_short("Age") {
            self.orb_age.store(i32::from(age), Ordering::Relaxed);
        }
        if let Some(value) = nbt.get_numeric_short("Value") {
            self.set_value(i32::from(value));
        }
        if let Some(count) = nbt.get_int("Count") {
            self.count.store(count.max(1), Ordering::Relaxed);
        }
    }

    /// Vanilla `ExperienceOrb.tick`.
    fn tick(&self, caller: &dyn EntityBase, server: &Server) {
        let entity = &self.entity;
        entity.tick(caller, server);
        if entity.is_removed() {
            return;
        }
        let world = entity.world.load_full();

        let colliding = !world.is_space_empty(entity.bounding_box.load());
        self.update_velocity(&world, colliding);

        if entity.tick_count.load(Ordering::Relaxed) % Self::ENTITY_SCAN_PERIOD == 1 {
            self.scan_for_merges(&world);
        }

        let following = self.follow_nearby_player(&world);
        if !following && colliding {
            let bounding_box = entity.bounding_box.load();
            if !world.is_space_empty(bounding_box.shift(entity.velocity.load())) {
                let pos = entity.pos.load();
                entity.push_out_of_blocks(Vector3::new(
                    pos.x,
                    f64::midpoint(bounding_box.min.y, bounding_box.max.y),
                    pos.z,
                ));
                entity.velocity_dirty.store(true, Ordering::SeqCst);
            }
        }

        let velocity = entity.velocity.load();
        let fall_speed = velocity.y;
        entity.move_entity(caller, velocity);
        entity.tick_block_collisions(caller);

        let mut friction = Self::AIR_DRAG;
        if entity.on_ground.load(Ordering::Relaxed) {
            let pos = entity.get_pos_with_y_offset(f64::from(0.999_999f32)).0;
            friction *= world.get_block(&pos).slipperiness;
        }
        let mut velocity = entity.velocity.load() * f64::from(friction);
        // Vanilla `verticalCollisionBelow`: a fall speed this large can only have hit the floor.
        if entity.vertical_collision.load(Ordering::Relaxed) && fall_speed < -self.get_gravity() {
            velocity.y = -fall_speed * 0.4;
        }
        entity.velocity.store(velocity);

        if self.orb_age.fetch_add(1, Ordering::Relaxed) + 1 >= Self::LIFETIME {
            entity.remove();
        }
    }

    fn get_entity(&self) -> &Entity {
        &self.entity
    }

    fn damage_with_context(
        &self,
        _caller: &dyn EntityBase,
        amount: f32,
        damage_type: DamageType,
        _position: Option<Vector3<f64>>,
        _source: Option<&dyn EntityBase>,
        cause: Option<&dyn EntityBase>,
    ) -> bool {
        if self.entity.is_invulnerable_to(&damage_type, cause) {
            return false;
        }

        let mut event = crate::plugin::api::events::entity::entity_damage::EntityDamageEvent::new(
            self.entity.entity_id,
            damage_type,
            amount,
        );
        if let Some(server) = self.entity.world.load().server.upgrade() {
            server.plugin_manager.fire_blocking(&server, &mut event);
        }
        if event.cancelled {
            return false;
        }
        if !event.damage.is_finite() || event.damage <= 0.0 {
            return false;
        }

        self.entity.velocity_dirty.store(true, Ordering::Relaxed);
        let mut health = self.health.load(Ordering::Relaxed);
        loop {
            let remaining = (health as f32 - event.damage) as i32;
            match self.health.compare_exchange_weak(
                health,
                remaining,
                Ordering::AcqRel,
                Ordering::Relaxed,
            ) {
                Ok(_) => {
                    if remaining <= 0 {
                        self.entity.remove();
                    }
                    return true;
                }
                Err(current) => health = current,
            }
        }
    }

    /// Vanilla `playerTouch`: one of the merged orbs per touch, repairing mending items first.
    fn on_player_collision(&self, player: &Arc<Player>) {
        if player.living_entity.health.load() > 0.0 && !player.is_spectator() {
            let can_pickup = if let Ok(mut delay) = player.experience_pick_up_delay.try_lock()
                && *delay == 0
            {
                *delay = 2;
                true
            } else {
                false
            };
            if can_pickup && player.living_entity.pickup(&self.entity, None, 1) {
                let remaining = player.apply_mending_from_xp(self.value());
                if remaining > 0 {
                    player.add_experience_points(remaining);
                }
                if self.count.fetch_sub(1, Ordering::Relaxed) <= 1 {
                    self.entity.remove();
                }
            }
        }
    }

    fn get_living_entity(&self) -> Option<&LivingEntity> {
        None
    }
    fn get_gravity(&self) -> f64 {
        if self.entity.has_no_gravity() {
            0.0
        } else {
            0.03
        }
    }

    fn cast_any(&self) -> &dyn std::any::Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unstuck_selects_nearest_free_center_and_leaves_enclosed_orbs_in_place() {
        let allowed = BoundingBox {
            min: Vector3::new(0.0, 0.0, 0.0),
            max: Vector3::new(1.0, 1.0, 1.0),
        };
        let center = Vector3::new(0.5, 0.5, 0.5);
        let wall = BoundingBox {
            min: Vector3::new(-1.0, -1.0, -1.0),
            max: Vector3::new(0.75, 2.0, 2.0),
        };
        let floor = BoundingBox {
            min: Vector3::new(-1.0, -1.0, -1.0),
            max: Vector3::new(2.0, 0.625, 2.0),
        };
        assert_eq!(
            closest_free_center(allowed, center, [wall, floor].into_iter()),
            Some(Vector3::new(0.75, 0.625, 0.5))
        );
        assert_eq!(
            closest_free_center(allowed, center, [allowed].into_iter()),
            None
        );
    }
}
