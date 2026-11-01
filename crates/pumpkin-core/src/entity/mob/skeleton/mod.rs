use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};

use pumpkin_data::BlockState;
use pumpkin_data::effect::StatusEffect;
use pumpkin_data::entity::EntityType;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::Sound;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::math::position::BlockPos;

use crate::entity::{
    Entity, EntityBase,
    ai::behavior::fleeing::{self, Flee},
    ai::behavior::hunting::{self, Hunt},
    ai::goal::{
        bow_attack::BowAttackGoal, flee_sun::FleeSunGoal, look_around::RandomLookAroundGoal,
        look_at_entity::LookAtEntityGoal, melee_attack::MeleeAttackGoal,
        restrict_sun::RestrictSunGoal, revenge::RevengeGoal, swim::SwimGoal,
        wander_around::WanderAroundGoal,
    },
    ai::target_match::TargetMatch,
    mob::{Mob, MobEntity, conversion::ConversionTracker, spawn::SpawnGroupData},
    player::Player,
    projectile::arrow::ArrowEntity,
};
use crate::world::World;

pub mod bogged;
pub mod parched;
#[allow(clippy::module_inception)]
pub mod skeleton;
pub mod stray;
pub mod wither;

const SKELETON_FLEES: &[Flee] = &[Flee::new(
    3,
    TargetMatch::Type(&EntityType::WOLF),
    6.0,
    1.0,
    1.2,
)];

const SKELETON_HUNTS: &[Hunt] = &[
    Hunt::new(2, TargetMatch::Type(&EntityType::PLAYER), true),
    Hunt::new(3, TargetMatch::Type(&EntityType::IRON_GOLEM), true),
    hunting::baby_turtles(3, true),
];

/// Vanilla `AbstractSkeleton.getAttackInterval` and `getHardAttackInterval`.
const ATTACK_INTERVAL: i32 = 40;
const HARD_ATTACK_INTERVAL: i32 = 20;
/// The longer intervals of the bogged and the parched (`INCREASED_*_ATTACK_INTERVAL`).
const INCREASED_ATTACK_INTERVAL: i32 = 70;
const INCREASED_HARD_ATTACK_INTERVAL: i32 = 50;

/// What sets one `AbstractSkeleton` subclass apart from the others.
pub struct SkeletonKind {
    pub step_sound: Sound,
    pub ambient_sound: Sound,
    pub attack_interval: i32,
    pub hard_attack_interval: i32,
    /// The effect and its duration in ticks the kind's arrows carry (`getArrow`).
    pub arrow_effect: Option<(&'static StatusEffect, i32)>,
    /// Target goals vanilla registers before `super.registerGoals()`.
    pub extra_hunts: &'static [Hunt],
}

impl SkeletonKind {
    #[must_use]
    pub fn of(entity_type: &EntityType) -> &'static Self {
        match entity_type.id {
            id if id == EntityType::STRAY.id => &stray::STRAY,
            id if id == EntityType::BOGGED.id => &bogged::BOGGED,
            id if id == EntityType::PARCHED.id => &parched::PARCHED,
            id if id == EntityType::WITHER_SKELETON.id => &wither::WITHER_SKELETON,
            _ => &skeleton::SKELETON,
        }
    }
}

/// Vanilla `AbstractSkeleton` and its subclasses, told apart by [`SkeletonKind`].
pub struct SkeletonEntityBase {
    pub mob_entity: MobEntity,
    pub kind: &'static SkeletonKind,
    /// Vanilla `Skeleton.freezingTracker`: a skeleton in powder snow becomes a stray.
    freezing: Option<ConversionTracker>,
    /// Vanilla `Bogged.DATA_SHEARED`.
    sheared: AtomicBool,
}

impl SkeletonEntityBase {
    pub fn new(entity: Entity) -> Arc<Self> {
        let kind = SkeletonKind::of(entity.entity_type);
        let is_skeleton = entity.entity_type.id == EntityType::SKELETON.id;
        let is_wither = entity.entity_type.id == EntityType::WITHER_SKELETON.id;
        let mut mob_entity = MobEntity::new(entity);
        if is_wither {
            wither::set_lava_malus(&mut mob_entity);
        }
        let mob = Self {
            mob_entity,
            kind,
            freezing: is_skeleton.then(skeleton::freezing_tracker),
            sheared: AtomicBool::new(false),
        };
        let mob_arc = Arc::new(mob);
        let mob_weak: Weak<dyn Mob> = {
            let mob_arc: Arc<dyn Mob> = mob_arc.clone();
            Arc::downgrade(&mob_arc)
        };
        {
            let goal_selector = &mob_arc.mob_entity.goals_selector;
            let target_selector = &mob_arc.mob_entity.target_selector;

            goal_selector.add_goal(0, Box::new(SwimGoal::default()));
            goal_selector.add_goal(2, Box::new(RestrictSunGoal::new()));
            goal_selector.add_goal(3, Box::new(FleeSunGoal::new(1.0)));
            fleeing::apply(goal_selector, SKELETON_FLEES);
            // Vanilla `reassessWeaponGoal`: the bow or the melee goal at 4, by the held weapon.
            goal_selector.add_goal(
                4,
                Box::new(
                    BowAttackGoal::new(1.0, kind.attack_interval, 15.0)
                        .with_hard_attack_interval(kind.hard_attack_interval),
                ),
            );
            goal_selector.add_goal(
                4,
                Box::new(
                    MeleeAttackGoal::new(1.2, false)
                        .gated_by(|mob| !BowAttackGoal::is_holding_bow(mob)),
                ),
            );
            goal_selector.add_goal(5, Box::new(WanderAroundGoal::water_avoiding(1.0)));
            goal_selector.add_goal(
                6,
                LookAtEntityGoal::with_default(mob_weak, &EntityType::PLAYER, 8.0),
            );
            goal_selector.add_goal(6, Box::new(RandomLookAroundGoal::default()));

            target_selector.add_goal(1, Box::new(RevengeGoal::new(true)));
            hunting::apply(target_selector, &mob_arc.mob_entity, kind.extra_hunts);
            hunting::apply(target_selector, &mob_arc.mob_entity, SKELETON_HUNTS);
        };

        mob_arc
    }

    const fn is(&self, entity_type: &EntityType) -> bool {
        self.mob_entity.living_entity.entity.entity_type.id == entity_type.id
    }
}

impl Mob for SkeletonEntityBase {
    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    fn mob_play_step_sound(&self, _pos: &BlockPos, _state: &BlockState) {
        self.play_entity_sound(self.kind.step_sound, 0.15, 1.0);
    }

    fn get_ambient_sound(&self) -> Option<Sound> {
        Some(self.kind.ambient_sound)
    }

    /// Vanilla `Monster.getWalkTargetValue`: the darker, the better.
    fn get_walk_target_value(&self, pos: &BlockPos) -> f32 {
        0.5 - self
            .get_entity()
            .world
            .load()
            .get_light_level_dependent_magic_value(pos)
    }

    /// Vanilla `Skeleton.canFreeze`: a skeleton turns into a stray instead.
    fn mob_can_freeze(&self) -> bool {
        self.freezing.is_none()
    }

    fn mob_tick(&self, _caller: &dyn EntityBase) {
        if let Some(freezing) = &self.freezing {
            freezing.tick(self);
        }
    }

    fn finalize_spawn(
        &self,
        _world: &Arc<World>,
        group_data: Option<SpawnGroupData>,
    ) -> Option<SpawnGroupData> {
        let group_data = self.finalize_spawn_super(group_data);
        if self.is(&EntityType::WITHER_SKELETON) {
            wither::finalize_spawn(self);
        }
        group_data
    }

    fn get_arrow(&self, arrow: ArrowEntity) -> ArrowEntity {
        if let Some((effect, duration)) = self.kind.arrow_effect {
            arrow.add_effect(effect, duration);
        }
        if self.is(&EntityType::WITHER_SKELETON) {
            wither::ignite_arrow(&arrow);
        }
        arrow
    }

    fn on_attack(&self, target: &dyn EntityBase) {
        if self.is(&EntityType::WITHER_SKELETON) {
            wither::wither_target(target);
        }
    }

    fn mob_interact(&self, player: &Arc<Player>, item_stack: &mut ItemStack) -> bool {
        if self.is(&EntityType::BOGGED) && bogged::try_shear(self, player, item_stack) {
            return true;
        }
        self.mob_entity
            .mob_interact(player, item_stack, self.can_be_leashed())
    }

    fn mob_init_data_tracker(&self) {
        let entity = self.get_entity();
        if let Some(freezing) = &self.freezing {
            freezing.sync(entity);
        }
        if self.is(&EntityType::BOGGED) {
            bogged::sync_sheared(self);
        }
    }

    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        if let Some(freezing) = &self.freezing {
            freezing.write_nbt(self, nbt);
        }
        if self.is(&EntityType::BOGGED) {
            nbt.put_bool("sheared", self.is_sheared());
        }
    }

    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        if let Some(freezing) = &self.freezing {
            freezing.read_nbt(self.get_entity(), nbt);
        }
        if self.is(&EntityType::BOGGED) {
            self.set_sheared(nbt.get_bool("sheared").unwrap_or(false));
        }
    }
}

impl SkeletonEntityBase {
    #[must_use]
    pub fn is_sheared(&self) -> bool {
        self.sheared.load(Ordering::Relaxed)
    }

    pub fn set_sheared(&self, sheared: bool) {
        self.sheared.store(sheared, Ordering::Relaxed);
        bogged::sync_sheared(self);
    }
}
