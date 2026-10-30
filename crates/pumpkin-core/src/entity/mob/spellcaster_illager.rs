//! Vanilla `SpellcasterIllager`: the spell state and goals the evoker and illusioner share.

use std::marker::PhantomData;
use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};

use pumpkin_data::sound::Sound;
use pumpkin_data::tracked_data;
use pumpkin_nbt::compound::NbtCompound;

use crate::entity::EntityBase;
use crate::entity::ai::goal::{Controls, Goal, to_goal_ticks};
use crate::entity::living::LivingEntity;
use crate::entity::mob::Mob;
use crate::entity::mob::raider::Raider;

const DEFAULT_SPELLCASTING_TICKS: i32 = 0;
/// Vanilla `SpellcasterUseSpellGoal.getCastWarmupTime`.
pub const DEFAULT_CAST_WARMUP_TIME: i32 = 20;

/// Vanilla `SpellcasterIllager.IllagerSpell`, by the id the client reads its particles from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(i8)]
pub enum IllagerSpell {
    None = 0,
    SummonVex = 1,
    Fangs = 2,
    Wololo = 3,
    Disappear = 4,
    Blindness = 5,
}

pub trait SpellcasterIllager: Raider {
    /// Vanilla `spellCastingTickCount`.
    fn spell_casting_tick_count(&self) -> &AtomicI32;

    /// Vanilla `getCastingSoundEvent`.
    fn get_casting_sound_event(&self) -> Sound;

    /// Whom the casting spell goal looks at, vanilla `SpellcasterCastingSpellGoal.tick`.
    fn casting_look_target(&self) -> Option<Arc<dyn EntityBase>> {
        self.get_mob_entity().get_target()
    }

    fn get_spell_casting_time(&self) -> i32 {
        self.spell_casting_tick_count().load(Ordering::Relaxed)
    }

    fn is_casting_spell(&self) -> bool {
        self.get_spell_casting_time() > 0
    }

    fn set_is_casting_spell(&self, spell: IllagerSpell) {
        self.get_entity().set_synced_data(
            tracked_data::spellcaster_illager::DATA_SPELL_CASTING_ID,
            spell as i8,
        );
    }

    /// The part of vanilla `SpellcasterIllager.customServerAiStep` after `super`.
    fn tick_spell_casting(&self) {
        let ticks = self.spell_casting_tick_count();
        if ticks.load(Ordering::Relaxed) > 0 {
            ticks.fetch_sub(1, Ordering::Relaxed);
        }
    }

    fn write_spellcaster_nbt(&self, nbt: &mut NbtCompound) {
        self.write_raider_nbt(nbt);
        nbt.put_int("SpellTicks", self.get_spell_casting_time());
    }

    fn read_spellcaster_nbt(&self, nbt: &NbtCompound) {
        self.read_raider_nbt(nbt);
        let ticks = nbt
            .get_int("SpellTicks")
            .unwrap_or(DEFAULT_SPELLCASTING_TICKS);
        self.spell_casting_tick_count()
            .store(ticks, Ordering::Relaxed);
    }
}

fn as_caster<T: SpellcasterIllager + 'static>(mob: &dyn Mob) -> Option<&T> {
    mob.cast_any().downcast_ref::<T>()
}

fn is_alive(entity: &dyn EntityBase) -> bool {
    entity
        .get_living_entity()
        .is_some_and(LivingEntity::is_alive)
}

fn has_living_target(caster: &dyn SpellcasterIllager) -> bool {
    caster
        .get_mob_entity()
        .get_target()
        .is_some_and(|target| is_alive(target.as_ref()))
}

/// Vanilla `SpellcasterCastingSpellGoal`: stands still and faces its target while a spell runs.
pub struct SpellcasterCastingSpellGoal<T> {
    caster: PhantomData<fn() -> T>,
}

impl<T> Default for SpellcasterCastingSpellGoal<T> {
    fn default() -> Self {
        Self {
            caster: PhantomData,
        }
    }
}

impl<T: SpellcasterIllager + 'static> Goal for SpellcasterCastingSpellGoal<T> {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        as_caster::<T>(mob).is_some_and(T::is_casting_spell)
    }

    fn start(&mut self, mob: &dyn Mob) {
        mob.get_mob_entity()
            .navigator
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .stop();
    }

    fn stop(&mut self, mob: &dyn Mob) {
        if let Some(caster) = as_caster::<T>(mob) {
            caster.set_is_casting_spell(IllagerSpell::None);
        }
    }

    fn tick(&mut self, mob: &dyn Mob) {
        let Some(caster) = as_caster::<T>(mob) else {
            return;
        };
        let Some(target) = caster.casting_look_target() else {
            return;
        };
        mob.get_mob_entity()
            .look_control
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .look_at_entity_with_range(
                &target,
                mob.get_max_head_rotation(),
                mob.get_max_look_pitch_change(),
            );
    }

    fn controls(&self) -> Controls {
        Controls::MOVE | Controls::LOOK
    }
}

/// The values of one spell, the abstract getters of vanilla `SpellcasterUseSpellGoal`.
#[derive(Clone, Copy)]
pub struct Spell {
    pub kind: IllagerSpell,
    pub cast_warmup_time: i32,
    pub casting_time: i32,
    pub casting_interval: i32,
    pub prepare_sound: Option<Sound>,
}

/// The state of vanilla `SpellcasterUseSpellGoal`. Each spell goal wraps one and adds its cast.
pub struct SpellcasterUseSpellGoal {
    spell: Spell,
    attack_warmup_delay: i32,
    next_attack_tick_count: i32,
}

impl SpellcasterUseSpellGoal {
    #[must_use]
    pub const fn new(spell: Spell) -> Self {
        Self {
            spell,
            attack_warmup_delay: 0,
            next_attack_tick_count: 0,
        }
    }

    /// Vanilla `canUse`: a living target, no spell running and the casting interval passed.
    #[must_use]
    pub fn can_use(&self, caster: &dyn SpellcasterIllager) -> bool {
        has_living_target(caster) && !caster.is_casting_spell() && self.is_interval_over(caster)
    }

    /// Vanilla `canContinueToUse`: a living target until the spell is cast.
    #[must_use]
    pub fn can_continue_to_use(&self, caster: &dyn SpellcasterIllager) -> bool {
        has_living_target(caster) && self.is_warming_up()
    }

    #[must_use]
    pub fn is_interval_over(&self, caster: &dyn SpellcasterIllager) -> bool {
        caster.get_entity().tick_count.load(Ordering::Relaxed) >= self.next_attack_tick_count
    }

    #[must_use]
    pub const fn is_warming_up(&self) -> bool {
        self.attack_warmup_delay > 0
    }

    pub fn start(&mut self, caster: &dyn SpellcasterIllager) {
        self.attack_warmup_delay = to_goal_ticks(self.spell.cast_warmup_time);
        caster
            .spell_casting_tick_count()
            .store(self.spell.casting_time, Ordering::Relaxed);
        self.next_attack_tick_count =
            caster.get_entity().tick_count.load(Ordering::Relaxed) + self.spell.casting_interval;
        if let Some(sound) = self.spell.prepare_sound {
            caster.play_entity_sound(sound, 1.0, 1.0);
        }
        caster.set_is_casting_spell(self.spell.kind);
    }

    /// Counts the warmup down and casts `perform` when it runs out.
    pub fn tick(&mut self, caster: &dyn SpellcasterIllager, perform: impl FnOnce()) {
        self.attack_warmup_delay -= 1;
        if self.attack_warmup_delay == 0 {
            perform();
            caster.play_entity_sound(caster.get_casting_sound_event(), 1.0, 1.0);
        }
    }
}
