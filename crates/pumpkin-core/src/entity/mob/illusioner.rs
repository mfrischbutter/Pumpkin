use std::sync::atomic::AtomicI32;
use std::sync::{Arc, Weak};

use pumpkin_data::data_component_impl::EquipmentSlot;
use pumpkin_data::effect::StatusEffect;
use pumpkin_data::entity::EntityType;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::potion::Effect;
use pumpkin_data::sound::Sound;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::Difficulty;

use crate::entity::ai::behavior::fleeing::{self, Flee};
use crate::entity::ai::behavior::hunting::{self, Hunt};
use crate::entity::ai::brain::BrainState;
use crate::entity::ai::target_match::TargetMatch;
use crate::entity::mob::equipment::RegionalDifficulty;
use crate::entity::mob::spawn::SpawnGroupData;
use crate::entity::{
    Entity, EntityBase,
    ai::goal::{
        Goal, bow_attack::BowAttackGoal, look_at_entity::LookAtEntityGoal, swim::SwimGoal,
        wander_around::WanderAroundGoal,
    },
    mob::{
        Mob, MobEntity, illager,
        patrol::{LongDistancePatrolGoal, PatrolData, PatrollingMonster},
        raider::{
            self, ObtainRaidLeaderBannerGoal, PathfindToRaidGoal, Raider, RaiderCelebrationGoal,
            RaiderData, RaiderMoveThroughVillageGoal,
        },
        spellcaster_illager::{
            DEFAULT_CAST_WARMUP_TIME, IllagerSpell, Spell, SpellcasterCastingSpellGoal,
            SpellcasterIllager, SpellcasterUseSpellGoal,
        },
    },
};
use crate::world::World;

const MIRROR_SPELL: Spell = Spell {
    kind: IllagerSpell::Disappear,
    cast_warmup_time: DEFAULT_CAST_WARMUP_TIME,
    casting_time: 20,
    casting_interval: 340,
    prepare_sound: Some(Sound::EntityIllusionerPrepareMirror),
};
const BLINDNESS_SPELL: Spell = Spell {
    kind: IllagerSpell::Blindness,
    cast_warmup_time: DEFAULT_CAST_WARMUP_TIME,
    casting_time: 20,
    casting_interval: 180,
    prepare_sound: Some(Sound::EntityIllusionerPrepareBlindness),
};
const INVISIBILITY_DURATION: i32 = 1200;
const BLINDNESS_DURATION: i32 = 400;

const ILLUSIONER_FLEES: &[Flee] = &[raider::flee_creaking(3, 1.0, 1.2)];

const ILLUSIONER_HUNTS: &[Hunt] = &[
    Hunt::new(2, TargetMatch::Type(&EntityType::PLAYER), true).forget_unseen_after(300),
    Hunt::new(3, TargetMatch::VILLAGER_LIKE, false).forget_unseen_after(300),
    Hunt::new(3, TargetMatch::Type(&EntityType::IRON_GOLEM), false).forget_unseen_after(300),
];

/// Represents an Illusioner, a spellcasting illager with a bow that turns invisible and blinds
/// its target.
///
/// Wiki: <https://minecraft.wiki/w/Illusioner>
pub struct IllusionerEntity {
    pub mob_entity: MobEntity,
    pub raider_data: RaiderData,
    spell_casting_tick_count: AtomicI32,
}

impl IllusionerEntity {
    #[must_use]
    pub fn new(entity: Entity) -> Arc<Self> {
        let mob_entity = MobEntity::new(entity);
        let illusioner = Self {
            mob_entity,
            raider_data: RaiderData::default(),
            spell_casting_tick_count: AtomicI32::new(0),
        };
        let mob_arc = Arc::new(illusioner);
        let mob_weak: Weak<dyn Mob> = {
            let mob_arc: Arc<dyn Mob> = mob_arc.clone();
            Arc::downgrade(&mob_arc)
        };

        {
            let goal_selector = &mob_arc.mob_entity.goals_selector;

            // Vanilla registers PatrollingMonster's and Raider's goals first; the insertion
            // order breaks priority ties.
            goal_selector.add_goal(4, Box::new(LongDistancePatrolGoal::new(0.7, 0.595)));
            goal_selector.add_goal(1, Box::new(ObtainRaidLeaderBannerGoal::default()));
            goal_selector.add_goal(3, Box::new(PathfindToRaidGoal::default()));
            goal_selector.add_goal(4, Box::new(RaiderMoveThroughVillageGoal::new(1.05)));
            goal_selector.add_goal(5, Box::new(RaiderCelebrationGoal));
            goal_selector.add_goal(0, Box::new(SwimGoal::default()));
            goal_selector.add_goal(1, Box::new(SpellcasterCastingSpellGoal::<Self>::default()));
            fleeing::apply(goal_selector, ILLUSIONER_FLEES);
            goal_selector.add_goal(4, Box::new(IllusionerMirrorSpellGoal::new()));
            goal_selector.add_goal(5, Box::new(IllusionerBlindnessSpellGoal::new()));
            goal_selector.add_goal(6, Box::new(BowAttackGoal::new(0.5, 20, 15.0)));
            goal_selector.add_goal(8, Box::new(WanderAroundGoal::new(0.6)));
            goal_selector.add_goal(
                9,
                Box::new(LookAtEntityGoal::new(
                    mob_weak.clone(),
                    &EntityType::PLAYER,
                    3.0,
                    1.0,
                    false,
                )),
            );
            goal_selector.add_goal(
                10,
                Box::new(LookAtEntityGoal::matching(
                    mob_weak,
                    TargetMatch::Mob,
                    8.0,
                    0.02,
                    false,
                )),
            );

            let target_selector = &mob_arc.mob_entity.target_selector;
            target_selector.add_goal(1, raider::revenge_goal());
            hunting::apply(target_selector, &mob_arc.mob_entity, ILLUSIONER_HUNTS);
        };

        mob_arc
    }
}

/// Vanilla `new MobEffectInstance(effect, duration)`.
const fn effect(effect_type: &'static StatusEffect, duration: i32) -> Effect {
    Effect {
        effect_type,
        duration,
        amplifier: 0,
        ambient: false,
        show_particles: true,
        show_icon: true,
        blend: false,
    }
}

fn as_illusioner(mob: &dyn Mob) -> Option<&IllusionerEntity> {
    mob.cast_any().downcast_ref::<IllusionerEntity>()
}

impl Mob for IllusionerEntity {
    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    fn as_patrolling_monster(&self) -> Option<&dyn PatrollingMonster> {
        Some(self)
    }

    fn as_raider(&self) -> Option<&dyn Raider> {
        Some(self)
    }

    fn can_attack(&self, target: &dyn EntityBase) -> bool {
        illager::can_attack(self, target)
    }

    fn mob_considers_entity_as_ally(
        &self,
        other: &dyn EntityBase,
        _brain: Option<&BrainState>,
    ) -> Option<bool> {
        Some(illager::considers_entity_as_ally(self, other))
    }

    fn get_ambient_sound(&self) -> Option<Sound> {
        Some(Sound::EntityIllusionerAmbient)
    }

    fn custom_server_ai_step(&self, _caller: &dyn EntityBase) {
        self.tick_spell_casting();
    }

    fn finalize_spawn(
        &self,
        _world: &Arc<World>,
        group_data: Option<SpawnGroupData>,
    ) -> Option<SpawnGroupData> {
        self.mob_entity
            .set_item_slot(&EquipmentSlot::MAIN_HAND, ItemStack::new(1, &Item::BOW));
        self.finalize_spawn_super(group_data)
    }

    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        self.write_spellcaster_nbt(nbt);
    }

    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        self.read_spellcaster_nbt(nbt);
    }
}

impl PatrollingMonster for IllusionerEntity {
    fn get_patrol_data(&self) -> &PatrolData {
        &self.raider_data.patrol_data
    }
}

impl Raider for IllusionerEntity {
    fn get_raider_data(&self) -> &RaiderData {
        &self.raider_data
    }

    fn get_celebrate_sound(&self) -> Sound {
        Sound::EntityIllusionerAmbient
    }
}

impl SpellcasterIllager for IllusionerEntity {
    fn spell_casting_tick_count(&self) -> &AtomicI32 {
        &self.spell_casting_tick_count
    }

    fn get_casting_sound_event(&self) -> Sound {
        Sound::EntityIllusionerCastSpell
    }
}

/// Vanilla `Illusioner.IllusionerMirrorSpellGoal`: turns invisible, which the client shows as
/// mirror images.
struct IllusionerMirrorSpellGoal {
    use_spell: SpellcasterUseSpellGoal,
}

impl IllusionerMirrorSpellGoal {
    const fn new() -> Self {
        Self {
            use_spell: SpellcasterUseSpellGoal::new(MIRROR_SPELL),
        }
    }
}

impl Goal for IllusionerMirrorSpellGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        let Some(illusioner) = as_illusioner(mob) else {
            return false;
        };
        self.use_spell.can_use(illusioner)
            && !illusioner
                .mob_entity
                .living_entity
                .has_effect(&StatusEffect::INVISIBILITY)
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        let Some(illusioner) = as_illusioner(mob) else {
            return false;
        };
        self.use_spell.can_continue_to_use(illusioner)
    }

    fn start(&mut self, mob: &dyn Mob) {
        if let Some(illusioner) = as_illusioner(mob) {
            self.use_spell.start(illusioner);
        }
    }

    fn tick(&mut self, mob: &dyn Mob) {
        let Some(illusioner) = as_illusioner(mob) else {
            return;
        };
        self.use_spell.tick(illusioner, || {
            let living = &illusioner.mob_entity.living_entity;
            living.add_effect(effect(&StatusEffect::INVISIBILITY, INVISIBILITY_DURATION));
        });
    }
}

/// Vanilla `Illusioner.IllusionerBlindnessSpellGoal`: blinds each new target once, from above
/// normal regional difficulty.
struct IllusionerBlindnessSpellGoal {
    use_spell: SpellcasterUseSpellGoal,
    last_target_id: i32,
}

impl IllusionerBlindnessSpellGoal {
    const fn new() -> Self {
        Self {
            use_spell: SpellcasterUseSpellGoal::new(BLINDNESS_SPELL),
            last_target_id: 0,
        }
    }
}

impl Goal for IllusionerBlindnessSpellGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        let Some(illusioner) = as_illusioner(mob) else {
            return false;
        };
        if !self.use_spell.can_use(illusioner) {
            return false;
        }
        let Some(target) = illusioner.mob_entity.get_target() else {
            return false;
        };
        if target.get_entity().entity_id == self.last_target_id {
            return false;
        }
        let entity = illusioner.get_entity();
        let difficulty = RegionalDifficulty::at(&entity.world.load(), entity.pos.load());
        difficulty.effective_difficulty > f32::from(Difficulty::Normal as u8)
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        let Some(illusioner) = as_illusioner(mob) else {
            return false;
        };
        self.use_spell.can_continue_to_use(illusioner)
    }

    fn start(&mut self, mob: &dyn Mob) {
        let Some(illusioner) = as_illusioner(mob) else {
            return;
        };
        self.use_spell.start(illusioner);
        if let Some(target) = illusioner.mob_entity.get_target() {
            self.last_target_id = target.get_entity().entity_id;
        }
    }

    fn tick(&mut self, mob: &dyn Mob) {
        let Some(illusioner) = as_illusioner(mob) else {
            return;
        };
        self.use_spell.tick(illusioner, || {
            if let Some(target) = illusioner.mob_entity.get_target()
                && let Some(living) = target.get_living_entity()
            {
                living.add_effect(effect(&StatusEffect::BLINDNESS, BLINDNESS_DURATION));
            }
        });
    }
}
