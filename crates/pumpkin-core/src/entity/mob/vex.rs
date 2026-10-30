use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::{Arc, Weak};

use crossbeam::atomic::AtomicCell;
use pumpkin_data::damage::DamageType;
use pumpkin_data::data_component_impl::EquipmentSlot;
use pumpkin_data::entity::EntityType;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::Sound;
use pumpkin_data::tracked_data;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::math::position::BlockPos;
use rand::RngExt;
use uuid::Uuid;

use crate::entity::ai::behavior::hunting::{self, Hunt};
use crate::entity::ai::brain::memory::codec::{block_pos_from_tag, block_pos_to_tag};
use crate::entity::ai::control::vex_move_control::VexMoveControl;
use crate::entity::ai::target_match::TargetMatch;
use crate::entity::ai::target_predicate::TargetPredicate;
use crate::entity::mob::equipment::RegionalDifficulty;
use crate::entity::mob::raider;
use crate::entity::mob::spawn::SpawnGroupData;
use crate::entity::{
    Entity, EntityBase,
    ai::goal::{
        Controls, Goal, look_at_entity::LookAtEntityGoal, swim::SwimGoal, to_goal_ticks,
        track_target::TrackTargetGoal,
    },
    living::LivingEntity,
    mob::{Mob, MobEntity},
};
use crate::plugin::api::events::entity::entity_target::TargetReason;
use crate::world::World;

/// Vanilla `Mth.ceil(PI * 5 / 4)`.
const TICKS_PER_FLAP: i32 = 4;
const FLAG_IS_CHARGING: i8 = 1;
/// Ticks between the starve damage of a vex whose summoned life ran out.
const LIMITED_LIFE_DAMAGE_INTERVAL: i32 = 20;
const LIMITED_LIFE_DAMAGE: f32 = 1.0;
/// One in this many goal ticks a goal of the vex picks a new move.
const MOVE_CHANCE: i32 = 7;
const CHARGE_SPEED: f64 = 1.0;
/// A charge starts only at more than this squared distance to the target.
const CHARGE_MIN_DISTANCE_SQ: f64 = 4.0;
/// Within this squared distance the charge keeps following the target's eyes.
const CHARGE_FOLLOW_DISTANCE_SQ: f64 = 9.0;
const RANDOM_MOVE_SPEED: f64 = 0.25;
const RANDOM_MOVE_ATTEMPTS: usize = 3;
const RANDOM_MOVE_LOOK_YAW: f32 = 180.0;
const RANDOM_MOVE_LOOK_PITCH: f32 = 20.0;

const VEX_HUNTS: &[Hunt] = &[Hunt::new(3, TargetMatch::Type(&EntityType::PLAYER), true)];

/// Represents a Vex, the flying spirit an evoker summons. It charges through blocks at its target.
///
/// Wiki: <https://minecraft.wiki/w/Vex>
pub struct VexEntity {
    pub mob_entity: MobEntity,
    owner: AtomicCell<Option<Uuid>>,
    bound_origin: AtomicCell<Option<BlockPos>>,
    has_limited_life: AtomicBool,
    limited_life_ticks: AtomicI32,
    charging: AtomicBool,
}

impl VexEntity {
    #[must_use]
    pub fn new(entity: Entity) -> Arc<Self> {
        let mut mob_entity = MobEntity::new(entity);
        *mob_entity
            .move_control
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            Box::new(VexMoveControl::default());
        let vex = Self {
            mob_entity,
            owner: AtomicCell::new(None),
            bound_origin: AtomicCell::new(None),
            has_limited_life: AtomicBool::new(false),
            limited_life_ticks: AtomicI32::new(0),
            charging: AtomicBool::new(false),
        };
        let mob_arc = Arc::new(vex);
        let mob_weak: Weak<dyn Mob> = {
            let mob_arc: Arc<dyn Mob> = mob_arc.clone();
            Arc::downgrade(&mob_arc)
        };

        {
            let goal_selector = &mob_arc.mob_entity.goals_selector;

            goal_selector.add_goal(0, Box::new(SwimGoal::default()));
            goal_selector.add_goal(4, Box::new(VexChargeAttackGoal));
            goal_selector.add_goal(8, Box::new(VexRandomMoveGoal));
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
            target_selector.add_goal(2, Box::new(VexCopyOwnerTargetGoal::new()));
            hunting::apply(target_selector, &mob_arc.mob_entity, VEX_HUNTS);
        };

        mob_arc
    }

    /// The owner if it is loaded, vanilla `getOwner`.
    #[must_use]
    pub fn get_owner(&self) -> Option<Arc<dyn EntityBase>> {
        let owner = self
            .get_entity()
            .world
            .load()
            .get_entity_by_uuid(self.owner.load()?)?;
        owner.get_living_entity().is_some().then_some(owner)
    }

    pub fn set_owner(&self, owner: &dyn EntityBase) {
        self.owner.store(Some(owner.get_entity().entity_uuid));
    }

    #[must_use]
    pub fn get_bound_origin(&self) -> Option<BlockPos> {
        self.bound_origin.load()
    }

    pub fn set_bound_origin(&self, bound_origin: Option<BlockPos>) {
        self.bound_origin.store(bound_origin);
    }

    /// Starts the starve damage after `life_ticks`, for summoned vexes.
    pub fn set_limited_life(&self, life_ticks: i32) {
        self.has_limited_life.store(true, Ordering::Relaxed);
        self.limited_life_ticks.store(life_ticks, Ordering::Relaxed);
    }

    #[must_use]
    pub fn is_charging(&self) -> bool {
        self.charging.load(Ordering::Relaxed)
    }

    pub fn set_is_charging(&self, charging: bool) {
        self.charging.store(charging, Ordering::Relaxed);
        let flags = if charging { FLAG_IS_CHARGING } else { 0 };
        self.get_entity()
            .set_synced_data(tracked_data::vex::DATA_FLAGS_ID, flags);
    }

    /// The part of vanilla `Vex.tick` after `super.tick()`.
    fn tick_limited_life(&self) {
        if self.has_limited_life.load(Ordering::Relaxed)
            && self.limited_life_ticks.fetch_sub(1, Ordering::Relaxed) - 1 <= 0
        {
            self.limited_life_ticks
                .store(LIMITED_LIFE_DAMAGE_INTERVAL, Ordering::Relaxed);
            self.damage(self, LIMITED_LIFE_DAMAGE, DamageType::STARVE);
        }
    }
}

impl Mob for VexEntity {
    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    fn get_ambient_sound(&self) -> Option<Sound> {
        Some(Sound::EntityVexAmbient)
    }

    fn mob_is_flapping(&self) -> bool {
        self.get_entity().tick_count.load(Ordering::Relaxed) % TICKS_PER_FLAP == 0
    }

    /// Vanilla `Vex.tick` runs the whole entity tick without physics.
    fn mob_tick(&self, _caller: &dyn EntityBase) {
        self.get_entity().no_physics.store(true, Ordering::Relaxed);
    }

    fn post_tick(&self) {
        let entity = self.get_entity();
        entity.no_physics.store(false, Ordering::Relaxed);
        entity.set_has_no_gravity(true);
        self.tick_limited_life();
    }

    fn finalize_spawn(
        &self,
        world: &Arc<World>,
        group_data: Option<SpawnGroupData>,
    ) -> Option<SpawnGroupData> {
        let difficulty = RegionalDifficulty::at(world, self.get_entity().pos.load());
        self.populate_default_equipment_slots(world, &difficulty);
        self.populate_default_equipment_enchantments(&difficulty);
        self.finalize_spawn_super(group_data)
    }

    fn populate_default_equipment_slots(
        &self,
        _world: &Arc<World>,
        _difficulty: &RegionalDifficulty,
    ) {
        self.mob_entity.set_item_slot(
            &EquipmentSlot::MAIN_HAND,
            ItemStack::new(1, &Item::IRON_SWORD),
        );
        self.mob_entity
            .living_entity
            .equipment_drop_chances
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(EquipmentSlot::MAIN_HAND, 0.0);
    }

    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        if let Some(bound_origin) = self.bound_origin.load() {
            nbt.put("bound_pos", block_pos_to_tag(bound_origin));
        }
        if self.has_limited_life.load(Ordering::Relaxed) {
            nbt.put_int(
                "life_ticks",
                self.limited_life_ticks.load(Ordering::Relaxed),
            );
        }
        if let Some(owner) = self.owner.load() {
            nbt.put_uuid("owner", owner);
        }
    }

    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        self.bound_origin
            .store(nbt.get("bound_pos").and_then(block_pos_from_tag));
        let life_ticks = nbt.get_int("life_ticks");
        self.has_limited_life
            .store(life_ticks.is_some(), Ordering::Relaxed);
        if let Some(life_ticks) = life_ticks {
            self.limited_life_ticks.store(life_ticks, Ordering::Relaxed);
        }
        self.owner.store(nbt.get_uuid("owner"));
    }
}

fn as_vex(mob: &dyn Mob) -> Option<&VexEntity> {
    mob.cast_any().downcast_ref::<VexEntity>()
}

fn is_alive(entity: &dyn EntityBase) -> bool {
    entity
        .get_living_entity()
        .is_some_and(LivingEntity::is_alive)
}

/// Vanilla `Vex.this.getMoveControl().hasWanted()`.
fn has_wanted_move(mob: &dyn Mob) -> bool {
    mob.get_mob_entity().steered().move_control().has_wanted()
}

/// Vanilla `Vex.this.moveControl.setWantedPosition`: the vex's own control, not a vehicle's.
fn set_wanted_move(mob: &dyn Mob, x: f64, y: f64, z: f64, speed: f64) {
    mob.get_mob_entity()
        .move_control
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .set_wanted_position(x, y, z, speed);
}

fn charge_at_eyes(mob: &dyn Mob, target: &dyn EntityBase) {
    let eyes = target.get_entity().get_eye_pos();
    set_wanted_move(mob, eyes.x, eyes.y, eyes.z, CHARGE_SPEED);
}

fn distance_sq(mob: &dyn Mob, target: &dyn EntityBase) -> f64 {
    mob.get_entity()
        .pos
        .load()
        .squared_distance_to_vec(&target.get_entity().pos.load())
}

/// Vanilla `Vex.VexChargeAttackGoal`: flies straight at the target's eyes and hits it on contact.
struct VexChargeAttackGoal;

impl Goal for VexChargeAttackGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        let Some(target) = mob.get_mob_entity().get_target() else {
            return false;
        };
        is_alive(target.as_ref())
            && !has_wanted_move(mob)
            && mob.get_random().random_range(0..to_goal_ticks(MOVE_CHANCE)) == 0
            && distance_sq(mob, target.as_ref()) > CHARGE_MIN_DISTANCE_SQ
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        has_wanted_move(mob)
            && as_vex(mob).is_some_and(VexEntity::is_charging)
            && mob
                .get_mob_entity()
                .get_target()
                .is_some_and(|target| is_alive(target.as_ref()))
    }

    fn start(&mut self, mob: &dyn Mob) {
        if let Some(target) = mob.get_mob_entity().get_target() {
            charge_at_eyes(mob, target.as_ref());
        }
        if let Some(vex) = as_vex(mob) {
            vex.set_is_charging(true);
        }
        mob.play_entity_sound(Sound::EntityVexCharge, 1.0, 1.0);
    }

    fn stop(&mut self, mob: &dyn Mob) {
        if let Some(vex) = as_vex(mob) {
            vex.set_is_charging(false);
        }
    }

    fn should_run_every_tick(&self) -> bool {
        true
    }

    fn tick(&mut self, mob: &dyn Mob) {
        let Some(target) = mob.get_mob_entity().get_target() else {
            return;
        };
        let hits = mob
            .get_entity()
            .bounding_box
            .load()
            .intersects(&target.get_entity().bounding_box.load());
        if hits {
            mob.do_hurt_target(target.as_ref());
            if let Some(vex) = as_vex(mob) {
                vex.set_is_charging(false);
            }
        } else if distance_sq(mob, target.as_ref()) < CHARGE_FOLLOW_DISTANCE_SQ {
            charge_at_eyes(mob, target.as_ref());
        }
    }

    fn controls(&self) -> Controls {
        Controls::MOVE
    }
}

/// Vanilla `Vex.VexRandomMoveGoal`: drifts to a random air block near its bound origin.
struct VexRandomMoveGoal;

impl Goal for VexRandomMoveGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        !has_wanted_move(mob) && mob.get_random().random_range(0..to_goal_ticks(MOVE_CHANCE)) == 0
    }

    fn should_continue(&mut self, _mob: &dyn Mob) -> bool {
        false
    }

    fn tick(&mut self, mob: &dyn Mob) {
        let entity = mob.get_entity();
        let origin = as_vex(mob)
            .and_then(VexEntity::get_bound_origin)
            .unwrap_or_else(|| entity.block_pos.load());
        let world = entity.world.load();
        let mut rng = mob.get_random();
        for _ in 0..RANDOM_MOVE_ATTEMPTS {
            let pos = origin.add(
                rng.random_range(0..15) - 7,
                rng.random_range(0..11) - 5,
                rng.random_range(0..15) - 7,
            );
            if !world.get_block_state(&pos).is_air() {
                continue;
            }
            let (x, y, z) = (
                f64::from(pos.0.x) + 0.5,
                f64::from(pos.0.y) + 0.5,
                f64::from(pos.0.z) + 0.5,
            );
            set_wanted_move(mob, x, y, z, RANDOM_MOVE_SPEED);
            let mob_entity = mob.get_mob_entity();
            if mob_entity.get_target().is_none() {
                mob_entity
                    .look_control
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .look_at_with_range(x, y, z, RANDOM_MOVE_LOOK_YAW, RANDOM_MOVE_LOOK_PITCH);
            }
            break;
        }
    }

    fn controls(&self) -> Controls {
        Controls::MOVE
    }
}

/// Vanilla `Vex.VexCopyOwnerTargetGoal`: takes over its owner's target. Vanilla gives it no flags,
/// so it neither blocks nor is blocked by the other target goals.
struct VexCopyOwnerTargetGoal {
    track_target_goal: TrackTargetGoal,
    target_predicate: TargetPredicate,
}

impl VexCopyOwnerTargetGoal {
    fn new() -> Self {
        Self {
            track_target_goal: TrackTargetGoal::with_default(false),
            target_predicate: TargetPredicate::create_non_attackable()
                .ignore_visibility()
                .ignore_distance_scaling_factor(),
        }
    }

    /// Vanilla `getOwner() instanceof Targeting owner ? owner.getTarget() : null`.
    fn owner_target(mob: &dyn Mob) -> Option<Arc<dyn EntityBase>> {
        let owner = as_vex(mob)?.get_owner()?;
        owner.get_mob()?.get_mob_entity().get_target()
    }
}

impl Goal for VexCopyOwnerTargetGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        let target = Self::owner_target(mob);
        self.track_target_goal
            .can_track(mob, target.as_deref(), &self.target_predicate)
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        self.track_target_goal.should_continue(mob)
    }

    fn start(&mut self, mob: &dyn Mob) {
        self.track_target_goal.start_targeting(
            mob,
            Self::owner_target(mob),
            TargetReason::OwnerAttackedTarget,
        );
        self.track_target_goal.start(mob);
    }

    fn stop(&mut self, mob: &dyn Mob) {
        self.track_target_goal.stop(mob);
    }

    fn controls(&self) -> Controls {
        Controls::empty()
    }
}
