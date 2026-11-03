//! Vanilla `SulfurCube`: a friendly cube that swallows a block and then plays like a ball.

use std::sync::atomic::{AtomicBool, AtomicI32, Ordering::Relaxed};
use std::sync::{Arc, Mutex, PoisonError};

use pumpkin_data::BlockState;
use pumpkin_data::damage::DamageType;
use pumpkin_data::data_component_impl::{
    BucketEntityDataImpl, EquipmentSlot, SulfurCubeContentImpl,
};
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::tracked_data;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::Hand;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use rand::RngExt;

use crate::entity::ageable::{AgeableData, AgeableMob};
use crate::entity::ai::brain::BrainState;
use crate::entity::bucketable::Bucketable;
use crate::entity::custom_sound::CustomSound;
use crate::entity::mob::cube_mob::{self, CubeMob, CubeMobState};
use crate::entity::mob::spawn::SpawnGroupData;
use crate::entity::mob::{Mob, MobEntity, MobMetadata, requires_custom_persistence_base};
use crate::entity::player::Player;
use crate::entity::{Entity, EntityBase};
use crate::world::World;

use archetype::BodyEffects;
pub use body::is_swallowable_item;

pub mod archetype;
mod body;
mod dispense;
mod fuse;
mod goals;
mod interact;
mod knockback;

/// Vanilla `SulfurCube.MAX_SIZE`.
const MAX_SIZE: i32 = 2;
/// Vanilla `SulfurCube.MIN_SIZE`.
const MIN_SIZE: i32 = 1;
/// Vanilla `SulfurCube.SPLIT_COUNT`.
const SPLIT_COUNT: i32 = 2;
/// Vanilla `SulfurCube.PICKUP_TIMER_DURATION`.
const PICKUP_TIMER_DURATION: i32 = 100;
/// Vanilla `SulfurCube.getFluidJumpThreshold`: this part of its height.
pub const FLUID_JUMP_THRESHOLD_SCALE: f64 = 0.2;
/// Vanilla `SulfurCube.setCubeMobHealth`: health per size.
const HEALTH_PER_SIZE: i32 = 4;

/// Vanilla `SulfurCube`.
///
/// Without a block it hops around, follows swallowable blocks and takes one from the ground or
/// the hand. With a block in the body slot it stops moving by itself and only moves when pushed,
/// hit or blown up, by the rules of the block's archetype.
///
/// Wiki: <https://minecraft.wiki/w/Sulfur_Cube>
pub struct SulfurCubeEntity {
    mob_entity: MobEntity,
    cube: Arc<CubeMobState>,
    ageable: AgeableData,
    pickup_timer: AtomicI32,
    push_sound_cooldown: AtomicI32,
    fuse: AtomicI32,
    from_bucket: AtomicBool,
    effects: Mutex<BodyEffects>,
    /// The body item `collectEquipmentChanges` last saw.
    last_body: Mutex<ItemStack>,
    /// Whether the goals of `registerGoals` are in, which only a cube without a block has.
    goals_registered: AtomicBool,
}

impl SulfurCubeEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let cube = Arc::new(Self {
            mob_entity: MobEntity::new(entity),
            cube: Arc::new(CubeMobState::default()),
            ageable: AgeableData::default(),
            pickup_timer: AtomicI32::new(0),
            push_sound_cooldown: AtomicI32::new(0),
            fuse: AtomicI32::new(-1),
            from_bucket: AtomicBool::new(false),
            effects: Mutex::new(BodyEffects::default()),
            last_body: Mutex::new(ItemStack::EMPTY.clone()),
            goals_registered: AtomicBool::new(false),
        });
        cube_mob::install_move_control(&cube.mob_entity, cube.cube.clone());
        *cube
            .mob_entity
            .look_control
            .lock()
            .unwrap_or_else(PoisonError::into_inner) =
            Box::new(goals::SulfurCubeLookControl::default());
        cube.register_goals();
        cube
    }

    /// Vanilla `SulfurCube.setSize`.
    pub fn set_size(&self, size: i32, update_health: bool) {
        self.set_cube_size(size, update_health);
        if update_health && size == MIN_SIZE && !self.is_baby() {
            self.set_baby(true);
        }
    }

    #[must_use]
    pub fn get_size(&self) -> i32 {
        self.cube.get_size()
    }

    fn effects(&self) -> std::sync::MutexGuard<'_, BodyEffects> {
        self.effects.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Vanilla `SulfurCube.registerGoals`.
    fn register_goals(&self) {
        let selector = &self.mob_entity.goals_selector;
        cube_mob::add_common_goals(selector);
        selector.add_goal(2, Box::new(goals::tempt_goal()));
        selector.add_goal(3, Box::new(goals::SulfurCubeSearchForItemsGoal::default()));
        self.goals_registered.store(true, Relaxed);
    }

    /// Vanilla `AbstractCubeMob.remove` with `SulfurCube.getSplitCount` and `setUpSplitCube`.
    fn split(&self) {
        let count = if self.is_primed() { 0 } else { SPLIT_COUNT };
        cube_mob::spawn_split_cubes(self, count, Self::new, |child, half_size| {
            child.set_size(half_size, true);
            child.set_baby(true);
        });
    }
}

impl CustomSound for SulfurCubeEntity {
    fn death_sound(&self) -> Option<Sound> {
        Some(if self.is_tiny() {
            Sound::EntitySmallSulfurCubeDeath
        } else {
            Sound::EntitySulfurCubeDeath
        })
    }

    fn hurt_sound(&self) -> Option<Sound> {
        Some(if self.is_tiny() {
            Sound::EntitySmallSulfurCubeHurt
        } else {
            Sound::EntitySulfurCubeHurt
        })
    }

    fn sound_volume(&self) -> f32 {
        self.cube_sound_volume()
    }
}

impl CubeMob for SulfurCubeEntity {
    fn cube_state(&self) -> &CubeMobState {
        &self.cube
    }

    fn jump_sound(&self) -> Sound {
        if self.is_tiny() {
            Sound::EntitySmallSulfurCubeJump
        } else {
            Sound::EntitySulfurCubeJump
        }
    }

    fn squish_sound(&self) -> Sound {
        if self.is_tiny() {
            Sound::EntitySmallSulfurCubeSquish
        } else if self.has_body_item() {
            Sound::EntitySulfurCubeBounce
        } else {
            Sound::EntitySulfurCubeSquish
        }
    }

    fn can_deal_damage(&self) -> bool {
        false
    }

    fn cube_max_health(&self, actual_size: i32) -> f64 {
        f64::from(HEALTH_PER_SIZE * actual_size)
    }

    fn hops(&self) -> bool {
        !self.has_body_item()
    }
}

impl AgeableMob for SulfurCubeEntity {
    fn get_ageable_data(&self) -> &AgeableData {
        &self.ageable
    }

    /// Vanilla `SulfurCube.ageBoundaryReached`: a grown cube has its full size and health.
    fn age_boundary_reached(&self) {
        if !self.is_baby() {
            self.set_size(MAX_SIZE, true);
        }
    }
}

impl Bucketable for SulfurCubeEntity {
    fn is_from_bucket(&self) -> bool {
        self.from_bucket.load(Relaxed)
    }

    fn set_from_bucket(&self, from_bucket: bool) {
        self.from_bucket.store(from_bucket, Relaxed);
        self.get_entity()
            .set_synced_data(tracked_data::sulfur_cube::FROM_BUCKET, from_bucket);
    }

    fn bucket_item(&self) -> &'static Item {
        &Item::SULFUR_CUBE_BUCKET
    }

    fn pickup_sound(&self) -> Sound {
        Sound::ItemBucketFillSulfurCube
    }

    fn can_be_picked_up_with_bucket(&self, stack: &ItemStack) -> bool {
        stack.item == &Item::BUCKET
    }

    /// Vanilla `SulfurCube.saveToBucketTag`, with its own age keys.
    fn save_to_bucket_tag(&self, bucket: &mut ItemStack) {
        self.save_default_data_to_bucket_tag(bucket);
        let body = self.body_item();
        if !body.is_empty() {
            bucket.set_data_component(SulfurCubeContentImpl { item: body });
        }
        let mut nbt = bucket
            .get_data_component::<BucketEntityDataImpl>()
            .and_then(|data| data.nbt.clone())
            .unwrap_or_default();
        nbt.put_int("age", self.get_age());
        nbt.put_bool("age_locked", self.is_age_locked());
        bucket.set_data_component(BucketEntityDataImpl { nbt: Some(nbt) });
    }

    /// Vanilla `SulfurCube.loadFromBucketTag`.
    fn load_from_bucket_tag(&self, nbt: &NbtCompound) {
        self.load_default_data_from_bucket_tag(nbt);
        self.set_age(nbt.get_int("age").unwrap_or(0));
        self.set_age_locked(nbt.get_bool("age_locked").unwrap_or(false));
    }
}

impl Mob for SulfurCubeEntity {
    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    fn as_custom_sound(&self) -> Option<&dyn CustomSound> {
        Some(self)
    }

    fn as_cube_mob(&self) -> Option<&dyn CubeMob> {
        Some(self)
    }

    fn as_ageable(&self) -> Option<&dyn AgeableMob> {
        Some(self)
    }

    fn as_bucketable(&self) -> Option<&dyn Bucketable> {
        Some(self)
    }

    fn mob_sound_source(&self) -> SoundCategory {
        SoundCategory::Neutral
    }

    fn mob_metadata(&self) -> Option<MobMetadata> {
        Some(MobMetadata::new().slime_size(self.get_size()))
    }

    fn mob_init_data_tracker(&self) {
        let entity = self.get_entity();
        if entity.age.load(Relaxed) < 0 {
            entity.set_synced_data(tracked_data::ageable_mob::DATA_BABY_ID, true);
        }
        entity.set_synced_data(
            tracked_data::sulfur_cube::FROM_BUCKET,
            self.is_from_bucket(),
        );
        entity.set_synced_data(tracked_data::sulfur_cube::MAX_FUSE, self.get_fuse());
    }

    /// Vanilla `SulfurCube.addAdditionalSaveData` and `AbstractCubeMob.addAdditionalSaveData`.
    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        nbt.put_int("Size", self.get_size() - 1);
        nbt.put_bool("wasOnGround", self.cube.was_on_ground());
        nbt.put_int("pickup_timer", self.pickup_timer.load(Relaxed));
        nbt.put_bool("from_bucket", self.is_from_bucket());
        nbt.put_int("fuse", self.get_fuse());
    }

    /// Vanilla `SulfurCube.readAdditionalSaveData` and `AbstractCubeMob.readAdditionalSaveData`.
    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        self.pickup_timer
            .store(nbt.get_int("pickup_timer").unwrap_or(0), Relaxed);
        self.set_from_bucket(nbt.get_bool("from_bucket").unwrap_or(false));
        let fuse = nbt.get_int("fuse").unwrap_or(-1);
        self.fuse.store(fuse, Relaxed);
        self.get_entity()
            .set_synced_data(tracked_data::sulfur_cube::MAX_FUSE, fuse);
        self.set_size(nbt.get_int("Size").unwrap_or(0) + 1, false);
        // Vanilla reads the size before the living data, so without a saved Health the cube
        // gets the max health of its size, not of the default one.
        if nbt.get_numeric_float("Health").is_none() {
            let living = &self.mob_entity.living_entity;
            living.set_health(living.get_max_health());
        }
        self.cube
            .set_was_on_ground(nbt.get_bool("wasOnGround").unwrap_or(false));
    }

    /// Vanilla `SulfurCube.getBaseExperienceReward`.
    fn get_base_experience_reward(&self) -> u32 {
        if self.is_baby() {
            0
        } else {
            1 + self.get_random().random_range(0..2)
        }
    }

    /// Vanilla `AbstractCubeMob.finalizeSpawn` with `SulfurCube.setSpawnSize`.
    fn finalize_spawn(
        &self,
        _world: &Arc<World>,
        group_data: Option<SpawnGroupData>,
    ) -> Option<SpawnGroupData> {
        let group_data = self.finalize_ageable_spawn(group_data);
        self.set_size(if self.is_baby() { MIN_SIZE } else { MAX_SIZE }, true);
        group_data
    }

    /// Vanilla `SulfurCube.getBreedOffspring`, with the `setBaby` of its callers.
    fn spawn_as_baby(&self) -> bool {
        self.set_size(MIN_SIZE, true);
        self.set_baby(true);
        true
    }

    /// Vanilla `SulfurCube.tick` and `AbstractCubeMob.tick` before `super.tick()`, then the
    /// equipment check of `LivingEntity.tick`.
    fn mob_tick(&self, _caller: &dyn EntityBase) {
        self.tick_fuse();
        if self.get_entity().is_removed() {
            return;
        }
        self.prime_when_on_powered_position();
        self.cube.tick_squish();
        self.collect_equipment_changes();
        self.sync_goals();
        self.sync_can_pick_up_loot();
    }

    /// Vanilla `AbstractCubeMob.tick` after `super.tick()`: the landing squish.
    fn post_tick(&self) {
        cube_mob::land_squish(self);
    }

    /// Vanilla `SulfurCube.customServerAiStep`.
    fn custom_server_ai_step(&self, _caller: &dyn EntityBase) {
        for timer in [&self.pickup_timer, &self.push_sound_cooldown] {
            if timer.load(Relaxed) > 0 {
                timer.fetch_sub(1, Relaxed);
            }
        }
    }

    /// The part of vanilla `Mob.aiStep` that picks up items.
    fn mob_ai_step(&self) {
        self.pick_up_items();
    }

    /// Vanilla `AbstractCubeMob.jumpFromGround`.
    fn on_jump_from_ground(&self) {
        cube_mob::jump_straight_up(self, 0.0);
    }

    /// Vanilla `AbstractCubeMob.remove`.
    fn on_removed(&self) {
        if cube_mob::should_split(self) {
            self.split();
        }
    }

    /// Vanilla `SulfurCube.requiresCustomPersistence`.
    fn requires_custom_persistence(&self) -> bool {
        requires_custom_persistence_base(self) || self.has_body_item() || self.is_from_bucket()
    }

    /// Vanilla `SulfurCube.canBeLeashed`.
    fn can_be_leashed(&self) -> bool {
        self.has_body_item()
    }

    /// Vanilla `SulfurCube.omnidirectionalAirMover`.
    fn omnidirectional_air_mover(&self) -> bool {
        self.has_body_item()
    }

    /// Vanilla `SulfurCube.maxUpStep`.
    fn mob_max_up_step(&self) -> Option<f32> {
        self.has_body_item().then_some(0.0)
    }

    /// Vanilla `SulfurCube.canFreeze`.
    fn mob_can_freeze(&self) -> bool {
        !self.has_body_item()
    }

    /// Vanilla `SulfurCube.canBreatheUnderwater`.
    fn mob_can_breathe_underwater(&self) -> bool {
        self.has_body_item()
    }

    /// Vanilla `SulfurCube.playStepSound`.
    fn mob_play_step_sound(&self, _pos: &BlockPos, state: &BlockState) {
        if !self.has_body_item() {
            self.play_block_step_sound(state);
        }
    }

    /// Vanilla `AbstractCubeMob.getMaxHeadXRot`.
    fn get_max_look_pitch_change(&self) -> f32 {
        0.0
    }

    /// Vanilla `SulfurCube.canPickUpLoot`.
    fn can_pick_up_loot(&self, _brain: Option<&BrainState>) -> bool {
        !self.has_body_item()
    }

    /// Vanilla `Mob.wantsToPickUp` with `SulfurCube.canHoldItem`.
    fn wants_to_pick_up(&self, stack: &ItemStack) -> bool {
        self.can_hold_item(stack)
    }

    /// Vanilla `SulfurCube.canDispenserEquipIntoSlot`.
    fn can_dispenser_equip_into_slot(&self, slot: &EquipmentSlot) -> bool {
        matches!(slot, EquipmentSlot::Body(_))
    }

    /// Vanilla `SulfurCube.travelInFluid` after `super.travelInFluid`.
    fn after_travel_in_fluid(&self) {
        self.float_in_fluid();
    }

    /// Vanilla `SulfurCube.hurtServer`.
    fn mob_intercept_hurt(
        &self,
        amount: f32,
        damage_type: DamageType,
        position: Option<Vector3<f64>>,
        source: Option<&dyn EntityBase>,
        cause: Option<&dyn EntityBase>,
    ) -> Option<bool> {
        self.intercept_hurt(amount, damage_type, position, source, cause)
    }

    /// Vanilla `SulfurCube.knockback`.
    fn mob_knockback(
        &self,
        power: f64,
        xd: f64,
        zd: f64,
        attacker: Option<&Entity>,
        damage: f32,
        comes_from_effect: bool,
    ) -> bool {
        self.body_knockback(power, xd, zd, attacker, damage, comes_from_effect)
    }

    /// Vanilla `SulfurCube.doPush`.
    fn mob_do_push(&self, entity: &Arc<dyn EntityBase>) {
        self.apply_contact_damage(entity.as_ref());
    }

    /// Vanilla `SulfurCube.playerTouch`.
    fn mob_player_collision(&self, player: &Arc<Player>) {
        self.player_push(player);
    }

    /// Vanilla `Mob.interact` with `SulfurCube.mobInteract`.
    fn mob_interact_with_hand(
        &self,
        player: &Arc<Player>,
        item_stack: &mut ItemStack,
        _hand: Hand,
    ) -> Option<bool> {
        Some(self.interact_with_item(player, item_stack))
    }
}
