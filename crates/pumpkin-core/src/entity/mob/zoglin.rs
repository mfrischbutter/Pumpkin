use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use pumpkin_data::BlockState;
use pumpkin_data::attributes::Attributes;
use pumpkin_data::entity::EntityType;
use pumpkin_data::sound::Sound;
use pumpkin_data::tracked_data;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::math::position::BlockPos;

use crate::entity::ai::brain::activity::ActivityData;
use crate::entity::ai::brain::behavior::fight::{
    melee_attack, set_walk_target_from_attack_target, start_attacking,
    stop_attacking_if_target_invalid,
};
use crate::entity::ai::brain::behavior::utils::is_other_target_much_further_away_than_current_attack_target;
use crate::entity::ai::brain::behavior::{
    BehaviorControl, LookAtTargetSink, MoveToTargetSink, Timed, set_entity_look_target_sometimes,
};
use crate::entity::ai::brain::memory::{MemoryModuleId, PackedMemories, types};
use crate::entity::ai::brain::sensing::{SensorType, is_entity_attackable};
use crate::entity::ai::brain::{Brain, BrainProvider, BrainTick};
use crate::entity::mob::hoglin_ai::{
    ATTACK_INTERVAL, BABY_ATTACK_INTERVAL, attack_and_throw, idle_movement, is_adult_mob,
    is_baby_mob, remember_entity, set_attack_target, take_remembered, throw_target,
};
use crate::entity::mob::{Mob, MobEntity, MobMetadata};
use crate::entity::{Entity, EntityBase};
use crate::world::World;
use pumpkin_data::damage::DamageType;
use pumpkin_data::environment_attribute::Activity;

pub struct ZoglinEntity {
    pub mob_entity: MobEntity,
    pub is_baby: AtomicBool,
    /// Who hurt it, for the next brain step: that can happen inside its own brain tick.
    hurt_by: Mutex<Option<Arc<dyn EntityBase>>>,
}

impl ZoglinEntity {
    pub const XP_REWARD: u32 = 5;
    const BABY_ATTACK_DAMAGE: f64 = 0.5;

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

        Arc::new(Self {
            mob_entity,
            is_baby: AtomicBool::new(false),
            hurt_by: Mutex::new(None),
        })
    }

    #[must_use]
    pub fn is_baby(&self) -> bool {
        self.is_baby.load(Ordering::Relaxed)
    }

    /// Vanilla `Zoglin.setBaby`; growing up keeps the lowered attack damage, as in vanilla.
    pub fn set_baby(&self, baby: bool) {
        self.mob_entity
            .set_baby_flag(&self.is_baby, tracked_data::zoglin::DATA_BABY_ID, baby);
        let living = &self.mob_entity.living_entity;
        living
            .entity
            .set_default_dimensions(Entity::age_dimensions(living.entity.entity_type, baby));
        if baby {
            living.set_attribute_base(&Attributes::ATTACK_DAMAGE, Self::BABY_ATTACK_DAMAGE);
        }
    }
}

impl Mob for ZoglinEntity {
    fn mob_play_step_sound(&self, _pos: &BlockPos, _state: &BlockState) {
        self.play_entity_sound(Sound::EntityZoglinStep, 0.15, 1.0);
    }

    fn mob_metadata(&self) -> Option<MobMetadata> {
        self.is_baby().then(|| MobMetadata::new().baby(true))
    }

    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    fn finalize_spawn(
        &self,
        _world: &Arc<World>,
        group_data: Option<crate::entity::mob::spawn::SpawnGroupData>,
    ) -> Option<crate::entity::mob::spawn::SpawnGroupData> {
        if rand::random::<f32>() < 0.2 {
            self.set_baby(true);
        }
        self.mob_entity.finalize_spawn_base();
        group_data
    }

    fn spawn_as_baby(&self) -> bool {
        self.set_baby(true);
        true
    }

    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        nbt.put_bool("IsBaby", self.is_baby());
    }

    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        self.set_baby(nbt.get_bool("IsBaby").unwrap_or(false));
    }

    /// Vanilla `Zoglin.doHurtTarget`.
    fn do_hurt_target(&self, target: &dyn EntityBase) -> bool {
        attack_and_throw(self, target, Sound::EntityZoglinAttack)
    }

    /// Vanilla `Zoglin.blockedByItem`.
    fn blocked_by_item(&self, defender: &dyn EntityBase) {
        if !self.is_baby() {
            throw_target(self, defender);
        }
    }

    /// Vanilla `Zoglin.hurtServer`, past the damage.
    fn on_damage(&self, _damage_type: DamageType, source: Option<&dyn EntityBase>) {
        if let Some(source) = source.filter(|source| source.get_living_entity().is_some()) {
            remember_entity(&self.hurt_by, self.get_entity(), source);
        }
    }

    /// Vanilla `Zoglin.getAmbientSound`.
    fn get_ambient_sound(&self) -> Option<Sound> {
        let angry = self
            .mob_entity
            .try_with_brain(|brain| brain.has_memory_value(types::ATTACK_TARGET.id()))?;
        Some(if angry {
            Sound::EntityZoglinAngry
        } else {
            Sound::EntityZoglinAmbient
        })
    }

    fn make_brain(&self, packed: &PackedMemories) -> Brain {
        BrainProvider {
            memory_types: &MEMORIES,
            sensor_types: &[
                SensorType::NearestLivingEntities,
                SensorType::NearestPlayers,
            ],
            activities,
        }
        .make_brain(self, packed)
    }

    /// Vanilla `Zoglin.customServerAiStep` with `updateActivity`.
    fn custom_server_ai_step(&self, _caller: &dyn EntityBase) {
        self.mob_entity.tick_brain(self);
        let hurt_by = take_remembered(&self.hurt_by);
        self.mob_entity.with_brain(self, |brain| {
            if let Some(attacker) = hurt_by
                && self.can_attack(attacker.as_ref())
                && !is_other_target_much_further_away_than_current_attack_target(
                    brain,
                    self,
                    attacker.as_ref(),
                    4.0,
                )
            {
                set_attack_target(brain, attacker);
            }
            let old = brain.get_active_non_core_activity();
            brain.set_active_activity_to_first_valid(&[Activity::Fight, Activity::Idle]);
            let new = brain.get_active_non_core_activity();
            if new == Some(Activity::Fight) && old != Some(Activity::Fight) {
                self.play_entity_sound(Sound::EntityZoglinAngry, 1.0, 1.0);
            }
            let target = brain.get(types::ATTACK_TARGET).cloned();
            self.mob_entity.set_attacking(target.is_some());
            // Vanilla `Zoglin.getTarget` reads the brain.
            self.mob_entity.set_target(target);
        });
    }
}

/// Vanilla `Zoglin.findNearestValidAttackTarget`.
fn find_attack_target(tick: &BrainTick<'_>) -> Option<Arc<dyn EntityBase>> {
    let ctx = tick.visibility();
    tick.state
        .get(types::NEAREST_VISIBLE_LIVING_ENTITIES)?
        .find_closest(&ctx, |target| {
            let kind = target.get_entity().entity_type;
            kind != &EntityType::ZOGLIN
                && kind != &EntityType::CREEPER
                && is_entity_attackable(&ctx, target.as_ref())
        })
}

const MEMORIES: [MemoryModuleId; 1] = [types::ATTACK_COOLING_DOWN.id()];

fn activities(_mob: &dyn Mob) -> Vec<ActivityData> {
    let idle: Vec<Box<dyn BehaviorControl>> = vec![
        Box::new(start_attacking(find_attack_target)),
        Box::new(set_entity_look_target_sometimes::create(8.0, 30, 60)),
        Box::new(idle_movement()),
    ];
    let fight: Vec<Box<dyn BehaviorControl>> = vec![
        Box::new(set_walk_target_from_attack_target(|_| 1.0)),
        Box::new(melee_attack(ATTACK_INTERVAL).only_if(is_adult_mob)),
        Box::new(melee_attack(BABY_ATTACK_INTERVAL).only_if(is_baby_mob)),
        Box::new(stop_attacking_if_target_invalid(|_, _| {})),
    ];
    vec![
        ActivityData::with_priority_start(
            Activity::Core,
            0,
            vec![
                Box::new(Timed::new(LookAtTargetSink::new(45, 90))),
                Box::new(Timed::new(MoveToTargetSink::default())),
            ],
        ),
        ActivityData::with_priority_start(Activity::Idle, 10, idle),
        ActivityData::with_gate_memory(Activity::Fight, 10, fight, types::ATTACK_TARGET.id()),
    ]
}
