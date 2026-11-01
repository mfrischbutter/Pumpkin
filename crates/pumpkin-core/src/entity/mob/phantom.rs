use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};

use crossbeam::atomic::AtomicCell;
use pumpkin_data::attributes::Attributes;
use pumpkin_data::entity::EntityType;
use pumpkin_data::sound::Sound;
use pumpkin_data::world::WorldEvent;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_nbt::tag::NbtTag;
use pumpkin_util::math::boundingbox::EntityDimensions;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_util::math::{approach, approach_degrees, degrees_difference_abs, wrap_degrees};
use pumpkin_world::chunk::ChunkHeightmapType;
use rand::RngExt;

use crate::entity::{
    Entity, EntityBase,
    ai::control::{Control, LookControlTrait, MoveControlTrait},
    ai::goal::{Controls, Goal, to_goal_ticks},
    ai::target_predicate::TargetPredicate,
    mob::{Mob, MobEntity, spawn::SpawnGroupData},
};
use crate::plugin::api::events::entity::entity_target::TargetReason;
use crate::world::World;

/// Vanilla `Phantom.TICKS_PER_FLAP`, `Mth.ceil(24.166098F)`.
const TICKS_PER_FLAP: i32 = 25;
/// Vanilla `Phantom.travel`: `travelFlying(input, 0.2F)`.
const AIR_SPEED: f32 = 0.2;
const MAX_SIZE: i32 = 64;

/// Vanilla `Phantom.AttackPhase`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum AttackPhase {
    Circle,
    Swoop,
}

/// What vanilla keeps on the `Phantom` for its controls and goals.
struct PhantomState {
    move_target_point: AtomicCell<Vector3<f64>>,
    anchor_point: AtomicCell<Option<BlockPos>>,
    attack_phase: AtomicCell<AttackPhase>,
}

pub struct PhantomEntity {
    pub mob_entity: MobEntity,
    state: Arc<PhantomState>,
    size: AtomicI32,
}

impl PhantomEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let state = Arc::new(PhantomState {
            move_target_point: AtomicCell::new(Vector3::new(0.0, 0.0, 0.0)),
            anchor_point: AtomicCell::new(None),
            attack_phase: AtomicCell::new(AttackPhase::Circle),
        });
        let mut mob_entity = MobEntity::new(entity);
        // Vanilla `Phantom` constructor: its own move and look controls.
        *mob_entity
            .move_control
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            Box::new(PhantomMoveControl::new(state.clone()));
        *mob_entity
            .look_control
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Box::new(PhantomLookControl);
        let phantom = Self {
            mob_entity,
            state: state.clone(),
            size: AtomicI32::new(0),
        };
        let mob_arc = Arc::new(phantom);

        {
            // Vanilla `Phantom.registerGoals`.
            let goal_selector = &mob_arc.mob_entity.goals_selector;
            goal_selector.add_goal(1, Box::new(PhantomAttackStrategyGoal::new(state.clone())));
            goal_selector.add_goal(2, Box::new(PhantomSweepAttackGoal::new(state.clone())));
            goal_selector.add_goal(3, Box::new(PhantomCircleAroundAnchorGoal::new(state)));
            mob_arc
                .mob_entity
                .target_selector
                .add_goal(1, Box::new(PhantomAttackPlayerTargetGoal::default()));
        };

        mob_arc
    }

    #[must_use]
    pub fn get_phantom_size(&self) -> i32 {
        self.size.load(Ordering::Relaxed)
    }

    /// Vanilla `setPhantomSize` and `updatePhantomSizeInfo`: 15% bigger and one more attack
    /// damage per size.
    pub fn set_phantom_size(&self, size: i32) {
        let size = size.clamp(0, MAX_SIZE);
        self.size.store(size, Ordering::Relaxed);
        let living = &self.mob_entity.living_entity;
        let entity = &living.entity;
        entity.set_synced_data(pumpkin_data::tracked_data::phantom::ID_SIZE, size);
        let entity_type = entity.entity_type;
        let dimensions = EntityDimensions::new(
            entity_type.dimension[0],
            entity_type.dimension[1],
            entity_type.eye_height,
        );
        let pos = entity.pos.load();
        entity.set_default_dimensions(dimensions.scale(0.15f32.mul_add(size as f32, 1.0)));
        entity.set_pos(pos);
        living.set_attribute_base(&Attributes::ATTACK_DAMAGE, f64::from(6 + size));
    }
}

impl Mob for PhantomEntity {
    fn mob_is_flapping(&self) -> bool {
        let entity = self.get_entity();
        // Vanilla `getUniqueFlapTickOffset` spreads the flaps by entity id.
        entity
            .entity_id
            .wrapping_mul(3)
            .wrapping_add(entity.tick_count.load(Ordering::Relaxed))
            % TICKS_PER_FLAP
            == 0
    }

    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    fn get_mob_gravity(&self) -> f64 {
        0.0
    }

    fn travel(&self, caller: &dyn EntityBase) -> bool {
        self.mob_entity
            .living_entity
            .travel_flying_at(caller, AIR_SPEED);
        true
    }

    /// Vanilla `checkFallDamage` does nothing for a phantom.
    fn should_check_fall_damage(&self) -> bool {
        false
    }

    fn get_ambient_sound(&self) -> Option<Sound> {
        Some(Sound::EntityPhantomAmbient)
    }

    /// Vanilla `Phantom.finalizeSpawn`: it circles 5 blocks above where it spawned.
    fn finalize_spawn(
        &self,
        _world: &Arc<World>,
        group_data: Option<SpawnGroupData>,
    ) -> Option<SpawnGroupData> {
        self.state
            .anchor_point
            .store(Some(self.get_entity().block_pos.load().up_height(5)));
        self.set_phantom_size(0);
        self.finalize_spawn_super(group_data)
    }

    fn mob_init_data_tracker(&self) {
        self.get_entity().set_synced_data(
            pumpkin_data::tracked_data::phantom::ID_SIZE,
            self.get_phantom_size(),
        );
    }

    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        if let Some(anchor) = self.state.anchor_point.load() {
            nbt.put(
                "anchor_pos",
                NbtTag::IntArray(vec![anchor.0.x, anchor.0.y, anchor.0.z]),
            );
        }
        nbt.put_int("size", self.get_phantom_size());
    }

    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        let anchor = nbt
            .get_int_array("anchor_pos")
            .filter(|pos| pos.len() == 3)
            .map(|pos| BlockPos::new(pos[0], pos[1], pos[2]));
        self.state.anchor_point.store(anchor);
        self.set_phantom_size(nbt.get_int("size").unwrap_or(0));
    }
}

/// Vanilla `Phantom.PhantomMoveControl`: turns towards the move target point and speeds up
/// while it flies straight.
struct PhantomMoveControl {
    state: Arc<PhantomState>,
    speed: f32,
}

impl PhantomMoveControl {
    const fn new(state: Arc<PhantomState>) -> Self {
        Self { state, speed: 0.1 }
    }
}

impl Control for PhantomMoveControl {}

impl MoveControlTrait for PhantomMoveControl {
    fn tick(&mut self, mob: &dyn Mob) {
        let entity = mob.get_entity();
        if entity.horizontal_collision.load(Ordering::Relaxed) {
            entity.yaw.store(entity.yaw.load() + 180.0);
            self.speed = 0.1;
        }

        let pos = entity.pos.load();
        let target = self.state.move_target_point.load();
        let (mut tdx, tdy, mut tdz) = (target.x - pos.x, target.y - pos.y, target.z - pos.z);
        let mut sd = tdx.hypot(tdz);
        if sd.abs() <= 1.0e-5 {
            return;
        }
        let y_relative_scale = 1.0 - (tdy * 0.7).abs() / sd;
        tdx *= y_relative_scale;
        tdz *= y_relative_scale;
        sd = tdx.hypot(tdz);
        let sd2 = (tdx * tdx + tdz * tdz + tdy * tdy).sqrt();
        let prev = entity.yaw.load();
        let angle = tdz.atan2(tdx) as f32;
        let a = wrap_degrees(prev + 90.0);
        let b = wrap_degrees(angle.to_degrees());
        let yaw = approach_degrees(a, b, 4.0) - 90.0;
        entity.yaw.store(yaw);
        // Vanilla `PhantomBodyRotationControl`: the head follows the body, the body the yaw.
        entity.head_yaw.store(entity.body_yaw.load());
        entity.body_yaw.store(yaw);
        self.speed = if degrees_difference_abs(prev, yaw) < 3.0 {
            approach(self.speed, 1.8, 0.005 * (1.8 / self.speed))
        } else {
            approach(self.speed, 0.2, 0.025)
        };

        let pitch = -((-tdy).atan2(sd).to_degrees()) as f32;
        entity.pitch.store(pitch);
        let move_angle = (yaw + 90.0).to_radians();
        let speed = f64::from(self.speed);
        let wanted = Vector3::new(
            speed * f64::from(move_angle.cos()) * (tdx / sd2).abs(),
            speed * f64::from(pitch.to_radians().sin()) * (tdy / sd2).abs(),
            speed * f64::from(move_angle.sin()) * (tdz / sd2).abs(),
        );
        let movement = entity.velocity.load();
        entity.velocity.store(movement + (wanted - movement) * 0.2);
    }
}

/// Vanilla `Phantom.PhantomLookControl`: the move control does all the turning.
struct PhantomLookControl;

impl Control for PhantomLookControl {}

impl LookControlTrait for PhantomLookControl {
    fn tick(&mut self, _mob: &dyn Mob) {}

    fn look_at_with_range(
        &mut self,
        _x: f64,
        _y: f64,
        _z: f64,
        _max_yaw_change: f32,
        _max_pitch_change: f32,
    ) {
    }

    fn wanted_position(&self) -> Vector3<f64> {
        Vector3::new(0.0, 0.0, 0.0)
    }

    fn is_looking_at_target(&self) -> bool {
        false
    }
}

/// Vanilla `Phantom.canAttack(level, target, TargetingConditions.DEFAULT)`.
fn can_attack(mob: &dyn Mob, target: &dyn EntityBase) -> bool {
    TargetPredicate::create_attackable().test(
        &mob.get_entity().world.load(),
        Some(mob as &dyn EntityBase),
        target,
    )
}

/// Vanilla `Phantom.PhantomAttackPlayerTargetGoal`: every 3 seconds, the highest player in
/// reach it may attack.
struct PhantomAttackPlayerTargetGoal {
    next_scan_tick: i32,
}

impl Default for PhantomAttackPlayerTargetGoal {
    fn default() -> Self {
        Self {
            next_scan_tick: to_goal_ticks(20),
        }
    }
}

impl Goal for PhantomAttackPlayerTargetGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        if self.next_scan_tick > 0 {
            self.next_scan_tick -= 1;
            return false;
        }
        self.next_scan_tick = to_goal_ticks(60);
        let entity = mob.get_entity();
        let world = entity.world.load();
        let area = entity.bounding_box.load().expand(16.0, 64.0, 16.0);
        let in_range = TargetPredicate::create_attackable().set_base_max_distance(64.0);
        let mut players: Vec<_> = world
            .get_nearby_players(entity.pos.load(), 96.0)
            .into_iter()
            .filter(|player| {
                area.intersects(&player.get_entity().bounding_box.load())
                    && in_range.test(&world, Some(mob as &dyn EntityBase), player.as_ref())
            })
            .collect();
        players.sort_by(|a, b| {
            b.get_entity()
                .pos
                .load()
                .y
                .total_cmp(&a.get_entity().pos.load().y)
        });
        for player in players {
            if can_attack(mob, player.as_ref()) {
                let target: Arc<dyn EntityBase> = player;
                mob.set_mob_target(Some(target), TargetReason::ClosestPlayer);
                return true;
            }
        }
        false
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        mob.get_mob_entity()
            .get_target()
            .is_some_and(|target| can_attack(mob, target.as_ref()))
    }

    fn controls(&self) -> Controls {
        Controls::empty()
    }
}

/// Vanilla `Phantom.PhantomAttackStrategyGoal`: circles high above its target and swoops
/// every 8 to 12 seconds.
struct PhantomAttackStrategyGoal {
    state: Arc<PhantomState>,
    next_sweep_tick: i32,
}

impl PhantomAttackStrategyGoal {
    const fn new(state: Arc<PhantomState>) -> Self {
        Self {
            state,
            next_sweep_tick: 0,
        }
    }

    fn set_anchor_above_target(&self, mob: &dyn Mob) {
        if self.state.anchor_point.load().is_none() {
            return;
        }
        let Some(target) = mob.get_mob_entity().get_target() else {
            return;
        };
        let above = 20 + mob.get_random().random_range(0..20);
        let mut anchor = target.get_entity().block_pos.load().up_height(above);
        let sea_level = mob.get_entity().world.load().sea_level;
        if anchor.0.y < sea_level {
            anchor = BlockPos::new(anchor.0.x, sea_level + 1, anchor.0.z);
        }
        self.state.anchor_point.store(Some(anchor));
    }
}

impl Goal for PhantomAttackStrategyGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        mob.get_mob_entity()
            .get_target()
            .is_some_and(|target| can_attack(mob, target.as_ref()))
    }

    fn start(&mut self, mob: &dyn Mob) {
        self.next_sweep_tick = to_goal_ticks(10);
        self.state.attack_phase.store(AttackPhase::Circle);
        self.set_anchor_above_target(mob);
    }

    fn stop(&mut self, mob: &dyn Mob) {
        if let Some(anchor) = self.state.anchor_point.load() {
            let world = mob.get_entity().world.load();
            let ground = world.get_heightmap_height(
                ChunkHeightmapType::MotionBlocking,
                anchor.0.x,
                anchor.0.z,
            );
            let above = 10 + mob.get_random().random_range(0..20);
            self.state.anchor_point.store(Some(BlockPos::new(
                anchor.0.x,
                ground + above,
                anchor.0.z,
            )));
        }
    }

    fn tick(&mut self, mob: &dyn Mob) {
        if self.state.attack_phase.load() != AttackPhase::Circle {
            return;
        }
        self.next_sweep_tick -= 1;
        if self.next_sweep_tick <= 0 {
            self.state.attack_phase.store(AttackPhase::Swoop);
            self.set_anchor_above_target(mob);
            let mut rng = mob.get_random();
            self.next_sweep_tick = to_goal_ticks((8 + rng.random_range(0..4)) * 20);
            let pitch = rng.random::<f32>().mul_add(0.1, 0.95);
            mob.play_entity_sound(Sound::EntityPhantomSwoop, 10.0, pitch);
        }
    }

    fn controls(&self) -> Controls {
        Controls::empty()
    }
}

/// Vanilla `PhantomMoveTargetGoal.touchingTarget`.
fn touching_target(state: &PhantomState, mob: &dyn Mob) -> bool {
    state
        .move_target_point
        .load()
        .squared_distance_to_vec(&mob.get_entity().pos.load())
        < 4.0
}

/// Vanilla `Phantom.PhantomCircleAroundAnchorGoal`: circles its anchor at a changing
/// distance and height.
struct PhantomCircleAroundAnchorGoal {
    state: Arc<PhantomState>,
    angle: f32,
    distance: f32,
    height: f32,
    clockwise: f32,
}

impl PhantomCircleAroundAnchorGoal {
    const fn new(state: Arc<PhantomState>) -> Self {
        Self {
            state,
            angle: 0.0,
            distance: 0.0,
            height: 0.0,
            clockwise: 1.0,
        }
    }

    fn select_next(&mut self, mob: &dyn Mob) {
        let anchor = self.state.anchor_point.load().unwrap_or_else(|| {
            let pos = mob.get_entity().block_pos.load();
            self.state.anchor_point.store(Some(pos));
            pos
        });
        self.angle += self.clockwise * 15.0f32.to_radians();
        self.state.move_target_point.store(Vector3::new(
            f64::from(anchor.0.x) + f64::from(self.distance * self.angle.cos()),
            f64::from(anchor.0.y) + f64::from(-4.0 + self.height),
            f64::from(anchor.0.z) + f64::from(self.distance * self.angle.sin()),
        ));
    }
}

impl Goal for PhantomCircleAroundAnchorGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        mob.get_mob_entity().get_target().is_none()
            || self.state.attack_phase.load() == AttackPhase::Circle
    }

    fn start(&mut self, mob: &dyn Mob) {
        let mut rng = mob.get_random();
        self.distance = rng.random::<f32>().mul_add(10.0, 5.0);
        self.height = rng.random::<f32>().mul_add(9.0, -4.0);
        self.clockwise = if rng.random::<bool>() { 1.0 } else { -1.0 };
        self.select_next(mob);
    }

    fn tick(&mut self, mob: &dyn Mob) {
        let mut rng = mob.get_random();
        if rng.random_range(0..to_goal_ticks(350)) == 0 {
            self.height = rng.random::<f32>().mul_add(9.0, -4.0);
        }
        if rng.random_range(0..to_goal_ticks(250)) == 0 {
            self.distance += 1.0;
            if self.distance > 15.0 {
                self.distance = 5.0;
                self.clockwise = -self.clockwise;
            }
        }
        if rng.random_range(0..to_goal_ticks(450)) == 0 {
            self.angle = rng.random::<f32>() * 2.0 * std::f32::consts::PI;
            self.select_next(mob);
        }
        if touching_target(&self.state, mob) {
            self.select_next(mob);
        }

        let entity = mob.get_entity();
        let world = entity.world.load();
        let y = entity.pos.load().y;
        let block_pos = entity.block_pos.load();
        let target_y = self.state.move_target_point.load().y;
        if target_y < y && !world.get_block_state(&block_pos.down()).is_air() {
            self.height = self.height.max(1.0);
            self.select_next(mob);
        }
        if target_y > y && !world.get_block_state(&block_pos.up()).is_air() {
            self.height = self.height.min(-1.0);
            self.select_next(mob);
        }
    }

    fn controls(&self) -> Controls {
        Controls::MOVE
    }
}

/// Vanilla `Phantom.PhantomSweepAttackGoal`: dives at its target and bites it, unless a cat
/// within 16 blocks scares it off.
struct PhantomSweepAttackGoal {
    state: Arc<PhantomState>,
    scared_of_cat: bool,
    cat_search_tick: i32,
}

impl PhantomSweepAttackGoal {
    const fn new(state: Arc<PhantomState>) -> Self {
        Self {
            state,
            scared_of_cat: false,
            cat_search_tick: 0,
        }
    }

    /// The cats around hiss, and scare the phantom off.
    fn look_for_cats(&mut self, mob: &dyn Mob) {
        let entity = mob.get_entity();
        let tick = entity.tick_count.load(Ordering::Relaxed);
        if tick <= self.cat_search_tick {
            return;
        }
        self.cat_search_tick = tick + 20;
        let area = entity.bounding_box.load().expand(16.0, 16.0, 16.0);
        let mut scared = false;
        for cat in entity.world.load().get_entities_at_box(&area) {
            if cat.get_entity().entity_type.id == EntityType::CAT.id && cat.get_entity().is_alive()
            {
                if let Some(cat) = cat.get_mob() {
                    cat.make_sound(Sound::EntityCatHiss);
                }
                scared = true;
            }
        }
        self.scared_of_cat = scared;
    }
}

impl Goal for PhantomSweepAttackGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        mob.get_mob_entity().get_target().is_some()
            && self.state.attack_phase.load() == AttackPhase::Swoop
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        let Some(target) = mob.get_mob_entity().get_target() else {
            return false;
        };
        if !target.get_entity().is_alive()
            || target.is_spectator()
            || target
                .get_player()
                .is_some_and(|player| player.gamemode.load() == pumpkin_util::GameMode::Creative)
            || !self.can_start(mob)
        {
            return false;
        }
        self.look_for_cats(mob);
        !self.scared_of_cat
    }

    fn stop(&mut self, mob: &dyn Mob) {
        mob.set_mob_target(None, TargetReason::ForgotTarget);
        self.state.attack_phase.store(AttackPhase::Circle);
    }

    fn tick(&mut self, mob: &dyn Mob) {
        let Some(target) = mob.get_mob_entity().get_target() else {
            return;
        };
        let target_entity = target.get_entity();
        let target_pos = target_entity.pos.load();
        let half_height = f64::from(target_entity.entity_dimension.load().height) * 0.5;
        self.state.move_target_point.store(Vector3::new(
            target_pos.x,
            target_pos.y + half_height,
            target_pos.z,
        ));
        let entity = mob.get_entity();
        if entity
            .bounding_box
            .load()
            .expand(0.2, 0.2, 0.2)
            .intersects(&target_entity.bounding_box.load())
        {
            mob.get_mob_entity()
                .try_attack(mob as &dyn EntityBase, target.as_ref());
            self.state.attack_phase.store(AttackPhase::Circle);
            if !entity.is_silent() {
                entity.world.load().sync_world_event(
                    WorldEvent::SoundPhantomBite,
                    entity.block_pos.load(),
                    0,
                );
            }
        } else if entity.horizontal_collision.load(Ordering::Relaxed)
            || mob
                .get_mob_entity()
                .living_entity
                .hurt_cooldown
                .load(Ordering::Relaxed)
                > 10
        {
            self.state.attack_phase.store(AttackPhase::Circle);
        }
    }

    fn controls(&self) -> Controls {
        Controls::MOVE
    }
}
