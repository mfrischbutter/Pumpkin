use crate::entity::EntityBase;
use pumpkin_util::math::vector3::Vector3;
use std::sync::atomic::Ordering;

#[derive(Clone, Copy)]
pub enum ProjectileDeflectionType {
    None,
    Simple,
    Redirected,
    TransferVelocityDirection,
}

impl ProjectileDeflectionType {
    /// Vanilla `ProjectileDeflection.REVERSE` with its `power`: `Projectile.deflect` passes 1.0
    /// by default, an arrow that did not hurt 0.2.
    pub fn reverse(projectile: &dyn EntityBase, power: Vector3<f64>) {
        let vel = rand::random::<f32>().mul_add(20.0, 170.0);

        let entity = projectile.get_entity();
        entity.velocity.store(entity.velocity.load().multiply(
            -power.x * 0.5,
            -power.y * 0.5,
            -power.z * 0.5,
        ));

        let yaw = entity.yaw.load() + vel;
        let pitch = entity.pitch.load();
        entity.set_rotation(yaw, pitch);
        // TODO: Add entity.lastYaw += vel
        entity.velocity_dirty.store(true, Ordering::Relaxed);
    }

    pub fn deflect(&self, projectile: &dyn EntityBase, hit_entity: Option<&dyn EntityBase>) {
        match self {
            Self::None => {}
            Self::Simple => Self::reverse(projectile, Vector3::new(1.0, 1.0, 1.0)),
            Self::Redirected => {
                if let Some(hit_entity) = hit_entity {
                    let rotation_vector = hit_entity.get_entity().rotation();

                    let entity = projectile.get_entity();
                    entity.velocity.store(rotation_vector.to_f64());
                    entity.velocity_dirty.store(true, Ordering::Relaxed);
                }
            }
            Self::TransferVelocityDirection => {
                if let Some(hit_entity) = hit_entity {
                    let hit_velocity = hit_entity.get_entity().velocity.load().normalize();

                    let entity = projectile.get_entity();
                    entity.velocity.store(hit_velocity);
                    entity.velocity_dirty.store(true, Ordering::Relaxed);
                }
            }
        }
    }
}
