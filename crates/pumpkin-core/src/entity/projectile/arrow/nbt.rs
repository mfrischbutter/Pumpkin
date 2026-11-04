//! Vanilla `AbstractArrow.addAdditionalSaveData` and `readAdditionalSaveData`.

use std::sync::atomic::Ordering;

use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::Sound;
use pumpkin_nbt::compound::NbtCompound;

use super::ArrowPickup;
use super::abstract_arrow::AbstractArrow;
use crate::entity::mob::enderman::{block_state_from_nbt, block_state_to_nbt};

fn write_stack(nbt: &mut NbtCompound, key: &str, stack: &ItemStack) {
    let mut item = NbtCompound::new();
    stack.write_item_stack(&mut item);
    nbt.put_compound(key, item);
}

fn read_stack(nbt: &NbtCompound, key: &str) -> Option<ItemStack> {
    nbt.get_compound(key).and_then(ItemStack::read_item_stack)
}

impl AbstractArrow {
    pub(crate) fn write_arrow_nbt(&self, nbt: &mut NbtCompound) {
        nbt.put_bool("HasBeenShot", self.has_been_shot.load(Ordering::Relaxed));
        nbt.put_short("life", self.life.load(Ordering::Relaxed) as i16);
        if let Some(state) = self.last_state.load() {
            nbt.put("inBlockState", block_state_to_nbt(state));
        }
        nbt.put_byte("shake", self.shake_time.load(Ordering::Relaxed) as i8);
        nbt.put_bool("inGround", self.is_in_ground());
        nbt.put_byte("pickup", self.pickup().to_byte() as i8);
        nbt.put_double("damage", self.get_base_damage());
        nbt.put_bool("crit", self.is_critical.load(Ordering::Relaxed));
        nbt.put_byte("PierceLevel", self.get_pierce_level() as i8);
        nbt.put_string(
            "SoundEvent",
            format!("minecraft:{}", self.sound_event.load().to_name()),
        );
        write_stack(nbt, "item", &self.pickup_item());
        if let Some(weapon) = self.get_weapon_item() {
            write_stack(nbt, "weapon", &weapon);
        }
    }

    /// Reads what is present, so a partial read keeps the rest.
    pub(crate) fn read_arrow_nbt(&self, nbt: &NbtCompound) {
        if let Some(shot) = nbt.get_bool("HasBeenShot") {
            self.has_been_shot.store(shot, Ordering::Relaxed);
        }
        // Pumpkin saved the life as an int before.
        if let Some(life) = nbt
            .get_short("life")
            .map(i32::from)
            .or_else(|| nbt.get_int("life"))
        {
            self.life.store(life.max(0) as u32, Ordering::Relaxed);
        }
        if let Some(tag) = nbt.get("inBlockState") {
            self.last_state.store(block_state_from_nbt(tag));
        }
        if let Some(shake) = nbt.get_byte("shake") {
            self.shake_time.store(shake as u8, Ordering::Relaxed);
        }
        if let Some(in_ground) = nbt
            .get_bool("inGround")
            .or_else(|| nbt.get_byte("inGround").map(|byte| byte != 0))
        {
            self.set_in_ground(in_ground);
        }
        if let Some(damage) = nbt.get_double("damage") {
            self.set_base_damage(damage);
        }
        if let Some(pickup) = nbt.get_byte("pickup") {
            self.set_pickup(ArrowPickup::from_byte(pickup as u8));
        }
        if let Some(crit) = nbt.get_bool("crit") {
            self.set_critical(crit);
        }
        if let Some(pierce) = nbt.get_byte("PierceLevel") {
            self.set_pierce_level(pierce.max(0) as u8);
        }
        if let Some(sound) = nbt
            .get_string("SoundEvent")
            .and_then(|name| Sound::from_name(name.strip_prefix("minecraft:").unwrap_or(name)))
        {
            self.set_sound_event(sound);
        }
        if let Some(item) = read_stack(nbt, "item") {
            *self
                .item_stack
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = item;
        }
        if let Some(weapon) = read_stack(nbt, "weapon") {
            *self
                .weapon
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(weapon);
        }
    }
}
