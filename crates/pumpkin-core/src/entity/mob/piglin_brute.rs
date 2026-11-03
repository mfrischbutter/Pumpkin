use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicI32, Ordering},
};

use std::sync::Mutex;

use pumpkin_data::Block;
use pumpkin_data::BlockState;
use pumpkin_data::entity::EntityType;
use pumpkin_data::environment_attribute::Activity;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::tracked_data;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::math::position::BlockPos;

use crate::entity::ai::brain::behavior::poi::global_pos;
use crate::entity::ai::brain::memory::{MemoryModuleId, PackedMemories, types};
use crate::entity::ai::brain::sensing::SensorType;
use crate::entity::ai::brain::{Brain, BrainProvider};
use crate::entity::mob::piglin_anger::{self, AbstractPiglin, QueuedChange};
use crate::entity::mob::piglin_brain;
use crate::entity::mob::spawn::SpawnGroupData;
use crate::entity::{
    Entity, EntityBase,
    ai::goal::open_door::OpenDoorGoal,
    mob::{Mob, MobEntity},
};
use crate::world::World;
use crate::world::level_accessor::LevelAccessor;

const MEMORIES: [MemoryModuleId; 9] = [
    types::NEAREST_VISIBLE_ADULT_PIGLINS.id(),
    types::NEARBY_ADULT_PIGLINS.id(),
    types::NEAREST_VISIBLE_NEMESIS.id(),
    types::ANGRY_AT.id(),
    types::ATTACK_TARGET.id(),
    types::ATTACK_COOLING_DOWN.id(),
    types::CANT_REACH_WALK_TARGET_SINCE.id(),
    types::HOME.id(),
    types::UNIVERSAL_ANGER.id(),
];
/// Vanilla `PiglinBruteAi.ACTIVITY_SOUND_LIKELIHOOD_PER_TICK`.
const ACTIVITY_SOUND_LIKELIHOOD_PER_TICK: f32 = 0.0125;

pub struct PiglinBruteEntity {
    pub mob_entity: MobEntity,
    queued: Mutex<Vec<QueuedChange>>,
    pub immune_to_zombification: AtomicBool,
    pub time_in_overworld: AtomicI32,
}

impl PiglinBruteEntity {
    pub const CONVERSION_TIME: i32 = 300;
    pub const XP_REWARD: u32 = 20;

    pub fn new(entity: Entity) -> Arc<Self> {
        let mob_entity = MobEntity::new(entity);
        let piglin = Self {
            mob_entity,
            queued: Mutex::new(Vec::new()),
            immune_to_zombification: AtomicBool::new(false),
            time_in_overworld: AtomicI32::new(0),
        };
        let mob_arc = Arc::new(piglin);
        {
            let goal_selector = &mob_arc.mob_entity.goals_selector;

            // Vanilla `InteractWithDoor`, until the brain opens doors itself.
            goal_selector.add_goal(0, Box::new(OpenDoorGoal::new(true)));
        };

        mob_arc
    }

    #[must_use]
    pub fn is_immune_to_zombification(&self) -> bool {
        self.immune_to_zombification.load(Ordering::Relaxed)
    }

    pub fn set_immune_to_zombification(&self, immune: bool) {
        self.immune_to_zombification
            .store(immune, Ordering::Relaxed);
        self.mob_entity.living_entity.entity.set_synced_data(
            tracked_data::piglin_brute::DATA_IMMUNE_TO_ZOMBIFICATION,
            immune,
        );
    }

    #[must_use]
    pub fn is_converting(&self, world: &World) -> bool {
        !self.is_immune_to_zombification()
            && !self.mob_entity.is_no_ai()
            && world.dimension.piglins_zombify
    }

    #[must_use]
    pub fn check_piglin_brute_spawn_rules(level: &dyn LevelAccessor, pos: &BlockPos) -> bool {
        let below = BlockPos::new(pos.0.x, pos.0.y - 1, pos.0.z);
        let state = level.get_block_state(&below);
        state.id != Block::NETHER_WART_BLOCK.default_state.id
    }

    fn convert_to_zombified(&self) {
        let entity = &self.mob_entity.living_entity.entity;
        let world = entity.world.load();
        let pos = entity.pos.load();

        if world.level_info.load().difficulty != pumpkin_util::Difficulty::Peaceful {
            world.play_sound(
                Sound::EntityPiglinBruteConvertedToZombified,
                SoundCategory::Hostile,
                &pos,
            );
        }

        // Vanilla `AbstractPiglin.finishConversion`: nausea for 10 seconds.
        let _ = crate::entity::mob::conversion::convert_to(
            self,
            &EntityType::ZOMBIFIED_PIGLIN,
            crate::entity::mob::conversion::ConversionParams::single(true, true),
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
}

impl Mob for PiglinBruteEntity {
    /// Vanilla `PiglinBrute.makeBrain`.
    fn make_brain(&self, packed: &PackedMemories) -> Brain {
        BrainProvider {
            memory_types: &MEMORIES,
            sensor_types: &[
                SensorType::NearestLivingEntities,
                SensorType::NearestPlayers,
                SensorType::NearestItems,
                SensorType::HurtBy,
                SensorType::PiglinBruteSpecific,
            ],
            activities: piglin_brain::brute_activities,
        }
        .make_brain(self, packed)
    }

    /// Vanilla `PiglinBrute.customServerAiStep` and `PiglinBruteAi.updateActivity`.
    fn custom_server_ai_step(&self, _caller: &dyn EntityBase) {
        self.mob_entity.tick_brain(self);
        let (target, fighting, switched) = self.mob_entity.with_brain(self, |brain| {
            let old = brain.get_active_non_core_activity();
            piglin_brain::brute_update_activity(brain);
            let new = brain.get_active_non_core_activity();
            (
                brain.get(types::ATTACK_TARGET).cloned(),
                new == Some(Activity::Fight),
                old != new,
            )
        });
        self.mob_entity.set_attacking(target.is_some());
        // Vanilla `getTarget` reads the brain.
        self.mob_entity.set_target(target);
        if fighting && (switched || rand::random::<f32>() < ACTIVITY_SOUND_LIKELIHOOD_PER_TICK) {
            self.play_entity_sound(Sound::EntityPiglinBruteAngry, 1.0, 1.0);
        }
    }

    /// Vanilla `PiglinBrute.finalizeSpawn`: where it was put is home.
    fn finalize_spawn(
        &self,
        world: &Arc<World>,
        group_data: Option<SpawnGroupData>,
    ) -> Option<SpawnGroupData> {
        let home = global_pos(world, self.get_entity().block_pos.load());
        self.mob_entity.with_brain(self, |brain| {
            brain.set(types::HOME, home);
        });
        self.finalize_spawn_super(group_data)
    }

    fn after_hurt(
        &self,
        _damage_type: pumpkin_data::damage::DamageType,
        source: Option<&dyn EntityBase>,
        cause: Option<&dyn EntityBase>,
    ) {
        let Some(id) = cause.or(source).map(|by| by.get_entity().entity_id) else {
            return;
        };
        self.queue(Box::new(move |tick| {
            if let Some(attacker) = tick
                .world
                .get_entity_by_id(id)
                .filter(|attacker| attacker.get_living_entity().is_some())
            {
                piglin_anger::brute_was_hurt_by(tick, &attacker);
            }
        }));
    }

    /// Vanilla `PiglinBrute.getAmbientSound`.
    fn get_ambient_sound(&self) -> Option<pumpkin_data::sound::Sound> {
        Some(pumpkin_data::sound::Sound::EntityPiglinBruteAmbient)
    }

    fn mob_play_step_sound(&self, _pos: &BlockPos, _state: &BlockState) {
        self.play_entity_sound(Sound::EntityPiglinBruteStep, 0.15, 1.0);
    }

    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    fn mob_init_data_tracker(&self) {
        let entity = self.get_entity();
        if self.is_immune_to_zombification() {
            entity.set_synced_data(
                tracked_data::piglin_brute::DATA_IMMUNE_TO_ZOMBIFICATION,
                true,
            );
        }
    }

    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        if self.is_immune_to_zombification() {
            nbt.put_bool("IsImmuneToZombification", true);
        }
        let time_in_overworld = self.time_in_overworld.load(Ordering::Relaxed);
        if time_in_overworld > 0 {
            nbt.put_int("TimeInOverworld", time_in_overworld);
        }
        nbt.put_bool("CanPickUpLoot", true);
    }

    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        // Vanilla `AbstractPiglin.readAdditionalSaveData`: they do unless told otherwise.
        self.mob_entity
            .set_can_pick_up_loot(nbt.get_bool("CanPickUpLoot").unwrap_or(true));
        if let Some(immune) = nbt.get_bool("IsImmuneToZombification") {
            self.set_immune_to_zombification(immune);
        }
        if let Some(time) = nbt.get_int("TimeInOverworld") {
            self.time_in_overworld.store(time, Ordering::Relaxed);
        }
    }

    fn mob_tick(&self, _caller: &dyn EntityBase) {
        let entity = &self.mob_entity.living_entity.entity;
        if !entity.is_alive() {
            return;
        }

        let world = entity.world.load();
        if self.is_converting(&world) {
            let time = self.time_in_overworld.fetch_add(1, Ordering::Relaxed) + 1;
            if time > Self::CONVERSION_TIME {
                self.convert_to_zombified();
            }
        } else {
            self.time_in_overworld.store(0, Ordering::Relaxed);
        }
    }

    fn get_base_experience_reward(&self) -> u32 {
        Self::XP_REWARD
    }
}

impl AbstractPiglin for PiglinBruteEntity {
    fn can_hunt(&self) -> bool {
        false
    }

    fn is_adult(&self) -> bool {
        true
    }

    fn queued_changes(&self) -> &Mutex<Vec<QueuedChange>> {
        &self.queued
    }

    fn hunted_recently(&self) -> bool {
        false
    }
}
