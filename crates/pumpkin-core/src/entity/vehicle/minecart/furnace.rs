//! Vanilla `MinecartFurnace`.

use std::sync::atomic::{AtomicI32, Ordering};

use crossbeam::atomic::AtomicCell;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::tag::{self, Taggable};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::math::vector3::Vector3;

use crate::entity::player::Player;
use crate::entity::{Entity, EntityBase};

pub(super) struct FurnaceMinecart {
    fuel: AtomicI32,
    push: AtomicCell<Vector3<f64>>,
}

impl FurnaceMinecart {
    const FUEL_TICKS_PER_ITEM: i32 = 3_600;
    const MAX_FUEL_TICKS: i32 = 32_000;

    pub(super) const fn new() -> Self {
        Self {
            fuel: AtomicI32::new(0),
            push: AtomicCell::new(Vector3::new(0.0, 0.0, 0.0)),
        }
    }

    fn set_has_fuel(entity: &Entity, fueled: bool) {
        entity.set_synced_data(
            pumpkin_data::tracked_data::furnace_minecart::ID_FUEL,
            fueled,
        );
    }

    /// Vanilla `MinecartFurnace.tick`. Clients draw the smoke themselves from the fuel flag.
    pub(super) fn tick(&self, entity: &Entity) {
        let mut fuel = self.fuel.load(Ordering::Relaxed);
        if fuel > 0 {
            fuel -= 1;
            self.fuel.store(fuel, Ordering::Relaxed);
        }
        if fuel <= 0 {
            self.push.store(Vector3::default());
        }
        Self::set_has_fuel(entity, fuel > 0);
    }

    /// Vanilla `MinecartFurnace.getMaxSpeed`'s factor on the behaviour's speed.
    pub(super) fn max_speed_factor(entity: &Entity) -> f64 {
        if entity.is_in_water() { 0.75 } else { 0.5 }
    }

    /// Vanilla `MinecartFurnace.applyNaturalSlowdown`, up to its call of the super method.
    pub(super) fn apply_push(&self, entity: &Entity, delta: Vector3<f64>) -> Vector3<f64> {
        let push = self.push.load();
        if push.length_squared() <= 1.0e-7 {
            return delta.multiply(0.98, 0.0, 0.98);
        }
        let push = Self::calculate_new_push_along(push, delta);
        self.push.store(push);
        let moved = delta.multiply(0.8, 0.0, 0.8) + push;
        if entity.is_in_water() {
            moved * 0.1
        } else {
            moved
        }
    }

    /// Vanilla `MinecartFurnace.calculateNewPushAlong`.
    fn calculate_new_push_along(push: Vector3<f64>, delta: Vector3<f64>) -> Vector3<f64> {
        if push.horizontal_length_squared() > 1.0e-4 && delta.horizontal_length_squared() > 0.001 {
            // Vanilla `Vec3.projectedOn`.
            let projected = delta * (push.dot(&delta) / delta.length_squared());
            projected.normalize() * push.length()
        } else {
            push
        }
    }

    /// Vanilla `MinecartFurnace.interact` and `addFuel`.
    pub(super) fn interact(
        &self,
        entity: &Entity,
        player: &Player,
        item_stack: &mut ItemStack,
    ) -> bool {
        let fuel = self.fuel.load(Ordering::Relaxed);
        if item_stack
            .get_item()
            .has_tag(&tag::Item::MINECRAFT_FURNACE_MINECART_FUEL)
            && fuel + Self::FUEL_TICKS_PER_ITEM <= Self::MAX_FUEL_TICKS
        {
            let fuel = fuel + Self::FUEL_TICKS_PER_ITEM;
            self.fuel.store(fuel, Ordering::Relaxed);
            if fuel > 0 {
                let cart_pos = entity.pos.load();
                let player_pos = player.get_entity().pos.load();
                self.push.store(Vector3::new(
                    cart_pos.x - player_pos.x,
                    0.0,
                    cart_pos.z - player_pos.z,
                ));
            }
            item_stack.decrement_unless_creative(player.gamemode.load(), 1);
        }
        true
    }

    pub(super) fn init_data_tracker(&self, entity: &Entity) {
        Self::set_has_fuel(entity, self.fuel.load(Ordering::Relaxed) > 0);
    }

    pub(super) fn write_nbt(&self, nbt: &mut NbtCompound) {
        let push = self.push.load();
        nbt.put_double("PushX", push.x);
        nbt.put_double("PushZ", push.z);
        nbt.put_short("Fuel", self.fuel.load(Ordering::Relaxed) as i16);
    }

    pub(super) fn read_nbt(&self, nbt: &NbtCompound) {
        self.push.store(Vector3::new(
            nbt.get_double("PushX").unwrap_or(0.0),
            0.0,
            nbt.get_double("PushZ").unwrap_or(0.0),
        ));
        self.fuel.store(
            i32::from(nbt.get_short("Fuel").unwrap_or(0)),
            Ordering::Relaxed,
        );
    }
}
