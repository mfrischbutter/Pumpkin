//! Vanilla `AbstractBoat`: where the boat is, and how water, ground and bubbles move it.

use std::sync::atomic::Ordering;

use pumpkin_data::Block;
use pumpkin_data::entity::EntityStatus;
use pumpkin_data::fluid::Fluid;
use pumpkin_data::game_event::GameEvent;
use pumpkin_protocol::bedrock::server::actor_event::ActorEventID;
use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_util::math::boundingbox::BoundingBox;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::{Axis, Vector3};

use super::BoatEntity;
use crate::entity::{Entity, EntityBase};
use crate::world::World;

/// Vanilla `AbstractBoat.BUBBLE_TIME`.
const BUBBLE_TIME: i32 = 60;
/// How far the bottom of a boat dropping into water ends up above the surface's level,
/// from vanilla `AbstractBoat.floatBoat`.
const LANDING_CLEARANCE: f64 = 0.101;

/// Vanilla `AbstractBoat.Status`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum BoatStatus {
    InWater,
    UnderWater,
    UnderFlowingWater,
    OnLand,
    InAir,
}

fn floor(value: f64) -> i32 {
    value.floor() as i32
}

fn ceil(value: f64) -> i32 {
    value.ceil() as i32
}

/// The water at `pos`, if any: vanilla `FluidState.getHeight` and `isSource`.
fn water_at(world: &World, pos: &BlockPos) -> Option<(f32, bool)> {
    let (fluid, state) = world.get_fluid_and_fluid_state(pos);
    fluid
        .matches_type(&Fluid::WATER)
        .then(|| (world.get_fluid_height(pos, fluid, &state), state.is_source))
}

impl BoatEntity {
    /// The start of vanilla `AbstractBoat.tick`.
    pub(super) fn update_status(&self) {
        self.old_status.store(self.status.load());
        self.status.store(Some(self.compute_status()));
    }

    /// Vanilla `AbstractBoat.isUnderWater`.
    pub(super) fn is_under_water(&self) -> bool {
        matches!(
            self.status.load(),
            Some(BoatStatus::UnderWater | BoatStatus::UnderFlowingWater)
        )
    }

    /// Vanilla `AbstractBoat.getStatus`.
    pub(super) fn compute_status(&self) -> BoatStatus {
        if let Some(water_status) = self.under_water_status() {
            self.water_level
                .store(self.vehicle.entity.bounding_box.load().max.y);
            return water_status;
        }
        if self.check_in_water() {
            return BoatStatus::InWater;
        }
        let friction = self.ground_friction();
        if friction > 0.0 {
            self.land_friction.store(friction);
            BoatStatus::OnLand
        } else {
            BoatStatus::InAir
        }
    }

    /// Vanilla `AbstractBoat.isUnderwater`.
    fn under_water_status(&self) -> Option<BoatStatus> {
        let entity = &self.vehicle.entity;
        let world = entity.world.load();
        let aabb = entity.bounding_box.load();
        let max_y = aabb.max.y + 0.001;
        let mut under_water = false;

        for x in floor(aabb.min.x)..ceil(aabb.max.x) {
            for y in floor(aabb.max.y)..ceil(max_y) {
                for z in floor(aabb.min.z)..ceil(aabb.max.z) {
                    let pos = BlockPos::new(x, y, z);
                    if let Some((height, is_source)) = water_at(&world, &pos)
                        && max_y < f64::from(y) + f64::from(height)
                    {
                        if !is_source {
                            return Some(BoatStatus::UnderFlowingWater);
                        }
                        under_water = true;
                    }
                }
            }
        }

        under_water.then_some(BoatStatus::UnderWater)
    }

    /// Vanilla `AbstractBoat.checkInWater`: also records the water level under the boat.
    fn check_in_water(&self) -> bool {
        let entity = &self.vehicle.entity;
        let world = entity.world.load();
        let bb = entity.bounding_box.load();
        let mut in_water = false;
        let mut water_level = -f64::MAX;

        for x in floor(bb.min.x)..ceil(bb.max.x) {
            for y in floor(bb.min.y)..ceil(bb.min.y + 0.001) {
                for z in floor(bb.min.z)..ceil(bb.max.z) {
                    if let Some((height, _)) = water_at(&world, &BlockPos::new(x, y, z)) {
                        let level = f64::from(y as f32 + height);
                        water_level = water_level.max(level);
                        in_water |= bb.min.y < level;
                    }
                }
            }
        }

        self.water_level.store(water_level);
        in_water
    }

    /// Vanilla `AbstractBoat.getGroundFriction`: the mean friction of the blocks under the hull,
    /// lily pads not counting. No block means 0, where vanilla's division gives `NaN`.
    fn ground_friction(&self) -> f32 {
        let entity = &self.vehicle.entity;
        let world = entity.world.load();
        let bb = entity.bounding_box.load();
        let probe = BoundingBox::new(
            Vector3::new(bb.min.x, bb.min.y - 0.001, bb.min.z),
            Vector3::new(bb.max.x, bb.min.y, bb.max.z),
        );
        let (x0, x1) = (floor(probe.min.x) - 1, ceil(probe.max.x) + 1);
        let (y0, y1) = (floor(probe.min.y) - 1, ceil(probe.max.y) + 1);
        let (z0, z1) = (floor(probe.min.z) - 1, ceil(probe.max.z) + 1);
        let mut friction = 0.0f32;
        let mut count = 0u32;

        for x in x0..x1 {
            for z in z0..z1 {
                let edges = i32::from(x == x0 || x == x1 - 1) + i32::from(z == z0 || z == z1 - 1);
                if edges == 2 {
                    continue;
                }
                for y in y0..y1 {
                    if edges > 0 && (y == y0 || y == y1 - 1) {
                        continue;
                    }
                    let pos = BlockPos::new(x, y, z);
                    let (block, state) = world.get_block_and_state(&pos);
                    if block.id != Block::LILY_PAD.id
                        && state
                            .get_block_collision_shapes_at(&pos)
                            .any(|shape| shape.at_pos(pos).intersects(&probe))
                    {
                        friction += block.slipperiness;
                        count += 1;
                    }
                }
            }
        }

        if count == 0 {
            0.0
        } else {
            friction / count as f32
        }
    }

    /// Vanilla `AbstractBoat.getWaterLevelAbove`: the first water surface above the hull's top.
    fn water_level_above(&self) -> f32 {
        let entity = &self.vehicle.entity;
        let world = entity.world.load();
        let aabb = entity.bounding_box.load();
        let max_y = ceil(aabb.max.y - self.last_yd.load());

        'layers: for y in floor(aabb.max.y)..max_y {
            let mut block_height = 0.0f32;
            for x in floor(aabb.min.x)..ceil(aabb.max.x) {
                for z in floor(aabb.min.z)..ceil(aabb.max.z) {
                    if let Some((height, _)) = water_at(&world, &BlockPos::new(x, y, z)) {
                        block_height = block_height.max(height);
                    }
                    if block_height >= 1.0 {
                        continue 'layers;
                    }
                }
            }
            if block_height < 1.0 {
                return y as f32 + block_height;
            }
        }

        (max_y + 1) as f32
    }

    /// Vanilla `AbstractBoat.floatBoat`. The rule that halves the land friction under a player
    /// lives on the client: the server only floats a boat no player steers.
    pub(super) fn float_boat(&self) {
        let entity = &self.vehicle.entity;
        let status = self.status.load();

        if self.old_status.load() == Some(BoatStatus::InAir)
            && !matches!(status, Some(BoatStatus::InAir | BoatStatus::OnLand))
        {
            self.land_in_water();
            return;
        }

        let mut vspeed = -self.get_gravity();
        let mut buoyancy = 0.0;
        let height = f64::from(entity.height());
        let inv_friction = match status {
            Some(BoatStatus::InWater) => {
                buoyancy = (self.water_level.load() - entity.pos.load().y) / height;
                0.9f32
            }
            Some(BoatStatus::UnderFlowingWater) => {
                vspeed = -7.0E-4;
                0.9f32
            }
            Some(BoatStatus::UnderWater) => {
                buoyancy = f64::from(0.01f32);
                0.45f32
            }
            Some(BoatStatus::InAir) => 0.9f32,
            Some(BoatStatus::OnLand) => self.land_friction.load(),
            None => 0.05f32,
        };

        let movement = entity.velocity.load();
        let friction = f64::from(inv_friction);
        let mut moved = Vector3::new(
            movement.x * friction,
            movement.y + vspeed,
            movement.z * friction,
        );
        if buoyancy > 0.0 {
            moved.y = (moved.y + buoyancy * (super::DEFAULT_GRAVITY / 0.65)) * 0.75;
        }
        entity.velocity.store(moved);
    }

    /// The landing branch of vanilla `AbstractBoat.floatBoat`: a boat that falls into water
    /// settles on the surface at once.
    fn land_in_water(&self) {
        let entity = &self.vehicle.entity;
        let bb = entity.bounding_box.load();
        let pos = entity.pos.load();
        self.water_level.store(bb.max.y);

        let target_y = f64::from(self.water_level_above() - entity.height()) + LANDING_CLEARANCE;
        let landed = bb.shift(Vector3::new(0.0, target_y - pos.y, 0.0));
        let world = entity.world.load();
        if Self::no_collision(&world, Some(entity), landed) {
            entity.set_pos(Vector3::new(pos.x, target_y, pos.z));
            let velocity = entity.velocity.load();
            entity
                .velocity
                .store(Vector3::new(velocity.x, 0.0, velocity.z));
            self.last_yd.store(0.0);
        }
        self.status.store(Some(BoatStatus::InWater));
    }

    /// Vanilla `AbstractBoat.checkFallDamage`: a boat keeps count of its fall and is not hurt by
    /// it, and neither are its riders. `ya` is what this tick's move did to the height.
    pub(super) fn check_fall_damage(&self, ya: f64) {
        let entity = &self.vehicle.entity;
        if entity.has_vehicle() {
            return;
        }
        if entity.on_ground.load(Ordering::Relaxed) || entity.is_in_water() {
            self.fall_distance.store(0.0);
            return;
        }
        let below = entity.block_pos.load().down();
        let water_below = entity
            .world
            .load()
            .get_fluid(&below)
            .matches_type(&Fluid::WATER);
        if !water_below && ya < 0.0 {
            self.fall_distance.store(self.fall_distance.load() - ya);
        }
    }

    /// Vanilla `Entity.checkFallDistanceAccumulation`: a taut leash caught the fall.
    pub(super) fn check_fall_distance_accumulation(&self) {
        if self.vehicle.entity.velocity.load().y > -0.5 && self.fall_distance.load() > 1.0 {
            self.fall_distance.store(1.0);
        }
    }

    /// The boxes vanilla's `EntityGetter.getEntityCollisions` finds for a boat, which
    /// `AbstractBoat.canVehicleCollide` also stops at everything pushable. `own` is left out with
    /// what rides with it, and is `None` for a boat that is not in the world yet.
    fn entity_colliders(
        world: &World,
        own: Option<&Entity>,
        area: &BoundingBox,
    ) -> Vec<BoundingBox> {
        world
            .get_all_at_box(&area.expand_all(1.0E-7))
            .into_iter()
            .filter(|other| {
                let other_entity = other.get_entity();
                !other.is_spectator()
                    && (other.is_collidable(None) || other.is_pushable())
                    && own.is_none_or(|own| {
                        own.entity_id != other_entity.entity_id
                            && !own.is_passenger_of_same_vehicle(other_entity)
                    })
            })
            .map(|other| other.get_entity().bounding_box.load())
            .collect()
    }

    /// Vanilla `level.noCollision(boat, box)`.
    pub(crate) fn no_collision(world: &World, own: Option<&Entity>, area: BoundingBox) -> bool {
        world.is_space_empty(area) && Self::entity_colliders(world, own, &area).is_empty()
    }

    /// The entity half of vanilla `Entity.collide`, in its axis order. `Entity::move_entity` only
    /// knows blocks, so the boat shortens its move by the other boats and pushables first.
    pub(super) fn collide_with_entities(&self, motion: Vector3<f64>) -> Vector3<f64> {
        let entity = &self.vehicle.entity;
        let bb = entity.bounding_box.load();
        let world = entity.world.load();
        let colliders = Self::entity_colliders(&world, Some(entity), &bb.stretch(motion));
        if colliders.is_empty() {
            return motion;
        }

        let mut adjusted = motion;
        for axis in std::iter::once(Axis::Y).chain(Axis::horizontal()) {
            let mut max_time = 1.0;
            for collider in &colliders {
                if let Some(time) = bb.calculate_collision_time(collider, adjusted, axis, max_time)
                {
                    max_time = time;
                }
            }
            if max_time < 1.0 {
                adjusted.set_axis(axis, adjusted.get_axis(axis) * max_time);
            }
        }
        adjusted
    }

    /// Vanilla `AbstractBoat.onAboveBubbleColumn`. The splash sound and particle are the
    /// client's, which runs this too; only the game event is left for the server.
    pub(super) fn note_above_bubble_column(&self, drag_down: bool) {
        self.is_above_bubble_column.store(true, Ordering::Relaxed);
        self.bubble_column_direction_is_down
            .store(drag_down, Ordering::Relaxed);
        if self.bubble_time.load(Ordering::Relaxed) == 0 {
            self.set_bubble_time(BUBBLE_TIME);
        }
        if !self.is_under_water() && rand::random::<u32>().is_multiple_of(100) {
            let controller = self.get_controlling_passenger();
            self.emit_game_event(GameEvent::Splash, controller.as_deref());
        }
    }

    /// Vanilla `AbstractBoat.setBubbleTime`.
    fn set_bubble_time(&self, ticks: i32) {
        if self.bubble_time.swap(ticks, Ordering::Relaxed) != ticks {
            self.vehicle.entity.set_synced_data(
                pumpkin_data::tracked_data::boat::ID_BUBBLE_TIME,
                VarInt(ticks),
            );
        }
    }

    /// Vanilla `AbstractBoat.tickBubbleColumn`: three seconds over a column sink or launch it.
    pub(super) fn tick_bubble_column(&self) {
        if !self.is_above_bubble_column.load(Ordering::Relaxed) {
            self.set_bubble_time(0);
        }

        let bubble_time = self.bubble_time.load(Ordering::Relaxed);
        if bubble_time <= 0 {
            return;
        }
        let bubble_time = bubble_time - 1;
        self.set_bubble_time(bubble_time);
        if BUBBLE_TIME - bubble_time - 1 > 0 && bubble_time == 0 {
            let drag_down = self.bubble_column_direction_is_down.load(Ordering::Relaxed);
            let entity = &self.vehicle.entity;
            let world = entity.world.load();
            if drag_down {
                self.eject_passengers();
                world.send_entity_status(entity, EntityStatus::BoatSink, ActorEventID::None);
            } else {
                world.send_entity_status(entity, EntityStatus::BoatLaunch, ActorEventID::None);
            }
            self.handle_bubble_column_effect(drag_down);
        }
        self.is_above_bubble_column.store(false, Ordering::Relaxed);
    }

    /// Vanilla `AbstractBoat.handleBubbleColumnEffect`: a driver's client does this from the
    /// entity event.
    fn handle_bubble_column_effect(&self, drag_down: bool) {
        if self.is_client_authoritative() {
            return;
        }
        let entity = &self.vehicle.entity;
        let movement = entity.velocity.load();
        let y = if drag_down { movement.y - 0.7 } else { 0.6 };
        entity
            .velocity
            .store(Vector3::new(movement.x, y, movement.z));
    }
}
