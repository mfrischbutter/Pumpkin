use std::f32::consts::PI;
use std::sync::atomic::AtomicI32;
use std::sync::{Arc, Mutex, Weak};

use pumpkin_data::BlockDirection;
use pumpkin_data::dye_color::DyeColor;
use pumpkin_data::entity::EntityType;
use pumpkin_data::game_event::GameEvent;
use pumpkin_data::sound::Sound;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_util::math::{cos, sin};
use rand::RngExt;

use crate::entity::ai::behavior::fleeing::{self, Flee};
use crate::entity::ai::behavior::hunting::{self, Hunt};
use crate::entity::ai::brain::BrainState;
use crate::entity::ai::target_match::TargetMatch;
use crate::entity::ai::target_predicate::TargetPredicate;
use crate::entity::living::LivingEntity;
use crate::entity::passive::sheep::SheepEntity;
use crate::entity::projectile::evoker_fangs::EvokerFangsEntity;
use crate::entity::{
    Entity, EntityBase,
    ai::goal::{
        Goal, look_at_entity::LookAtEntityGoal, swim::SwimGoal, wander_around::WanderAroundGoal,
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
        vex::VexEntity,
    },
};
use crate::world::game_event::GameEventContext;

const SUMMON_VEX_SPELL: Spell = Spell {
    kind: IllagerSpell::SummonVex,
    cast_warmup_time: DEFAULT_CAST_WARMUP_TIME,
    casting_time: 100,
    casting_interval: 340,
    prepare_sound: Some(Sound::EntityEvokerPrepareSummon),
};
const FANGS_SPELL: Spell = Spell {
    kind: IllagerSpell::Fangs,
    cast_warmup_time: DEFAULT_CAST_WARMUP_TIME,
    casting_time: 40,
    casting_interval: 100,
    prepare_sound: Some(Sound::EntityEvokerPrepareAttack),
};
const WOLOLO_SPELL: Spell = Spell {
    kind: IllagerSpell::Wololo,
    cast_warmup_time: 40,
    casting_time: 60,
    casting_interval: 140,
    prepare_sound: Some(Sound::EntityEvokerPrepareWololo),
};

const VEX_COUNT_RANGE: f64 = 16.0;
/// The summon spell is cast while fewer vexes than a random one up to this many are near.
const MAX_NEARBY_VEXES: usize = 8;
const VEXES_PER_SUMMON: usize = 3;
/// How far from the evoker on each horizontal axis a summoned vex appears.
const VEX_SPREAD: i32 = 2;
const VEX_MIN_LIFE_SECONDS: i32 = 30;
const VEX_EXTRA_LIFE_SECONDS: i32 = 90;
const TICKS_PER_SECOND: i32 = 20;

const WOLOLO_RANGE: f64 = 16.0;
const WOLOLO_SEARCH_HEIGHT: f64 = 4.0;

/// Within this squared distance the fangs ring the evoker instead of running at the target.
const FANGS_RING_DISTANCE_SQ: f64 = 9.0;
const INNER_RING_FANGS: i32 = 5;
const INNER_RING_RADIUS: f64 = 1.5;
const OUTER_RING_FANGS: i32 = 8;
const OUTER_RING_RADIUS: f64 = 2.5;
const OUTER_RING_DELAY: u32 = 3;
const OUTER_RING_ANGLE_OFFSET: f32 = (std::f64::consts::PI * 2.0 / 5.0) as f32;
const FANGS_LINE_LENGTH: u32 = 16;
const FANGS_LINE_SPACING: f64 = 1.25;

const EVOKER_FLEES: &[Flee] = &[
    Flee::new(2, TargetMatch::Type(&EntityType::PLAYER), 8.0, 0.6, 1.0),
    raider::flee_creaking(3, 0.6, 1.0),
];

const EVOKER_HUNTS: &[Hunt] = &[
    Hunt::new(2, TargetMatch::Type(&EntityType::PLAYER), true).forget_unseen_after(300),
    Hunt::new(3, TargetMatch::VILLAGER_LIKE, false).forget_unseen_after(300),
    Hunt::new(3, TargetMatch::Type(&EntityType::IRON_GOLEM), false),
];

/// Represents an Evoker, a spellcasting illager that summons vexes, conjures fangs and turns
/// blue sheep red.
///
/// Wiki: <https://minecraft.wiki/w/Evoker>
pub struct EvokerEntity {
    pub mob_entity: MobEntity,
    pub raider_data: RaiderData,
    spell_casting_tick_count: AtomicI32,
    wololo_target: Mutex<Option<Arc<dyn EntityBase>>>,
}

impl EvokerEntity {
    #[must_use]
    pub fn new(entity: Entity) -> Arc<Self> {
        let mob_entity = MobEntity::new(entity);
        let evoker = Self {
            mob_entity,
            raider_data: RaiderData::default(),
            spell_casting_tick_count: AtomicI32::new(0),
            wololo_target: Mutex::new(None),
        };
        let mob_arc = Arc::new(evoker);
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
            fleeing::apply(goal_selector, EVOKER_FLEES);
            goal_selector.add_goal(4, Box::new(EvokerSummonSpellGoal::new()));
            goal_selector.add_goal(5, Box::new(EvokerAttackSpellGoal::new()));
            goal_selector.add_goal(6, Box::new(EvokerWololoSpellGoal::new()));
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
            hunting::apply(target_selector, &mob_arc.mob_entity, EVOKER_HUNTS);
        };

        mob_arc
    }

    fn wololo_target(&self) -> Option<Arc<dyn EntityBase>> {
        self.wololo_target
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    fn set_wololo_target(&self, target: Option<Arc<dyn EntityBase>>) {
        *self
            .wololo_target
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = target;
    }

    /// Vanilla `Evoker.considersEntityAsAlly`: also the vexes it or its allies own.
    fn considers_as_ally(&self, other: &dyn EntityBase) -> bool {
        let id = self.get_entity().entity_id;
        if other.get_entity().entity_id == id || illager::considers_entity_as_ally(self, other) {
            return true;
        }
        // A summoned vex's owner is an evoker, which has no owner itself, so it is the root.
        let owner = other
            .cast_any()
            .downcast_ref::<VexEntity>()
            .and_then(VexEntity::get_owner);
        let Some(owner) = owner else {
            return false;
        };
        owner.get_entity().entity_id == id
            || illager::considers_entity_as_ally(self, owner.as_ref())
    }

    /// Vanilla `EvokerSummonSpellGoal.performSpellCasting`.
    fn summon_vexes(&self) {
        let entity = self.get_entity();
        let world = entity.world.load_full();
        let team = self.get_team_name();
        let block_pos = entity.block_pos.load();
        let mut rng = self.get_random();
        for _ in 0..VEXES_PER_SUMMON {
            let pos = block_pos.add(
                -VEX_SPREAD + rng.random_range(0..=2 * VEX_SPREAD),
                1,
                -VEX_SPREAD + rng.random_range(0..=2 * VEX_SPREAD),
            );
            let vex = VexEntity::new(Entity::new(world.clone(), pos.to_f64(), &EntityType::VEX));
            vex.finalize_spawn(&world, None);
            vex.set_owner(self);
            vex.set_bound_origin(Some(pos));
            let life_seconds = VEX_MIN_LIFE_SECONDS + rng.random_range(0..VEX_EXTRA_LIFE_SECONDS);
            vex.set_limited_life(TICKS_PER_SECOND * life_seconds);
            if let Some(team) = &team {
                vex.get_entity().join_team(team);
            }
            world.spawn_entity(vex);
            world.emit_game_event_with_context(
                GameEvent::EntityPlace.name(),
                pos.to_centered_f64(),
                &GameEventContext {
                    source_entity: Some(self),
                },
            );
        }
    }

    /// Vanilla `EvokerAttackSpellGoal.performSpellCasting`: rings of fangs around the evoker when
    /// the target is close, else a line of them towards it.
    fn cast_fangs(&self) {
        let Some(target) = self.mob_entity.get_target() else {
            return;
        };
        let pos = self.get_entity().pos.load();
        let target_pos = target.get_entity().pos.load();
        let min_y = target_pos.y.min(pos.y);
        let max_y = target_pos.y.max(pos.y) + 1.0;
        let angle_towards_target = (target_pos.z - pos.z).atan2(target_pos.x - pos.x) as f32;
        if pos.squared_distance_to_vec(&target_pos) < FANGS_RING_DISTANCE_SQ {
            for i in 0..INNER_RING_FANGS {
                let angle = angle_towards_target + i as f32 * PI * 0.4;
                let x = pos.x + f64::from(cos(angle)) * INNER_RING_RADIUS;
                let z = pos.z + f64::from(sin(angle)) * INNER_RING_RADIUS;
                self.create_spell_entity(x, z, min_y, max_y, angle, 0);
            }
            for i in 0..OUTER_RING_FANGS {
                let angle = angle_towards_target
                    + i as f32 * PI * 2.0 / OUTER_RING_FANGS as f32
                    + OUTER_RING_ANGLE_OFFSET;
                let x = pos.x + f64::from(cos(angle)) * OUTER_RING_RADIUS;
                let z = pos.z + f64::from(sin(angle)) * OUTER_RING_RADIUS;
                self.create_spell_entity(x, z, min_y, max_y, angle, OUTER_RING_DELAY);
            }
        } else {
            for i in 0..FANGS_LINE_LENGTH {
                let reach = FANGS_LINE_SPACING * f64::from(i + 1);
                let x = pos.x + f64::from(cos(angle_towards_target)) * reach;
                let z = pos.z + f64::from(sin(angle_towards_target)) * reach;
                self.create_spell_entity(x, z, min_y, max_y, angle_towards_target, i);
            }
        }
    }

    /// Vanilla `EvokerAttackSpellGoal.createSpellEntity`: fangs on the first sturdy floor
    /// between `max_y` and one block below `min_y`.
    fn create_spell_entity(&self, x: f64, z: f64, min_y: f64, max_y: f64, angle: f32, delay: u32) {
        let entity = self.get_entity();
        let world = entity.world.load_full();
        let lowest_y = min_y.floor() as i32 - 1;
        let mut pos = BlockPos::containing(x, max_y, z);
        let floor = loop {
            let below = pos.down();
            if world
                .get_block_state(&below)
                .is_side_solid(BlockDirection::Up)
            {
                break Some(pos);
            }
            pos = below;
            if pos.0.y < lowest_y {
                break None;
            }
        };
        let Some(pos) = floor else {
            return;
        };
        let top_offset = world
            .get_block_state(&pos)
            .get_block_collision_shapes()
            .map(|shape| shape.max.y)
            .reduce(f64::max)
            .unwrap_or(0.0);
        let spawn_pos = Vector3::new(x, f64::from(pos.0.y) + top_offset, z);
        let fangs_entity = Entity::new(world.clone(), spawn_pos, &EntityType::EVOKER_FANGS);
        let fangs = EvokerFangsEntity::new(fangs_entity, delay, angle.to_degrees(), Some(entity));
        world.spawn_entity(Arc::new(fangs));
        world.emit_game_event_with_context(
            GameEvent::EntityPlace.name(),
            spawn_pos,
            &GameEventContext {
                source_entity: Some(self),
            },
        );
    }

    /// Vanilla `EvokerWololoSpellGoal.performSpellCasting`.
    fn wololo(&self) {
        let Some(target) = self.wololo_target() else {
            return;
        };
        let alive = target
            .get_living_entity()
            .is_some_and(LivingEntity::is_alive);
        if alive && let Some(sheep) = target.cast_any().downcast_ref::<SheepEntity>() {
            sheep.set_color(DyeColor::Red.id());
        }
    }
}

fn as_evoker(mob: &dyn Mob) -> Option<&EvokerEntity> {
    mob.cast_any().downcast_ref::<EvokerEntity>()
}

fn is_blue_sheep(entity: &dyn EntityBase) -> bool {
    entity
        .cast_any()
        .downcast_ref::<SheepEntity>()
        .is_some_and(|sheep| sheep.get_color() == DyeColor::Blue.id())
}

impl Mob for EvokerEntity {
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
        Some(self.considers_as_ally(other))
    }

    fn get_ambient_sound(&self) -> Option<Sound> {
        Some(Sound::EntityEvokerAmbient)
    }

    fn custom_server_ai_step(&self, _caller: &dyn EntityBase) {
        self.tick_spell_casting();
    }

    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        self.write_spellcaster_nbt(nbt);
    }

    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        self.read_spellcaster_nbt(nbt);
    }
}

impl PatrollingMonster for EvokerEntity {
    fn get_patrol_data(&self) -> &PatrolData {
        &self.raider_data.patrol_data
    }
}

impl Raider for EvokerEntity {
    fn get_raider_data(&self) -> &RaiderData {
        &self.raider_data
    }

    fn get_celebrate_sound(&self) -> Sound {
        Sound::EntityEvokerCelebrate
    }
}

impl SpellcasterIllager for EvokerEntity {
    fn spell_casting_tick_count(&self) -> &AtomicI32 {
        &self.spell_casting_tick_count
    }

    fn get_casting_sound_event(&self) -> Sound {
        Sound::EntityEvokerCastSpell
    }

    /// Vanilla `EvokerCastingSpellGoal.tick`: the sheep it turns when it has no target.
    fn casting_look_target(&self) -> Option<Arc<dyn EntityBase>> {
        self.mob_entity
            .get_target()
            .or_else(|| self.wololo_target())
    }
}

/// Vanilla `Evoker.EvokerSummonSpellGoal`: three vexes while fewer than a random 1 to 8 are near.
struct EvokerSummonSpellGoal {
    use_spell: SpellcasterUseSpellGoal,
    vex_count_targeting: TargetPredicate,
}

impl EvokerSummonSpellGoal {
    fn new() -> Self {
        Self {
            use_spell: SpellcasterUseSpellGoal::new(SUMMON_VEX_SPELL),
            vex_count_targeting: TargetPredicate::create_non_attackable()
                .set_base_max_distance(VEX_COUNT_RANGE)
                .ignore_visibility()
                .ignore_distance_scaling_factor(),
        }
    }
}

impl Goal for EvokerSummonSpellGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        let Some(evoker) = as_evoker(mob) else {
            return false;
        };
        if !self.use_spell.can_use(evoker) {
            return false;
        }
        let entity = evoker.get_entity();
        let world = entity.world.load();
        let search =
            entity
                .bounding_box
                .load()
                .expand(VEX_COUNT_RANGE, VEX_COUNT_RANGE, VEX_COUNT_RANGE);
        let targeting = &self.vex_count_targeting;
        let vexes = world
            .get_entities_at_box(&search)
            .into_iter()
            .filter(|candidate| {
                candidate.get_entity().entity_type == &EntityType::VEX
                    && targeting.test(&world, Some(evoker), candidate.as_ref())
            })
            .count();
        mob.get_random().random_range(0..MAX_NEARBY_VEXES) + 1 > vexes
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        let Some(evoker) = as_evoker(mob) else {
            return false;
        };
        self.use_spell.can_continue_to_use(evoker)
    }

    fn start(&mut self, mob: &dyn Mob) {
        if let Some(evoker) = as_evoker(mob) {
            self.use_spell.start(evoker);
        }
    }

    fn tick(&mut self, mob: &dyn Mob) {
        if let Some(evoker) = as_evoker(mob) {
            self.use_spell.tick(evoker, || evoker.summon_vexes());
        }
    }
}

/// Vanilla `Evoker.EvokerAttackSpellGoal`: fangs at the target.
struct EvokerAttackSpellGoal {
    use_spell: SpellcasterUseSpellGoal,
}

impl EvokerAttackSpellGoal {
    const fn new() -> Self {
        Self {
            use_spell: SpellcasterUseSpellGoal::new(FANGS_SPELL),
        }
    }
}

impl Goal for EvokerAttackSpellGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        let Some(evoker) = as_evoker(mob) else {
            return false;
        };
        self.use_spell.can_use(evoker)
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        let Some(evoker) = as_evoker(mob) else {
            return false;
        };
        self.use_spell.can_continue_to_use(evoker)
    }

    fn start(&mut self, mob: &dyn Mob) {
        if let Some(evoker) = as_evoker(mob) {
            self.use_spell.start(evoker);
        }
    }

    fn tick(&mut self, mob: &dyn Mob) {
        if let Some(evoker) = as_evoker(mob) {
            self.use_spell.tick(evoker, || evoker.cast_fangs());
        }
    }
}

/// Vanilla `Evoker.EvokerWololoSpellGoal`: turns a blue sheep nearby red while it has no target.
struct EvokerWololoSpellGoal {
    use_spell: SpellcasterUseSpellGoal,
    wololo_targeting: TargetPredicate,
}

impl EvokerWololoSpellGoal {
    fn new() -> Self {
        Self {
            use_spell: SpellcasterUseSpellGoal::new(WOLOLO_SPELL),
            wololo_targeting: TargetPredicate::create_non_attackable()
                .set_base_max_distance(WOLOLO_RANGE),
        }
    }
}

impl Goal for EvokerWololoSpellGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        let Some(evoker) = as_evoker(mob) else {
            return false;
        };
        if evoker.mob_entity.get_target().is_some()
            || evoker.is_casting_spell()
            || !self.use_spell.is_interval_over(evoker)
        {
            return false;
        }
        let entity = evoker.get_entity();
        let world = entity.world.load();
        if !world.level_info.load().game_rules.mob_griefing {
            return false;
        }
        let search =
            entity
                .bounding_box
                .load()
                .expand(WOLOLO_RANGE, WOLOLO_SEARCH_HEIGHT, WOLOLO_RANGE);
        let targeting = &self.wololo_targeting;
        let mut sheep: Vec<_> = world
            .get_entities_at_box(&search)
            .into_iter()
            .filter(|candidate| {
                is_blue_sheep(candidate.as_ref())
                    && targeting.test(&world, Some(evoker), candidate.as_ref())
            })
            .collect();
        if sheep.is_empty() {
            return false;
        }
        let index = mob.get_random().random_range(0..sheep.len());
        evoker.set_wololo_target(Some(sheep.swap_remove(index)));
        true
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        let Some(evoker) = as_evoker(mob) else {
            return false;
        };
        evoker.wololo_target().is_some() && self.use_spell.is_warming_up()
    }

    fn start(&mut self, mob: &dyn Mob) {
        if let Some(evoker) = as_evoker(mob) {
            self.use_spell.start(evoker);
        }
    }

    fn stop(&mut self, mob: &dyn Mob) {
        if let Some(evoker) = as_evoker(mob) {
            evoker.set_wololo_target(None);
        }
    }

    fn tick(&mut self, mob: &dyn Mob) {
        if let Some(evoker) = as_evoker(mob) {
            self.use_spell.tick(evoker, || evoker.wololo());
        }
    }
}
