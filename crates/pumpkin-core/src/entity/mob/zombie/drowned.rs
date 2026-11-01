use pumpkin_data::Block;
use pumpkin_data::BlockState;
use pumpkin_data::attributes::Attributes;
use pumpkin_data::data_component_impl::EquipmentSlot;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::Sound;
use pumpkin_util::Hand;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use rand::RngExt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};

use crate::entity::ai::behavior::hunting::{self, Hunt};
use crate::entity::ai::control::move_control::{MoveControl, Operation};
use crate::entity::ai::control::{Control, MoveControlTrait};
use crate::entity::ai::goal::move_to_target_pos::{MoveToTargetPos, MoveToTargetPosGoal};
use crate::entity::ai::goal::ranged_attack::RangedAttackGoal;
use crate::entity::ai::goal::wander_around::WanderAroundGoal;
use crate::entity::ai::goal::zombie_attack::ZombieAttackGoal;
use crate::entity::ai::goal::{Controls, Goal, ParentHandle};
use crate::entity::ai::pathfinder::node::PathType;
use crate::entity::ai::pathfinder::{Navigator, NavigatorGoal};
use crate::entity::ai::target_match::TargetMatch;
use crate::entity::ai::util::default_random_pos;
use crate::entity::custom_sound::CustomSound;
use crate::entity::living::LivingEntity;
use crate::entity::mob::zombie::ZombieEntityBase;
use crate::entity::mob::{RangedAttackMob, spawn::SpawnGroupData};
use crate::entity::projectile::arrow::ArrowPickup;
use crate::entity::projectile::trident::TridentEntity;
use crate::entity::spawn_util::is_face_full_up;
use crate::entity::{
    Entity, EntityBase,
    mob::{Mob, MobEntity},
};
use crate::world::World;
use pumpkin_data::entity::EntityType;
use pumpkin_nbt::compound::NbtCompound;

const DROWNED_HUNTS: &[Hunt] = &[
    Hunt::new(2, TargetMatch::Type(&EntityType::PLAYER), true).matching(ok_target),
    Hunt::new(3, TargetMatch::VILLAGER_LIKE, false),
    Hunt::new(3, TargetMatch::Type(&EntityType::IRON_GOLEM), true),
    Hunt::new(3, TargetMatch::Type(&EntityType::AXOLOTL), true),
    hunting::baby_turtles(5, true),
];

/// Vanilla `Drowned.NAUTILUS_SHELL_CHANCE`.
const NAUTILUS_SHELL_CHANCE: f32 = 0.03;

/// Vanilla `okTarget`: players only at night or in water.
fn ok_target(_hunter: &dyn Mob, target: &LivingEntity, world: &World) -> bool {
    !world.is_bright_outside() || target.entity.is_in_water()
}

/// Vanilla `Drowned.okTarget` for the mob's current target.
fn ok_current_target(mob: &dyn Mob) -> bool {
    mob.get_mob_entity().get_target().is_some_and(|target| {
        !mob.get_entity().world.load().is_bright_outside() || target.get_entity().is_in_water()
    })
}

/// Vanilla `Drowned.wantsToSwim`: while it searches land or its target is in water.
fn wants_to_swim(mob: &dyn Mob, searching_for_land: &AtomicBool) -> bool {
    searching_for_land.load(Ordering::Relaxed)
        || mob
            .get_mob_entity()
            .get_target()
            .is_some_and(|target| target.get_entity().is_in_water())
}

pub struct DrownedEntity {
    entity: Arc<ZombieEntityBase>,
    /// Vanilla `Drowned.searchingForLand`, shared with its move control and goals.
    searching_for_land: Arc<AtomicBool>,
}

impl DrownedEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        Self::with_can_break_doors(entity, false)
    }

    #[must_use]
    pub fn with_can_break_doors(entity: Entity, can_break_doors: bool) -> Arc<Self> {
        let searching_for_land = Arc::new(AtomicBool::new(false));
        let entity = ZombieEntityBase::without_behaviour_goals(entity);
        Self::set_up_movement(&entity.mob_entity, &searching_for_land);
        entity.set_can_break_doors(can_break_doors, &*entity);
        let drowned = Arc::new(Self {
            entity,
            searching_for_land,
        });
        drowned.add_behaviour_goals();
        drowned
    }

    /// Vanilla `Drowned` constructor and `createNavigation`.
    fn set_up_movement(mob_entity: &MobEntity, searching_for_land: &Arc<AtomicBool>) {
        *mob_entity
            .move_control
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            Box::new(DrownedMoveControl::new(searching_for_land.clone()));
        let mut navigator = mob_entity
            .navigator
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *navigator = Navigator::amphibious(false);
        navigator.set_pathfinding_malus(PathType::Water, 0.0);
    }

    /// Vanilla `Drowned.addBehaviourGoals`.
    fn add_behaviour_goals(self: &Arc<Self>) {
        let mob = &self.entity.mob_entity;
        let ranged: Weak<dyn RangedAttackMob> = {
            let ranged: Arc<dyn RangedAttackMob> = self.clone();
            Arc::downgrade(&ranged)
        };
        let searching = &self.searching_for_land;
        let goal_selector = &mob.goals_selector;
        goal_selector.add_goal(1, Box::new(DrownedGoToWaterGoal::new(1.0)));
        goal_selector.add_goal(
            2,
            Box::new(DrownedTridentAttackGoal {
                inner: RangedAttackGoal::new(ranged, 1.0, 40, 10.0),
            }),
        );
        goal_selector.add_goal(
            2,
            Box::new(DrownedAttackGoal {
                inner: ZombieAttackGoal::new(1.0, false),
            }),
        );
        goal_selector.add_goal(5, Box::new(DrownedGoToBeachGoal::new(searching.clone())));
        goal_selector.add_goal(6, Box::new(DrownedSwimUpGoal::new(searching.clone())));
        goal_selector.add_goal(7, Box::new(WanderAroundGoal::new(1.0)));

        let target_selector = &mob.target_selector;
        target_selector.add_goal(
            1,
            Box::new(
                ZombieEntityBase::revenge_goal()
                    .ignoring(|entity_type| entity_type == &EntityType::DROWNED),
            ),
        );
        hunting::apply(target_selector, mob, DROWNED_HUNTS);
    }

    fn holds_trident(&self) -> bool {
        self.entity
            .mob_entity
            .living_entity
            .entity_equipment
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&EquipmentSlot::MAIN_HAND)
            .item
            .id
            == Item::TRIDENT.id
    }
}

impl Mob for DrownedEntity {
    /// Vanilla `Drowned.getAmbientSound`.
    fn get_ambient_sound(&self) -> Option<Sound> {
        Some(if self.get_entity().is_in_water() {
            Sound::EntityDrownedAmbientWater
        } else {
            Sound::EntityDrownedAmbient
        })
    }

    fn as_custom_sound(&self) -> Option<&dyn CustomSound> {
        Some(self)
    }

    fn mob_play_step_sound(&self, _pos: &BlockPos, _state: &BlockState) {
        self.play_entity_sound(Sound::EntityDrownedStep, 0.15, 1.0);
    }

    fn get_mob_entity(&self) -> &MobEntity {
        &self.entity.mob_entity
    }

    /// Vanilla `Drowned.checkSpawnObstruction`: only entities obstruct, the water does not.
    fn check_spawn_obstruction(&self, world: &World) -> bool {
        world.is_unobstructed(&self.entity.mob_entity.living_entity.entity)
    }

    /// Vanilla `Drowned.isPushedByFluid`: not while it swims.
    fn mob_is_pushed_by_fluids(&self) -> bool {
        !self.get_entity().is_swimming()
    }

    /// Vanilla `Drowned.updateSwimming`.
    fn mob_tick(&self, caller: &dyn EntityBase) {
        let entity = self.get_entity();
        entity.set_swimming(
            !self.entity.mob_entity.is_no_ai()
                && entity.is_under_water()
                && wants_to_swim(self, &self.searching_for_land),
        );
        self.entity.mob_tick(caller);
    }

    /// Vanilla `Drowned.travelInWater`: a slow, steady swim under water while it wants to.
    fn travel_in_water(&self, caller: &dyn EntityBase, gravity: f64, falling: bool) {
        let living = &self.entity.mob_entity.living_entity;
        let entity = &living.entity;
        if entity.is_under_water() && wants_to_swim(self, &self.searching_for_land) {
            entity.update_velocity_from_input(living.movement_input.load(), 0.01);
            living.make_move(caller);
            entity.velocity.store(entity.velocity.load() * 0.9);
        } else {
            living.travel_in_water(caller, gravity, falling);
        }
    }

    /// Vanilla `Drowned.finalizeSpawn`: one in 33 holds a nautilus shell it always drops.
    // TODO: the zombie nautilus jockey needs the spawn reason.
    fn finalize_spawn(
        &self,
        _world: &Arc<World>,
        group_data: Option<SpawnGroupData>,
    ) -> Option<SpawnGroupData> {
        let group_data = self.finalize_spawn_super(group_data);
        let mob_entity = &self.entity.mob_entity;
        let offhand_empty = mob_entity
            .living_entity
            .entity_equipment
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&EquipmentSlot::OFF_HAND)
            .is_empty();
        if offhand_empty && self.get_random().random::<f32>() < NAUTILUS_SHELL_CHANCE {
            mob_entity.set_item_slot_and_drop_when_killed(
                &EquipmentSlot::OFF_HAND,
                ItemStack::new(1, &Item::NAUTILUS_SHELL),
            );
        }
        group_data
    }

    fn spawn_as_baby(&self) -> bool {
        self.entity.spawn_as_baby()
    }

    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        self.entity.mob_write_nbt(nbt);
    }

    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        self.entity.mob_read_nbt(nbt);
    }
}

impl CustomSound for DrownedEntity {
    fn hurt_sound(&self) -> Option<Sound> {
        Some(if self.get_entity().is_in_water() {
            Sound::EntityDrownedHurtWater
        } else {
            Sound::EntityDrownedHurt
        })
    }

    fn death_sound(&self) -> Option<Sound> {
        Some(if self.get_entity().is_in_water() {
            Sound::EntityDrownedDeathWater
        } else {
            Sound::EntityDrownedDeath
        })
    }
}

impl RangedAttackMob for DrownedEntity {
    /// Vanilla `Drowned.performRangedAttack`: throws its trident, or a new one.
    fn perform_ranged_attack(&self, target: &Arc<dyn EntityBase>, _power: f32) {
        let entity = self.get_entity();
        let world = entity.world.load_full();
        let held = self
            .entity
            .mob_entity
            .living_entity
            .entity_equipment
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&EquipmentSlot::MAIN_HAND);
        let stack = if held.item.id == Item::TRIDENT.id {
            held
        } else {
            ItemStack::new(1, &Item::TRIDENT)
        };
        let trident = TridentEntity::new_shot(
            Entity::new(world.clone(), entity.pos.load(), &EntityType::TRIDENT),
            entity,
            stack,
            ArrowPickup::Disallowed,
        );
        let pos = entity.pos.load();
        let target_entity = target.get_entity();
        let target_pos = target_entity.pos.load();
        let dx = target_pos.x - pos.x;
        let dy = (target_pos.y + f64::from(target_entity.entity_dimension.load().height) / 3.0)
            - trident.entity.pos.load().y;
        let dz = target_pos.z - pos.z;
        let distance = dx.hypot(dz);
        let difficulty = world.level_info.load().difficulty as i32;
        trident.set_velocity(
            dx,
            distance.mul_add(0.2, dy),
            dz,
            1.6,
            f64::from(14 - difficulty * 4),
        );
        world.spawn_entity(Arc::new(trident));
        let pitch = 1.0 / self.get_random().random::<f32>().mul_add(0.4, 0.8);
        self.play_entity_sound(Sound::EntityDrownedShoot, 1.0, pitch);
    }
}

/// Vanilla `Drowned.DrownedMoveControl`: swims towards the wanted position while it wants
/// to swim in water, else walks like any mob, sinking when not on the ground.
struct DrownedMoveControl {
    inner: MoveControl,
    searching_for_land: Arc<AtomicBool>,
}

impl DrownedMoveControl {
    fn new(searching_for_land: Arc<AtomicBool>) -> Self {
        Self {
            inner: MoveControl::default(),
            searching_for_land,
        }
    }
}

impl Control for DrownedMoveControl {}

impl MoveControlTrait for DrownedMoveControl {
    fn tick(&mut self, mob: &dyn Mob) {
        let mob_entity = mob.get_mob_entity();
        let living = &mob_entity.living_entity;
        let entity = &living.entity;
        if !(wants_to_swim(mob, &self.searching_for_land) && entity.is_in_water()) {
            if !entity.on_ground.load(Ordering::Relaxed) {
                entity
                    .velocity
                    .store(entity.velocity.load() + Vector3::new(0.0, -0.008, 0.0));
            }
            self.inner.tick(mob);
            return;
        }

        let pos = entity.pos.load();
        let target_above = mob_entity
            .get_target()
            .is_some_and(|target| target.get_entity().pos.load().y > pos.y);
        if target_above || self.searching_for_land.load(Ordering::Relaxed) {
            entity
                .velocity
                .store(entity.velocity.load() + Vector3::new(0.0, 0.002, 0.0));
        }
        if self.inner.operation != Operation::MoveTo || mob.is_navigator_idle() {
            living.set_speed(0.0);
            return;
        }

        let xd = self.inner.wanted_x - pos.x;
        let mut yd = self.inner.wanted_y - pos.y;
        let zd = self.inner.wanted_z - pos.z;
        let dd = (xd * xd + yd * yd + zd * zd).sqrt();
        yd /= dd;
        let yaw_target = (zd.atan2(xd).to_degrees() as f32) - 90.0;
        let yaw = self.change_angle(entity.yaw.load(), yaw_target, 90.0);
        entity.yaw.store(yaw);
        entity.body_yaw.store(yaw);
        let target_speed = (self.inner.speed_modifier
            * living.get_attribute_value(&Attributes::MOVEMENT_SPEED))
            as f32;
        let speed = 0.125f32.mul_add(target_speed - living.get_speed(), living.get_speed());
        living.set_speed(speed);
        let speed = f64::from(speed);
        entity.velocity.store(
            entity.velocity.load()
                + Vector3::new(speed * xd * 0.005, speed * yd * 0.1, speed * zd * 0.005),
        );
    }

    fn set_wanted_position(&mut self, x: f64, y: f64, z: f64, speed_modifier: f64) {
        self.inner.set_wanted_position(x, y, z, speed_modifier);
    }

    fn strafe(&mut self, forwards: f32, right: f32) {
        self.inner.strafe(forwards, right);
    }

    fn has_wanted(&self) -> bool {
        self.inner.has_wanted()
    }

    fn get_speed_modifier(&self) -> f64 {
        self.inner.get_speed_modifier()
    }

    fn get_wanted_position(&self) -> Vector3<f64> {
        self.inner.get_wanted_position()
    }
}

/// Vanilla `Drowned.DrownedAttackGoal`: the zombie attack, only on a target it may chase.
struct DrownedAttackGoal {
    inner: Box<ZombieAttackGoal>,
}

impl Goal for DrownedAttackGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        self.inner.can_start(mob) && ok_current_target(mob)
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        self.inner.should_continue(mob) && ok_current_target(mob)
    }

    fn start(&mut self, mob: &dyn Mob) {
        self.inner.start(mob);
    }

    fn stop(&mut self, mob: &dyn Mob) {
        self.inner.stop(mob);
    }

    fn tick(&mut self, mob: &dyn Mob) {
        self.inner.tick(mob);
    }

    fn should_run_every_tick(&self) -> bool {
        self.inner.should_run_every_tick()
    }

    fn controls(&self) -> Controls {
        self.inner.controls()
    }
}

/// Vanilla `Drowned.DrownedTridentAttackGoal`: throws its trident every 2 seconds within 10
/// blocks, holding it up while the goal runs.
struct DrownedTridentAttackGoal {
    inner: RangedAttackGoal,
}

impl Goal for DrownedTridentAttackGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        self.inner.can_start(mob)
            && mob
                .cast_any()
                .downcast_ref::<DrownedEntity>()
                .is_some_and(DrownedEntity::holds_trident)
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        self.inner.should_continue(mob)
    }

    fn start(&mut self, mob: &dyn Mob) {
        self.inner.start(mob);
        let mob_entity = mob.get_mob_entity();
        mob_entity.set_attacking(true);
        let held = mob_entity
            .living_entity
            .entity_equipment
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&EquipmentSlot::MAIN_HAND);
        mob_entity
            .living_entity
            .set_active_hand(Hand::Right, held, i32::MAX);
    }

    fn stop(&mut self, mob: &dyn Mob) {
        self.inner.stop(mob);
        let mob_entity = mob.get_mob_entity();
        mob_entity.living_entity.clear_active_hand();
        mob_entity.set_attacking(false);
    }

    fn tick(&mut self, mob: &dyn Mob) {
        self.inner.tick(mob);
    }

    fn should_run_every_tick(&self) -> bool {
        self.inner.should_run_every_tick()
    }

    fn controls(&self) -> Controls {
        self.inner.controls()
    }
}

/// Vanilla `Drowned.DrownedGoToWaterGoal`: by day, out of water, it walks to water nearby.
struct DrownedGoToWaterGoal {
    speed_modifier: f64,
    wanted: Vector3<f64>,
}

impl DrownedGoToWaterGoal {
    const fn new(speed_modifier: f64) -> Self {
        Self {
            speed_modifier,
            wanted: Vector3::new(0.0, 0.0, 0.0),
        }
    }

    /// Vanilla `getWaterPos`: ten tries for a water block around it.
    fn water_pos(mob: &dyn Mob) -> Option<Vector3<f64>> {
        let entity = mob.get_entity();
        let base = entity.block_pos.load();
        let world = entity.world.load();
        for _ in 0..10 {
            let pos = {
                let mut rng = mob.get_random();
                BlockPos::new(
                    base.0.x + rng.random_range(0..20) - 10,
                    base.0.y + 2 - rng.random_range(0..8),
                    base.0.z + rng.random_range(0..20) - 10,
                )
            };
            if world.get_block(&pos).id == Block::WATER.id {
                return Some(Vector3::new(
                    f64::from(pos.0.x) + 0.5,
                    f64::from(pos.0.y),
                    f64::from(pos.0.z) + 0.5,
                ));
            }
        }
        None
    }
}

impl Goal for DrownedGoToWaterGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        let entity = mob.get_entity();
        if !entity.world.load().is_bright_outside() || entity.is_in_water() {
            return false;
        }
        let Some(wanted) = Self::water_pos(mob) else {
            return false;
        };
        self.wanted = wanted;
        true
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        !mob.is_navigator_idle()
    }

    fn start(&mut self, mob: &dyn Mob) {
        let pos = mob.get_entity().pos.load();
        mob.get_mob_entity()
            .steered()
            .navigator()
            .set_progress(NavigatorGoal::new(pos, self.wanted, self.speed_modifier));
    }

    fn controls(&self) -> Controls {
        Controls::MOVE
    }
}

/// Vanilla `Drowned.DrownedGoToBeachGoal`'s target: a block to stand on with two free
/// blocks above it.
struct BeachTarget;

impl MoveToTargetPos for BeachTarget {
    fn is_target_pos(&self, world: Arc<World>, block_pos: BlockPos) -> bool {
        let above = block_pos.up();
        world.get_block_state(&above).is_air()
            && world.get_block_state(&above.up()).is_air()
            && is_face_full_up(world.get_block_state(&block_pos))
    }
}

/// Vanilla `Drowned.DrownedGoToBeachGoal`: at night, near the surface, it swims ashore.
struct DrownedGoToBeachGoal {
    inner: MoveToTargetPosGoal<BeachTarget>,
    searching_for_land: Arc<AtomicBool>,
    _target: Box<BeachTarget>,
}

impl DrownedGoToBeachGoal {
    fn new(searching_for_land: Arc<AtomicBool>) -> Self {
        let target = Box::new(BeachTarget);
        // SAFETY: the target has its own heap allocation owned by this goal, so it never moves
        // and outlives the handle, and it is only ever read through shared references.
        let handle: ParentHandle<BeachTarget> = unsafe { ParentHandle::new(&target) };
        Self {
            inner: MoveToTargetPosGoal::new(handle, 1.0, 8, 2),
            searching_for_land,
            _target: target,
        }
    }
}

impl Goal for DrownedGoToBeachGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        let entity = mob.get_entity();
        let world = entity.world.load();
        self.inner.can_start(mob)
            && !world.is_bright_outside()
            && entity.is_in_water()
            && entity.pos.load().y >= f64::from(world.sea_level - 3)
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        self.inner.should_continue(mob)
    }

    fn start(&mut self, mob: &dyn Mob) {
        self.searching_for_land.store(false, Ordering::Relaxed);
        self.inner.start(mob);
    }

    fn stop(&mut self, mob: &dyn Mob) {
        self.inner.stop(mob);
    }

    fn tick(&mut self, mob: &dyn Mob) {
        self.inner.tick(mob);
    }

    fn should_run_every_tick(&self) -> bool {
        self.inner.should_run_every_tick()
    }

    fn controls(&self) -> Controls {
        self.inner.controls()
    }
}

/// Vanilla `Drowned.DrownedSwimUpGoal`: at night, deep under water, it swims up towards the
/// surface, searching land on the way.
struct DrownedSwimUpGoal {
    searching_for_land: Arc<AtomicBool>,
    stuck: bool,
}

impl DrownedSwimUpGoal {
    const fn new(searching_for_land: Arc<AtomicBool>) -> Self {
        Self {
            searching_for_land,
            stuck: false,
        }
    }

    /// Vanilla `Drowned.closeToNextPos`: within 2 blocks of where its path ends.
    fn close_to_next_pos(mob: &dyn Mob) -> bool {
        let target = mob.get_mob_entity().steered().navigator().get_target_pos();
        target.is_some_and(|target| {
            mob.get_entity()
                .pos
                .load()
                .squared_distance_to_vec(&Vector3::new(
                    f64::from(target.0.x),
                    f64::from(target.0.y),
                    f64::from(target.0.z),
                ))
                < 4.0
        })
    }
}

impl Goal for DrownedSwimUpGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        let entity = mob.get_entity();
        let world = entity.world.load();
        !world.is_bright_outside()
            && entity.is_in_water()
            && entity.pos.load().y < f64::from(world.sea_level - 2)
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        self.can_start(mob) && !self.stuck
    }

    fn start(&mut self, _mob: &dyn Mob) {
        self.searching_for_land.store(true, Ordering::Relaxed);
        self.stuck = false;
    }

    fn stop(&mut self, _mob: &dyn Mob) {
        self.searching_for_land.store(false, Ordering::Relaxed);
    }

    fn tick(&mut self, mob: &dyn Mob) {
        let entity = mob.get_entity();
        let pos = entity.pos.load();
        let sea_level = f64::from(entity.world.load().sea_level);
        if pos.y >= sea_level - 1.0 || !(mob.is_navigator_idle() || Self::close_to_next_pos(mob)) {
            return;
        }
        let towards = Vector3::new(pos.x, sea_level - 1.0, pos.z);
        let Some(next) =
            default_random_pos::get_pos_towards(mob, 4, 8, towards, std::f64::consts::FRAC_PI_2)
        else {
            self.stuck = true;
            return;
        };
        mob.get_mob_entity()
            .steered()
            .navigator()
            .set_progress(NavigatorGoal::new(pos, next, 1.0));
    }

    fn controls(&self) -> Controls {
        Controls::empty()
    }
}
