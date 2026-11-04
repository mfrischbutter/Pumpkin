//! Vanilla `DragonSittingFlamingPhase`: breathes a cloud in front of it, four times at most.

use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use pumpkin_data::entity::EntityType;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;

use super::{EnderDragonPhase, Phase};
use crate::entity::area_effect_cloud::AreaEffectCloudEntity;
use crate::entity::boss::ender_dragon::{EnderDragonEntity, part::HEAD};
use crate::entity::{Entity, EntityBase};

const FLAME_DURATION: i32 = 200;
const SITTING_FLAME_ATTACKS_COUNT: i32 = 4;
const WARMUP_TIME: i32 = 10;
const FLAME_RADIUS: f32 = 5.0;

#[derive(Default)]
pub struct SittingFlamingPhase {
    flame_ticks: AtomicI32,
    flame_count: AtomicI32,
    flame: Mutex<Option<Arc<dyn EntityBase>>>,
}

impl SittingFlamingPhase {
    fn flame(&self) -> MutexGuard<'_, Option<Arc<dyn EntityBase>>> {
        self.flame
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub fn reset_flame_count(&self) {
        self.flame_count.store(0, Ordering::Relaxed);
    }

    fn start_flame(&self, dragon: &EnderDragonEntity) {
        let own = dragon.position();
        let head = dragon.parts[HEAD].entity.pos.load();
        let look = Vector3::new(head.x - own.x, 0.0, head.z - own.z);
        let length = look.x.hypot(look.z).max(1.0E-4);
        let radius = f64::from(FLAME_RADIUS);
        let x = head.x + look.x / length * radius / 2.0;
        let z = head.z + look.z / length * radius / 2.0;

        let world = dragon.mob_entity.living_entity.entity.world.load_full();
        let initial_y = head.y + 0.5;
        let mut y = initial_y;
        while world.get_block_state(&BlockPos::floored(x, y, z)).is_air() {
            y -= 1.0;
            if y < 0.0 {
                y = initial_y;
                break;
            }
        }
        let at = Vector3::new(x, y.floor() + 1.0, z);
        let cloud = AreaEffectCloudEntity::dragon_breath(
            Entity::new(world.clone(), at, &EntityType::AREA_EFFECT_CLOUD),
            FLAME_RADIUS,
            FLAME_DURATION,
            0.0,
            0,
        );
        AreaEffectCloudEntity::set_owner_of(
            cloud.as_ref(),
            Some(&dragon.mob_entity.living_entity.entity),
        );
        *self.flame() = Some(cloud.clone());
        world.spawn_entity(cloud);
    }
}

impl Phase for SittingFlamingPhase {
    fn tick(&self, dragon: &EnderDragonEntity) {
        let ticks = self.flame_ticks.fetch_add(1, Ordering::Relaxed) + 1;
        if ticks >= FLAME_DURATION {
            if self.flame_count.load(Ordering::Relaxed) >= SITTING_FLAME_ATTACKS_COUNT {
                dragon.set_phase(EnderDragonPhase::Takeoff);
            } else {
                dragon.set_phase(EnderDragonPhase::SittingScanning);
            }
        } else if ticks == WARMUP_TIME {
            self.start_flame(dragon);
        }
    }

    fn begin(&self) {
        self.flame_ticks.store(0, Ordering::Relaxed);
        self.flame_count.fetch_add(1, Ordering::Relaxed);
    }

    fn end(&self) {
        if let Some(flame) = self.flame().take() {
            flame.get_entity().remove();
        }
    }
}
