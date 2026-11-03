//! Vanilla `SulfurCube.knockback`: a cube with a block flies like a ball, by where it was hit.

use std::sync::atomic::Ordering::Relaxed;

use pumpkin_data::attributes::Attributes;
use pumpkin_data::tag::{self, Taggable};
use pumpkin_util::math::clamped_map;
use pumpkin_util::math::vector3::Vector3;

use super::SulfurCubeEntity;
use crate::entity::combat::knockback_after_resistance;
use crate::entity::{Entity, EntityBase};

/// Vanilla `SulfurCube.HORIZONTAL_HIT_ANGLE_SCALE`.
const HORIZONTAL_HIT_ANGLE_SCALE: f32 = 1.6;
/// Vanilla `SulfurCube.VERTICAL_HIT_ANGLE_SCALE`.
const VERTICAL_HIT_ANGLE_SCALE: f32 = 0.5;
/// Vanilla `SulfurCube.VERTICAL_POSITION_ANGLE_SCALE`.
const VERTICAL_POSITION_ANGLE_SCALE: f32 = 0.8;
/// Vanilla `SulfurCube.EXTRA_KNOCKBACK_DAMPENING`.
const EXTRA_KNOCKBACK_DAMPENING: f32 = 0.25;
/// Vanilla `LivingEntity.dealDefaultKnockback`: the power of a hit.
const DEFAULT_KNOCKBACK_POWER: f64 = 0.4;
/// Vanilla `SulfurCube.knockback`: the horizontal power's dampening, and its limit.
const HORIZONTAL_POWER_SCALE: f32 = 0.4;
const VERTICAL_POWER_SCALE: f64 = 1.2;
const POWER_LIMIT: f32 = 128.0;

/// Vanilla `Vec2`, as far as the knockback needs it.
#[derive(Clone, Copy)]
struct Vec2 {
    x: f32,
    y: f32,
}

impl Vec2 {
    /// Vanilla `Vec2.rotate`.
    fn rotate(self, angle: f32) -> Self {
        let (sine, cosine) = angle.sin_cos();
        Self {
            x: self.x * cosine - self.y * sine,
            y: self.y * cosine + self.x * sine,
        }
    }
}

/// Vanilla `SulfurCube.applyHorizontalHitAngleScale`: a glancing hit turns the push further.
fn apply_horizontal_hit_angle_scale(
    original_angle: Vec2,
    attacker_position: Vector3<f64>,
    attacker_aim: Vector3<f64>,
    target_center: Vector3<f64>,
) -> Vec2 {
    let to_target = (target_center - attacker_position).normalize();
    let angle_diff = (attacker_aim.x * to_target.z - attacker_aim.z * to_target.x)
        .atan2(attacker_aim.x * to_target.x + attacker_aim.z * to_target.z)
        as f32;
    original_angle.rotate(angle_diff * HORIZONTAL_HIT_ANGLE_SCALE)
}

/// Vanilla `SulfurCube.applyVerticalHitAnglePowerTransfer`: a hit from below goes up, not away.
fn apply_vertical_hit_angle_power_transfer(
    power: Vec2,
    attacker_position: Vector3<f64>,
    attacker_aim: Vector3<f64>,
    target_center: Vector3<f64>,
    target_height: f32,
) -> Vec2 {
    let half_height = f64::from(0.5f32 * target_height);
    let to_top = (target_center.add_raw(0.0, half_height, 0.0) - attacker_position).normalize();
    let to_bottom = (target_center.add_raw(0.0, -half_height, 0.0) - attacker_position).normalize();
    let factor = clamped_map(attacker_aim.y, to_top.y, to_bottom.y, -1.0, 1.0) as f32;
    let mut transferred = (factor * VERTICAL_HIT_ANGLE_SCALE).abs();
    if factor < 0.0 {
        transferred = -transferred;
    }
    Vec2 {
        x: power.x * (1.0 - transferred),
        y: power.y * (1.0 + transferred),
    }
}

/// Vanilla `SulfurCube.applyVerticalPositionAnglePowerRotation`: hitting from above or below
/// tilts the push, never past the archetype's powers.
fn apply_vertical_position_angle_power_rotation(
    power: Vec2,
    original_power: Vec2,
    attacker_feet: Vector3<f64>,
    target_feet: Vector3<f64>,
) -> Vec2 {
    let feet_to_feet = target_feet - attacker_feet;
    let angle = (-feet_to_feet.y).atan2(feet_to_feet.horizontal_length()) as f32;
    let rotated = power.rotate(-angle * VERTICAL_POSITION_ANGLE_SCALE);
    let horizontal_ratio = if original_power.x > 0.0 {
        rotated.x.abs() / original_power.x
    } else {
        0.0
    };
    let vertical_ratio = if original_power.y > 0.0 {
        rotated.y.abs() / original_power.y
    } else {
        0.0
    };
    let max_ratio = horizontal_ratio.max(vertical_ratio);
    if max_ratio > 1.0 {
        Vec2 {
            x: rotated.x / max_ratio,
            y: rotated.y / max_ratio,
        }
    } else {
        rotated
    }
}

/// Vanilla `AABB.getCenter`.
fn bounding_box_center(entity: &Entity) -> Vector3<f64> {
    let bounding_box = entity.bounding_box.load();
    Vector3::new(
        f64::midpoint(bounding_box.min.x, bounding_box.max.x),
        f64::midpoint(bounding_box.min.y, bounding_box.max.y),
        f64::midpoint(bounding_box.min.z, bounding_box.max.z),
    )
}

impl SulfurCubeEntity {
    /// Vanilla `SulfurCube.knockback` when it has a block and the hit has an attacker. `false`
    /// leaves the knockback to `LivingEntity.knockback`.
    pub(super) fn body_knockback(
        &self,
        power: f64,
        xd: f64,
        zd: f64,
        attacker: Option<&Entity>,
        damage: f32,
        comes_from_effect: bool,
    ) -> bool {
        let Some(attacker) = attacker else {
            return false;
        };
        if !self.has_body_item() {
            return false;
        }
        let (modifier, hit_sound) = {
            let effects = self.effects();
            (
                effects.knockback_modifier,
                effects.sound_settings.hit_sound,
            )
        };
        let original_power = Vec2 {
            x: modifier.horizontal_power,
            y: modifier.vertical_power,
        };
        let living = &self.mob_entity.living_entity;
        let entity = &living.entity;
        let attacker_eye = attacker.get_eye_pos();
        let attacker_aim = Vector3::<f64>::rotation_vector(
            f64::from(attacker.pitch.load()),
            f64::from(attacker.yaw.load()),
        )
        .normalize();
        let center = bounding_box_center(entity);

        let original_angle = Vec2 {
            x: xd as f32,
            y: zd as f32,
        };
        let direction =
            apply_horizontal_hit_angle_scale(original_angle, attacker_eye, attacker_aim, center);
        let transferred = apply_vertical_hit_angle_power_transfer(
            original_power,
            attacker_eye,
            attacker_aim,
            center,
            entity.height(),
        );
        let rotated = apply_vertical_position_angle_power_rotation(
            transferred,
            original_power,
            attacker.pos.load(),
            entity.pos.load(),
        );

        let dampening = if comes_from_effect {
            power as f32 * EXTRA_KNOCKBACK_DAMPENING
        } else {
            1.0
        };
        let resistance = living.get_attribute_value(&Attributes::KNOCKBACK_RESISTANCE);
        let scale = damage.sqrt() * dampening * (1.0 - resistance) as f32;
        let horizontal_power =
            (rotated.x * scale * HORIZONTAL_POWER_SCALE).clamp(-POWER_LIMIT, POWER_LIMIT);
        let vertical_power = (rotated.y * scale).clamp(-POWER_LIMIT, POWER_LIMIT);

        let horizontal = Vector3::new(f64::from(direction.x), 0.0, f64::from(direction.y))
            .normalize()
            * f64::from(horizontal_power);
        let velocity = entity.velocity.load();
        entity.velocity.store(Vector3::new(
            velocity.x - horizontal.x,
            velocity.y + f64::from(vertical_power) * VERTICAL_POWER_SCALE,
            velocity.z - horizontal.z,
        ));
        entity.velocity_dirty.store(true, Relaxed);
        self.play_entity_sound(hit_sound, 1.0, 1.0);
        true
    }

    /// Vanilla `LivingEntity.dealDefaultKnockback`, for a hit its body takes without harm.
    pub(super) fn deal_default_knockback(
        &self,
        damage: f32,
        position: Option<Vector3<f64>>,
        source: Option<&dyn EntityBase>,
        cause: Option<&dyn EntityBase>,
    ) {
        let living = &self.mob_entity.living_entity;
        let own = living.entity.pos.load();
        let projectile = source.filter(|direct| {
            direct
                .get_entity()
                .entity_type
                .has_tag(&tag::EntityType::MINECRAFT_IMPACT_PROJECTILES)
        });
        let (xd, zd) = if let Some(projectile) = projectile {
            // Vanilla `Projectile.calculateHorizontalHurtKnockbackDirection`.
            let velocity = projectile.get_entity().velocity.load();
            (-velocity.x, -velocity.z)
        } else if let Some(from) =
            position.or_else(|| source.map(|direct| direct.get_entity().pos.load()))
        {
            (from.x - own.x, from.z - own.z)
        } else {
            (0.0, 0.0)
        };
        let attacker = cause.map(|attacker| attacker.get_entity());
        if !self.body_knockback(DEFAULT_KNOCKBACK_POWER, xd, zd, attacker, damage, false) {
            let resistance = living.get_attribute_value(&Attributes::KNOCKBACK_RESISTANCE);
            living.entity.apply_knockback(
                knockback_after_resistance(DEFAULT_KNOCKBACK_POWER, resistance),
                xd,
                zd,
            );
        }
    }
}
