use std::sync::{
    Arc, Weak,
    atomic::{AtomicBool, AtomicI32, Ordering},
};

use crossbeam::atomic::AtomicCell;
use pumpkin_data::BlockState;
use pumpkin_data::block_properties::TurtleEggLikeProperties;
use pumpkin_data::damage::DamageType;
use pumpkin_data::entity::EntityType;
use pumpkin_data::fluid::Fluid;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::tag::{self, Taggable};
use pumpkin_data::world::WorldEvent;
use pumpkin_data::{Block, attributes::Attributes};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_nbt::tag::NbtTag;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_world::world::BlockFlags;
use rand::RngExt;

use crate::entity::ai::control::move_control::{MoveControl, Operation};
use crate::entity::ai::control::{Control, MoveControlTrait};
use crate::entity::ai::goal::move_to_target_pos::{MoveToTargetPos, MoveToTargetPosGoal};
use crate::entity::ai::goal::{Controls, Goal, ParentHandle, to_goal_ticks};
use crate::entity::ai::pathfinder::node::PathType;
use crate::entity::ai::pathfinder::{Navigator, NavigatorGoal};
use crate::entity::ai::util::default_random_pos;
use crate::entity::custom_sound::CustomSound;
use crate::entity::{
    Entity, EntityBase,
    ageable::{AgeableData, AgeableMob},
    ai::goal::{
        breed::BreedGoal, escape_danger::EscapeDangerGoal, look_at_entity::LookAtEntityGoal,
        tempt::TemptGoal, wander_around::WanderAroundGoal,
    },
    mob::{Mob, MobEntity, spawn::SpawnGroupData},
    passive::animal::Animal,
    player::Player,
};
use crate::world::World;
use crate::world::loot::{LootContextParameters, generate_loot_from_handle};

/// Turtle is prey for wolves and cats.
pub fn is_baby_on_land(mob: &dyn Mob) -> bool {
    mob.as_ageable().is_some_and(AgeableMob::is_baby) && !mob.get_entity().is_in_water()
}

/// Vanilla `TurtleEggBlock.isSand`.
fn is_sand(world: &World, pos: &BlockPos) -> bool {
    world.get_block(pos).has_tag(&tag::Block::MINECRAFT_SAND)
}

fn in_water(world: &World, pos: &BlockPos) -> bool {
    let fluid = world.get_fluid(pos).id;
    fluid == Fluid::WATER.id || fluid == Fluid::FLOWING_WATER.id
}

pub struct TurtleEntity {
    pub mob_entity: MobEntity,
    pub ageable_data: AgeableData,
    pub has_egg: AtomicBool,
    pub laying_egg: AtomicBool,
    lay_egg_counter: AtomicI32,
    home_pos: AtomicCell<BlockPos>,
    travel_pos: AtomicCell<Option<BlockPos>>,
    going_home: AtomicBool,
}

fn as_turtle(mob: &dyn Mob) -> Option<&TurtleEntity> {
    mob.cast_any().downcast_ref::<TurtleEntity>()
}

impl TurtleEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let mut mob_entity = MobEntity::new(entity);
        Self::set_up_movement(&mut mob_entity);
        let turtle = Self {
            mob_entity,
            ageable_data: AgeableData::default(),
            has_egg: AtomicBool::new(false),
            laying_egg: AtomicBool::new(false),
            lay_egg_counter: AtomicI32::new(0),
            home_pos: AtomicCell::new(BlockPos::new(0, 0, 0)),
            travel_pos: AtomicCell::new(None),
            going_home: AtomicBool::new(false),
        };
        let mob_arc = Arc::new(turtle);
        let mob_weak: Weak<dyn Mob> = {
            let mob_arc: Arc<dyn Mob> = mob_arc.clone();
            Arc::downgrade(&mob_arc)
        };

        {
            // Vanilla `Turtle.registerGoals`.
            let goal_selector = &mob_arc.mob_entity.goals_selector;
            goal_selector.add_goal(0, EscapeDangerGoal::new(1.2).water_first(7));
            goal_selector.add_goal(1, BreedGoal::new(1.0));
            goal_selector.add_goal(1, Box::new(TurtleLayEggGoal::new()));
            goal_selector.add_goal(
                2,
                Box::new(TemptGoal::with_tag(
                    1.1,
                    &tag::Item::MINECRAFT_TURTLE_FOOD,
                    false,
                )),
            );
            let water_speed = if mob_arc.is_baby() { 2.0 } else { 1.0 };
            goal_selector.add_goal(3, Box::new(TurtleGoToWaterGoal::new(water_speed)));
            goal_selector.add_goal(4, Box::new(TurtleGoHomeGoal::default()));
            goal_selector.add_goal(7, Box::new(TurtleTravelGoal { stuck: false }));
            goal_selector.add_goal(
                8,
                LookAtEntityGoal::with_default(mob_weak, &EntityType::PLAYER, 8.0),
            );
            goal_selector.add_goal(
                9,
                Box::new(TurtleRandomStrollGoal(WanderAroundGoal::with_interval(
                    1.0, 100,
                ))),
            );
        };

        mob_arc
    }

    /// Vanilla `Turtle` constructor and `createNavigation`.
    fn set_up_movement(mob_entity: &mut MobEntity) {
        *mob_entity
            .move_control
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            Box::new(TurtleMoveControl::default());
        let navigator = mob_entity
            .navigator
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *navigator = Navigator::amphibious(false);
        navigator.set_pathfinding_malus(PathType::Water, 0.0);
        navigator.set_pathfinding_malus(PathType::DoorIronClosed, -1.0);
        navigator.set_pathfinding_malus(PathType::DoorWoodClosed, -1.0);
        navigator.set_pathfinding_malus(PathType::DoorOpen, -1.0);
    }

    #[must_use]
    pub fn has_egg(&self) -> bool {
        self.has_egg.load(Ordering::Relaxed)
    }

    pub fn set_has_egg(&self, has_egg: bool) {
        self.has_egg.store(has_egg, Ordering::Relaxed);
        let entity = self.get_entity();
        entity.set_synced_data(pumpkin_data::tracked_data::turtle::HAS_EGG, has_egg);
    }

    #[must_use]
    pub fn is_laying_egg(&self) -> bool {
        self.laying_egg.load(Ordering::Relaxed)
    }

    /// Vanilla `setLayingEgg`: the counter starts at 1 while laying.
    pub fn set_laying_egg(&self, laying_egg: bool) {
        self.lay_egg_counter
            .store(i32::from(laying_egg), Ordering::Relaxed);
        self.laying_egg.store(laying_egg, Ordering::Relaxed);
        let entity = self.get_entity();
        entity.set_synced_data(pumpkin_data::tracked_data::turtle::LAYING_EGG, laying_egg);
    }

    /// Vanilla `setHomePos`: the beach it returns to for laying eggs.
    pub fn set_home_pos(&self, pos: BlockPos) {
        self.home_pos.store(pos);
    }

    fn going_home(&self) -> bool {
        self.going_home.load(Ordering::Relaxed)
    }

    /// Whether home is within `distance` of the turtle, like vanilla `closerToCenterThan`.
    fn home_within(&self, distance: f64) -> bool {
        let home = self.home_pos.load();
        let center = Vector3::new(
            f64::from(home.0.x) + 0.5,
            f64::from(home.0.y) + 0.5,
            f64::from(home.0.z) + 0.5,
        );
        center.squared_distance_to_vec(&self.get_entity().pos.load()) < distance * distance
    }

    /// Vanilla `Turtle.ageBoundaryReached`: a grown up turtle drops its scute.
    fn drop_grow_gift(&self) {
        let entity = self.get_entity();
        let world = entity.world.load_full();
        if !world.level_info.load().game_rules.mob_drops {
            return;
        }
        if let Some(table) = world.get_loot_table("minecraft:gameplay/turtle_grow") {
            let params = LootContextParameters {
                world: Some(&world),
                position: Some(entity.pos.load()),
                this_entity: Some(self),
                ..Default::default()
            };
            for stack in generate_loot_from_handle(&table, self.get_random().random(), &params) {
                entity.spawn_at_location(stack);
            }
        }
    }
}

impl AgeableMob for TurtleEntity {
    fn get_ageable_data(&self) -> &AgeableData {
        &self.ageable_data
    }

    fn age_boundary_reached(&self) {
        if !self.is_baby() {
            self.drop_grow_gift();
        }
    }
}

impl Animal for TurtleEntity {
    /// Vanilla `Turtle.isFood`.
    fn is_food(&self, item_stack: &ItemStack) -> bool {
        item_stack.item.has_tag(&tag::Item::MINECRAFT_TURTLE_FOOD)
    }

    /// Vanilla `TurtleBreedGoal.canUse` and `canFallInLove`: not while it carries an egg.
    fn can_mate(&self, partner: &dyn EntityBase) -> bool {
        !self.has_egg()
            && partner
                .cast_any()
                .downcast_ref::<Self>()
                .is_none_or(|partner| !partner.has_egg())
    }

    fn breeds_eggs(&self) -> bool {
        true
    }

    fn on_bred_egg(&self) {
        self.set_has_egg(true);
    }
}

impl CustomSound for TurtleEntity {
    fn hurt_sound(&self) -> Option<Sound> {
        Some(if self.is_baby() {
            Sound::EntityTurtleHurtBaby
        } else {
            Sound::EntityTurtleHurt
        })
    }

    fn death_sound(&self) -> Option<Sound> {
        Some(if self.is_baby() {
            Sound::EntityTurtleDeathBaby
        } else {
            Sound::EntityTurtleDeath
        })
    }
}

impl Mob for TurtleEntity {
    /// Vanilla `Turtle.getAmbientSound`.
    fn get_ambient_sound(&self) -> Option<Sound> {
        let entity = self.get_entity();
        (entity.on_ground.load(Ordering::Relaxed) && !entity.is_in_water() && !self.is_baby())
            .then_some(Sound::EntityTurtleAmbientLand)
    }

    /// Vanilla `Turtle.getAmbientSoundInterval`.
    fn get_ambient_sound_interval(&self) -> i32 {
        200
    }

    fn as_custom_sound(&self) -> Option<&dyn CustomSound> {
        Some(self)
    }

    fn mob_play_step_sound(&self, _pos: &BlockPos, _state: &BlockState) {
        let sound = if self.is_baby() {
            Sound::EntityTurtleShambleBaby
        } else {
            Sound::EntityTurtleShamble
        };
        self.play_entity_sound(sound, 0.15, 1.0);
    }

    fn mob_play_swim_sound(&self, volume: f32) {
        self.play_default_swim_sound(volume * 1.5);
    }

    /// Vanilla `Turtle.isPushedByFluid`.
    fn mob_is_pushed_by_fluids(&self) -> bool {
        false
    }

    fn can_be_leashed(&self) -> bool {
        false
    }

    fn as_ageable(&self) -> Option<&dyn AgeableMob> {
        Some(self)
    }

    fn as_animal(&self) -> Option<&dyn Animal> {
        Some(self)
    }

    /// Vanilla `Turtle.finalizeSpawn`: its home is where it spawned.
    fn finalize_spawn(
        &self,
        _world: &Arc<World>,
        group_data: Option<SpawnGroupData>,
    ) -> Option<SpawnGroupData> {
        self.home_pos.store(self.get_entity().block_pos.load());
        self.finalize_spawn_super(group_data)
    }

    /// Vanilla `Turtle.getWalkTargetValue`: water unless it is going home, then sand.
    fn get_walk_target_value(&self, pos: &BlockPos) -> f32 {
        let world = self.get_entity().world.load();
        if (!self.going_home() && in_water(&world, pos)) || is_sand(&world, &pos.down()) {
            10.0
        } else {
            world.get_light_level_dependent_magic_value(pos) - 0.5
        }
    }

    /// Vanilla `Turtle.travelInWater`: it sinks unless it hunts or nears home.
    fn travel_in_water(&self, caller: &dyn EntityBase, _gravity: f64, _falling: bool) {
        let living = &self.mob_entity.living_entity;
        let entity = &living.entity;
        entity.update_velocity_from_input(living.movement_input.load(), 0.1);
        living.make_move(caller);
        let mut velocity = entity.velocity.load() * 0.9;
        if self.mob_entity.get_target().is_none() && (!self.going_home() || !self.home_within(20.0))
        {
            velocity.y -= 0.005;
        }
        entity.velocity.store(velocity);
    }

    /// Vanilla `Turtle.aiStep`: every 5 ticks of laying, sand particles.
    fn mob_tick(&self, _caller: &dyn EntityBase) {
        let counter = self.lay_egg_counter.load(Ordering::Relaxed);
        let entity = self.get_entity();
        if entity.is_alive() && self.is_laying_egg() && counter >= 1 && counter % 5 == 0 {
            let pos = entity.block_pos.load();
            let world = entity.world.load();
            if is_sand(&world, &pos.down()) {
                let below = world.get_block_state_id(&pos.down());
                world.sync_world_event(
                    WorldEvent::ParticlesAndSoundDestroyBlock,
                    pos,
                    i32::from(below.as_u16()),
                );
            }
        }
    }

    /// Vanilla `Turtle.thunderHit`: lightning kills a turtle.
    fn mob_on_lightning_strike(
        &self,
        caller: &dyn EntityBase,
        _lightning: &crate::entity::lightning::LightningBoltEntity,
    ) {
        caller.damage(caller, f32::MAX, DamageType::LIGHTNING_BOLT);
    }

    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        self.write_ageable_nbt(nbt);
        let home = self.home_pos.load();
        nbt.put(
            "home_pos",
            NbtTag::IntArray(vec![home.0.x, home.0.y, home.0.z]),
        );
        nbt.put_bool("has_egg", self.has_egg());
    }

    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        let home = nbt
            .get_int_array("home_pos")
            .filter(|pos| pos.len() == 3)
            .map_or_else(
                || self.get_entity().block_pos.load(),
                |pos| BlockPos::new(pos[0], pos[1], pos[2]),
            );
        self.home_pos.store(home);
        self.read_ageable_nbt(nbt);
        self.set_has_egg(nbt.get_bool("has_egg").unwrap_or(false));
    }

    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    fn mob_init_data_tracker(&self) {
        let entity = self.get_entity();
        let is_baby = entity.age.load(Ordering::Relaxed) < 0;
        if is_baby {
            entity.set_synced_data(pumpkin_data::tracked_data::turtle::DATA_BABY_ID, true);
        }
        entity.set_synced_data(pumpkin_data::tracked_data::turtle::HAS_EGG, self.has_egg());
        entity.set_synced_data(
            pumpkin_data::tracked_data::turtle::LAYING_EGG,
            self.is_laying_egg(),
        );
    }

    /// Vanilla `canFallInLove`: an adult carrying an egg takes no food.
    fn mob_interact(&self, player: &Arc<Player>, item_stack: &mut ItemStack) -> bool {
        if self.has_egg() && !self.is_baby() && self.is_food(item_stack) {
            return false;
        }
        self.animal_interact(player, item_stack, Sound::EntityTurtleAmbientLand)
    }
}

/// Vanilla `Turtle.TurtleMoveControl`: slower in water away from home and as a baby, and
/// slow on land.
#[derive(Default)]
struct TurtleMoveControl {
    inner: MoveControl,
}

impl TurtleMoveControl {
    fn update_speed(turtle: &TurtleEntity) {
        let living = &turtle.mob_entity.living_entity;
        let entity = &living.entity;
        if entity.is_in_water() {
            entity
                .velocity
                .store(entity.velocity.load().add_raw(0.0, 0.005, 0.0));
            if !turtle.home_within(16.0) {
                living.set_speed((living.get_speed() / 2.0).max(0.08));
            }
            if turtle.is_baby() {
                living.set_speed((living.get_speed() / 3.0).max(0.06));
            }
        } else if entity.on_ground.load(Ordering::Relaxed) {
            living.set_speed((living.get_speed() / 2.0).max(0.06));
        }
    }
}

impl Control for TurtleMoveControl {}

impl MoveControlTrait for TurtleMoveControl {
    fn tick(&mut self, mob: &dyn Mob) {
        let Some(turtle) = as_turtle(mob) else {
            self.inner.tick(mob);
            return;
        };
        Self::update_speed(turtle);
        let living = &turtle.mob_entity.living_entity;
        let entity = &living.entity;
        if self.inner.operation != Operation::MoveTo || mob.is_navigator_idle() {
            living.set_speed(0.0);
            return;
        }
        let pos = entity.pos.load();
        let xd = self.inner.wanted_x - pos.x;
        let mut yd = self.inner.wanted_y - pos.y;
        let zd = self.inner.wanted_z - pos.z;
        let dd = (xd * xd + yd * yd + zd * zd).sqrt();
        if dd < 1.0e-5 {
            living.set_speed(0.0);
            return;
        }
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
        entity.velocity.store(entity.velocity.load().add_raw(
            0.0,
            f64::from(speed) * yd * 0.1,
            0.0,
        ));
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

/// A target block for the turtle's move-to-block goals.
struct TurtleTarget {
    is_target: fn(&World, &BlockPos) -> bool,
    path_interval: i32,
}

impl MoveToTargetPos for TurtleTarget {
    fn is_target_pos(&self, world: Arc<World>, block_pos: BlockPos) -> bool {
        (self.is_target)(&world, &block_pos)
    }

    fn path_recalculation_interval(&self) -> i32 {
        self.path_interval
    }
}

/// A move-to-block goal over a [`TurtleTarget`] it owns.
struct OwnedMoveToBlock {
    goal: MoveToTargetPosGoal<TurtleTarget>,
    _target: Box<TurtleTarget>,
}

impl OwnedMoveToBlock {
    fn new(
        is_target: fn(&World, &BlockPos) -> bool,
        path_interval: i32,
        speed: f64,
        range: i32,
    ) -> Self {
        let target = Box::new(TurtleTarget {
            is_target,
            path_interval,
        });
        // SAFETY: the target has its own heap allocation owned by this goal, so it never moves
        // and outlives the handle, and it is only ever read through shared references.
        let handle: ParentHandle<TurtleTarget> = unsafe { ParentHandle::new(&target) };
        Self {
            goal: MoveToTargetPosGoal::with_default(handle, speed, range),
            _target: target,
        }
    }
}

/// Vanilla `Turtle.TurtleLayEggGoal`: near home with an egg, it digs into sand with air
/// above for 10 seconds and lays one to four eggs.
struct TurtleLayEggGoal(OwnedMoveToBlock);

impl TurtleLayEggGoal {
    fn new() -> Self {
        Self(OwnedMoveToBlock::new(
            |world, pos| world.get_block_state(&pos.up()).is_air() && is_sand(world, pos),
            40,
            1.0,
            16,
        ))
    }

    fn lay(turtle: &TurtleEntity, egg_pos: BlockPos) {
        let entity = turtle.get_entity();
        let world = entity.world.load_full();
        let pitch = turtle.get_random().random::<f32>().mul_add(0.2, 0.9);
        world.play_sound_raw(
            Sound::EntityTurtleLayEgg as u16,
            SoundCategory::Blocks,
            &entity.block_pos.load().to_f64(),
            0.3,
            pitch,
        );
        let mut eggs = TurtleEggLikeProperties::default(&Block::TURTLE_EGG);
        eggs.eggs = turtle.get_random().random_range(1..=4);
        world.set_block_state(
            &egg_pos,
            eggs.to_state_id(&Block::TURTLE_EGG),
            BlockFlags::NOTIFY_ALL,
        );
        world.emit_game_event(
            pumpkin_data::game_event::GameEvent::BlockPlace.name(),
            egg_pos.to_f64(),
        );
        turtle.set_has_egg(false);
        turtle.set_laying_egg(false);
        turtle.mob_entity.love_ticks.store(600, Ordering::Relaxed);
    }
}

impl Goal for TurtleLayEggGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        as_turtle(mob).is_some_and(|turtle| turtle.has_egg() && turtle.home_within(9.0))
            && self.0.goal.can_start(mob)
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        self.0.goal.should_continue(mob)
            && as_turtle(mob).is_some_and(|turtle| turtle.has_egg() && turtle.home_within(9.0))
    }

    fn start(&mut self, mob: &dyn Mob) {
        self.0.goal.start(mob);
    }

    fn stop(&mut self, mob: &dyn Mob) {
        self.0.goal.stop(mob);
    }

    fn tick(&mut self, mob: &dyn Mob) {
        self.0.goal.tick(mob);
        let Some(turtle) = as_turtle(mob) else {
            return;
        };
        if turtle.get_entity().is_in_water() || !self.0.goal.is_reached_target() {
            return;
        }
        let counter = turtle.lay_egg_counter.load(Ordering::Relaxed);
        if counter < 1 {
            turtle.set_laying_egg(true);
        } else if counter > 200 {
            Self::lay(turtle, self.0.goal.target_pos.up());
        }
        if turtle.is_laying_egg() {
            turtle.lay_egg_counter.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn should_run_every_tick(&self) -> bool {
        true
    }

    fn controls(&self) -> Controls {
        self.0.goal.controls()
    }
}

/// Vanilla `Turtle.TurtleGoToWaterGoal`: on land, it walks to water within 24 blocks.
struct TurtleGoToWaterGoal(OwnedMoveToBlock);

impl TurtleGoToWaterGoal {
    fn new(speed: f64) -> Self {
        let mut goal = OwnedMoveToBlock::new(
            |world, pos| world.get_block(pos) == &Block::WATER,
            160,
            speed,
            24,
        );
        goal.goal.lowest_y = -1;
        Self(goal)
    }
}

impl Goal for TurtleGoToWaterGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        let Some(turtle) = as_turtle(mob) else {
            return false;
        };
        let in_water = turtle.get_entity().is_in_water();
        if turtle.is_baby() && !in_water {
            return self.0.goal.can_start(mob);
        }
        !turtle.going_home() && !in_water && !turtle.has_egg() && self.0.goal.can_start(mob)
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        !mob.get_entity().is_in_water()
            && self.0.goal.trying_time <= 1200
            && mob
                .get_entity()
                .world
                .load()
                .get_block(&self.0.goal.target_pos)
                == &Block::WATER
    }

    fn start(&mut self, mob: &dyn Mob) {
        self.0.goal.start(mob);
    }

    fn stop(&mut self, mob: &dyn Mob) {
        self.0.goal.stop(mob);
    }

    fn tick(&mut self, mob: &dyn Mob) {
        self.0.goal.tick(mob);
    }

    fn should_run_every_tick(&self) -> bool {
        true
    }

    fn controls(&self) -> Controls {
        self.0.goal.controls()
    }
}

/// Vanilla `Turtle.TurtleGoHomeGoal`: with an egg, or now and then when more than 64 blocks
/// away, it heads for the beach where it hatched.
#[derive(Default)]
struct TurtleGoHomeGoal {
    stuck: bool,
    close_to_home_try_ticks: i32,
}

impl TurtleGoHomeGoal {
    fn next_pos(mob: &dyn Mob, home: Vector3<f64>, close_to_home: bool) -> Option<Vector3<f64>> {
        let next =
            default_random_pos::get_pos_towards(mob, 16, 3, home, std::f64::consts::PI / 10.0)
                .or_else(|| {
                    default_random_pos::get_pos_towards(
                        mob,
                        8,
                        7,
                        home,
                        std::f64::consts::FRAC_PI_2,
                    )
                })?;
        let world = mob.get_entity().world.load();
        if !close_to_home && world.get_block(&next.to_block_pos()) != &Block::WATER {
            return default_random_pos::get_pos_towards(
                mob,
                16,
                5,
                home,
                std::f64::consts::FRAC_PI_2,
            );
        }
        Some(next)
    }
}

impl Goal for TurtleGoHomeGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        let Some(turtle) = as_turtle(mob) else {
            return false;
        };
        if turtle.is_baby() {
            return false;
        }
        if turtle.has_egg() {
            return true;
        }
        mob.get_random().random_range(0..to_goal_ticks(700)) == 0 && !turtle.home_within(64.0)
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        as_turtle(mob).is_some_and(|turtle| !turtle.home_within(7.0))
            && !self.stuck
            && self.close_to_home_try_ticks <= to_goal_ticks(600)
    }

    fn start(&mut self, mob: &dyn Mob) {
        if let Some(turtle) = as_turtle(mob) {
            turtle.going_home.store(true, Ordering::Relaxed);
        }
        self.stuck = false;
        self.close_to_home_try_ticks = 0;
    }

    fn stop(&mut self, mob: &dyn Mob) {
        if let Some(turtle) = as_turtle(mob) {
            turtle.going_home.store(false, Ordering::Relaxed);
        }
    }

    fn tick(&mut self, mob: &dyn Mob) {
        let Some(turtle) = as_turtle(mob) else {
            return;
        };
        let close_to_home = turtle.home_within(16.0);
        if close_to_home {
            self.close_to_home_try_ticks += 1;
        }
        if !mob.is_navigator_idle() {
            return;
        }
        let home = turtle.home_pos.load();
        let home = Vector3::new(
            f64::from(home.0.x) + 0.5,
            f64::from(home.0.y),
            f64::from(home.0.z) + 0.5,
        );
        let Some(next) = Self::next_pos(mob, home, close_to_home) else {
            self.stuck = true;
            return;
        };
        let pos = mob.get_entity().pos.load();
        mob.get_mob_entity()
            .steered()
            .navigator()
            .set_progress(NavigatorGoal::new(pos, next, 1.0));
    }
}

/// Vanilla `Turtle.TurtleTravelGoal`: in water, it swims towards a far point up to 512 blocks
/// away.
struct TurtleTravelGoal {
    stuck: bool,
}

impl Goal for TurtleTravelGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        as_turtle(mob).is_some_and(|turtle| {
            !turtle.going_home() && !turtle.has_egg() && turtle.get_entity().is_in_water()
        })
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        !mob.is_navigator_idle()
            && !self.stuck
            && as_turtle(mob).is_some_and(|turtle| {
                !turtle.going_home() && !turtle.mob_entity.is_in_love() && !turtle.has_egg()
            })
    }

    fn start(&mut self, mob: &dyn Mob) {
        let Some(turtle) = as_turtle(mob) else {
            return;
        };
        let entity = turtle.get_entity();
        let pos = entity.pos.load();
        let (xt, mut yt, zt) = {
            let mut rng = mob.get_random();
            (
                rng.random_range(0..1025) - 512,
                rng.random_range(0..9) - 4,
                rng.random_range(0..1025) - 512,
            )
        };
        if f64::from(yt) + pos.y > f64::from(entity.world.load().sea_level - 1) {
            yt = 0;
        }
        turtle.travel_pos.store(Some(BlockPos::floored(
            f64::from(xt) + pos.x,
            f64::from(yt) + pos.y,
            f64::from(zt) + pos.z,
        )));
        self.stuck = false;
    }

    fn stop(&mut self, mob: &dyn Mob) {
        if let Some(turtle) = as_turtle(mob) {
            turtle.travel_pos.store(None);
        }
    }

    fn tick(&mut self, mob: &dyn Mob) {
        let Some(turtle) = as_turtle(mob) else {
            return;
        };
        let Some(travel) = turtle.travel_pos.load() else {
            self.stuck = true;
            return;
        };
        if !mob.is_navigator_idle() {
            return;
        }
        let target = Vector3::new(
            f64::from(travel.0.x) + 0.5,
            f64::from(travel.0.y),
            f64::from(travel.0.z) + 0.5,
        );
        let next =
            default_random_pos::get_pos_towards(mob, 16, 3, target, std::f64::consts::PI / 10.0)
                .or_else(|| {
                    default_random_pos::get_pos_towards(
                        mob,
                        8,
                        7,
                        target,
                        std::f64::consts::FRAC_PI_2,
                    )
                })
                .filter(|next| {
                    // Vanilla only travels where the chunks 34 blocks around are loaded.
                    let (x, z) = (next.x.floor() as i32, next.z.floor() as i32);
                    mob.get_entity()
                        .world
                        .load()
                        .has_chunks_at(x - 34, z - 34, x + 34, z + 34)
                });
        let Some(next) = next else {
            self.stuck = true;
            return;
        };
        let pos = mob.get_entity().pos.load();
        mob.get_mob_entity()
            .steered()
            .navigator()
            .set_progress(NavigatorGoal::new(pos, next, 1.0));
    }
}

/// Vanilla `Turtle.TurtleRandomStrollGoal`: only on land, not on the way home or with an egg.
struct TurtleRandomStrollGoal(WanderAroundGoal);

impl Goal for TurtleRandomStrollGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        as_turtle(mob).is_some_and(|turtle| {
            !turtle.get_entity().is_in_water() && !turtle.going_home() && !turtle.has_egg()
        }) && self.0.can_start(mob)
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        self.0.should_continue(mob)
    }

    fn start(&mut self, mob: &dyn Mob) {
        self.0.start(mob);
    }

    fn stop(&mut self, mob: &dyn Mob) {
        self.0.stop(mob);
    }

    fn controls(&self) -> Controls {
        self.0.controls()
    }
}
