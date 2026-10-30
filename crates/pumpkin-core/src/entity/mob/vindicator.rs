use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};

use arc_swap::ArcSwap;
use pumpkin_data::data_component_impl::EquipmentSlot;
use pumpkin_data::enchantment_provider::EnchantmentProvider;
use pumpkin_data::entity::EntityType;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::Sound;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::Difficulty;
use pumpkin_util::text::TextComponent;
use rand::RngExt;

use crate::entity::ai::behavior::fleeing::{self, Flee};
use crate::entity::ai::behavior::hunting::{self, Hunt};
use crate::entity::ai::goal::{
    Controls, Goal, active_target::ActiveTargetGoal, break_door::BreakDoorGoal,
    look_at_entity::LookAtEntityGoal, melee_attack::MeleeAttackGoal, swim::SwimGoal, to_goal_ticks,
    wander_around::WanderAroundGoal,
};
use crate::entity::ai::target_match::TargetMatch;
use crate::entity::living::LivingEntity;
use crate::entity::mob::equipment::RegionalDifficulty;
use crate::entity::mob::spawn::SpawnGroupData;
use crate::entity::{
    Entity, EntityBase,
    mob::{
        Mob, MobEntity,
        patrol::{LongDistancePatrolGoal, PatrolData, PatrollingMonster},
        raider::{
            self, HoldGroundAttackGoal, ObtainRaidLeaderBannerGoal, PathfindToRaidGoal, Raider,
            RaiderCelebrationGoal, RaiderData, RaiderMoveThroughVillageGoal, RaiderOpenDoorGoal,
        },
    },
};
use crate::world::World;
use crate::world::raid::Raid;

const TAG_JOHNNY: &str = "Johnny";
const JOHNNY_NAME: &str = "Johnny";
/// Vanilla passes 6, so the 240 tick minimum of `BreakDoorGoal` applies.
const DOOR_BREAK_TIME: i32 = 6;
/// One in this many goal ticks a raiding vindicator tries to break a door.
const BREAK_DOOR_CHANCE: i32 = 10;

const VINDICATOR_FLEES: &[Flee] = &[raider::flee_creaking(1, 1.0, 1.2)];

const VINDICATOR_HUNTS: &[Hunt] = &[
    Hunt::new(2, TargetMatch::Type(&EntityType::PLAYER), true),
    Hunt::new(3, TargetMatch::VILLAGER_LIKE, true),
    Hunt::new(3, TargetMatch::Type(&EntityType::IRON_GOLEM), true),
];

/// Represents a Vindicator, an axe-wielding illager of mansions, patrols and raids.
///
/// Wiki: <https://minecraft.wiki/w/Vindicator>
pub struct VindicatorEntity {
    pub mob_entity: MobEntity,
    pub raider_data: RaiderData,
    is_johnny: AtomicBool,
    /// The custom name last checked for Johnny, so only a new name counts.
    seen_custom_name: ArcSwap<Option<TextComponent>>,
}

impl VindicatorEntity {
    #[must_use]
    pub fn new(entity: Entity) -> Arc<Self> {
        let mob_entity = MobEntity::new(entity);
        let vindicator = Self {
            mob_entity,
            raider_data: RaiderData::default(),
            is_johnny: AtomicBool::new(false),
            seen_custom_name: ArcSwap::new(Arc::new(None)),
        };
        let mob_arc = Arc::new(vindicator);
        let mob_weak: Weak<dyn Mob> = {
            let mob_arc: Arc<dyn Mob> = mob_arc.clone();
            Arc::downgrade(&mob_arc)
        };

        {
            let goal_selector = &mob_arc.mob_entity.goals_selector;
            let target_selector = &mob_arc.mob_entity.target_selector;

            // Vanilla registers PatrollingMonster's and Raider's goals first; the insertion
            // order breaks priority ties.
            goal_selector.add_goal(4, Box::new(LongDistancePatrolGoal::new(0.7, 0.595)));
            goal_selector.add_goal(1, Box::new(ObtainRaidLeaderBannerGoal::default()));
            goal_selector.add_goal(3, Box::new(PathfindToRaidGoal::default()));
            goal_selector.add_goal(4, Box::new(RaiderMoveThroughVillageGoal::new(1.05)));
            goal_selector.add_goal(5, Box::new(RaiderCelebrationGoal));
            goal_selector.add_goal(0, Box::new(SwimGoal::default()));
            fleeing::apply(goal_selector, VINDICATOR_FLEES);
            goal_selector.add_goal(2, Box::new(VindicatorBreakDoorGoal::new()));
            goal_selector.add_goal(3, Box::new(RaiderOpenDoorGoal::default()));
            goal_selector.add_goal(4, Box::new(HoldGroundAttackGoal::new(10.0)));
            goal_selector.add_goal(5, Box::new(MeleeAttackGoal::new(1.0, false)));

            target_selector.add_goal(1, raider::revenge_goal());
            hunting::apply(target_selector, &mob_arc.mob_entity, VINDICATOR_HUNTS);
            target_selector.add_goal(
                4,
                Box::new(VindicatorJohnnyAttackGoal::new(&mob_arc.mob_entity)),
            );

            goal_selector.add_goal(8, Box::new(WanderAroundGoal::new(0.6)));
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
        };

        mob_arc
    }

    #[must_use]
    pub fn is_johnny(&self) -> bool {
        self.is_johnny.load(Ordering::Relaxed)
    }

    /// Vanilla `Vindicator.setCustomName`: naming a vindicator Johnny makes it Johnny for good.
    /// Pumpkin has no name hook, so a new name is noticed on the next tick.
    fn notice_johnny_name(&self) {
        let name = self.get_entity().custom_name.load_full();
        if Arc::ptr_eq(&name, &self.seen_custom_name.load()) {
            return;
        }
        let named_johnny = (*name)
            .as_ref()
            .is_some_and(|name| name.clone().get_text() == JOHNNY_NAME);
        self.seen_custom_name.store(name);
        if named_johnny {
            self.is_johnny.store(true, Ordering::Relaxed);
        }
    }
}

/// Vanilla `VindicatorBreakDoorGoal`: during a raid on normal or hard, a vindicator breaks the
/// doors in its way.
struct VindicatorBreakDoorGoal(BreakDoorGoal);

impl VindicatorBreakDoorGoal {
    fn new() -> Self {
        Self(BreakDoorGoal::with_door_break_time(
            DOOR_BREAK_TIME,
            Arc::new(|difficulty: Difficulty| {
                matches!(difficulty, Difficulty::Normal | Difficulty::Hard)
            }),
        ))
    }
}

impl Goal for VindicatorBreakDoorGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        mob.as_raider().is_some_and(Raider::has_active_raid)
            && mob
                .get_random()
                .random_range(0..to_goal_ticks(BREAK_DOOR_CHANCE))
                == 0
            && self.0.can_start(mob)
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        mob.as_raider().is_some_and(Raider::has_active_raid) && self.0.should_continue(mob)
    }

    fn start(&mut self, mob: &dyn Mob) {
        self.0.start(mob);
        mob.get_mob_entity()
            .living_entity
            .no_action_time
            .store(0, Ordering::Relaxed);
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
        Controls::MOVE
    }
}

/// Vanilla `VindicatorJohnnyAttackGoal`: Johnny attacks every living thing it can reach.
struct VindicatorJohnnyAttackGoal(Box<ActiveTargetGoal>);

impl VindicatorJohnnyAttackGoal {
    fn new(mob: &MobEntity) -> Self {
        Self(Box::new(ActiveTargetGoal::new(
            mob,
            TargetMatch::Any,
            0,
            true,
            true,
            // Vanilla `LivingEntity.attackable`, false only for armor stands.
            Some(|target: &LivingEntity, _: &World| {
                target.entity.entity_type != &EntityType::ARMOR_STAND
            }),
        )))
    }
}

impl Goal for VindicatorJohnnyAttackGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        mob.cast_any()
            .downcast_ref::<VindicatorEntity>()
            .is_some_and(VindicatorEntity::is_johnny)
            && self.0.can_start(mob)
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        self.0.should_continue(mob)
    }

    fn start(&mut self, mob: &dyn Mob) {
        self.0.start(mob);
        mob.get_mob_entity()
            .living_entity
            .no_action_time
            .store(0, Ordering::Relaxed);
    }

    fn stop(&mut self, mob: &dyn Mob) {
        self.0.stop(mob);
    }

    fn controls(&self) -> Controls {
        self.0.controls()
    }
}

impl Mob for VindicatorEntity {
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
        Some(Sound::EntityVindicatorAmbient)
    }

    fn can_attack(&self, target: &dyn EntityBase) -> bool {
        target.get_entity().entity_type != &EntityType::GHAST
            && !raider::is_baby_villager(target)
            && self.mob_entity.living_entity.can_attack(target)
    }

    fn remove_when_far_away(&self, distance_sq: f64) -> bool {
        self.raider_remove_when_far_away(distance_sq)
    }

    fn requires_custom_persistence(&self) -> bool {
        self.raider_requires_custom_persistence()
    }

    fn mob_tick(&self, _caller: &dyn EntityBase) {
        self.notice_johnny_name();
    }

    /// Vanilla `Vindicator.customServerAiStep`: paths lead through doors only in a raided village.
    fn custom_server_ai_step(&self, _caller: &dyn EntityBase) {
        let entity = self.get_entity();
        let is_raided = entity
            .world
            .load()
            .raids
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get_raid_at(&entity.block_pos.load())
            .is_some();
        let steered = self.mob_entity.steered();
        let mut navigator = steered.navigator();
        if navigator.can_navigate_ground() {
            navigator.set_can_open_doors(is_raided);
        }
    }

    fn finalize_spawn(
        &self,
        world: &Arc<World>,
        group_data: Option<SpawnGroupData>,
    ) -> Option<SpawnGroupData> {
        // Vanilla `Raider.finalizeSpawn`: only a naturally spawned witch stays out of raids.
        self.set_can_join_raid(true);
        let group_data = self.finalize_spawn_super(group_data);
        self.mob_entity
            .steered()
            .navigator()
            .set_can_open_doors(true);
        let difficulty = RegionalDifficulty::at(world, self.get_entity().pos.load());
        self.populate_default_equipment_slots(world, &difficulty);
        self.populate_default_equipment_enchantments(&difficulty);
        group_data
    }

    fn populate_default_equipment_slots(
        &self,
        _world: &Arc<World>,
        _difficulty: &RegionalDifficulty,
    ) {
        if self.raider_data.raid_id.load().is_none() {
            self.mob_entity.set_item_slot(
                &EquipmentSlot::MAIN_HAND,
                ItemStack::new(1, &Item::IRON_AXE),
            );
        }
    }

    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        self.write_raider_nbt(nbt);
        if self.is_johnny() {
            nbt.put_bool(TAG_JOHNNY, true);
        }
    }

    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        self.read_raider_nbt(nbt);
        self.is_johnny
            .store(nbt.get_bool(TAG_JOHNNY).unwrap_or(false), Ordering::Relaxed);
        // The saved flag wins over the loaded name, like vanilla's load order.
        self.seen_custom_name
            .store(self.get_entity().custom_name.load_full());
    }
}

impl PatrollingMonster for VindicatorEntity {
    fn get_patrol_data(&self) -> &PatrolData {
        &self.raider_data.patrol_data
    }
}

impl Raider for VindicatorEntity {
    fn get_raider_data(&self) -> &RaiderData {
        &self.raider_data
    }

    fn get_celebrate_sound(&self) -> Sound {
        Sound::EntityVindicatorCelebrate
    }

    fn apply_raid_buffs(&self, raid: &Raid, wave: i32, _is_captain: bool) {
        let mut axe = ItemStack::new(1, &Item::IRON_AXE);
        if self.get_random().random::<f32>() <= raid.get_enchant_odds() {
            let provider = if wave > Raid::get_num_groups(Difficulty::Normal) {
                &EnchantmentProvider::RAID_VINDICATOR_POST_WAVE_5
            } else {
                &EnchantmentProvider::RAID_VINDICATOR
            };
            let world = self.get_entity().world.load_full();
            let difficulty = RegionalDifficulty::at(&world, self.get_entity().pos.load());
            provider.apply_to_stack(&mut axe, difficulty.special_multiplier);
        }
        self.mob_entity
            .set_item_slot(&EquipmentSlot::MAIN_HAND, axe);
    }
}
