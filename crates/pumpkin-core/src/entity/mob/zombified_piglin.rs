use std::ops::RangeInclusive;
use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};

use pumpkin_data::BlockState;
use pumpkin_data::attributes::Attributes;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::math::position::BlockPos;

use crate::entity::ai::behavior::neutral::apply_targets;
use crate::entity::ai::goal::{
    revenge::RevengeGoal, wander_around::WanderAroundGoal, zombie_attack::ZombieAttackGoal,
};
use crate::entity::ai::pathfinder::node::PathType;
use crate::entity::ai::util::goal_utils;
use crate::entity::attributes::{Modifier, ModifierOperation, send_attribute_updates_for_living};
use crate::entity::custom_sound::CustomSound;
use crate::entity::mob::neutral::{NeutralData, NeutralMob};
use crate::entity::mob::zombie::ZombieEntityBase;
use crate::entity::mob::{Mob, MobEntity};
use crate::entity::{Entity, EntityBase};
use crate::plugin::api::events::entity::entity_target::TargetReason;
use crate::plugin::api::events::entity::pig_zombie_anger::PigZombieAngerEvent;

/// Vanilla `SPEED_MODIFIER_ATTACKING_ID`.
const SPEED_MODIFIER_ATTACKING_ID: &str = "minecraft:attacking";
/// Vanilla `SPEED_MODIFIER_ATTACKING`: added to the movement speed while angry.
const SPEED_MODIFIER_ATTACKING: f64 = 0.05;
/// Vanilla `FIRST_ANGER_SOUND_DELAY`: 0 to 1 seconds.
const FIRST_ANGER_SOUND_DELAY: RangeInclusive<i32> = 0..=20;
/// Vanilla `ALERT_INTERVAL`: 4 to 6 seconds.
const ALERT_INTERVAL: RangeInclusive<i32> = 80..=120;
/// Vanilla constructor: `setPathfindingMalus(PathType.LAVA, 8.0F)`.
const LAVA_PATHFINDING_MALUS: f32 = 8.0;
/// Vanilla `playAngerSound`: `getSoundVolume() * 2.0F`, `getVoicePitch() * 1.8F`.
const ANGER_SOUND_VOLUME_SCALE: f32 = 2.0;
const ANGER_SOUND_PITCH_SCALE: f32 = 1.8;

/// Vanilla `ZombifiedPiglin`: a zombie that stays neutral until provoked.
pub struct ZombifiedPiglinEntity {
    entity: Arc<ZombieEntityBase>,
    neutral_data: NeutralData,
    /// Vanilla `playFirstAngerSoundIn`.
    play_first_anger_sound_in: AtomicI32,
    /// Vanilla `ticksUntilNextAlert`.
    ticks_until_next_alert: AtomicI32,
}

impl ZombifiedPiglinEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let entity = ZombieEntityBase::without_behaviour_goals(entity);
        {
            // Vanilla `addBehaviourGoals`.
            let mob = &entity.mob_entity;
            let goal_selector = &mob.goals_selector;
            let target_selector = &mob.target_selector;

            goal_selector.add_goal(2, ZombieAttackGoal::new(1.0, false));
            goal_selector.add_goal(7, Box::new(WanderAroundGoal::water_avoiding(1.0)));

            target_selector.add_goal(1, Box::new(RevengeGoal::new(true).alerting_others()));
            apply_targets(target_selector, mob, 2, 3, true);
        };
        entity
            .mob_entity
            .navigator
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .set_pathfinding_malus(PathType::Lava, LAVA_PATHFINDING_MALUS);

        Arc::new(Self {
            entity,
            neutral_data: NeutralData::default(),
            play_first_anger_sound_in: AtomicI32::new(0),
            ticks_until_next_alert: AtomicI32::new(0),
        })
    }

    fn has_attacking_speed(&self) -> bool {
        self.entity
            .mob_entity
            .living_entity
            .attributes
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&Attributes::MOVEMENT_SPEED.id)
            .is_some_and(|speed| {
                speed
                    .modifiers
                    .iter()
                    .any(|modifier| modifier.id == SPEED_MODIFIER_ATTACKING_ID)
            })
    }

    fn set_attacking_speed(&self, attacking: bool) {
        let living = &self.entity.mob_entity.living_entity;
        living.update_attribute(&Attributes::MOVEMENT_SPEED, |speed| {
            if attacking {
                speed.add_or_replace_modifier(Modifier {
                    id: SPEED_MODIFIER_ATTACKING_ID.to_string(),
                    amount: SPEED_MODIFIER_ATTACKING,
                    operation: ModifierOperation::Add,
                });
            } else {
                speed.remove_modifier(SPEED_MODIFIER_ATTACKING_ID);
            }
        });
        send_attribute_updates_for_living(living, vec![Attributes::MOVEMENT_SPEED]);
    }

    fn maybe_play_first_anger_sound(&self) {
        let remaining = self.play_first_anger_sound_in.load(Ordering::Relaxed);
        if remaining > 0 {
            self.play_first_anger_sound_in
                .store(remaining - 1, Ordering::Relaxed);
            if remaining == 1 {
                self.play_anger_sound();
            }
        }
    }

    fn play_anger_sound(&self) {
        let pitch = self.entity.mob_entity.living_entity.get_pitch();
        self.play_entity_sound(
            Sound::EntityZombifiedPiglinAngry,
            self.sound_volume() * ANGER_SOUND_VOLUME_SCALE,
            pitch * ANGER_SOUND_PITCH_SCALE,
        );
    }

    fn maybe_alert_others(&self, target: &Arc<dyn EntityBase>) {
        let remaining = self.ticks_until_next_alert.load(Ordering::Relaxed);
        if remaining > 0 {
            self.ticks_until_next_alert
                .store(remaining - 1, Ordering::Relaxed);
            return;
        }

        if self.has_line_of_sight(target.get_entity()) {
            self.alert_others(target);
        }

        self.ticks_until_next_alert
            .store(rand::random_range(ALERT_INTERVAL), Ordering::Relaxed);
    }

    /// Hands the current target to nearby zombified piglins that have none of their own.
    fn alert_others(&self, target: &Arc<dyn EntityBase>) {
        for other in goal_utils::nearby_same_type(self) {
            let Some(other_mob) = other.get_mob() else {
                continue;
            };
            if other_mob.get_mob_entity().get_target().is_some()
                || other.is_allied_to(target.as_ref())
            {
                continue;
            }
            other_mob.set_mob_target(Some(target.clone()), TargetReason::PigZombieTarget);
        }
    }
}

impl NeutralMob for ZombifiedPiglinEntity {
    fn get_neutral_data(&self) -> &NeutralData {
        &self.neutral_data
    }

    fn approve_grudge(&self, target: &dyn EntityBase, ticks: i32) -> Option<i32> {
        let entity = self.get_entity();
        let Some(server) = entity.world.load().server.upgrade() else {
            return Some(ticks);
        };
        let mut event =
            PigZombieAngerEvent::new(entity.entity_id, Some(target.get_entity().entity_id), ticks);
        server.plugin_manager.fire_blocking(&server, &mut event);
        (!event.cancelled).then_some(event.new_anger)
    }

    fn updates_anger_in_ai_step(&self) -> bool {
        true
    }
}

impl CustomSound for ZombifiedPiglinEntity {
    fn hurt_sound(&self) -> Option<Sound> {
        self.get_entity().entity_type.hurt_sound
    }

    fn death_sound(&self) -> Option<Sound> {
        self.get_entity().entity_type.death_sound
    }

    fn sound_category(&self) -> SoundCategory {
        self.mob_sound_source()
    }
}

impl Mob for ZombifiedPiglinEntity {
    fn mob_play_step_sound(&self, _pos: &BlockPos, _state: &BlockState) {
        self.play_entity_sound(Sound::EntityZombieStep, 0.15, 1.0);
    }

    fn get_mob_entity(&self) -> &MobEntity {
        &self.entity.mob_entity
    }

    fn spawn_as_baby(&self) -> bool {
        self.entity.spawn_as_baby()
    }

    fn as_neutral(&self) -> Option<&dyn NeutralMob> {
        Some(self)
    }

    fn as_custom_sound(&self) -> Option<&dyn CustomSound> {
        Some(self)
    }

    fn get_ambient_sound(&self) -> Option<Sound> {
        Some(if self.is_angry() {
            Sound::EntityZombifiedPiglinAngry
        } else {
            Sound::EntityZombifiedPiglinAmbient
        })
    }

    /// Vanilla `setTarget`: a first target restarts the anger sound and alert timers.
    fn apply_mob_target(&self, target: Option<Arc<dyn EntityBase>>) {
        let mob_entity = self.get_mob_entity();
        if target.is_some() && mob_entity.get_target().is_none() {
            self.play_first_anger_sound_in.store(
                rand::random_range(FIRST_ANGER_SOUND_DELAY),
                Ordering::Relaxed,
            );
            self.ticks_until_next_alert
                .store(rand::random_range(ALERT_INTERVAL), Ordering::Relaxed);
        }
        mob_entity.set_target(target);
    }

    fn custom_server_ai_step(&self, _caller: &dyn EntityBase) {
        if self.is_angry() {
            if !self.entity.is_baby() && !self.has_attacking_speed() {
                self.set_attacking_speed(true);
            }
            self.maybe_play_first_anger_sound();
        } else if self.has_attacking_speed() {
            self.set_attacking_speed(false);
        }

        self.update_persistent_anger();
        if let Some(target) = self.get_mob_entity().get_target() {
            self.maybe_alert_others(&target);
        }
    }

    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        self.entity.mob_write_nbt(nbt);
    }

    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        self.entity.mob_read_nbt(nbt);
    }
}
