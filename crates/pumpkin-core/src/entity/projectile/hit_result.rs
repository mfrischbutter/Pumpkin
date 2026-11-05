//! Vanilla `ProjectileUtil` and the hit steps of `Projectile`, for the projectiles that follow
//! them: the arrows, the trident, the fishing hook and the firework rocket.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use pumpkin_data::BlockDirection;
use pumpkin_data::entity::EntityType;
use pumpkin_data::game_event::GameEvent;
use pumpkin_data::tag::{self, Taggable};
use pumpkin_util::math::boundingbox::BoundingBox;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_world::clip::{
    BlockHitResult, ClipBlock, ClipContext, ClipFluid, intersects_aabb_with_hit,
};

use super::{ProjectileHit, deflected_by_target, fire_projectile_hit_event};
use crate::entity::boss::ender_dragon::projectile_targets;
use crate::entity::decoration::armor_stand::ArmorStandEntity;
use crate::entity::projectile_deflection::ProjectileDeflectionType;
use crate::entity::{Entity, EntityBase};
use crate::world::World;
use crate::world::game_event::GameEventContext;

/// Vanilla `ProjectileUtil.DEFAULT_ENTITY_HIT_RESULT_MARGIN`.
const DEFAULT_ENTITY_HIT_RESULT_MARGIN: f32 = 0.3;
/// The entities without a living body that vanilla's `isPickable` lets projectiles hit, besides
/// those answering [`EntityBase::can_hit`].
const PICKABLE_TYPES: [&EntityType; 8] = [
    &EntityType::END_CRYSTAL,
    &EntityType::FALLING_BLOCK,
    &EntityType::GLOW_ITEM_FRAME,
    &EntityType::ITEM_FRAME,
    &EntityType::LEASH_KNOT,
    &EntityType::PAINTING,
    &EntityType::SHULKER_BULLET,
    &EntityType::TNT,
];

/// An entity a projectile's path goes through: vanilla `EntityHitResult`.
pub struct EntityHit {
    pub(crate) entity: Arc<dyn EntityBase>,
    pub(crate) location: Vector3<f64>,
}

impl EntityHit {
    pub(crate) fn into_projectile_hit(self, velocity: Vector3<f64>) -> ProjectileHit {
        ProjectileHit::Entity {
            entity: self.entity,
            hit_pos: self.location,
            normal: hit_normal(velocity),
        }
    }
}

/// Vanilla `Level.clipIncludingBorder`.
pub struct ClipResult {
    pub(crate) hit: BlockHitResult,
    /// Vanilla `BlockHitResult.isWorldBorderHit`.
    pub(crate) world_border: bool,
}

impl ClipResult {
    pub(crate) fn to_projectile_hit(&self, velocity: Vector3<f64>) -> ProjectileHit {
        ProjectileHit::Block {
            pos: self.hit.block_pos,
            face: self.hit.direction,
            hit_pos: self.hit.location,
            normal: hit_normal(velocity),
        }
    }
}

/// What vanilla `ProjectileUtil.getHitResultOnMoveVector` found along the movement.
pub enum MoveHit {
    Miss,
    WorldBorder,
    Hit(ProjectileHit),
}

/// How vanilla `Projectile.hitTargetOrDeflectSelf` went.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum HitOutcome {
    Hit,
    Deflected,
    /// A plugin cancelled the hit, which vanilla does not know: the projectile flies on.
    Cancelled,
}

fn hit_normal(velocity: Vector3<f64>) -> Vector3<f64> {
    velocity.normalize().multiply(-1.0, -1.0, -1.0)
}

/// Vanilla `ProjectileUtil.computeMargin`: young projectiles need a closer hit.
pub fn compute_margin(source: &Entity) -> f64 {
    let age = source.tick_count.load(Ordering::Relaxed) - 2;
    f64::from((age as f32 / 20.0).clamp(0.0, DEFAULT_ENTITY_HIT_RESULT_MARGIN))
}

/// Vanilla `Projectile.lerpRotation`.
pub fn lerp_rotation(mut previous: f32, target: f32) -> f32 {
    while target - previous < -180.0 {
        previous -= 360.0;
    }
    while target - previous >= 180.0 {
        previous += 360.0;
    }
    0.2f32.mul_add(target - previous, previous)
}

/// Vanilla `Projectile.updateRotation`.
pub fn update_rotation(entity: &Entity) {
    let movement = entity.velocity.load();
    let yaw = (movement.x.atan2(movement.z) as f32).to_degrees();
    let pitch = (movement.y.atan2(movement.horizontal_length()) as f32).to_degrees();
    entity.set_rotation(
        lerp_rotation(entity.yaw.load(), yaw),
        lerp_rotation(entity.pitch.load(), pitch),
    );
}

/// The first steps of vanilla `Projectile.tick`: the shot's game event, once.
pub fn emit_shoot_event(projectile: &dyn EntityBase, has_been_shot: &AtomicBool) {
    if has_been_shot.swap(true, Ordering::Relaxed) {
        return;
    }
    let entity = projectile.get_entity();
    let owner = projectile.get_owner_entity();
    entity.world.load().emit_game_event_with_context(
        GameEvent::ProjectileShoot.name(),
        entity.pos.load(),
        &GameEventContext {
            source_entity: owner.as_deref(),
        },
    );
}

/// Vanilla `Level.clipIncludingBorder` with `Block.COLLIDER` and `Fluid.NONE`.
pub fn clip_including_border(world: &World, from: Vector3<f64>, to: Vector3<f64>) -> ClipResult {
    let hit = world.clip(&ClipContext::new(
        from,
        to,
        ClipBlock::Collider,
        ClipFluid::None,
    ));
    let clamped = {
        let border = world
            .worldborder
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let half = border.new_diameter / 2.0;
        (border.contains(from.x, from.z) && !border.contains(hit.location.x, hit.location.z)).then(
            || {
                Vector3::new(
                    hit.location
                        .x
                        .clamp(border.center_x - half, border.center_x + half - 1.0e-5),
                    hit.location.y,
                    hit.location
                        .z
                        .clamp(border.center_z - half, border.center_z + half - 1.0e-5),
                )
            },
        )
    };
    clamped.map_or(
        ClipResult {
            hit,
            world_border: false,
        },
        |location| {
            let delta = location.sub(&from);
            let direction = BlockDirection::get_nearest(delta.x, delta.y, delta.z);
            ClipResult {
                hit: BlockHitResult::hit(location, direction, BlockPos::floored_v(location), true),
                world_border: true,
            }
        },
    )
}

/// Vanilla `AABB.clip`: where the segment enters the box, nothing when it starts inside.
fn clip_box(
    bounding_box: &BoundingBox,
    from: Vector3<f64>,
    to: Vector3<f64>,
) -> Option<Vector3<f64>> {
    match intersects_aabb_with_hit(from, to, bounding_box.min, bounding_box.max) {
        Some((_, _, location, false)) => Some(location),
        _ => None,
    }
}

/// Vanilla `getBoundingBox().expandTowards(getDeltaMovement()).inflate(1.0)`.
pub fn search_area(source: &Entity) -> BoundingBox {
    let velocity = source.velocity.load();
    source
        .bounding_box
        .load()
        .expand_towards(velocity.x, velocity.y, velocity.z)
        .expand_all(1.0)
}

/// `level.getEntities(source, area, matching)` with a dragon standing in for its parts, since
/// vanilla's dragon is not pickable and its parts are.
fn candidates(
    world: &World,
    source: &Entity,
    area: &BoundingBox,
    can_hit: &dyn Fn(&dyn EntityBase) -> bool,
) -> Vec<Arc<dyn EntityBase>> {
    world
        .get_all_at_box(area)
        .iter()
        .flat_map(projectile_targets)
        .filter(|target| target.get_entity().entity_id != source.entity_id && can_hit(&**target))
        .collect()
}

/// Vanilla `ProjectileUtil.getEntityHitResult`: the nearest entity along the segment.
pub fn nearest_entity_hit(
    world: &World,
    source: &Entity,
    (from, to): (Vector3<f64>, Vector3<f64>),
    area: &BoundingBox,
    margin: f64,
    can_hit: &dyn Fn(&dyn EntityBase) -> bool,
) -> Option<EntityHit> {
    let mut nearest = f64::MAX;
    let mut hit = None;
    for target in candidates(world, source, area, can_hit) {
        let inflated = target.get_entity().bounding_box.load().expand_all(margin);
        if let Some(location) = clip_box(&inflated, from, to) {
            let distance = from.squared_distance_to_vec(&location);
            if distance < nearest {
                nearest = distance;
                hit = Some(EntityHit {
                    entity: target,
                    location,
                });
            }
        }
    }
    hit
}

/// Vanilla `ProjectileUtil.getManyEntityHitResult` without the from-entity and surface options:
/// every entity along the segment, those only in the margin when no block is in between.
pub fn many_entity_hits(
    world: &World,
    source: &Entity,
    (from, to): (Vector3<f64>, Vector3<f64>),
    area: &BoundingBox,
    margin: f64,
    can_hit: &dyn Fn(&dyn EntityBase) -> bool,
) -> Vec<EntityHit> {
    let mut hits = Vec::new();
    for target in candidates(world, source, area, can_hit) {
        let bounding_box = target.get_entity().bounding_box.load();
        if let Some(location) = clip_box(&bounding_box, from, to) {
            hits.push(EntityHit {
                entity: target,
                location,
            });
        } else if margin > 0.0
            && let Some(outside) = clip_box(&bounding_box.expand_all(margin), from, to)
        {
            let center = bounding_box.min.lerp(&bounding_box.max, 0.5);
            let towards = clip_including_border(world, outside, center).hit.location;
            if clip_box(&bounding_box, outside, towards).is_some() {
                hits.push(EntityHit {
                    entity: target,
                    location: outside,
                });
            }
        }
    }
    hits
}

/// Vanilla `ProjectileUtil.getHitResultOnMoveVector`.
pub fn hit_result_on_move_vector(
    source: &Entity,
    can_hit: &dyn Fn(&dyn EntityBase) -> bool,
) -> MoveHit {
    let velocity = source.velocity.load();
    let world = source.world.load();
    let from = source.pos.load();
    let clip = clip_including_border(&world, from, from.add(&velocity));
    if let Some(hit) = nearest_entity_hit(
        &world,
        source,
        (from, clip.hit.location),
        &search_area(source),
        compute_margin(source),
        can_hit,
    ) {
        return MoveHit::Hit(hit.into_projectile_hit(velocity));
    }
    if clip.hit.is_miss {
        MoveHit::Miss
    } else if clip.world_border {
        MoveHit::WorldBorder
    } else {
        MoveHit::Hit(clip.to_projectile_hit(velocity))
    }
}

/// Vanilla `Entity.canBeHitByProjectile`: alive and `isPickable`, which the engine splits over
/// [`EntityBase::can_be_hit_by_projectile`], [`EntityBase::can_hit`] and the living check.
pub fn can_be_hit(target: &dyn EntityBase) -> bool {
    if !target.can_be_hit_by_projectile() {
        return false;
    }
    if let Some(living) = target.get_living_entity() {
        return living.is_alive()
            && target
                .cast_any()
                .downcast_ref::<ArmorStandEntity>()
                .is_none_or(|stand| !stand.is_marker());
    }
    let entity = target.get_entity();
    entity.is_alive()
        && (target.can_hit()
            || PICKABLE_TYPES.contains(&entity.entity_type)
            || entity
                .entity_type
                .has_tag(&tag::EntityType::MINECRAFT_REDIRECTABLE_PROJECTILE))
}

/// Vanilla `Projectile.hitTargetOrDeflectSelf` and `onHit`, the world border aside.
pub fn hit_target_or_deflect_self(projectile: &dyn EntityBase, hit: ProjectileHit) -> HitOutcome {
    if deflected_by_target(projectile, &hit) {
        return HitOutcome::Deflected;
    }
    let world = projectile.get_entity().world.load();
    let land_position = match &hit {
        ProjectileHit::Block { pos, .. } => {
            if world.get_block_state(pos).is_air() {
                return HitOutcome::Hit;
            }
            pos.0.to_f64().add_raw(0.5, 0.5, 0.5)
        }
        ProjectileHit::Entity { hit_pos, .. } => *hit_pos,
    };
    if !fire_projectile_hit_event(projectile, &hit) {
        return HitOutcome::Cancelled;
    }
    if let ProjectileHit::Entity { entity: target, .. } = &hit {
        redirect_projectile(projectile, &**target);
    }
    projectile.on_hit(hit);
    world.emit_game_event_with_context(
        GameEvent::ProjectileLand.name(),
        land_position,
        &GameEventContext {
            source_entity: Some(projectile),
        },
    );
    HitOutcome::Hit
}

/// Vanilla `Projectile.onRedirectProjectile`: a fireball that is hit flies where the shooter of
/// this projectile looks, and is theirs now.
fn redirect_projectile(projectile: &dyn EntityBase, target: &dyn EntityBase) {
    let redirectable = target
        .get_entity()
        .entity_type
        .has_tag(&tag::EntityType::MINECRAFT_REDIRECTABLE_PROJECTILE);
    if !redirectable {
        return;
    }
    let Some(target_owner) = target.get_projectile_owner() else {
        return;
    };
    let owner = projectile.get_owner_entity();
    ProjectileDeflectionType::Redirected.deflect(target, owner.as_deref());
    target_owner.set(owner.as_deref().map(EntityBase::get_entity));
}
