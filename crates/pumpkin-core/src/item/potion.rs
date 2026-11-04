use crate::entity::living::{EffectSource, LivingEntity};
use pumpkin_data::data_component_impl::{PotionContentsImpl, PotionDurationScaleImpl};
use pumpkin_data::effect::StatusEffect;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::potion::{Effect, Potion};
use pumpkin_data::tag;

/// An effect of a potion: (`StatusEffect`, `duration_ticks`, `amplifier`, `ambient`,
/// `show_particles`, `show_icon`).
pub type PotionEffect = (&'static StatusEffect, i32, u8, bool, bool, bool);

/// Vanilla `PotionContents.BASE_POTION_COLOR`.
pub const BASE_POTION_COLOR: i32 = -13_083_194;
/// Vanilla `MobEffectInstance.INFINITE_DURATION`.
pub const INFINITE_DURATION: i32 = -1;
/// Vanilla `ThrownSplashPotion.onHitAsPotion`: effects this short are not worth adding.
const SPLASH_MIN_DURATION: i32 = 20;

/// Utilities for reading potion contents from an `ItemStack` and applying effects.
pub struct PotionContents;

/// Source context for applying potion effects (affects scaling rules).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PotionApplicationSource {
    /// Normal application (drinking)
    Normal,
    Arrow,
}

impl PotionApplicationSource {
    const fn instant_scale(self, scale: f32) -> Option<f32> {
        match self {
            Self::Arrow => None,
            Self::Normal => Some(scale),
        }
    }

    const fn duration_scale(self, scale: f32) -> f32 {
        match self {
            Self::Arrow | Self::Normal => scale,
        }
    }
}

impl PotionContents {
    /// Read effects from an `ItemStack`'s `PotionContents` data component.
    #[must_use]
    pub fn read_potion_effects(stack: &ItemStack) -> Vec<PotionEffect> {
        stack
            .get_data_component::<PotionContentsImpl>()
            .map_or_else(Vec::new, Self::effects_of)
    }

    /// Vanilla `PotionContents.getAllEffects`: the potion's effects, then the custom ones.
    #[must_use]
    pub fn effects_of(contents: &PotionContentsImpl) -> Vec<PotionEffect> {
        let mut out = Vec::new();
        if let Some(potion) = Self::potion_of(contents) {
            for e in potion.effects {
                out.push((
                    e.effect_type,
                    e.duration,
                    e.amplifier,
                    e.ambient,
                    e.show_particles,
                    e.show_icon,
                ));
            }
        }
        for ce in &contents.custom_effects {
            if let Some(se) = StatusEffect::from_minecraft_name(&ce.effect_id) {
                out.push((
                    se,
                    ce.duration,
                    ce.amplifier as u8,
                    ce.ambient,
                    ce.show_particles,
                    ce.show_icon,
                ));
            }
        }
        out
    }

    /// Vanilla `PotionContents.potion`.
    #[must_use]
    pub fn potion_of(contents: &PotionContentsImpl) -> Option<&'static Potion> {
        contents
            .potion_id
            .and_then(|id| u8::try_from(id).ok())
            .and_then(Potion::from_id)
    }

    /// Vanilla `PotionContents.is(TagKey)`: custom effects take a potion out of its tags.
    #[must_use]
    pub fn is_in_tag(stack: &ItemStack, potion_tag: &'static tag::Tag) -> bool {
        stack
            .get_data_component::<PotionContentsImpl>()
            .is_some_and(|contents| {
                contents.custom_effects.is_empty()
                    && Self::potion_of(contents)
                        .is_some_and(|potion| potion_tag.1.contains(&u16::from(potion.id)))
            })
    }

    /// Vanilla `Potion.hasInstantEffects` of the stack's potion, custom effects do not count.
    #[must_use]
    pub fn has_instant_potion(stack: &ItemStack) -> bool {
        stack
            .get_data_component::<PotionContentsImpl>()
            .and_then(Self::potion_of)
            .is_some_and(|potion| {
                potion.effects.iter().any(|effect| {
                    effect.effect_type == &StatusEffect::INSTANT_DAMAGE
                        || effect.effect_type == &StatusEffect::INSTANT_HEALTH
                })
            })
    }

    /// Vanilla `PotionContents.getColor`.
    #[must_use]
    pub fn color(contents: &PotionContentsImpl) -> i32 {
        contents.custom_color.unwrap_or_else(|| {
            Self::mixed_color(&Self::effects_of(contents)).unwrap_or(BASE_POTION_COLOR)
        })
    }

    /// Vanilla `PotionContents.getColor` of the stack's contents.
    #[must_use]
    pub fn stack_color(stack: &ItemStack) -> i32 {
        stack
            .get_data_component::<PotionContentsImpl>()
            .map_or(BASE_POTION_COLOR, Self::color)
    }

    /// Vanilla `PotionContents.getColorOptional`: visible effects, weighted by their level.
    fn mixed_color(effects: &[PotionEffect]) -> Option<i32> {
        let (mut red, mut green, mut blue, mut total_weight) = (0i32, 0i32, 0i32, 0i32);
        for &(effect, _, amplifier, _, visible, _) in effects {
            if visible {
                let weight = i32::from(amplifier) + 1;
                red += weight * ((effect.color >> 16) & 0xFF);
                green += weight * ((effect.color >> 8) & 0xFF);
                blue += weight * (effect.color & 0xFF);
                total_weight += weight;
            }
        }
        (total_weight != 0).then(|| {
            (0xFFi32 << 24)
                | ((red / total_weight) << 16)
                | ((green / total_weight) << 8)
                | (blue / total_weight)
        })
    }

    /// The stack's `POTION_DURATION_SCALE`, 1 if it has none.
    #[must_use]
    pub fn duration_scale(stack: &ItemStack) -> f32 {
        stack
            .get_data_component::<PotionDurationScaleImpl>()
            .map_or(1.0, |component| component.scale)
    }

    /// Vanilla `MobEffectInstance.mapDuration`: infinite and zero durations stay.
    #[must_use]
    pub fn map_duration(duration: i32, mapper: impl FnOnce(i32) -> i32) -> i32 {
        if duration == INFINITE_DURATION || duration == 0 {
            duration
        } else {
            mapper(duration)
        }
    }

    /// Vanilla `MobEffectInstance.withScaledDuration`.
    #[must_use]
    pub fn with_scaled_duration(duration: i32, scale: f32) -> i32 {
        Self::map_duration(duration, |ticks| {
            ((ticks as f32 * scale).floor() as i32).max(1)
        })
    }

    /// The effect instance of an entry.
    #[must_use]
    pub const fn to_effect(entry: &PotionEffect) -> Effect {
        let &(effect_type, duration, amplifier, ambient, show_particles, show_icon) = entry;
        Effect {
            effect_type,
            duration,
            amplifier,
            ambient,
            show_particles,
            show_icon,
            blend: false,
        }
    }

    /// The loop of vanilla `ThrownSplashPotion.onHitAsPotion` for one target: instant effects
    /// are weighted by `scale`, durations by `scale` and the stack's `duration_scale`.
    pub fn apply_splash_effects(
        target: &LivingEntity,
        effects: &[PotionEffect],
        scale: f64,
        duration_scale: f32,
        source: EffectSource<'_>,
    ) {
        for entry in effects {
            let &(effect_type, duration, amplifier, ambient, visible, _) = entry;
            if target.apply_instant_effect(effect_type, amplifier, Some(scale), Some(source)) {
                continue;
            }
            let duration = Self::map_duration(duration, |ticks| {
                (scale * f64::from(ticks) * f64::from(duration_scale) + 0.5) as i32
            });
            // Vanilla `MobEffectInstance.endsWithin`.
            if duration != INFINITE_DURATION && duration <= SPLASH_MIN_DURATION {
                continue;
            }
            target.add_effect(Effect {
                effect_type,
                duration,
                amplifier,
                ambient,
                show_particles: visible,
                show_icon: visible,
                blend: false,
            });
        }
    }

    /// Apply instant or duration effects to a target living entity.
    ///
    /// `direct` is the entity that applied the effects (a thrown potion) and `owner` the one
    /// that threw it; like vanilla `applyInstantaneousEffect`, they make instant damage
    /// indirect magic attributed to them.
    pub fn apply_effects_to(
        target: &LivingEntity,
        effects: Vec<PotionEffect>,
        scale: f32,
        source: PotionApplicationSource,
        effect_source: Option<EffectSource<'_>>,
    ) {
        for (effect_type, duration, amplifier, ambient, show_particles, show_icon) in effects {
            // Like vanilla, instant effects are applied once and never added to the active
            // effects, where they would linger.
            if !target.apply_instant_effect(
                effect_type,
                amplifier,
                source.instant_scale(scale).map(f64::from),
                effect_source,
            ) {
                // Duration scaling
                let duration_scale = source.duration_scale(scale);

                let dur = if duration == -1 {
                    -1
                } else {
                    ((duration as f32) * duration_scale).max(1.0) as i32
                };
                let eff = Effect {
                    effect_type,
                    duration: dur,
                    amplifier,
                    ambient,
                    show_particles,
                    show_icon,
                    blend: false,
                };
                target.add_effect(eff);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::PotionApplicationSource;
    use pumpkin_data::data_component_impl::PotionDurationScaleImpl;
    use pumpkin_data::item::Item;
    use pumpkin_data::item_stack::ItemStack;

    #[test]
    fn tipped_arrow_scale_shortens_duration_without_reducing_instant_potency() {
        let tipped_arrow = ItemStack::new(1, &Item::TIPPED_ARROW);
        let scale = tipped_arrow
            .get_data_component::<PotionDurationScaleImpl>()
            .expect("tipped arrows should define a potion duration scale")
            .scale;

        assert_eq!(PotionApplicationSource::Arrow.duration_scale(scale), 0.125);
        assert_eq!(
            (160.0 * PotionApplicationSource::Arrow.duration_scale(scale)) as i32,
            20
        );
        assert_eq!(PotionApplicationSource::Arrow.instant_scale(scale), None);
    }
}
