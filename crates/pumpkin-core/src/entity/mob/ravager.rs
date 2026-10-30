use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Arc, Weak};

use pumpkin_data::BlockState;
use pumpkin_data::attributes::Attributes;
use pumpkin_data::damage::DamageType;
use pumpkin_data::entity::{EntityStatus, EntityType};
use pumpkin_data::game_event::GameEvent;
use pumpkin_data::sound::Sound;
use pumpkin_data::tag::{self, Taggable};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_protocol::bedrock::server::ActorEventID;
use pumpkin_util::math::lerp;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_world::world::BlockFlags;
use rand::RngExt;

use crate::entity::ageable::AgeableMob;
use crate::entity::ai::behavior::hunting::{self, Hunt};
use crate::entity::ai::pathfinder::node::PathType;
use crate::entity::ai::target_match::TargetMatch;
use crate::entity::combat;
use crate::entity::{
    Entity, EntityBase,
    ai::goal::{
        look_at_entity::LookAtEntityGoal, melee_attack::MeleeAttackGoal, swim::SwimGoal,
        wander_around::WanderAroundGoal,
    },
    mob::{
        Mob, MobEntity,
        patrol::{LongDistancePatrolGoal, PatrolData, PatrollingMonster},
        raider::{
            self, ObtainRaidLeaderBannerGoal, PathfindToRaidGoal, Raider, RaiderCelebrationGoal,
            RaiderData, RaiderMoveThroughVillageGoal,
        },
    },
};
use crate::world::World;
use crate::world::game_event::GameEventContext;

const BASE_MOVEMENT_SPEED: f64 = 0.3;
const ATTACK_MOVEMENT_SPEED: f64 = 0.35;
/// The share of the way to the wanted speed the base speed moves each tick.
const MOVEMENT_SPEED_LERP: f64 = 0.1;
const ATTACK_DURATION: i32 = 10;
const STUN_DURATION: i32 = 40;
const STUN_CHANCE: f64 = 0.5;
const ROAR_DURATION: i32 = 20;
/// The roar tick at which the roar hits.
const ROAR_HIT_TICK: i32 = 10;
const ROAR_RADIUS: f64 = 4.0;
const ROAR_DAMAGE: f32 = 6.0;
const STRONG_KNOCKBACK_STRENGTH: f64 = 4.0;
const STRONG_KNOCKBACK_LIFT: f64 = 0.2;
const MIN_KNOCKBACK_DISTANCE_SQR: f64 = 0.001;
/// How far around its box a ravager stuck on a wall breaks leaves.
const LEAVES_BREAK_RANGE: f64 = 0.2;

const RAVAGER_HUNTS: &[Hunt] = &[
    Hunt::new(3, TargetMatch::Type(&EntityType::PLAYER), true),
    Hunt::new(4, TargetMatch::VILLAGER_LIKE, true).only_mobs(is_adult),
    Hunt::new(4, TargetMatch::Type(&EntityType::IRON_GOLEM), true),
];

fn is_adult(mob: &dyn Mob) -> bool {
    !mob.as_ageable().is_some_and(AgeableMob::is_baby)
}

/// A large raider beast that rams its targets, breaks leaves and roars after being stunned.
///
/// Wiki: <https://minecraft.wiki/w/Ravager>
pub struct RavagerEntity {
    pub mob_entity: MobEntity,
    pub raider_data: RaiderData,
    attack_tick: AtomicI32,
    stunned_tick: AtomicI32,
    roar_tick: AtomicI32,
}

impl RavagerEntity {
    #[must_use]
    pub fn new(entity: Entity) -> Arc<Self> {
        let mob_entity = MobEntity::new(entity);
        let ravager = Self {
            mob_entity,
            raider_data: RaiderData::default(),
            attack_tick: AtomicI32::new(0),
            stunned_tick: AtomicI32::new(0),
            roar_tick: AtomicI32::new(0),
        };
        let mob_arc = Arc::new(ravager);
        let mob_weak: Weak<dyn Mob> = {
            let mob_arc: Arc<dyn Mob> = mob_arc.clone();
            Arc::downgrade(&mob_arc)
        };

        mob_arc
            .mob_entity
            .navigator
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .set_pathfinding_malus(PathType::Leaves, 0.0);

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
            goal_selector.add_goal(4, Box::new(MeleeAttackGoal::new(1.0, true)));
            goal_selector.add_goal(5, Box::new(WanderAroundGoal::water_avoiding(0.4)));
            goal_selector.add_goal(
                6,
                LookAtEntityGoal::with_default(mob_weak.clone(), &EntityType::PLAYER, 6.0),
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
            target_selector.add_goal(2, raider::revenge_goal());
            hunting::apply(target_selector, &mob_arc.mob_entity, RAVAGER_HUNTS);
        };

        mob_arc
    }

    /// The speed part of vanilla `Ravager.aiStep`: stands still while immobile, else eases
    /// towards its walking or attacking speed.
    fn update_movement_speed(&self) {
        let living = &self.mob_entity.living_entity;
        let base = living.get_attribute_base(&Attributes::MOVEMENT_SPEED);
        let new_base = if self.is_mob_immobile() {
            0.0
        } else {
            let max_speed = if self.mob_entity.get_target().is_some() {
                ATTACK_MOVEMENT_SPEED
            } else {
                BASE_MOVEMENT_SPEED
            };
            lerp(MOVEMENT_SPEED_LERP, base, max_speed)
        };
        // Like vanilla `setBaseValue`, an unchanged value is not synced again.
        if new_base.to_bits() != base.to_bits() {
            living.set_attribute_base(&Attributes::MOVEMENT_SPEED, new_base);
        }
    }

    /// The leaf breaking of vanilla `Ravager.aiStep`: a ravager pushing against a wall breaks
    /// the leaves around it, or jumps when there are none.
    fn break_leaves(&self) {
        let living = &self.mob_entity.living_entity;
        let entity = &living.entity;
        let world = entity.world.load_full();
        if !entity.horizontal_collision.load(Ordering::Relaxed)
            || !world.level_info.load().game_rules.mob_griefing
        {
            return;
        }

        let area = entity.bounding_box.load().expand_all(LEAVES_BREAK_RANGE);
        let min = BlockPos::floored_v(area.min);
        let max = BlockPos::floored_v(area.max);
        let mut destroyed_block = false;
        // `BlockPos.betweenClosed` order: x fastest, z slowest.
        for z in min.0.z..=max.0.z {
            for y in min.0.y..=max.0.y {
                for x in min.0.x..=max.0.x {
                    let pos = BlockPos::new(x, y, z);
                    let block = world.get_block(&pos);
                    if !block.has_tag(&tag::Block::MINECRAFT_LEAVES) {
                        continue;
                    }
                    let broken = world.break_block(&pos, None, BlockFlags::NOTIFY_ALL);
                    if broken.is_some() {
                        world.emit_game_event_with_context(
                            GameEvent::BlockDestroy.name(),
                            pos.to_centered_f64(),
                            &GameEventContext {
                                source_entity: Some(self),
                            },
                        );
                        destroyed_block = true;
                    }
                }
            }
        }

        if !destroyed_block && entity.on_ground.load(Ordering::Relaxed) {
            living.jump_from_ground(self);
        }
    }

    /// The tick counters of vanilla `Ravager.aiStep`: the roar hits halfway through, and a
    /// stun ends with a roar.
    fn tick_counters(&self) {
        if self.roar_tick.load(Ordering::Relaxed) > 0
            && self.roar_tick.fetch_sub(1, Ordering::Relaxed) - 1 == ROAR_HIT_TICK
        {
            self.roar();
        }

        if self.attack_tick.load(Ordering::Relaxed) > 0 {
            self.attack_tick.fetch_sub(1, Ordering::Relaxed);
        }

        // The stun particles are client side, driven by the stun entity event.
        if self.stunned_tick.load(Ordering::Relaxed) > 0
            && self.stunned_tick.fetch_sub(1, Ordering::Relaxed) == 1
        {
            self.play_entity_sound(Sound::EntityRavagerRoar, 1.0, 1.0);
            self.roar_tick.store(ROAR_DURATION, Ordering::Relaxed);
        }
    }

    /// Vanilla `Ravager.roar`: hurts and throws back everything alive around it but other
    /// ravagers. Players are thrown back by their own client on the roar event.
    fn roar(&self) {
        let living = &self.mob_entity.living_entity;
        if !living.is_alive() {
            return;
        }
        let entity = &living.entity;
        let world = entity.world.load();
        let mob_griefing = world.level_info.load().game_rules.mob_griefing;
        let area = entity.bounding_box.load().expand_all(ROAR_RADIUS);

        for target in world.get_all_at_box(&area) {
            let Some(target_living) = target.get_living_entity() else {
                continue;
            };
            let target_type = target.get_entity().entity_type;
            if target_type == &EntityType::RAVAGER
                || !target_living.is_alive()
                || (!mob_griefing && target_type == &EntityType::ARMOR_STAND)
            {
                continue;
            }

            if !TargetMatch::ILLAGER.matches(target_type) {
                target.damage_with_context(
                    target.as_ref(),
                    ROAR_DAMAGE,
                    DamageType::MOB_ATTACK,
                    None,
                    Some(self),
                    Some(self),
                );
            }

            if target.get_player().is_none() {
                self.strong_knockback(target.get_entity());
            }
        }

        world.emit_game_event_with_context(
            GameEvent::EntityAction.name(),
            entity.pos.load(),
            &GameEventContext {
                source_entity: Some(self),
            },
        );
        world.send_entity_status(entity, EntityStatus::RavagerRoared, ActorEventID::None);
    }

    /// Vanilla `Ravager.strongKnockback`; the push falls off with the squared distance.
    fn strong_knockback(&self, target: &Entity) {
        let pos = self.mob_entity.living_entity.entity.pos.load();
        let target_pos = target.pos.load();
        let xd = target_pos.x - pos.x;
        let zd = target_pos.z - pos.z;
        let dd = xd.mul_add(xd, zd * zd).max(MIN_KNOCKBACK_DISTANCE_SQR);
        let push = Vector3::new(
            xd / dd * STRONG_KNOCKBACK_STRENGTH,
            STRONG_KNOCKBACK_LIFT,
            zd / dd * STRONG_KNOCKBACK_STRENGTH,
        );
        target.velocity.store(target.velocity.load() + push);
        target.velocity_dirty.store(true, Ordering::Relaxed);
    }
}

impl Mob for RavagerEntity {
    fn mob_play_step_sound(&self, _pos: &BlockPos, _state: &BlockState) {
        self.play_entity_sound(Sound::EntityRavagerStep, 0.15, 1.0);
    }

    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    fn as_patrolling_monster(&self) -> Option<&dyn PatrollingMonster> {
        Some(self)
    }

    fn as_raider(&self) -> Option<&dyn Raider> {
        Some(self)
    }

    fn get_ambient_sound(&self) -> Option<Sound> {
        Some(Sound::EntityRavagerAmbient)
    }

    fn get_max_head_rotation(&self) -> f32 {
        45.0
    }

    fn is_mob_immobile(&self) -> bool {
        self.mob_entity.living_entity.is_dead_or_dying()
            || self.attack_tick.load(Ordering::Relaxed) > 0
            || self.stunned_tick.load(Ordering::Relaxed) > 0
            || self.roar_tick.load(Ordering::Relaxed) > 0
    }

    fn has_line_of_sight(&self, target: &Entity) -> bool {
        if self.stunned_tick.load(Ordering::Relaxed) > 0
            || self.roar_tick.load(Ordering::Relaxed) > 0
        {
            return false;
        }
        let mob_entity = &self.mob_entity;
        mob_entity
            .sensing
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .has_line_of_sight(&mob_entity.living_entity.entity, target)
    }

    /// Vanilla `Ravager.checkSpawnObstruction`: only liquid obstructs it.
    fn check_spawn_obstruction(&self, world: &World) -> bool {
        !world.contains_any_liquid(self.mob_entity.living_entity.entity.bounding_box.load())
    }

    /// Vanilla `Ravager.aiStep` after `super.aiStep()`.
    fn mob_ai_step(&self) {
        if !self.mob_entity.living_entity.is_alive() {
            return;
        }
        self.update_movement_speed();
        self.break_leaves();
        self.tick_counters();
    }

    /// Vanilla `Ravager.doHurtTarget`, then `Mob.doHurtTarget` with its extra knockback.
    fn do_hurt_target(&self, target: &dyn EntityBase) -> bool {
        let living = &self.mob_entity.living_entity;
        let entity = &living.entity;
        self.attack_tick.store(ATTACK_DURATION, Ordering::Relaxed);
        entity.world.load().send_entity_status(
            entity,
            EntityStatus::StartAttacking,
            ActorEventID::StartAttacking,
        );
        self.play_entity_sound(Sound::EntityRavagerAttack, 1.0, 1.0);

        let damaged = self.mob_entity.try_attack(self, target);
        if damaged {
            let knockback = living.get_attribute_value(&Attributes::ATTACK_KNOCKBACK);
            if knockback > 0.0 && target.get_living_entity().is_some() {
                // `handle_knockback` halves the strength like `LivingEntity.getKnockback`.
                combat::handle_knockback(entity, target, knockback);
                target
                    .get_entity()
                    .velocity_dirty
                    .store(true, Ordering::Relaxed);
            }
            self.on_attack(target);
        }
        damaged
    }

    /// Vanilla `Ravager.blockedByItem`: half the time the shield stuns it, else it throws the
    /// defender back. Not while roaring.
    fn blocked_by_item(&self, defender: &dyn EntityBase) {
        if self.roar_tick.load(Ordering::Relaxed) != 0 {
            return;
        }
        if self.get_random().random::<f64>() < STUN_CHANCE {
            self.stunned_tick.store(STUN_DURATION, Ordering::Relaxed);
            self.play_entity_sound(Sound::EntityRavagerStunned, 1.0, 1.0);
            let entity = &self.mob_entity.living_entity.entity;
            let world = entity.world.load();
            world.send_entity_status(entity, EntityStatus::RavagerStunned, ActorEventID::None);
            defender.push(self);
        } else {
            self.strong_knockback(defender.get_entity());
        }
        defender
            .get_entity()
            .velocity_dirty
            .store(true, Ordering::Relaxed);
    }

    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        self.write_raider_nbt(nbt);
        nbt.put_int("AttackTick", self.attack_tick.load(Ordering::Relaxed));
        nbt.put_int("StunTick", self.stunned_tick.load(Ordering::Relaxed));
        nbt.put_int("RoarTick", self.roar_tick.load(Ordering::Relaxed));
    }

    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        self.read_raider_nbt(nbt);
        self.attack_tick.store(
            nbt.get_int("AttackTick").unwrap_or_default(),
            Ordering::Relaxed,
        );
        self.stunned_tick.store(
            nbt.get_int("StunTick").unwrap_or_default(),
            Ordering::Relaxed,
        );
        self.roar_tick.store(
            nbt.get_int("RoarTick").unwrap_or_default(),
            Ordering::Relaxed,
        );
    }
}

impl PatrollingMonster for RavagerEntity {
    fn get_patrol_data(&self) -> &PatrolData {
        &self.raider_data.patrol_data
    }

    fn can_be_leader(&self) -> bool {
        false
    }
}

impl Raider for RavagerEntity {
    fn get_raider_data(&self) -> &RaiderData {
        &self.raider_data
    }

    fn get_celebrate_sound(&self) -> Sound {
        Sound::EntityRavagerCelebrate
    }
}
