use pumpkin_data::attributes::Attributes;
use pumpkin_data::effect::StatusEffect;
use pumpkin_data::entity::EntityType;
use pumpkin_data::sound::Sound;
use pumpkin_util::math::vector3::Vector3;

use super::super::BrainTick;
use super::super::memory::{MemoryModuleId, MemoryStatus, types};
use super::timed::Behavior;
use crate::entity::EntityBase;
use crate::entity::ai::target_predicate::TargetPredicate;
use crate::entity::living::LivingEntity;

const CONDITIONS: [(MemoryModuleId, MemoryStatus); 2] = [
    (types::CHARGE_COOLDOWN_TICKS.id(), MemoryStatus::ValueAbsent),
    (types::ATTACK_TARGET.id(), MemoryStatus::ValuePresent),
];

/// The levels of `effect` on `living`, 0 without it.
fn effect_levels(living: &LivingEntity, effect: &'static StatusEffect) -> f32 {
    living
        .get_effect(effect)
        .map_or(0.0, |active| f32::from(active.amplifier) + 1.0)
}

/// Vanilla `ChargeAttack`: rushes straight at the target and knocks away what it runs into.
pub struct ChargeAttack {
    time_between_attacks: i32,
    speed: f32,
    knockback_force: f32,
    max_charge_distance: f64,
    max_detection_distance: f64,
    charge_sound: Sound,
    velocity: Vector3<f64>,
    start_position: Vector3<f64>,
}

impl ChargeAttack {
    #[must_use]
    pub fn new(
        time_between_attacks: i32,
        speed: f32,
        knockback_force: f32,
        max_charge_distance: f64,
        max_detection_distance: f64,
        charge_sound: Sound,
    ) -> Self {
        Self {
            time_between_attacks,
            speed,
            knockback_force,
            max_charge_distance,
            max_detection_distance,
            charge_sound,
            velocity: Vector3::default(),
            start_position: Vector3::default(),
        }
    }

    fn is_tame(tick: &BrainTick<'_>) -> bool {
        tick.mob
            .as_tamable()
            .is_some_and(crate::entity::passive::tamable::TamableAnimal::is_tame)
    }

    /// Vanilla `canStillUse`, shared with the sound check of `start`.
    fn keeps_charging(&self, tick: &BrainTick<'_>) -> bool {
        let Some(target) = tick.state.get(types::ATTACK_TARGET) else {
            return false;
        };
        if Self::is_tame(tick) {
            return false;
        }
        let pos = tick.mob.get_entity().pos.load();
        let charged = pos.squared_distance_to_vec(&self.start_position);
        let to_target = target.get_entity().pos.load().squared_distance_to_vec(&pos);
        charged < self.max_charge_distance * self.max_charge_distance
            && to_target < self.max_detection_distance * self.max_detection_distance
            && tick.mob.has_line_of_sight(target.get_entity())
            && !tick
                .state
                .has_memory_value(types::CHARGE_COOLDOWN_TICKS.id())
    }

    /// Vanilla's `ATTACK_TARGET_CONDITIONS`: armour stands only under mob griefing.
    fn may_hit(tick: &BrainTick<'_>, target: &dyn EntityBase) -> bool {
        let entity = target.get_entity();
        let griefing = tick.world.level_info.load().game_rules.mob_griefing;
        (griefing || entity.entity_type != &EntityType::ARMOR_STAND)
            && entity.entity_id != tick.mob.get_entity().entity_id
            && target.get_living_entity().is_some()
            && TargetPredicate::create_attackable().test(tick.world, Some(tick.mob), target)
    }

    /// Vanilla `dealKnockBack` with `causeExtraKnockback`.
    fn deal_knockback(&self, tick: &BrainTick<'_>, target: &dyn EntityBase) {
        let mob = tick.mob;
        let living = &mob.get_mob_entity().living_entity;
        let boost = 0.25
            * (effect_levels(living, &StatusEffect::SPEED)
                - effect_levels(living, &StatusEffect::SLOWNESS));
        let move_speed = living.get_attribute_value(&Attributes::MOVEMENT_SPEED) as f32;
        let factor = (self.speed * move_speed).clamp(0.2, 2.0) + boost;
        let strength = factor * self.knockback_force;
        if strength <= 0.0 {
            return;
        }
        let entity = mob.get_entity();
        let yaw = f64::from(entity.yaw.load()).to_radians();
        target
            .get_entity()
            .knockback(f64::from(strength), yaw.sin(), -yaw.cos());
        entity
            .velocity
            .store(entity.velocity.load().multiply(0.6, 1.0, 0.6));
    }
}

impl Behavior for ChargeAttack {
    fn entry_conditions(&self) -> &[(MemoryModuleId, MemoryStatus)] {
        &CONDITIONS
    }

    fn can_still_use(&mut self, tick: &mut BrainTick<'_>) -> bool {
        self.keeps_charging(tick)
    }

    fn start(&mut self, tick: &mut BrainTick<'_>) {
        let entity = tick.mob.get_entity();
        let pos = entity.pos.load();
        self.start_position = pos;
        let Some(target) = tick.state.get(types::ATTACK_TARGET) else {
            return;
        };
        let direction = (target.get_entity().pos.load() - pos).normalize();
        self.velocity = direction * f64::from(self.speed);
        if self.keeps_charging(tick) {
            tick.mob.play_entity_sound(self.charge_sound, 1.0, 1.0);
        }
    }

    fn tick(&mut self, tick: &mut BrainTick<'_>) {
        let Some(target) = tick.state.get(types::ATTACK_TARGET).cloned() else {
            return;
        };
        let entity = tick.mob.get_entity();
        entity.look_at(target.get_entity().get_eye_pos());
        entity.velocity.store(self.velocity);
        let hit = tick
            .world
            .get_entities_at_box(&entity.bounding_box.load())
            .into_iter()
            .find(|candidate| Self::may_hit(tick, candidate.as_ref()));
        let Some(hit) = hit else {
            return;
        };
        if entity.has_passenger(hit.get_entity().entity_id) {
            return;
        }
        if let Some(this) = tick.world.get_entity_by_id(entity.entity_id) {
            tick.mob
                .get_mob_entity()
                .try_attack(this.as_ref(), hit.as_ref());
        }
        self.deal_knockback(tick, hit.as_ref());
        self.stop(tick);
    }

    fn stop(&mut self, tick: &mut BrainTick<'_>) {
        tick.state
            .set(types::CHARGE_COOLDOWN_TICKS, self.time_between_attacks);
        tick.state.erase(types::ATTACK_TARGET.id());
    }

    fn debug_name(&self) -> &'static str {
        "ChargeAttack"
    }
}
