use std::sync::{
    Arc, Weak,
    atomic::{AtomicBool, AtomicI32, Ordering},
};

use pumpkin_data::BlockState;
use pumpkin_data::entity::EntityType;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::Sound;
use pumpkin_data::tag::{self, Tag};
use pumpkin_util::math::position::BlockPos;

use crate::entity::ai::behavior::hunting::{self, Hunt};
use crate::entity::ai::target_match::TargetMatch;
use crate::entity::{
    Entity, EntityBase,
    ageable::{AgeableData, AgeableMob},
    ai::behavior::neutral::apply_targets,
    ai::goal::{
        Controls, Goal,
        active_target::{ActiveTargetGoal, TargetCondition},
        escape_danger::EscapeDangerGoal,
        follow_parent::FollowParentGoal,
        look_around::RandomLookAroundGoal,
        look_at_entity::LookAtEntityGoal,
        melee_attack::MeleeAttackGoal,
        revenge::RevengeGoal,
        swim::SwimGoal,
        wander_around::WanderAroundGoal,
    },
    mob::{
        Mob, MobEntity,
        neutral::{NeutralData, NeutralMob},
    },
    passive::animal::Animal,
    player::Player,
};

/// Vanilla `playWarningSound`: ticks until the warning can sound again.
const WARNING_SOUND_INTERVAL: i32 = 40;
/// Vanilla `PolarBearMeleeAttackGoal`: stands up this many ticks before the attack is ready.
const STAND_UP_TICKS: i32 = 10;
/// Vanilla `PolarBearMeleeAttackGoal`: warns within the target's width plus this many blocks.
const WARNING_DISTANCE: f32 = 3.0;
/// Vanilla `PolarBearAttackPlayersGoal`: search interval, cub search box and follow distance.
const ATTACK_PLAYERS_INTERVAL: i32 = 20;
const CUB_SEARCH_XZ: f64 = 8.0;
const CUB_SEARCH_Y: f64 = 4.0;
const ATTACK_PLAYERS_RANGE_SCALE: f64 = 0.5;

fn is_baby(mob: &dyn Mob) -> bool {
    mob.as_ageable().is_some_and(AgeableMob::is_baby)
}

/// Vanilla `registerGoals`: cubs panic at any harm, adults only at environmental harm.
fn panic_causes(mob: &dyn Mob) -> &'static Tag {
    if is_baby(mob) {
        &tag::DamageType::MINECRAFT_PANIC_CAUSES
    } else {
        &tag::DamageType::MINECRAFT_PANIC_ENVIRONMENTAL_CAUSES
    }
}

const POLAR_BEAR_HUNTS: &[Hunt] =
    &[Hunt::new(4, TargetMatch::Type(&EntityType::FOX), true).when(TargetCondition::Adult)];

pub struct PolarBearEntity {
    pub mob_entity: MobEntity,
    pub ageable_data: AgeableData,
    pub neutral_data: NeutralData,
    pub standing: AtomicBool,
    warning_sound_ticks: AtomicI32,
}

impl PolarBearEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let mob_entity = MobEntity::new(entity);
        let polar_bear = Self {
            mob_entity,
            ageable_data: AgeableData::default(),
            neutral_data: NeutralData::default(),
            standing: AtomicBool::new(false),
            warning_sound_ticks: AtomicI32::new(0),
        };
        let mob_arc = Arc::new(polar_bear);
        let mob_weak: Weak<dyn Mob> = {
            let mob_arc: Arc<dyn Mob> = mob_arc.clone();
            Arc::downgrade(&mob_arc)
        };

        {
            let goal_selector = &mob_arc.mob_entity.goals_selector;

            goal_selector.add_goal(0, Box::new(SwimGoal::default()));
            goal_selector.add_goal(1, Box::new(PolarBearMeleeAttackGoal::new()));
            goal_selector.add_goal(1, EscapeDangerGoal::with_panic_causes(2.0, panic_causes));
            goal_selector.add_goal(4, Box::new(FollowParentGoal::new(1.25)));
            goal_selector.add_goal(5, Box::new(WanderAroundGoal::new(1.0)));
            goal_selector.add_goal(
                6,
                LookAtEntityGoal::with_default(mob_weak, &EntityType::PLAYER, 6.0),
            );
            goal_selector.add_goal(7, Box::new(RandomLookAroundGoal::default()));
        };

        {
            let target_selector = &mob_arc.mob_entity.target_selector;

            target_selector.add_goal(
                1,
                Box::new(
                    RevengeGoal::new(true)
                        .raising_alarm_when(is_baby)
                        .alerting_only(|other| !is_baby(other)),
                ),
            );
            target_selector.add_goal(
                2,
                Box::new(PolarBearAttackPlayersGoal::new(&mob_arc.mob_entity)),
            );
            apply_targets(target_selector, &mob_arc.mob_entity, 3, 5, false);
            hunting::apply(target_selector, &mob_arc.mob_entity, POLAR_BEAR_HUNTS);
        };

        mob_arc
    }

    #[must_use]
    pub fn is_standing(&self) -> bool {
        self.standing.load(Ordering::Relaxed)
    }

    pub fn set_standing(&self, standing: bool) {
        self.standing.store(standing, Ordering::Relaxed);
        let entity = self.get_entity();
        entity.set_synced_data(
            pumpkin_data::tracked_data::polar_bear::DATA_STANDING_ID,
            standing,
        );
    }

    /// Vanilla `playWarningSound`.
    fn play_warning_sound(&self) {
        if self.warning_sound_ticks.load(Ordering::Relaxed) <= 0 {
            self.make_sound(Sound::EntityPolarBearWarning);
            self.warning_sound_ticks
                .store(WARNING_SOUND_INTERVAL, Ordering::Relaxed);
        }
    }
}

impl AgeableMob for PolarBearEntity {
    fn get_ageable_data(&self) -> &AgeableData {
        &self.ageable_data
    }

    fn default_baby_spawn_chance(&self) -> f32 {
        crate::entity::ageable::POLAR_BEAR_BABY_SPAWN_CHANCE
    }
}

impl Animal for PolarBearEntity {
    fn is_food(&self, _item_stack: &ItemStack) -> bool {
        false
    }
}

impl NeutralMob for PolarBearEntity {
    fn get_neutral_data(&self) -> &NeutralData {
        &self.neutral_data
    }
}

impl Mob for PolarBearEntity {
    fn mob_play_step_sound(&self, _pos: &BlockPos, _state: &BlockState) {
        self.play_entity_sound(Sound::EntityPolarBearStep, 0.15, 1.0);
    }

    fn get_ambient_sound(&self) -> Option<Sound> {
        Some(if self.is_baby() {
            Sound::EntityPolarBearAmbientBaby
        } else {
            Sound::EntityPolarBearAmbient
        })
    }

    /// Vanilla `tick`: the warning cooldown runs down after the AI step.
    fn post_tick(&self) {
        if self.warning_sound_ticks.load(Ordering::Relaxed) > 0 {
            self.warning_sound_ticks.fetch_sub(1, Ordering::Relaxed);
        }
    }

    fn as_ageable(&self) -> Option<&dyn AgeableMob> {
        Some(self)
    }

    fn as_animal(&self) -> Option<&dyn Animal> {
        Some(self)
    }

    fn as_neutral(&self) -> Option<&dyn NeutralMob> {
        Some(self)
    }

    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    fn mob_init_data_tracker(&self) {
        let entity = self.get_entity();
        let is_baby = entity.age.load(Ordering::Relaxed) < 0;
        if is_baby {
            entity.set_synced_data(pumpkin_data::tracked_data::polar_bear::DATA_BABY_ID, true);
        }
        entity.set_synced_data(
            pumpkin_data::tracked_data::polar_bear::DATA_STANDING_ID,
            self.is_standing(),
        );
    }

    fn mob_interact(&self, player: &Arc<Player>, item_stack: &mut ItemStack) -> bool {
        self.animal_interact(player, item_stack, Sound::EntityPolarBearAmbient)
    }
}

/// Vanilla `PolarBearMeleeAttackGoal`: the bear rears up and warns before it strikes.
struct PolarBearMeleeAttackGoal {
    melee_attack_goal: MeleeAttackGoal,
}

impl PolarBearMeleeAttackGoal {
    fn new() -> Self {
        Self {
            melee_attack_goal: MeleeAttackGoal::new(1.25, true),
        }
    }

    /// Vanilla `checkAndPerformAttack`: hits without a swing, stands up close to the target.
    fn check_and_perform_attack(
        &mut self,
        bear: &PolarBearEntity,
        mob: &dyn Mob,
        target: &dyn EntityBase,
    ) {
        let goal = &mut self.melee_attack_goal;
        let warning_distance = target.get_entity().width() + WARNING_DISTANCE;
        let distance_sq = mob
            .get_entity()
            .pos
            .load()
            .squared_distance_to_vec(&target.get_entity().pos.load());
        if goal.can_perform_attack(mob, target) {
            goal.reset_attack_cooldown();
            mob.do_hurt_target(target);
            bear.set_standing(false);
        } else if distance_sq < f64::from(warning_distance * warning_distance) {
            if goal.is_time_to_attack() {
                bear.set_standing(false);
                goal.reset_attack_cooldown();
            }
            if goal.cooldown <= STAND_UP_TICKS {
                bear.set_standing(true);
                bear.play_warning_sound();
            }
        } else {
            goal.reset_attack_cooldown();
            bear.set_standing(false);
        }
    }
}

fn as_polar_bear(mob: &dyn Mob) -> Option<&PolarBearEntity> {
    mob.cast_any().downcast_ref::<PolarBearEntity>()
}

impl Goal for PolarBearMeleeAttackGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        self.melee_attack_goal.can_start(mob)
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        self.melee_attack_goal.should_continue(mob)
    }

    fn start(&mut self, mob: &dyn Mob) {
        self.melee_attack_goal.start(mob);
    }

    fn stop(&mut self, mob: &dyn Mob) {
        if let Some(bear) = as_polar_bear(mob) {
            bear.set_standing(false);
        }
        self.melee_attack_goal.stop(mob);
    }

    fn tick(&mut self, mob: &dyn Mob) {
        let Some(target) = self.melee_attack_goal.tick_chase(mob) else {
            return;
        };
        if let Some(bear) = as_polar_bear(mob) {
            self.check_and_perform_attack(bear, mob, target.as_ref());
        }
    }

    fn should_run_every_tick(&self) -> bool {
        self.melee_attack_goal.should_run_every_tick()
    }

    fn controls(&self) -> Controls {
        self.melee_attack_goal.controls()
    }
}

/// Vanilla `PolarBearAttackPlayersGoal`: adults go for players near a cub.
struct PolarBearAttackPlayersGoal {
    active_target_goal: Box<ActiveTargetGoal>,
}

impl PolarBearAttackPlayersGoal {
    fn new(mob: &MobEntity) -> Self {
        let players = TargetMatch::Type(&EntityType::PLAYER);
        Self {
            active_target_goal: ActiveTargetGoal::with_default(mob, players, true)
                .every(ATTACK_PLAYERS_INTERVAL)
                .scale_range(ATTACK_PLAYERS_RANGE_SCALE),
        }
    }

    fn has_cub_nearby(mob: &dyn Mob) -> bool {
        let entity = mob.get_entity();
        let bounding_box = entity.bounding_box.load();
        let search_box = bounding_box.expand(CUB_SEARCH_XZ, CUB_SEARCH_Y, CUB_SEARCH_XZ);
        let world = entity.world.load();
        world
            .get_entities_at_box(&search_box)
            .iter()
            .filter_map(|other| other.get_mob())
            .any(|other| {
                other.get_entity().entity_type == &EntityType::POLAR_BEAR && is_baby(other)
            })
    }
}

impl Goal for PolarBearAttackPlayersGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        !is_baby(mob) && self.active_target_goal.can_start(mob) && Self::has_cub_nearby(mob)
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        self.active_target_goal.should_continue(mob)
    }

    fn start(&mut self, mob: &dyn Mob) {
        self.active_target_goal.start(mob);
    }

    fn stop(&mut self, mob: &dyn Mob) {
        self.active_target_goal.stop(mob);
    }

    fn controls(&self) -> Controls {
        self.active_target_goal.controls()
    }
}
