//! The explosive archetype's fuse: priming, the countdown and the explosion.

use std::sync::Arc;
use std::sync::atomic::Ordering::Relaxed;

use pumpkin_data::game_event::GameEvent;
use pumpkin_data::sound::Sound;
use pumpkin_data::tracked_data;
use pumpkin_util::math::vector3::Vector3;

use super::SulfurCubeEntity;
use super::archetype::ExplosionData;
use crate::block::blocks::redstone::has_own_or_neighbour_signal;
use crate::entity::EntityBase;
use crate::entity::mob::Mob;
use crate::entity::tnt::TNTEntity;
use crate::world::ExplosionInteraction;
use crate::world::explosion::{ExplosionDamageCalculator, UsedPortalDamageCalculator};

/// Vanilla `SulfurCube.tickFuse`: `getY(0.0625)` is where it explodes.
const EXPLOSION_HEIGHT_SHARE: f64 = 0.0625;

impl SulfurCubeEntity {
    /// Vanilla `SulfurCube.getFuse`.
    #[must_use]
    pub fn get_fuse(&self) -> i32 {
        self.fuse.load(Relaxed)
    }

    /// Vanilla `SulfurCube.isPrimed`.
    #[must_use]
    pub fn is_primed(&self) -> bool {
        self.get_fuse() >= 0
    }

    fn explosion_data(&self) -> Option<ExplosionData> {
        self.effects().explosion
    }

    /// Vanilla `SulfurCube.canExplode`.
    #[must_use]
    pub fn can_explode(&self) -> bool {
        self.explosion_data().is_some()
            && self.mob_entity.living_entity.is_alive()
            && !self.is_primed()
    }

    /// Vanilla `SulfurCube.primeTime`: `imminent` is the short fuse of a chain reaction.
    pub fn prime_time(&self, imminent: bool) -> bool {
        let entity = self.get_entity();
        let world = entity.world.load();
        let Some(explosion) = self.explosion_data() else {
            return false;
        };
        if !self.mob_entity.living_entity.is_alive()
            || !world.level_info.load().game_rules.tnt_explodes
            || self.is_primed()
        {
            return false;
        }
        let fuse_time = if imminent {
            TNTEntity::random_short_fuse(explosion.fuse as u32) as i32
        } else {
            explosion.fuse
        };
        entity.set_invulnerable(true);
        self.fuse.store(fuse_time, Relaxed);
        entity.set_synced_data(tracked_data::sulfur_cube::MAX_FUSE, fuse_time);
        self.make_sound(Sound::EntityTntPrimed);
        world.emit_game_event(GameEvent::PrimeFuse.name(), entity.pos.load());
        true
    }

    /// Vanilla `SulfurCube.tickFuse`.
    pub(super) fn tick_fuse(&self) {
        let fuse = self.get_fuse();
        if fuse > 0 {
            self.fuse.store(fuse - 1, Relaxed);
        }
        let Some(explosion) = self.explosion_data() else {
            return;
        };
        if self.get_fuse() != 0 {
            return;
        }
        let entity = self.get_entity();
        entity.drop_leash();
        self.mob_entity.living_entity.dead.store(true, Relaxed);
        let world = entity.world.load_full();
        let game_rules = &world.level_info.load().game_rules;
        if game_rules.tnt_explodes {
            let interaction = if game_rules.mob_griefing {
                ExplosionInteraction::Tnt
            } else {
                ExplosionInteraction::None
            };
            let calculator = (entity.portal_cooldown.load(Relaxed) > 0).then(|| {
                Arc::new(UsedPortalDamageCalculator) as Arc<dyn ExplosionDamageCalculator>
            });
            let pos = entity.pos.load();
            let y = f64::from(entity.height()).mul_add(EXPLOSION_HEIGHT_SHARE, pos.y);
            world.explode_with_calculator(
                Some(entity),
                Vector3::new(pos.x, y, pos.z),
                explosion.power as f32,
                interaction,
                calculator,
            );
        }
        entity.remove();
    }

    /// Vanilla `SulfurCube.primeWhenOnPoweredPosition`.
    pub(super) fn prime_when_on_powered_position(&self) {
        if !self.can_explode() {
            return;
        }
        let entity = self.get_entity();
        let world = entity.world.load();
        if has_own_or_neighbour_signal(&world, &entity.block_pos.load()) {
            self.prime_time(false);
        }
    }
}
