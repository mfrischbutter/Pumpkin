//! Vanilla `MinecartTNT`.

use std::sync::atomic::{AtomicI32, Ordering};

use crossbeam::atomic::AtomicCell;
use pumpkin_data::entity::EntityStatus;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_protocol::bedrock::server::actor_event::ActorEventID;
use rand::RngExt;

use crate::entity::Entity;

/// Vanilla `MinecartTNT.primeFuse`'s fuse.
const PRIMED_FUSE: i32 = 80;
const NOT_PRIMED: i32 = -1;
/// Vanilla `tick`: from this speed a horizontal collision sets the cart off.
const EXPLODE_ON_IMPACT_SPEED_SQUARED: f32 = 0.01;
/// Vanilla `causeFallDamage`: from this height a landing sets the cart off.
const EXPLODE_ON_FALL_DISTANCE: f64 = 3.0;

pub(super) struct TntMinecart {
    fuse: AtomicI32,
    explosion_power: AtomicCell<f32>,
    explosion_speed_factor: AtomicCell<f32>,
}

impl TntMinecart {
    pub(super) const fn new() -> Self {
        Self {
            fuse: AtomicI32::new(NOT_PRIMED),
            explosion_power: AtomicCell::new(4.0),
            explosion_speed_factor: AtomicCell::new(1.0),
        }
    }

    pub(super) fn fuse(&self) -> i32 {
        self.fuse.load(Ordering::Relaxed)
    }

    /// Vanilla `MinecartTNT.primeFuse`.
    fn prime_fuse(&self, entity: &Entity) {
        let world = entity.world.load();
        if !world.level_info.load().game_rules.tnt_explodes {
            return;
        }
        self.fuse.store(PRIMED_FUSE, Ordering::Relaxed);
        world.send_entity_status(entity, EntityStatus::TntPrime, ActorEventID::PrimeTNTCart);
        if !entity.is_silent() {
            world.play_sound(
                Sound::EntityTntPrimed,
                SoundCategory::Blocks,
                &entity.pos.load(),
            );
        }
    }

    /// The priming branch of vanilla `MinecartTNT.destroy`. The fuse is set even when the
    /// `tntExplodes` game rule keeps `primeFuse` from running.
    pub(super) fn prime_with_random_fuse(&self, entity: &Entity) {
        self.prime_fuse(entity);
        let mut rng = rand::rng();
        self.fuse.store(
            rng.random_range(0..20) + rng.random_range(0..20),
            Ordering::Relaxed,
        );
    }

    /// Vanilla `MinecartTNT.activateMinecart`.
    pub(super) fn activate(&self, entity: &Entity, powered: bool) {
        if powered && self.fuse() < 0 {
            self.prime_fuse(entity);
        }
    }

    /// Vanilla `MinecartTNT.tick`. Clients draw the smoke themselves from the priming event.
    ///
    /// Like vanilla, the impact check runs even when the fuse just went off.
    pub(super) fn tick(&self, entity: &Entity) {
        let fuse = self.fuse();
        if fuse > 0 {
            self.fuse.store(fuse - 1, Ordering::Relaxed);
        } else if fuse == 0 {
            self.explode(entity, entity.velocity.load().horizontal_length_squared());
        }
        if entity.horizontal_collision.load(Ordering::SeqCst) {
            let speed_squared = entity.velocity.load().horizontal_length_squared();
            if speed_squared >= f64::from(EXPLODE_ON_IMPACT_SPEED_SQUARED) {
                self.explode(entity, speed_squared);
            }
        }
    }

    /// Vanilla `MinecartTNT.causeFallDamage`.
    pub(super) fn cause_fall_damage(&self, entity: &Entity, fall_distance: f64) {
        if fall_distance >= EXPLODE_ON_FALL_DISTANCE {
            let power = fall_distance / 10.0;
            self.explode(entity, power * power);
        }
    }

    /// Vanilla `MinecartTNT.explode`. The cart goes first so its explosion does not hit it, which
    /// vanilla's explosion does by leaving out its source.
    pub(super) fn explode(&self, entity: &Entity, speed_squared: f64) {
        let world = entity.world.load();
        let primed = self.fuse() > NOT_PRIMED;
        if !world.level_info.load().game_rules.tnt_explodes {
            if primed {
                entity.remove();
            }
            return;
        }

        let power = Self::explosion_strength(
            self.explosion_power.load(),
            self.explosion_speed_factor.load(),
            speed_squared,
            rand::rng().random_range(0.0..1.0),
        );
        let pos = entity.pos.load();
        entity.remove();
        if primed {
            world.explode_tnt_minecart(entity, pos, power);
        } else {
            world.explode(
                Some(entity),
                pos,
                power,
                crate::world::ExplosionInteraction::Tnt,
            );
        }
    }

    /// What can blow up in the cart's next tick, for the tick planner.
    pub(super) fn may_explode_soon(&self, entity: &Entity, fall_distance: f64) -> bool {
        let fuse = self.fuse();
        (0..=1).contains(&fuse)
            || fall_distance >= EXPLODE_ON_FALL_DISTANCE
            || entity.velocity.load().horizontal_length_squared()
                >= f64::from(EXPLODE_ON_IMPACT_SPEED_SQUARED)
    }

    pub(super) fn write_nbt(&self, nbt: &mut NbtCompound) {
        nbt.put_int("fuse", self.fuse());
        let power = self.explosion_power.load();
        if power != 4.0 {
            nbt.put_float("explosion_power", power);
        }
        let speed_factor = self.explosion_speed_factor.load();
        if speed_factor != 1.0 {
            nbt.put_float("explosion_speed_factor", speed_factor);
        }
    }

    pub(super) fn read_nbt(&self, nbt: &NbtCompound) {
        self.fuse
            .store(nbt.get_int("fuse").unwrap_or(NOT_PRIMED), Ordering::Relaxed);
        self.explosion_power.store(
            nbt.get_float("explosion_power")
                .unwrap_or(4.0)
                .clamp(0.0, 128.0),
        );
        self.explosion_speed_factor.store(
            nbt.get_float("explosion_speed_factor")
                .unwrap_or(1.0)
                .clamp(0.0, 128.0),
        );
    }

    fn explosion_strength(
        base: f32,
        speed_factor: f32,
        horizontal_speed_squared: f64,
        random: f32,
    ) -> f32 {
        let speed = horizontal_speed_squared.sqrt().min(5.0) as f32;
        base + speed_factor * random * 1.5 * speed
    }
}

#[cfg(test)]
mod tests {
    use super::TntMinecart;

    #[test]
    fn tnt_minecart_explosion_bonus_is_speed_capped() {
        assert_eq!(TntMinecart::explosion_strength(4.0, 1.0, 100.0, 1.0), 11.5);
        assert_eq!(TntMinecart::explosion_strength(4.0, 1.0, 100.0, 0.0), 4.0);
    }
}
