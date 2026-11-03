use super::{Mob, MobEntity, MobMetadata};
use crate::entity::ai::goal::break_door::BreakDoorGoal;
use crate::entity::ai::goal::destroy_egg::DestroyEggGoal;
use crate::entity::ai::goal::look_around::RandomLookAroundGoal;
use crate::entity::ai::goal::revenge::RevengeGoal;
use crate::entity::ai::goal::wander_around::WanderAroundGoal;
use crate::entity::ai::goal::zombie_attack::ZombieAttackGoal;
use crate::entity::attributes::{AttributeInstance, Modifier, ModifierOperation};
use crate::entity::mob::conversion::ConversionTracker;
use crate::entity::mob::equipment::RegionalDifficulty;
use crate::entity::mob::spawn::{self, EntitySpawnReason, SpawnGroupData, finalize_spawn};
use crate::entity::r#type::from_type;
use crate::entity::{
    Entity, EntityBase,
    ai::behavior::hunting::{self, Hunt},
    ai::goal::look_at_entity::LookAtEntityGoal,
    ai::target_match::TargetMatch,
};
use crate::world::World;
use crossbeam::atomic::AtomicCell;
use pumpkin_data::attributes::Attributes;
use pumpkin_data::data_component_impl::EquipmentSlot;
use pumpkin_data::entity::EntityType;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::world::WorldEvent;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::Difficulty;
use rand::RngExt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};
use uuid::Uuid;

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

/// Vanilla `Zombie.getSpawnAsBabyOdds`.
const BABY_CHANCE: f32 = 0.05;
/// Vanilla `Zombie.finalizeSpawn`: a baby's chance of riding a chicken nearby, else of one
/// spawned for it; how far sideways and up the box around it reaches for one.
const CHICKEN_JOCKEY_CHANCE: f32 = 0.05;
const CHICKEN_SEARCH_RANGE: (f64, f64) = (5.0, 3.0);
/// Vanilla `Zombie.finalizeSpawn`: the door breaking chance per special difficulty.
const CAN_BREAK_DOORS_CHANCE: f32 = 0.1;
/// Vanilla `Zombie.handleAttributes`: the reinforcement chance's range, the knockback
/// resistance bonus's range, the follow range factor, the leader chance per special difficulty,
/// a leader's reinforcement bonus (base, range) and health multiplier (base, range).
const REINFORCEMENTS_CHANCE_RANGE: f64 = 0.1;
const KNOCKBACK_RESISTANCE_BONUS_RANGE: f64 = 0.05;
const FOLLOW_RANGE_BONUS_FACTOR: f64 = 1.5;
const LEADER_CHANCE: f32 = 0.05;
const LEADER_REINFORCEMENTS_BONUS: (f64, f64) = (0.5, 0.25);
const LEADER_HEALTH_BONUS: (f64, f64) = (1.0, 3.0);
const LEADER_ZOMBIE_BONUS_ID: &str = "minecraft:leader_zombie_bonus";
const ZOMBIE_RANDOM_SPAWN_BONUS_ID: &str = "minecraft:zombie_random_spawn_bonus";

/// Vanilla `Zombie.finalizeSpawn`'s chicken jockey roll, carried to the first tick, where the
/// zombie is in the world and can ride.
#[derive(Clone, Copy)]
enum ChickenJockey {
    /// An unridden chicken within `CHICKEN_SEARCH_RANGE`.
    Existing,
    /// A chicken spawned for it.
    New,
}

pub struct ZombieEntityBase {
    pub mob_entity: MobEntity,
    pub can_break_doors: AtomicBool,
    pub is_baby: AtomicBool,
    chicken_jockey: AtomicCell<Option<ChickenJockey>>,
    /// Vanilla `Zombie.drowningTracker`, for the kinds that `convertsInWater`.
    drowning: Option<ConversionTracker>,
}

/// Vanilla `Zombie.drowningTracker`: 600 ticks with the eyes in water start a 300 tick
/// conversion, a zombie into a drowned and a husk into a zombie (`convertsToWhenDrowning`).
fn drowning_tracker(entity_type: &EntityType) -> Option<ConversionTracker> {
    let (converts_to, event) = match entity_type.id {
        id if id == EntityType::ZOMBIE.id => {
            (&EntityType::DROWNED, WorldEvent::SoundZombieToDrowned)
        }
        id if id == EntityType::HUSK.id => (&EntityType::ZOMBIE, WorldEvent::SoundHuskToZombie),
        _ => return None,
    };
    Some(ConversionTracker::new(
        converts_to,
        event,
        pumpkin_data::tracked_data::zombie::DATA_DROWNED_CONVERSION_ID,
        |mob| mob.get_entity().is_submerged_in_water(),
        ("InWaterTime", 600),
        ("DrownedConversionTime", 300),
    ))
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
        let drowning = drowning_tracker(entity.entity_type);
        let mob_entity = MobEntity::new(entity);
        let zombie = Self {
            mob_entity,
            can_break_doors: AtomicBool::new(false),
            is_baby: AtomicBool::new(false),
            chicken_jockey: AtomicCell::new(None),
            drowning,
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

impl ZombieEntityBase {
    /// Vanilla `Zombie.finalizeSpawn`'s jockey roll for a baby: one in twenty rides a chicken
    /// nearby, else one in twenty gets a chicken of its own.
    fn roll_chicken_jockey<R: rand::Rng + ?Sized>(&self, rng: &mut R) {
        let rides_one_nearby = rng.random::<f32>() < CHICKEN_JOCKEY_CHANCE;
        let jockey = if rides_one_nearby {
            ChickenJockey::Existing
        } else if rng.random::<f32>() < CHICKEN_JOCKEY_CHANCE {
            ChickenJockey::New
        } else {
            return;
        };
        self.chicken_jockey.store(Some(jockey));
    }

    /// The rest of the jockey roll once the zombie is in the world: the chicken found or made,
    /// marked as a jockey's and ridden.
    fn mount_chicken(&self, jockey: ChickenJockey) {
        let entity = self.get_entity();
        let world = entity.world.load_full();
        let Some(this) = world.get_entity_by_id(entity.entity_id) else {
            return;
        };
        let chicken = match jockey {
            ChickenJockey::Existing => {
                let (sideways, up) = CHICKEN_SEARCH_RANGE;
                let search = entity.bounding_box.load().expand(sideways, up, sideways);
                world
                    .get_entities_at_box(&search)
                    .into_iter()
                    .find(|nearby| {
                        let nearby = nearby.get_entity();
                        nearby.entity_type == &EntityType::CHICKEN && !nearby.has_passengers()
                    })
            }
            ChickenJockey::New => {
                let chicken = from_type(
                    &EntityType::CHICKEN,
                    entity.pos.load(),
                    &world,
                    Uuid::new_v4(),
                );
                chicken.get_entity().set_rotation(entity.yaw.load(), 0.0);
                finalize_spawn(&chicken, &world, EntitySpawnReason::Jockey, None);
                world.spawn_entity(chicken.clone()).then_some(chicken)
            }
        };
        let Some(chicken) = chicken else {
            return;
        };
        if let Some(chicken_mob) = chicken.get_mob().and_then(|mob| mob.as_chicken()) {
            chicken_mob.set_chicken_jockey(true);
        }
        chicken.get_entity().add_passenger(chicken.clone(), this);
    }

    /// Vanilla `Zombie.handleAttributes`: a random reinforcement chance, a knockback resistance
    /// bonus, a follow range bonus above one, and the leader roll: more reinforcements, more
    /// health (full unless converted, loaded or travelled) and door breaking.
    fn handle_attributes<R: rand::Rng + ?Sized>(&self, multiplier: f32, rng: &mut R) {
        let living = &self.mob_entity.living_entity;
        let leader = {
            let mut attributes = living
                .attributes
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            // Vanilla `randomizeReinforcementsChance`.
            let reinforcements = attributes
                .entry(Attributes::SPAWN_REINFORCEMENTS.id)
                .or_insert_with(|| AttributeInstance::new(0.0));
            reinforcements.base_value = rng.random::<f64>() * REINFORCEMENTS_CHANCE_RANGE;
            reinforcements.mark_dirty();
            if let Some(resistance) = attributes.get_mut(&Attributes::KNOCKBACK_RESISTANCE.id) {
                resistance.add_or_replace_permanent_modifier(Modifier {
                    id: spawn::RANDOM_SPAWN_BONUS_ID.to_string(),
                    amount: rng.random::<f64>() * KNOCKBACK_RESISTANCE_BONUS_RANGE,
                    operation: ModifierOperation::Add,
                });
            }
            let follow_range_bonus =
                rng.random::<f64>() * FOLLOW_RANGE_BONUS_FACTOR * f64::from(multiplier);
            if follow_range_bonus > 1.0
                && let Some(follow_range) = attributes.get_mut(&Attributes::FOLLOW_RANGE.id)
            {
                follow_range.add_or_replace_permanent_modifier(Modifier {
                    id: ZOMBIE_RANDOM_SPAWN_BONUS_ID.to_string(),
                    amount: follow_range_bonus,
                    operation: ModifierOperation::MultiplyTotal,
                });
            }
            let leader = rng.random::<f32>() < multiplier * LEADER_CHANCE;
            if leader {
                let (base, range) = LEADER_REINFORCEMENTS_BONUS;
                if let Some(reinforcements) =
                    attributes.get_mut(&Attributes::SPAWN_REINFORCEMENTS.id)
                {
                    reinforcements.add_or_replace_permanent_modifier(Modifier {
                        id: LEADER_ZOMBIE_BONUS_ID.to_string(),
                        amount: rng.random::<f64>() * range + base,
                        operation: ModifierOperation::Add,
                    });
                }
                let (base, range) = LEADER_HEALTH_BONUS;
                if let Some(max_health) = attributes.get_mut(&Attributes::MAX_HEALTH.id) {
                    max_health.add_or_replace_permanent_modifier(Modifier {
                        id: LEADER_ZOMBIE_BONUS_ID.to_string(),
                        amount: rng.random::<f64>() * range + base,
                        operation: ModifierOperation::MultiplyTotal,
                    });
                }
            }
            leader
        };
        if leader {
            if !matches!(
                self.mob_entity.spawn_reason(),
                Some(
                    EntitySpawnReason::Conversion
                        | EntitySpawnReason::Load
                        | EntitySpawnReason::DimensionTravel
                )
            ) {
                living.set_health(living.get_max_health());
            }
            self.set_can_break_doors(true, self);
        }
    }
}

impl Mob for ZombieEntityBase {
    /// Vanilla `Zombie.finalizeSpawn`: the group's baby roll with a baby's chicken jockey, door
    /// breaking, the Halloween head and the random attribute bonuses. Loot pickup and the
    /// equipment are the base's, skipped on a conversion.
    fn finalize_spawn(
        &self,
        world: &Arc<World>,
        group_data: Option<SpawnGroupData>,
    ) -> Option<SpawnGroupData> {
        let group_data = self.finalize_spawn_super(group_data);
        let mut rng = rand::rng();
        let multiplier =
            RegionalDifficulty::at(world, self.get_entity().pos.load()).special_multiplier;
        let group_data = group_data.unwrap_or_else(|| SpawnGroupData::Zombie {
            is_baby: rng.random::<f32>() < BABY_CHANCE,
            can_spawn_jockey: true,
        });
        if let SpawnGroupData::Zombie {
            is_baby,
            can_spawn_jockey,
        } = group_data
        {
            if is_baby {
                self.set_baby(true);
                if can_spawn_jockey {
                    self.roll_chicken_jockey(&mut rng);
                }
            }
            self.set_can_break_doors(
                rng.random::<f32>() < multiplier * CAN_BREAK_DOORS_CHANCE,
                self,
            );
        }
        spawn::roll_halloween_head(&self.mob_entity, &mut rng);
        self.handle_attributes(multiplier, &mut rng);
        Some(group_data)
    }

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

    fn mob_tick(&self, _caller: &dyn EntityBase) {
        if let Some(jockey) = self.chicken_jockey.take() {
            self.mount_chicken(jockey);
        }
        if let Some(drowning) = &self.drowning {
            drowning.tick(self);
        }
    }

    fn mob_init_data_tracker(&self) {
        if let Some(drowning) = &self.drowning {
            drowning.sync(self.get_entity());
        }
    }

    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        // Vanilla Zombie.addAdditionalSaveData; shared by every zombie-family mob.
        nbt.put_bool("IsBaby", self.is_baby());
        if self.can_break_doors() {
            nbt.put_bool("CanBreakDoors", true);
        }
        if let Some(drowning) = &self.drowning {
            drowning.write_nbt(self, nbt);
        }
    }

    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        self.set_baby(nbt.get_bool("IsBaby").unwrap_or(false));
        if let Some(can_break_doors) = nbt.get_bool("CanBreakDoors") {
            self.set_can_break_doors(can_break_doors, self);
        }
        if let Some(drowning) = &self.drowning {
            drowning.read_nbt(self.get_entity(), nbt);
        }
    }
}
