use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::{Arc, Weak};

use pumpkin_data::entity::EntityType;
use pumpkin_data::sound::Sound;
use pumpkin_data::tracked_data;
use pumpkin_util::math::atomic_f32::AtomicF32;
use pumpkin_util::math::vector3::Vector3;

use crate::entity::ai::behavior::hunting::{self, Hunt};
use crate::entity::ai::goal::revenge::RevengeGoal;
use crate::entity::ai::target_match::TargetMatch;
use crate::entity::ai::util::RandomExt;
use crate::entity::{
    Entity, EntityBase,
    ai::{
        goal::{
            blaze_attack::BlazeAttackGoal, look_around::RandomLookAroundGoal,
            look_at_entity::LookAtEntityGoal, move_towards_restriction::MoveTowardsRestrictionGoal,
            wander_around::WanderAroundGoal,
        },
        pathfinder::node::PathType,
    },
    mob::{Mob, MobEntity, MobMetadata},
};

const BLAZE_HUNTS: &[Hunt] = &[Hunt::new(2, TargetMatch::Type(&EntityType::PLAYER), true)];

const INITIAL_ALLOWED_HEIGHT_OFFSET: f32 = 0.5;
const HEIGHT_OFFSET_CHANGE_INTERVAL: i32 = 100;
const HEIGHT_OFFSET_MODE: f64 = 0.5;
const HEIGHT_OFFSET_DEVIATION: f64 = 6.891;
const RISE_SPEED: f32 = 0.3;
const RISE_FACTOR: f32 = 0.3;
const FALL_SLOWDOWN: f64 = 0.6;

/// A hostile Nether mob that hovers and shoots volleys of small fireballs.
///
/// Wiki: <https://minecraft.wiki/w/Blaze>
pub struct BlazeEntity {
    pub entity: Arc<MobEntity>,
    pub is_charged: AtomicBool,
    allowed_height_offset: AtomicF32,
    next_height_offset_change_tick: AtomicI32,
}

impl BlazeEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let entity = Arc::new(MobEntity::new(entity));
        let blaze = Self {
            entity,
            is_charged: AtomicBool::new(false),
            allowed_height_offset: AtomicF32::new(INITIAL_ALLOWED_HEIGHT_OFFSET),
            next_height_offset_change_tick: AtomicI32::new(0),
        };
        let mob_arc = Arc::new(blaze);
        let mob_weak: Weak<dyn Mob> = {
            let mob_arc: Arc<dyn Mob> = mob_arc.clone();
            Arc::downgrade(&mob_arc)
        };

        {
            let mut navigator = mob_arc
                .entity
                .navigator
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            navigator.set_pathfinding_malus(PathType::Water, -1.0);
            navigator.set_pathfinding_malus(PathType::Lava, 8.0);
            navigator.set_pathfinding_malus(PathType::DangerFire, 0.0);
            navigator.set_pathfinding_malus(PathType::DamageFire, 0.0);
        };

        {
            let goal_selector = &mob_arc.entity.goals_selector;
            let target_selector = &mob_arc.entity.target_selector;

            goal_selector.add_goal(4, Box::new(BlazeAttackGoal::new(Arc::downgrade(&mob_arc))));
            goal_selector.add_goal(5, Box::new(MoveTowardsRestrictionGoal::new(1.0)));
            goal_selector.add_goal(
                7,
                Box::new(WanderAroundGoal::water_avoiding_with_probability(1.0, 0.0)),
            );
            goal_selector.add_goal(
                8,
                LookAtEntityGoal::with_default(mob_weak, &EntityType::PLAYER, 8.0),
            );
            goal_selector.add_goal(8, Box::new(RandomLookAroundGoal::default()));

            target_selector.add_goal(1, Box::new(RevengeGoal::new(true).alerting_others()));
            hunting::apply(target_selector, &mob_arc.entity, BLAZE_HUNTS);
        };

        mob_arc
    }

    pub fn is_charged(&self) -> bool {
        self.is_charged.load(Ordering::Relaxed)
    }

    /// Vanilla `Blaze.setCharged`; the client shows a charged blaze as burning.
    pub fn set_charged(&self, charged: bool) {
        if self.is_charged.swap(charged, Ordering::Relaxed) == charged {
            return;
        }
        let entity = &self.entity.living_entity.entity;
        entity.set_synced_data(tracked_data::blaze::DATA_FLAGS_ID, i8::from(charged));
        if let Some(meta) = self.mob_metadata() {
            entity.send_bedrock_actor_data(&meta.to_bedrock_actor_data());
        }
    }
}

impl Mob for BlazeEntity {
    fn get_mob_entity(&self) -> &MobEntity {
        &self.entity
    }

    fn mob_metadata(&self) -> Option<MobMetadata> {
        let mut meta = MobMetadata::new();
        if self.is_charged() {
            meta = meta.blaze_on_fire(true);
        }
        Some(meta)
    }

    fn get_ambient_sound(&self) -> Option<Sound> {
        Some(Sound::EntityBlazeAmbient)
    }

    fn is_sensitive_to_water(&self) -> bool {
        true
    }

    /// Vanilla `Blaze.aiStep` before `super.aiStep()`: falls slowly.
    fn before_mob_ai_step(&self) {
        let entity = &self.entity.living_entity.entity;
        let velocity = entity.velocity.load();
        if !entity.on_ground.load(Ordering::Relaxed) && velocity.y < 0.0 {
            entity
                .velocity
                .store(velocity.multiply(1.0, FALL_SLOWDOWN, 1.0));
        }
    }

    /// Vanilla `Blaze.customServerAiStep`: rises towards a target whose eyes are above its own.
    fn custom_server_ai_step(&self, _caller: &dyn EntityBase) {
        if self
            .next_height_offset_change_tick
            .fetch_sub(1, Ordering::Relaxed)
            <= 1
        {
            self.next_height_offset_change_tick
                .store(HEIGHT_OFFSET_CHANGE_INTERVAL, Ordering::Relaxed);
            let offset = self
                .get_random()
                .triangle(HEIGHT_OFFSET_MODE, HEIGHT_OFFSET_DEVIATION);
            self.allowed_height_offset
                .store(offset as f32, Ordering::Relaxed);
        }

        let entity = &self.entity.living_entity.entity;
        let allowed_height_offset = f64::from(self.allowed_height_offset.load(Ordering::Relaxed));
        if let Some(target) = self.entity.get_target()
            && target.get_entity().get_eye_y() > entity.get_eye_y() + allowed_height_offset
            && self.can_attack(target.as_ref())
        {
            let velocity = entity.velocity.load();
            let rise = (f64::from(RISE_SPEED) - velocity.y) * f64::from(RISE_FACTOR);
            entity
                .velocity
                .store(Vector3::new(velocity.x, velocity.y + rise, velocity.z));
            entity.velocity_dirty.store(true, Ordering::Relaxed);
        }
    }
}
