use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};

use crossbeam::atomic::AtomicCell;
use pumpkin_data::attributes::Attributes;
use pumpkin_data::data_component_impl::EquipmentSlot;
use pumpkin_data::entity::EntityType;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::Sound;
use pumpkin_data::tag::{self, Taggable};
use pumpkin_data::tracked_data;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_util::text::TextComponent;
use rand::RngExt;

use crate::entity::EntityBase;
use crate::entity::ageable::AgeableMob;
use crate::entity::ai::behavior::fleeing::Flee;
use crate::entity::ai::goal::open_door::OpenDoorGoal;
use crate::entity::ai::goal::revenge::RevengeGoal;
use crate::entity::ai::goal::{Controls, Goal};
use crate::entity::ai::pathfinder::NavigatorGoal;
use crate::entity::ai::pathfinder::path::Path;
use crate::entity::ai::target_match::TargetMatch;
use crate::entity::item::ItemEntity;
use crate::entity::living::LivingEntity;
use crate::entity::mob::Mob;
use crate::entity::mob::patrol::{PatrolData, PatrollingMonster};
use crate::plugin::api::events::entity::entity_target::TargetReason;
use crate::world::raid::Raid;

/// Vanilla `HoldGroundAttackGoal.shoutTargeting` range.
const SHOUT_RANGE: f64 = 8.0;
/// One in this many ticks a raider holding its ground plays its ambient sound.
const HOLD_GROUND_AMBIENT_SOUND_CHANCE: i32 = 50;
/// Game ticks a banner the raider found no path to is skipped.
const UNREACHABLE_BANNER_COOLDOWN: i64 = 600;
const BANNER_SEARCH_VERTICAL_RANGE: f64 = 8.0;
const BANNER_PICKUP_DISTANCE: f64 = 1.414;
const BANNER_SPEED_MODIFIER: f64 = 1.15;
const CELEBRATE_SOUND_CHANCE: i32 = 100;
const CELEBRATE_JUMP_CHANCE: i32 = 50;

#[must_use]
pub fn is_raider(entity_type: &'static EntityType) -> bool {
    entity_type.has_tag(&tag::EntityType::MINECRAFT_RAIDERS)
}

/// Vanilla `HurtByTargetGoal(this, Raider.class).setAlertOthers()`.
#[must_use]
pub fn revenge_goal() -> Box<RevengeGoal> {
    Box::new(RevengeGoal::new(true).ignoring(is_raider).alerting_others())
}

/// Vanilla `AbstractIllager.canAttack`: illagers spare baby villagers.
#[must_use]
pub fn is_baby_villager(target: &dyn EntityBase) -> bool {
    TargetMatch::VILLAGER_LIKE.matches(target.get_entity().entity_type)
        && target
            .get_mob()
            .and_then(Mob::as_ageable)
            .is_some_and(AgeableMob::is_baby)
}

/// Illagers keep 8 blocks from a creaking.
#[must_use]
pub const fn flee_creaking(priority: u8, walk_speed: f64, sprint_speed: f64) -> Flee {
    Flee::new(
        priority,
        TargetMatch::Type(&EntityType::CREAKING),
        8.0,
        walk_speed,
        sprint_speed,
    )
}

#[must_use]
pub fn create_ominous_banner() -> ItemStack {
    use pumpkin_data::data_component::DataComponent;
    use pumpkin_data::data_component_impl::{CustomNameImpl, DataComponentImpl};
    let mut stack = ItemStack::new(1, &Item::WHITE_BANNER);
    stack.patch.push((
        DataComponent::CustomName,
        Some(
            CustomNameImpl {
                name: TextComponent::translate("block.minecraft.ominous_banner", []),
            }
            .to_dyn(),
        ),
    ));
    stack
}

#[must_use]
pub const fn is_ominous_banner(stack: &ItemStack) -> bool {
    stack.item.id == Item::WHITE_BANNER.id
}

/// Runs `f` on the raider's raid, vanilla `getCurrentRaid`. Takes the world's raid lock.
#[must_use]
pub fn with_current_raid<R>(raider: &dyn Raider, f: impl FnOnce(&Raid) -> R) -> Option<R> {
    let raid_id = raider.get_raider_data().raid_id.load()?;
    let world = raider.get_mob_entity().living_entity.entity.world.load();
    let raids = world
        .raids
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    raids.get(raid_id).map(f)
}

pub struct RaiderData {
    pub patrol_data: PatrolData,
    pub wave: AtomicI32,
    pub can_join_raid: AtomicBool,
    pub ticks_outside_raid: AtomicI32,
    pub is_celebrating: AtomicBool,
    pub raid_id: AtomicCell<Option<i32>>,
}

impl Default for RaiderData {
    fn default() -> Self {
        Self {
            patrol_data: PatrolData::default(),
            wave: AtomicI32::new(0),
            can_join_raid: AtomicBool::new(false),
            ticks_outside_raid: AtomicI32::new(0),
            is_celebrating: AtomicBool::new(false),
            raid_id: AtomicCell::new(None),
        }
    }
}

pub trait Raider: PatrollingMonster {
    fn get_raider_data(&self) -> &RaiderData;

    fn can_join_raid(&self) -> bool {
        self.get_raider_data().can_join_raid.load(Ordering::Relaxed)
    }

    fn set_can_join_raid(&self, can_join: bool) {
        self.get_raider_data()
            .can_join_raid
            .store(can_join, Ordering::Relaxed);
    }

    fn get_wave(&self) -> i32 {
        self.get_raider_data().wave.load(Ordering::Relaxed)
    }

    fn set_wave(&self, wave: i32) {
        self.get_raider_data().wave.store(wave, Ordering::Relaxed);
    }

    fn is_celebrating(&self) -> bool {
        self.get_raider_data()
            .is_celebrating
            .load(Ordering::Relaxed)
    }

    fn set_celebrating(&self, celebrating: bool) {
        self.get_raider_data()
            .is_celebrating
            .store(celebrating, Ordering::Relaxed);
        let entity = &self.get_mob_entity().living_entity.entity;
        entity.set_synced_data(tracked_data::pillager::IS_CELEBRATING, celebrating);
    }

    fn has_active_raid(&self) -> bool {
        self.get_raider_data().raid_id.load().is_some()
    }

    fn is_captain(&self) -> bool {
        self.is_patrol_leader()
    }

    fn get_celebrate_sound(&self) -> Sound;

    /// Vanilla `Raider.applyRaidBuffs`, run by the raid on each raider it spawns.
    fn apply_raid_buffs(&self, _raid: &Raid, _wave: i32, _is_captain: bool) {}

    /// Vanilla `Raider.removeWhenFarAway`: never while in a raid.
    fn raider_remove_when_far_away(&self, distance_sq: f64) -> bool {
        self.get_raider_data().raid_id.load().is_none()
            && self.patrol_remove_when_far_away(distance_sq)
    }

    /// Vanilla `Raider.requiresCustomPersistence`: a raider in a raid stays loaded.
    fn raider_requires_custom_persistence(&self) -> bool {
        self.get_raider_data().raid_id.load().is_some()
    }

    fn write_raider_nbt(&self, nbt: &mut NbtCompound) {
        self.write_patrol_nbt(nbt);
        let data = self.get_raider_data();
        nbt.put_int("Wave", data.wave.load(Ordering::Relaxed));
        nbt.put_bool("CanJoinRaid", data.can_join_raid.load(Ordering::Relaxed));
        if let Some(raid_id) = data.raid_id.load() {
            nbt.put_int("RaidId", raid_id);
        }
    }

    fn read_raider_nbt(&self, nbt: &NbtCompound) {
        self.read_patrol_nbt(nbt);
        let data = self.get_raider_data();
        if let Some(wave) = nbt.get_int("Wave") {
            data.wave.store(wave, Ordering::Relaxed);
        }
        if let Some(can_join) = nbt.get_bool("CanJoinRaid") {
            data.can_join_raid.store(can_join, Ordering::Relaxed);
        }
        if let Some(raid_id) = nbt.get_int("RaidId") {
            data.raid_id.store(Some(raid_id));
        }
    }
}

/// Vanilla `Raider.HoldGroundAttackGoal`: a patrolling raider outside a raid stops, calls the
/// raiders around it onto its target and waits until the target comes close.
pub struct HoldGroundAttackGoal {
    hostile_radius_sqr: f64,
}

impl HoldGroundAttackGoal {
    #[must_use]
    pub fn new(hostile_radius: f32) -> Self {
        Self {
            hostile_radius_sqr: f64::from(hostile_radius * hostile_radius),
        }
    }

    /// The raiders `shoutTargeting` finds: alive, within 8 blocks, sight and invisibility
    /// ignored.
    fn nearby_raiders(mob: &dyn Mob) -> Vec<Arc<dyn EntityBase>> {
        let entity = mob.get_entity();
        let pos = entity.pos.load();
        let area = entity
            .bounding_box
            .load()
            .expand(SHOUT_RANGE, SHOUT_RANGE, SHOUT_RANGE);
        entity
            .world
            .load()
            .get_entities_at_box(&area)
            .into_iter()
            .filter(|candidate| {
                let candidate_entity = candidate.get_entity();
                candidate_entity.entity_id != entity.entity_id
                    && candidate.get_mob().is_some_and(|m| m.as_raider().is_some())
                    && candidate
                        .get_living_entity()
                        .is_some_and(LivingEntity::is_alive)
                    && candidate_entity.pos.load().squared_distance_to_vec(&pos)
                        <= SHOUT_RANGE * SHOUT_RANGE
            })
            .collect()
    }
}

impl Goal for HoldGroundAttackGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        let Some(raider) = mob.as_raider() else {
            return false;
        };
        let mob_entity = mob.get_mob_entity();
        raider.get_raider_data().raid_id.load().is_none()
            && raider.is_patrolling()
            && mob_entity.get_target().is_some()
            && !mob_entity.is_attacking()
            && mob_entity
                .living_entity
                .get_last_hurt_by_mob()
                .is_none_or(|attacker| attacker.get_entity().entity_type != &EntityType::PLAYER)
    }

    fn start(&mut self, mob: &dyn Mob) {
        mob.get_mob_entity().steered().navigator().stop();
        let target = mob.get_mob_entity().get_target();
        for raider in Self::nearby_raiders(mob) {
            if let Some(raider) = raider.get_mob() {
                raider.set_mob_target(target.clone(), TargetReason::Unknown);
            }
        }
    }

    fn stop(&mut self, mob: &dyn Mob) {
        let Some(target) = mob.get_mob_entity().get_target() else {
            return;
        };
        for raider in Self::nearby_raiders(mob) {
            if let Some(raider) = raider.get_mob() {
                raider.set_mob_target(Some(target.clone()), TargetReason::Unknown);
                raider.get_mob_entity().set_attacking(true);
            }
        }
        mob.get_mob_entity().set_attacking(true);
    }

    fn should_run_every_tick(&self) -> bool {
        true
    }

    fn controls(&self) -> Controls {
        Controls::MOVE | Controls::LOOK
    }

    fn tick(&mut self, mob: &dyn Mob) {
        let Some(target) = mob.get_mob_entity().get_target() else {
            return;
        };
        let mob_pos = mob.get_entity().pos.load();
        let target_pos = target.get_entity().pos.load();
        if mob_pos.squared_distance_to_vec(&target_pos) > self.hostile_radius_sqr {
            mob.get_mob_entity()
                .look_control
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .look_at_entity_with_range(&target, 30.0, 30.0);
            if mob
                .get_random()
                .random_range(0..HOLD_GROUND_AMBIENT_SOUND_CHANCE)
                == 0
            {
                mob.play_ambient_sound();
            }
        } else {
            mob.get_mob_entity().set_attacking(true);
        }
    }
}

/// Vanilla `Raider.ObtainRaidLeaderBannerGoal`: a raider of a wave without a living leader
/// walks to a dropped ominous banner and takes the lead.
#[derive(Default)]
pub struct ObtainRaidLeaderBannerGoal {
    unreachable_banner_cache: HashMap<i32, i64>,
    path_to_banner: Option<Path>,
    pursued_banner: Option<Arc<dyn EntityBase>>,
}

impl ObtainRaidLeaderBannerGoal {
    /// Vanilla `cannotPickUpBanner`.
    fn cannot_pick_up_banner(raider: &dyn Raider) -> bool {
        if !raider.has_active_raid() {
            return true;
        }
        let wave = raider.get_wave();
        let Some((is_over, leader)) = with_current_raid(raider, |raid| {
            (raid.is_over(), raid.group_to_leader_map.get(&wave).copied())
        }) else {
            return true;
        };
        if is_over || !raider.can_be_leader() {
            return true;
        }
        let living = &raider.get_mob_entity().living_entity;
        let head = living
            .entity_equipment
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&EquipmentSlot::HEAD);
        if is_ominous_banner(&head) {
            return true;
        }
        leader.is_some_and(|uuid| {
            living
                .entity
                .world
                .load()
                .get_entity_by_uuid(uuid)
                .is_some_and(|leader| {
                    leader
                        .get_living_entity()
                        .is_some_and(LivingEntity::is_alive)
                })
        })
    }

    /// Vanilla `Raider.ALLOWED_ITEMS`.
    fn is_allowed_banner(candidate: &dyn EntityBase) -> bool {
        candidate.get_item_entity().is_some_and(|item| {
            !item.has_pickup_delay()
                && item.get_entity().is_alive()
                && is_ominous_banner(
                    &item
                        .get_item_stack()
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner),
                )
        })
    }
}

impl Goal for ObtainRaidLeaderBannerGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        let Some(raider) = mob.as_raider() else {
            return false;
        };
        if Self::cannot_pick_up_banner(raider) {
            return false;
        }
        let living = &mob.get_mob_entity().living_entity;
        let world = living.entity.world.load();
        let follow_range = living.get_attribute_value(&Attributes::FOLLOW_RANGE);
        let area = living.entity.bounding_box.load().expand(
            follow_range,
            BANNER_SEARCH_VERTICAL_RANGE,
            follow_range,
        );
        let game_time = world.get_world_age();
        let mut unreachable = HashMap::new();
        for banner in world.get_entities_at_box(&area) {
            if !Self::is_allowed_banner(banner.as_ref()) {
                continue;
            }
            let banner_id = banner.get_entity().entity_id;
            let unreachable_until = self
                .unreachable_banner_cache
                .get(&banner_id)
                .copied()
                .unwrap_or(i64::MIN);
            if game_time < unreachable_until {
                unreachable.insert(banner_id, unreachable_until);
                continue;
            }
            let steered = mob.get_mob_entity().steered();
            let owner = &steered.mob_entity().living_entity;
            let path = steered
                .navigator()
                .create_path(owner, banner.get_entity().pos.load(), 1);
            if let Some(path) = path.filter(Path::can_reach) {
                self.path_to_banner = Some(path);
                self.pursued_banner = Some(banner);
                return true;
            }
            unreachable.insert(banner_id, game_time + UNREACHABLE_BANNER_COOLDOWN);
        }
        self.unreachable_banner_cache = unreachable;
        false
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        let Some(raider) = mob.as_raider() else {
            return false;
        };
        // Vanilla checks the path it handed to the navigation, which the navigation advances.
        self.pursued_banner
            .as_ref()
            .is_some_and(|banner| !banner.get_entity().is_removed())
            && !mob.get_mob_entity().steered().navigator().is_done()
            && !Self::cannot_pick_up_banner(raider)
    }

    fn start(&mut self, mob: &dyn Mob) {
        let steered = mob.get_mob_entity().steered();
        let owner = &steered.mob_entity().living_entity;
        steered
            .navigator()
            .move_to_path(self.path_to_banner.take(), BANNER_SPEED_MODIFIER, owner);
    }

    fn stop(&mut self, _mob: &dyn Mob) {
        self.path_to_banner = None;
        self.pursued_banner = None;
    }

    fn controls(&self) -> Controls {
        Controls::MOVE
    }

    fn tick(&mut self, mob: &dyn Mob) {
        let Some(raider) = mob.as_raider() else {
            return;
        };
        let Some(banner) = &self.pursued_banner else {
            return;
        };
        let Some(item) = banner.get_item_entity() else {
            return;
        };
        let distance_sq = banner
            .get_entity()
            .pos
            .load()
            .squared_distance_to_vec(&mob.get_entity().pos.load());
        if distance_sq < BANNER_PICKUP_DISTANCE * BANNER_PICKUP_DISTANCE {
            pick_up_banner(raider, item);
        }
    }
}

/// The banner branch of vanilla `Raider.pickUpItem`: a raider of an active raid whose wave has
/// no leader wears the ominous banner and leads the wave.
pub fn pick_up_banner(raider: &dyn Raider, item: &ItemEntity) {
    let stack = item
        .get_item_stack()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    if !raider.has_active_raid() || !is_ominous_banner(&stack) {
        return;
    }
    let wave = raider.get_wave();
    if with_current_raid(raider, |raid| raid.group_to_leader_map.contains_key(&wave))
        .unwrap_or(false)
    {
        return;
    }
    let mob_entity = raider.get_mob_entity();
    let living = &mob_entity.living_entity;
    if !living.pickup(item.get_entity(), Some(&stack), u32::from(stack.item_count)) {
        return;
    }
    let current = living
        .entity_equipment
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(&EquipmentSlot::HEAD);
    let drop_chance = mob_entity.drop_chance(&EquipmentSlot::HEAD);
    if !current.is_empty() && (raider.get_random().random::<f32>() - 0.1).max(0.0) < drop_chance {
        living.entity.spawn_at_location(current);
    }
    mob_entity.set_item_slot(&EquipmentSlot::HEAD, stack);
    item.get_entity().remove();
    if let Some(raid_id) = raider.get_raider_data().raid_id.load() {
        let world = living.entity.world.load();
        let mut raids = world
            .raids
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(raid) = raids.get_mut(raid_id) {
            raid.group_to_leader_map
                .insert(wave, living.entity.entity_uuid);
        }
    }
    raider.set_patrol_leader(true);
}

/// Vanilla `Raider.RaiderCelebration`: raiders of a raid the village lost cheer and jump.
pub struct RaiderCelebrationGoal;

impl RaiderCelebrationGoal {
    fn raid_is_lost(mob: &dyn Mob) -> bool {
        let Some(raider) = mob.as_raider() else {
            return false;
        };
        mob.get_mob_entity().living_entity.is_alive()
            && mob.get_mob_entity().get_target().is_none()
            && with_current_raid(raider, Raid::is_loss).unwrap_or(false)
    }
}

impl Goal for RaiderCelebrationGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        Self::raid_is_lost(mob)
    }

    fn start(&mut self, mob: &dyn Mob) {
        if let Some(raider) = mob.as_raider() {
            raider.set_celebrating(true);
        }
    }

    fn stop(&mut self, mob: &dyn Mob) {
        if let Some(raider) = mob.as_raider() {
            raider.set_celebrating(false);
        }
    }

    fn controls(&self) -> Controls {
        Controls::MOVE
    }

    fn tick(&mut self, mob: &dyn Mob) {
        let Some(raider) = mob.as_raider() else {
            return;
        };
        let entity = mob.get_entity();
        if !entity.silent.load(Ordering::Relaxed)
            && mob
                .get_random()
                .random_range(0..self.get_tick_count(CELEBRATE_SOUND_CHANCE))
                == 0
        {
            mob.make_sound(raider.get_celebrate_sound());
        }
        if !entity.has_vehicle()
            && mob
                .get_random()
                .random_range(0..self.get_tick_count(CELEBRATE_JUMP_CHANCE))
                == 0
        {
            mob.get_mob_entity()
                .jump_control
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .jump();
        }
    }
}

/// Vanilla `AbstractIllager.RaiderOpenDoorGoal`: an illager opens doors only during a raid.
#[derive(Default)]
pub struct RaiderOpenDoorGoal(OpenDoorGoal);

impl Goal for RaiderOpenDoorGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        self.0.can_start(mob) && mob.as_raider().is_some_and(Raider::has_active_raid)
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

    fn tick(&mut self, mob: &dyn Mob) {
        self.0.tick(mob);
    }

    fn should_run_every_tick(&self) -> bool {
        self.0.should_run_every_tick()
    }

    fn controls(&self) -> Controls {
        self.0.controls()
    }
}

/// Goal for moving through village homes during active raids.
pub struct RaiderMoveThroughVillageGoal {
    speed_modifier: f64,
}

impl RaiderMoveThroughVillageGoal {
    #[must_use]
    pub const fn new(speed_modifier: f64) -> Self {
        Self { speed_modifier }
    }
}

impl Goal for RaiderMoveThroughVillageGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        let Some(raider) = mob.as_raider() else {
            return false;
        };
        if !raider.has_active_raid() {
            return false;
        }
        let target = mob.get_mob_entity().get_target().clone();
        target.is_none()
    }

    fn controls(&self) -> Controls {
        Controls::MOVE
    }

    fn tick(&mut self, mob: &dyn Mob) {
        let entity = mob.get_entity();
        let pos = entity.pos.load();
        let steered = mob.get_mob_entity().steered();
        let mut nav = steered.navigator();

        if nav.is_idle() {
            let dx: f64 = (rand::random::<f64>() - 0.5) * 32.0;
            let dz: f64 = (rand::random::<f64>() - 0.5) * 32.0;
            let dest = Vector3::new(pos.x + dx, pos.y, pos.z + dz);
            nav.set_progress(NavigatorGoal {
                current_progress: pos,
                destination: dest,
                speed: self.speed_modifier,
            });
        }
    }
}

pub use crate::entity::ai::goal::pathfind_to_raid::PathfindToRaidGoal;
