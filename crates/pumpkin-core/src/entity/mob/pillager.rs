use std::sync::{
    Arc, Mutex, Weak,
    atomic::{AtomicBool, Ordering},
};

use pumpkin_data::data_component_impl::EquipmentSlot;
use pumpkin_data::enchantment_provider::EnchantmentProvider;
use pumpkin_data::entity::EntityType;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::Sound;
use pumpkin_data::tag;
use pumpkin_data::tracked_data;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_nbt::tag::NbtTag;
use pumpkin_util::Difficulty;
use rand::RngExt;

use crate::entity::ai::behavior::fleeing::{self, Flee};
use crate::entity::ai::behavior::hunting::{self, Hunt};
use crate::entity::ai::target_match::TargetMatch;
use crate::entity::mob::equipment::RegionalDifficulty;
use crate::entity::mob::spawn::SpawnGroupData;
use crate::entity::{
    Entity, EntityBase,
    ai::goal::{
        look_at_entity::LookAtEntityGoal, ranged_crossbow_attack::RangedCrossbowAttackGoal,
        swim::SwimGoal, wander_around::WanderAroundGoal,
    },
    mob::{
        Mob, MobEntity,
        crossbow_attack_mob::CrossbowAttackMob,
        patrol::{LongDistancePatrolGoal, PatrolData, PatrollingMonster},
        raider::{
            self, HoldGroundAttackGoal, ObtainRaidLeaderBannerGoal, PathfindToRaidGoal, Raider,
            RaiderCelebrationGoal, RaiderData, RaiderMoveThroughVillageGoal,
        },
    },
};
use crate::world::World;
use crate::world::raid::Raid;

const TAG_INVENTORY: &str = "Inventory";
/// One in this many spawned pillagers gets the spawn crossbow enchantment on top.
const SPAWN_CROSSBOW_ENCHANT_CHANCE: i32 = 300;

const PILLAGER_FLEES: &[Flee] = &[raider::flee_creaking(1, 1.0, 1.2)];

const PILLAGER_HUNTS: &[Hunt] = &[
    Hunt::new(2, TargetMatch::Type(&EntityType::PLAYER), true),
    Hunt::new(3, TargetMatch::VILLAGER_LIKE, false),
    Hunt::new(3, TargetMatch::Type(&EntityType::IRON_GOLEM), true),
];

/// Represents a Pillager, a crossbow-wielding illager of outposts, patrols and raids.
///
/// Wiki: <https://minecraft.wiki/w/Pillager>
pub struct PillagerEntity {
    pub mob_entity: MobEntity,
    pub raider_data: RaiderData,
    pub is_charging_crossbow: AtomicBool,
    pub inventory: Mutex<Vec<ItemStack>>,
}

impl PillagerEntity {
    pub const INVENTORY_SIZE: usize = 5;

    #[must_use]
    pub fn new(entity: Entity) -> Arc<Self> {
        let mob_arc = Arc::new(Self {
            mob_entity: MobEntity::new(entity),
            raider_data: RaiderData::default(),
            is_charging_crossbow: AtomicBool::new(false),
            inventory: Mutex::new(Vec::new()),
        });

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
            fleeing::apply(goal_selector, PILLAGER_FLEES);
            goal_selector.add_goal(2, Box::new(HoldGroundAttackGoal::new(10.0)));
            goal_selector.add_goal(3, Box::new(RangedCrossbowAttackGoal::new(1.0, 8.0)));
            goal_selector.add_goal(8, Box::new(WanderAroundGoal::new(0.6)));
            goal_selector.add_goal(
                9,
                Box::new(LookAtEntityGoal::new(
                    mob_weak.clone(),
                    &EntityType::PLAYER,
                    15.0,
                    1.0,
                    false,
                )),
            );
            goal_selector.add_goal(
                10,
                Box::new(LookAtEntityGoal::matching(
                    mob_weak,
                    TargetMatch::Mob,
                    15.0,
                    0.02,
                    false,
                )),
            );

            target_selector.add_goal(1, raider::revenge_goal());
            hunting::apply(target_selector, &mob_arc.mob_entity, PILLAGER_HUNTS);
        };

        mob_arc
    }

    #[must_use]
    pub fn is_charging_crossbow(&self) -> bool {
        self.is_charging_crossbow.load(Ordering::Relaxed)
    }

    pub fn set_charging_crossbow(&self, is_charging: bool) {
        self.is_charging_crossbow
            .store(is_charging, Ordering::Relaxed);
        self.mob_entity
            .living_entity
            .entity
            .set_synced_data(tracked_data::pillager::IS_CHARGING_CROSSBOW, is_charging);
    }

    pub fn add_to_inventory(&self, item: ItemStack) -> Option<ItemStack> {
        let mut inv = self
            .inventory
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if inv.len() < Self::INVENTORY_SIZE {
            inv.push(item);
            None
        } else {
            Some(item)
        }
    }
}

impl Mob for PillagerEntity {
    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    fn as_patrolling_monster(&self) -> Option<&dyn PatrollingMonster> {
        Some(self)
    }

    fn as_raider(&self) -> Option<&dyn Raider> {
        Some(self)
    }

    fn as_crossbow_attack_mob(&self) -> Option<&dyn CrossbowAttackMob> {
        Some(self)
    }

    fn get_ambient_sound(&self) -> Option<Sound> {
        Some(Sound::EntityPillagerAmbient)
    }

    fn get_preferred_weapon_type(&self) -> Option<&'static tag::Tag> {
        Some(&tag::Item::MINECRAFT_PILLAGER_PREFERRED_WEAPONS)
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

    fn mob_init_data_tracker(&self) {
        self.set_charging_crossbow(self.is_charging_crossbow.load(Ordering::Relaxed));
    }

    fn finalize_spawn(
        &self,
        world: &Arc<World>,
        group_data: Option<SpawnGroupData>,
    ) -> Option<SpawnGroupData> {
        let difficulty = RegionalDifficulty::at(world, self.get_entity().pos.load());
        self.populate_default_equipment_slots(world, &difficulty);
        self.populate_default_equipment_enchantments(&difficulty);
        // Vanilla `Raider.finalizeSpawn`: only a naturally spawned witch stays out of raids.
        self.set_can_join_raid(true);
        self.finalize_spawn_super(group_data)
    }

    fn populate_default_equipment_slots(
        &self,
        _world: &Arc<World>,
        _difficulty: &RegionalDifficulty,
    ) {
        self.mob_entity.set_item_slot(
            &EquipmentSlot::MAIN_HAND,
            ItemStack::new(1, &Item::CROSSBOW),
        );
    }

    fn enchant_spawned_weapon(&self, difficulty: &RegionalDifficulty) {
        self.enchant_spawned_equipment(
            &EquipmentSlot::MAIN_HAND,
            MobEntity::MAX_ENCHANTED_WEAPON_CHANCE,
            difficulty,
        );
        if self
            .get_random()
            .random_range(0..SPAWN_CROSSBOW_ENCHANT_CHANCE)
            == 0
        {
            let mut equipment = self
                .mob_entity
                .living_entity
                .entity_equipment
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(weapon) = equipment.equipment.get_mut(&EquipmentSlot::MAIN_HAND)
                && weapon.item == &Item::CROSSBOW
            {
                EnchantmentProvider::PILLAGER_SPAWN_CROSSBOW
                    .apply_to_stack(weapon, difficulty.special_multiplier);
            }
        }
    }

    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        self.write_raider_nbt(nbt);

        let inv = self
            .inventory
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !inv.is_empty() {
            let mut items_tag = Vec::new();
            for item in inv.iter() {
                if !item.is_empty() {
                    let mut item_nbt = NbtCompound::new();
                    item.write_item_stack(&mut item_nbt);
                    items_tag.push(NbtTag::Compound(item_nbt));
                }
            }
            if !items_tag.is_empty() {
                nbt.put_list(TAG_INVENTORY, items_tag);
            }
        }
    }

    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        self.read_raider_nbt(nbt);

        if let Some(inv_list) = nbt.get_list(TAG_INVENTORY) {
            let mut inv = self
                .inventory
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            inv.clear();
            for tag in inv_list {
                if inv.len() >= Self::INVENTORY_SIZE {
                    break;
                }
                if let Some(compound) = tag.extract_compound()
                    && let Some(stack) = ItemStack::read_item_stack(compound)
                {
                    inv.push(stack);
                }
            }
        }
        // Vanilla `Pillager.readAdditionalSaveData`: a loaded pillager always picks up loot.
        self.mob_entity.set_can_pick_up_loot(true);
    }
}

impl PatrollingMonster for PillagerEntity {
    fn get_patrol_data(&self) -> &PatrolData {
        &self.raider_data.patrol_data
    }
}

impl Raider for PillagerEntity {
    fn get_raider_data(&self) -> &RaiderData {
        &self.raider_data
    }

    fn get_celebrate_sound(&self) -> Sound {
        Sound::EntityPillagerCelebrate
    }

    fn apply_raid_buffs(&self, raid: &Raid, wave: i32, _is_captain: bool) {
        if self.get_random().random::<f32>() > raid.get_enchant_odds() {
            return;
        }
        let provider = if wave > Raid::get_num_groups(Difficulty::Normal) {
            &EnchantmentProvider::RAID_PILLAGER_POST_WAVE_5
        } else if wave > Raid::get_num_groups(Difficulty::Easy) {
            &EnchantmentProvider::RAID_PILLAGER_POST_WAVE_3
        } else {
            return;
        };
        let mut crossbow = ItemStack::new(1, &Item::CROSSBOW);
        let world = self.get_entity().world.load_full();
        let difficulty = RegionalDifficulty::at(&world, self.get_entity().pos.load());
        provider.apply_to_stack(&mut crossbow, difficulty.special_multiplier);
        self.mob_entity
            .set_item_slot(&EquipmentSlot::MAIN_HAND, crossbow);
    }
}

impl CrossbowAttackMob for PillagerEntity {
    fn set_charging_crossbow(&self, is_charging: bool) {
        self.set_charging_crossbow(is_charging);
    }

    fn is_charging_crossbow(&self) -> bool {
        self.is_charging_crossbow()
    }

    fn on_crossbow_attack_performed(&self) {
        self.mob_entity
            .living_entity
            .no_action_time
            .store(0, Ordering::Relaxed);
    }
}
