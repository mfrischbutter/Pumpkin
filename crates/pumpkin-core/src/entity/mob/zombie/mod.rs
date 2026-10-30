use super::{Mob, MobEntity, MobMetadata};
use crate::entity::ai::goal::break_door::BreakDoorGoal;
use crate::entity::ai::goal::destroy_egg::DestroyEggGoal;
use crate::entity::ai::goal::look_around::RandomLookAroundGoal;
use crate::entity::ai::goal::revenge::RevengeGoal;
use crate::entity::ai::goal::wander_around::WanderAroundGoal;
use crate::entity::ai::goal::zombie_attack::ZombieAttackGoal;
use crate::entity::mob::equipment::RegionalDifficulty;
use crate::entity::{
    Entity,
    ai::behavior::hunting::{self, Hunt},
    ai::goal::look_at_entity::LookAtEntityGoal,
    ai::target_match::TargetMatch,
};
use crate::world::World;
use pumpkin_data::data_component_impl::EquipmentSlot;
use pumpkin_data::entity::EntityType;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::Difficulty;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};

pub mod drowned;
pub mod husk;
#[allow(clippy::module_inception)]
pub mod zombie;
pub mod zombie_villager;

const ZOMBIE_HUNTS: &[Hunt] = &[
    Hunt::new(2, TargetMatch::Type(&EntityType::PLAYER), true),
    Hunt::new(3, TargetMatch::VILLAGER_LIKE, false),
    Hunt::new(3, TargetMatch::Type(&EntityType::IRON_GOLEM), true),
    hunting::baby_turtles(5, true),
];

pub struct ZombieEntityBase {
    pub mob_entity: MobEntity,
    pub can_break_doors: AtomicBool,
    pub is_baby: AtomicBool,
}

impl ZombieEntityBase {
    pub fn new(entity: Entity) -> Arc<Self> {
        Self::with_can_break_doors(entity, false)
    }

    pub fn with_can_break_doors(entity: Entity, can_break_doors: bool) -> Arc<Self> {
        let zombie = Self::without_behaviour_goals(entity);
        zombie.add_behaviour_goals();
        zombie.set_can_break_doors(can_break_doors, &*zombie);
        zombie
    }

    /// Vanilla `HurtByTargetGoal(this).setAlertOthers(ZombifiedPiglin.class)`.
    #[must_use]
    pub fn revenge_goal() -> RevengeGoal {
        RevengeGoal::new(true)
            .alerting_others_except(|entity_type| entity_type == &EntityType::ZOMBIFIED_PIGLIN)
    }

    /// Vanilla `Zombie.registerGoals` without `addBehaviourGoals`, for subclasses that replace
    /// the behaviour goals, like the drowned.
    pub fn without_behaviour_goals(entity: Entity) -> Arc<Self> {
        let mob_entity = MobEntity::new(entity);
        let zombie = Self {
            mob_entity,
            can_break_doors: AtomicBool::new(false),
            is_baby: AtomicBool::new(false),
        };
        let mob_arc = Arc::new(zombie);
        let mob_weak: Weak<dyn Mob> = {
            let mob_arc: Arc<dyn Mob> = mob_arc.clone();
            Arc::downgrade(&mob_arc)
        };

        let goal_selector = &mob_arc.mob_entity.goals_selector;
        goal_selector.add_goal(4, DestroyEggGoal::new(1.0, 3));
        goal_selector.add_goal(
            8,
            LookAtEntityGoal::with_default(mob_weak, &EntityType::PLAYER, 8.0),
        );
        goal_selector.add_goal(8, Box::new(RandomLookAroundGoal::default()));

        mob_arc
    }

    /// Vanilla `Zombie.addBehaviourGoals`.
    // TODO: `SpearUseGoal` (2) needs mobs that use kinetic weapons and `MoveThroughVillageGoal`
    // (6) needs village POIs.
    fn add_behaviour_goals(&self) {
        let goal_selector = &self.mob_entity.goals_selector;
        goal_selector.add_goal(3, ZombieAttackGoal::new(1.0, false));
        goal_selector.add_goal(7, Box::new(WanderAroundGoal::water_avoiding(1.0)));

        let target_selector = &self.mob_entity.target_selector;
        target_selector.add_goal(1, Box::new(Self::revenge_goal()));
        hunting::apply(target_selector, &self.mob_entity, ZOMBIE_HUNTS);
    }

    #[must_use]
    pub fn can_break_doors(&self) -> bool {
        self.can_break_doors.load(Ordering::Relaxed)
    }

    /// Vanilla `Zombie.setCanBreakDoors`: only a zombie with ground navigation breaks doors, and
    /// then its paths lead through them.
    pub fn set_can_break_doors(&self, can_break_doors: bool, mob: &dyn Mob) {
        let mut navigator = self
            .mob_entity
            .navigator
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let navigates_ground = navigator.can_navigate_ground();
        let can_break_doors = can_break_doors && navigates_ground;
        if self
            .can_break_doors
            .swap(can_break_doors, Ordering::Relaxed)
            == can_break_doors
        {
            return;
        }
        if navigates_ground {
            navigator.set_can_open_doors(can_break_doors);
        }
        drop(navigator);

        if can_break_doors {
            self.mob_entity
                .goals_selector
                .add_goal(1, Box::new(BreakDoorGoal::default()));
        } else {
            self.mob_entity
                .goals_selector
                .remove_goal::<BreakDoorGoal>(mob);
        }
    }
}

impl ZombieEntityBase {
    /// Vanilla `Zombie.setBaby`: the baby state is only the synced flag, it never ages up.
    // TODO: vanilla baby zombies get the +50% SPEED_MODIFIER_BABY and 2.5x XP
    // (`Zombie.getBaseExperienceReward`), unlike passive babies which drop none.
    pub fn set_baby(&self, baby: bool) {
        self.mob_entity.set_baby_flag(
            &self.is_baby,
            pumpkin_data::tracked_data::zombie::BABY,
            baby,
        );
    }

    #[must_use]
    pub fn is_baby(&self) -> bool {
        self.is_baby.load(Ordering::Relaxed)
    }
}

impl Mob for ZombieEntityBase {
    fn mob_metadata(&self) -> Option<MobMetadata> {
        self.is_baby().then(|| MobMetadata::new().baby(true))
    }

    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    fn spawn_as_baby(&self) -> bool {
        self.set_baby(true);
        true
    }

    fn populate_default_equipment_slots(
        &self,
        _world: &Arc<World>,
        difficulty: &RegionalDifficulty,
    ) {
        // Default armor slots (super.populateDefaultEquipmentSlots)
        if rand::random::<f32>()
            < MobEntity::MAX_WEARING_ARMOR_CHANCE * difficulty.special_multiplier
        {
            let mut armor_type = rand::random_range(0..3);
            for _ in 1..=3 {
                if rand::random::<f32>() < MobEntity::WEARING_ARMOR_UPGRADE_MATERIAL_CHANCE {
                    armor_type += 1;
                }
            }

            let partial_chance = if difficulty.base_difficulty == Difficulty::Hard {
                0.1f32
            } else {
                0.25f32
            };

            let living = &self.mob_entity.living_entity;
            let mut equipment = living
                .entity_equipment
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let mut first = true;

            for slot in &MobEntity::EQUIPMENT_POPULATION_ORDER {
                let current = equipment.get(slot);
                if !first && rand::random::<f32>() < partial_chance {
                    break;
                }
                first = false;
                if current.is_empty()
                    && let Some(item) = MobEntity::get_equipment_for_slot(slot, armor_type)
                {
                    equipment.put(slot, ItemStack::new(1, item));
                }
            }
        }

        let weapon_chance = if difficulty.base_difficulty == Difficulty::Hard {
            0.05f32
        } else {
            0.01f32
        };
        if rand::random::<f32>() < weapon_chance {
            let r = rand::random_range(0..6);
            let weapon_item = match r {
                0 => &Item::IRON_SWORD,
                1 => &Item::IRON_SPEAR,
                _ => &Item::IRON_SHOVEL,
            };
            let living = &self.mob_entity.living_entity;
            let mut equipment = living
                .entity_equipment
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            equipment.put(&EquipmentSlot::MAIN_HAND, ItemStack::new(1, weapon_item));
        }
    }

    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        // Vanilla Zombie.addAdditionalSaveData; shared by every zombie-family mob.
        nbt.put_bool("IsBaby", self.is_baby());
        if self.can_break_doors() {
            nbt.put_bool("CanBreakDoors", true);
        }
    }

    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        self.set_baby(nbt.get_bool("IsBaby").unwrap_or(false));
        if let Some(can_break_doors) = nbt.get_bool("CanBreakDoors") {
            self.set_can_break_doors(can_break_doors, self);
        }
    }
}
