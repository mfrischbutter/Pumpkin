use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::{Arc, Mutex};

use crossbeam::atomic::AtomicCell;
use pumpkin_data::attributes::Attributes;
use pumpkin_data::damage::DamageType;
use pumpkin_data::entity::EntityType;
use pumpkin_data::environment_attribute::Activity;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::tag;
use pumpkin_data::tag::Taggable;
use pumpkin_data::{Block, BlockState};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::math::boundingbox::EntityDimensions;
use pumpkin_util::math::position::BlockPos;
use rand::RngExt;

use super::hoglin_ai::{
    MEMORIES, activities, attack_and_throw, piglins_outnumber_hoglins, remember_entity,
    set_attack_target, take_remembered, throw_target,
};
use super::{Mob, MobEntity};
use crate::entity::ageable::{AgeableData, AgeableMob};
use crate::entity::ai::brain::behavior::utils::{
    get_target_nearest_me, is_other_target_much_further_away_than_current_attack_target,
};
use crate::entity::ai::brain::memory::{PackedMemories, types};
use crate::entity::ai::brain::sensing::SensorType;
use crate::entity::ai::brain::{Brain, BrainProvider, BrainState};
use crate::entity::passive::animal::Animal;
use crate::entity::player::Player;
use crate::entity::{Entity, EntityBase};
use crate::world::World;
use crate::world::level_accessor::LevelAccessor;

/// Vanilla `Hoglin.BABY_ATTACK_DAMAGE`.
const BABY_ATTACK_DAMAGE: f64 = 0.5;
const ATTACK_DAMAGE: f64 = 6.0;

pub struct HoglinEntity {
    pub mob_entity: MobEntity,
    ageable_data: AgeableData,
    pub immune_to_zombification: AtomicBool,
    pub time_in_overworld: AtomicI32,
    pub cannot_be_hunted: AtomicBool,
    /// Who this hoglin hit and who hurt it, see `hoglin_ai::remember_entity`.
    hit_target: Mutex<Option<Arc<dyn EntityBase>>>,
    hurt_by: Mutex<Option<Arc<dyn EntityBase>>>,
    /// The brain's `NEAREST_REPELLENT`, for `get_walk_target_value` inside its tick.
    repellent: AtomicCell<Option<BlockPos>>,
}

impl HoglinEntity {
    pub const CONVERSION_TIME: i32 = 300;
    pub const XP_REWARD: u32 = 5;
    pub const BABY_XP_REWARD: u32 = 3;
    /// Vanilla `Hoglin.BABY_DIMENSIONS`.
    pub const BABY_DIMENSIONS: EntityDimensions = EntityDimensions {
        width: 0.75,
        height: 0.85,
        eye_height: 0.625,
    };

    pub fn new(entity: Entity) -> Arc<Self> {
        let mob_entity = MobEntity::new(entity);
        {
            let mut attributes = mob_entity
                .living_entity
                .attributes
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(health) = attributes.get_mut(&Attributes::MAX_HEALTH.id) {
                health.base_value = 40.0;
                health.dirty.store(true, Ordering::Relaxed);
            }
            if let Some(speed) = attributes.get_mut(&Attributes::MOVEMENT_SPEED.id) {
                speed.base_value = 0.3;
                speed.dirty.store(true, Ordering::Relaxed);
            }
            if let Some(knockback_res) = attributes.get_mut(&Attributes::KNOCKBACK_RESISTANCE.id) {
                knockback_res.base_value = 0.6;
                knockback_res.dirty.store(true, Ordering::Relaxed);
            }
            if let Some(attack_kb) = attributes.get_mut(&Attributes::ATTACK_KNOCKBACK.id) {
                attack_kb.base_value = 1.0;
                attack_kb.dirty.store(true, Ordering::Relaxed);
            }
            if let Some(damage) = attributes.get_mut(&Attributes::ATTACK_DAMAGE.id) {
                damage.base_value = 6.0;
                damage.dirty.store(true, Ordering::Relaxed);
            }
        }
        mob_entity.living_entity.health.store(40.0);

        let hoglin = Self {
            mob_entity,
            ageable_data: AgeableData::default(),
            immune_to_zombification: AtomicBool::new(false),
            time_in_overworld: AtomicI32::new(0),
            cannot_be_hunted: AtomicBool::new(false),
            hit_target: Mutex::new(None),
            hurt_by: Mutex::new(None),
            repellent: AtomicCell::new(None),
        };
        Arc::new(hoglin)
    }

    #[must_use]
    pub fn is_immune_to_zombification(&self) -> bool {
        self.immune_to_zombification.load(Ordering::Relaxed)
    }

    pub fn set_immune_to_zombification(&self, immune: bool) {
        self.immune_to_zombification
            .store(immune, Ordering::Relaxed);
        self.mob_entity.living_entity.entity.set_synced_data(
            pumpkin_data::tracked_data::hoglin::DATA_IMMUNE_TO_ZOMBIFICATION,
            immune,
        );
    }

    #[must_use]
    pub fn is_converting(&self, world: &World) -> bool {
        !self.is_immune_to_zombification()
            && !self.mob_entity.is_no_ai()
            && world.dimension.piglins_zombify
    }

    /// Vanilla `Hoglin.canBeHunted`.
    #[must_use]
    pub fn can_be_hunted(&self) -> bool {
        !self.is_baby() && !self.cannot_be_hunted.load(Ordering::Relaxed)
    }

    fn finish_conversion(&self) {
        // Vanilla `Hoglin.finishConversion`: nausea for 10 seconds.
        let _ = crate::entity::mob::conversion::convert_to(
            self,
            &EntityType::ZOGLIN,
            crate::entity::mob::conversion::ConversionParams::single(true, false),
            |converted| {
                converted
                    .get_mob_entity()
                    .living_entity
                    .add_effect(pumpkin_data::potion::Effect {
                        effect_type: &pumpkin_data::effect::StatusEffect::NAUSEA,
                        duration: 200,
                        amplifier: 0,
                        ambient: false,
                        show_particles: true,
                        show_icon: true,
                        blend: false,
                    });
            },
        );
    }

    /// Vanilla `HoglinAi.getSoundForCurrentActivity`.
    fn sound_for_activity(
        &self,
        activity: Option<Activity>,
        near_repellent: bool,
    ) -> Option<Sound> {
        let world = self.get_entity().world.load();
        let activity = activity?;
        Some(
            if activity == Activity::Avoid || self.is_converting(&world) {
                Sound::EntityHoglinRetreat
            } else if activity == Activity::Fight {
                Sound::EntityHoglinAngry
            } else if near_repellent {
                Sound::EntityHoglinRetreat
            } else {
                Sound::EntityHoglinAmbient
            },
        )
    }

    /// Vanilla `HoglinAi.updateActivity`.
    fn update_activity(&self, brain: &mut BrainState) {
        let old = brain.get_active_non_core_activity();
        brain.set_active_activity_to_first_valid(&[
            Activity::Fight,
            Activity::Avoid,
            Activity::Idle,
        ]);
        let new = brain.get_active_non_core_activity();
        if old != new
            && let Some(sound) =
                self.sound_for_activity(new, brain.has_memory_value(types::NEAREST_REPELLENT.id()))
        {
            self.play_entity_sound(sound, 1.0, 1.0);
        }
        self.mob_entity
            .set_attacking(brain.has_memory_value(types::ATTACK_TARGET.id()));
    }

    /// Vanilla `HoglinAi.onHitTarget`.
    fn on_hit_target(&self, brain: &mut BrainState, target: &Arc<dyn EntityBase>) {
        if self.is_baby() {
            return;
        }
        if target.get_entity().entity_type == &EntityType::PIGLIN
            && piglins_outnumber_hoglins(brain)
        {
            self.set_avoid_target(brain, target.clone());
            Self::broadcast_retreat(brain, target);
        } else {
            Self::broadcast_attack_target(brain, target);
        }
    }

    fn for_each_visible_adult_hoglin(
        brain: &BrainState,
        mut action: impl FnMut(&Self, &mut BrainState),
    ) {
        let hoglins = brain
            .get(types::NEAREST_VISIBLE_ADULT_HOGLINS)
            .cloned()
            .unwrap_or_default();
        for other in hoglins {
            if let Some(other) = other.cast_any().downcast_ref::<Self>() {
                other
                    .mob_entity
                    .try_with_brain(|other_brain| action(other, other_brain));
            }
        }
    }

    fn broadcast_retreat(brain: &BrainState, target: &Arc<dyn EntityBase>) {
        Self::for_each_visible_adult_hoglin(brain, |other, other_brain| {
            other.retreat_from_nearest_target(other_brain, target.clone());
        });
    }

    /// Vanilla `HoglinAi.retreatFromNearestTarget`.
    fn retreat_from_nearest_target(&self, brain: &mut BrainState, target: Arc<dyn EntityBase>) {
        let mut nearest = target;
        for memory in [types::AVOID_TARGET, types::ATTACK_TARGET] {
            if let Some(current) = brain.get(memory).cloned() {
                nearest = get_target_nearest_me(self, current, nearest);
            }
        }
        self.set_avoid_target(brain, nearest);
    }

    /// Vanilla `HoglinAi.setAvoidTarget`: 5 to 20 seconds.
    fn set_avoid_target(&self, brain: &mut BrainState, target: Arc<dyn EntityBase>) {
        brain.erase(types::ATTACK_TARGET.id());
        brain.erase(types::WALK_TARGET.id());
        let duration = self.get_random().random_range(100..=400);
        brain.set_with_expiry(types::AVOID_TARGET, target, duration);
    }

    /// Vanilla `HoglinAi.wasHurtBy`.
    fn was_hurt_by(&self, brain: &mut BrainState, attacker: Arc<dyn EntityBase>) {
        brain.erase(types::PACIFIED.id());
        brain.erase(types::BREED_TARGET.id());
        if self.is_baby() {
            self.retreat_from_nearest_target(brain, attacker);
        } else {
            self.maybe_retaliate(brain, &attacker);
        }
    }

    /// Vanilla `HoglinAi.maybeRetaliate`.
    fn maybe_retaliate(&self, brain: &mut BrainState, attacker: &Arc<dyn EntityBase>) {
        let kind = attacker.get_entity().entity_type;
        if (brain.is_active(Activity::Avoid) && kind == &EntityType::PIGLIN)
            || kind == &EntityType::HOGLIN
            || is_other_target_much_further_away_than_current_attack_target(
                brain,
                self,
                attacker.as_ref(),
                4.0,
            )
            || !self.can_attack(attacker.as_ref())
        {
            return;
        }
        set_attack_target(brain, attacker.clone());
        Self::broadcast_attack_target(brain, attacker);
    }

    fn broadcast_attack_target(brain: &BrainState, target: &Arc<dyn EntityBase>) {
        Self::for_each_visible_adult_hoglin(brain, |other, other_brain| {
            other.set_attack_target_if_closer_than_current(other_brain, target);
        });
    }

    /// Vanilla `HoglinAi.setAttackTargetIfCloserThanCurrent`.
    fn set_attack_target_if_closer_than_current(
        &self,
        brain: &mut BrainState,
        target: &Arc<dyn EntityBase>,
    ) {
        if brain.has_memory_value(types::PACIFIED.id()) {
            return;
        }
        let nearest = brain.get(types::ATTACK_TARGET).cloned().map_or_else(
            || target.clone(),
            |current| get_target_nearest_me(self, current, target.clone()),
        );
        set_attack_target(brain, nearest);
    }
}

impl AgeableMob for HoglinEntity {
    fn get_ageable_data(&self) -> &AgeableData {
        &self.ageable_data
    }

    /// Vanilla `Hoglin.ageBoundaryReached`.
    fn age_boundary_reached(&self) {
        let living = &self.mob_entity.living_entity;
        let baby = self.is_baby();
        living.set_attribute_base(
            &Attributes::ATTACK_DAMAGE,
            if baby {
                BABY_ATTACK_DAMAGE
            } else {
                ATTACK_DAMAGE
            },
        );
        living.entity.set_default_dimensions(if baby {
            Self::BABY_DIMENSIONS
        } else {
            Entity::type_dimensions(living.entity.entity_type)
        });
    }
}

impl Animal for HoglinEntity {
    fn is_food(&self, item_stack: &ItemStack) -> bool {
        item_stack.item.has_tag(&tag::Item::MINECRAFT_HOGLIN_FOOD)
    }

    /// Vanilla `Hoglin.getBreedOffspring`.
    fn get_breed_offspring(&self, _partner: &dyn EntityBase) -> Arc<dyn EntityBase> {
        let entity = self.get_entity();
        let baby = Self::new(Entity::new(
            entity.world.load_full(),
            entity.pos.load(),
            &EntityType::HOGLIN,
        ));
        baby.mob_entity.set_persistence_required();
        baby
    }
}

impl Mob for HoglinEntity {
    /// Vanilla `Hoglin.getWalkTargetValue`.
    fn get_walk_target_value(&self, pos: &BlockPos, level: &dyn LevelAccessor) -> f32 {
        let near_repellent = self.repellent.load().is_some_and(|repellent| {
            let delta = repellent.0 - pos.0;
            i64::from(delta.x).pow(2) + i64::from(delta.y).pow(2) + i64::from(delta.z).pow(2) < 64
        });
        if near_repellent {
            -1.0
        } else if level.get_block(&pos.down()) == &Block::CRIMSON_NYLIUM {
            10.0
        } else {
            0.0
        }
    }

    /// Vanilla `Hoglin.removeWhenFarAway`.
    fn remove_when_far_away(&self, _distance_sq: f64) -> bool {
        true
    }

    fn mob_play_step_sound(&self, _pos: &BlockPos, _state: &BlockState) {
        self.play_entity_sound(Sound::EntityHoglinStep, 0.15, 1.0);
    }

    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    fn as_ageable(&self) -> Option<&dyn AgeableMob> {
        Some(self)
    }

    fn as_animal(&self) -> Option<&dyn Animal> {
        Some(self)
    }

    /// Vanilla `Hoglin.getSoundSource`.
    fn mob_sound_source(&self) -> SoundCategory {
        SoundCategory::Hostile
    }

    /// Vanilla `Hoglin.getAmbientSound`.
    fn get_ambient_sound(&self) -> Option<Sound> {
        self.mob_entity
            .try_with_brain(|brain| {
                self.sound_for_activity(
                    brain.get_active_non_core_activity(),
                    brain.has_memory_value(types::NEAREST_REPELLENT.id()),
                )
            })
            .flatten()
    }

    fn make_brain(&self, packed: &PackedMemories) -> Brain {
        BrainProvider {
            memory_types: &MEMORIES,
            sensor_types: &[
                SensorType::NearestLivingEntities,
                SensorType::NearestPlayers,
                SensorType::NearestAdult,
                SensorType::HoglinSpecific,
            ],
            activities,
        }
        .make_brain(self, packed)
    }

    /// Vanilla `Hoglin.customServerAiStep`.
    fn custom_server_ai_step(&self, _caller: &dyn EntityBase) {
        self.mob_entity.tick_brain(self);
        let hit_target = take_remembered(&self.hit_target);
        let hurt_by = take_remembered(&self.hurt_by);
        self.mob_entity.with_brain(self, |brain| {
            if let Some(target) = hit_target {
                self.on_hit_target(brain, &target);
            }
            if let Some(attacker) = hurt_by {
                self.was_hurt_by(brain, attacker);
            }
            self.update_activity(brain);
            self.repellent
                .store(brain.get(types::NEAREST_REPELLENT).copied());
            // Vanilla `getTarget` reads the brain.
            self.mob_entity
                .set_target(brain.get(types::ATTACK_TARGET).cloned());
        });

        let world = self.get_entity().world.load();
        if self.is_converting(&world) {
            let time = self.time_in_overworld.fetch_add(1, Ordering::Relaxed) + 1;
            if time > Self::CONVERSION_TIME {
                self.play_entity_sound(Sound::EntityHoglinConvertedToZombified, 1.0, 1.0);
                self.finish_conversion();
            }
        } else {
            self.time_in_overworld.store(0, Ordering::Relaxed);
        }
    }

    /// Vanilla `Hoglin.doHurtTarget`.
    fn do_hurt_target(&self, target: &dyn EntityBase) -> bool {
        remember_entity(&self.hit_target, self.get_entity(), target);
        attack_and_throw(self, target, Sound::EntityHoglinAttack)
    }

    /// Vanilla `Hoglin.blockedByItem`.
    fn blocked_by_item(&self, defender: &dyn EntityBase) {
        if !self.is_baby() {
            throw_target(self, defender);
        }
    }

    /// Vanilla `Hoglin.hurtServer`, past the damage.
    fn on_damage(&self, _damage_type: DamageType, source: Option<&dyn EntityBase>) {
        if let Some(source) = source.filter(|source| source.get_living_entity().is_some()) {
            remember_entity(&self.hurt_by, self.get_entity(), source);
        }
    }

    /// Vanilla `Hoglin.finalizeSpawn`.
    fn finalize_spawn(
        &self,
        _world: &Arc<World>,
        group_data: Option<crate::entity::mob::spawn::SpawnGroupData>,
    ) -> Option<crate::entity::mob::spawn::SpawnGroupData> {
        if self.get_random().random::<f32>() < 0.2 {
            self.set_baby(true);
        }
        self.finalize_spawn_super(group_data)
    }

    fn spawn_as_baby(&self) -> bool {
        self.set_baby(true);
        true
    }

    fn get_base_experience_reward(&self) -> u32 {
        if self.is_baby() {
            Self::BABY_XP_REWARD
        } else {
            Self::XP_REWARD
        }
    }

    fn should_drop_loot(&self) -> bool {
        self.mob_entity.living_entity.should_drop_loot()
    }

    fn should_drop_experience(&self) -> bool {
        true
    }

    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        nbt.put_bool("IsImmuneToZombification", self.is_immune_to_zombification());
        nbt.put_int(
            "TimeInOverworld",
            self.time_in_overworld.load(Ordering::Relaxed),
        );
        nbt.put_bool(
            "CannotBeHunted",
            self.cannot_be_hunted.load(Ordering::Relaxed),
        );
    }

    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        self.set_immune_to_zombification(nbt.get_bool("IsImmuneToZombification").unwrap_or(false));
        self.time_in_overworld.store(
            nbt.get_int("TimeInOverworld").unwrap_or(0),
            Ordering::Relaxed,
        );
        self.cannot_be_hunted.store(
            nbt.get_bool("CannotBeHunted").unwrap_or(false),
            Ordering::Relaxed,
        );
    }

    /// Vanilla `Hoglin.mobInteract`: being fed makes it stay; a pacified one will not eat.
    fn mob_interact(&self, player: &Arc<Player>, item_stack: &mut ItemStack) -> bool {
        let pacified = self
            .mob_entity
            .try_with_brain(|brain| brain.has_memory_value(types::PACIFIED.id()))
            .unwrap_or(false);
        if pacified && self.is_food(item_stack) {
            return false;
        }
        let consumed = self.animal_interact(player, item_stack);
        if consumed {
            self.mob_entity.set_persistence_required();
        }
        consumed
    }
}
