use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicI32, Ordering},
};

use crossbeam::atomic::AtomicCell;
use pumpkin_data::Block;
use pumpkin_data::BlockState;
use pumpkin_data::Enchantment;
use pumpkin_data::data_component_impl::EquipmentSlot;
use pumpkin_data::entity::EntityType;
use pumpkin_data::environment_attribute::Activity;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::tag::{self, Taggable};
use pumpkin_data::tracked_data;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_nbt::tag::NbtTag;
use pumpkin_util::math::boundingbox::EntityDimensions;
use pumpkin_util::math::position::BlockPos;

use crate::entity::ai::brain::memory::{MemoryModuleId, PackedMemories, types};
use crate::entity::ai::brain::sensing::SensorType;
use crate::entity::ai::brain::{Brain, BrainProvider};
use crate::entity::mob::piglin_anger::{self, AbstractPiglin, QueuedChange};
use crate::entity::mob::piglin_brain;
use crate::entity::mob::spawn::SpawnGroupData;
use crate::entity::mob::{MobMetadata, equipment as mob_equipment};
use crate::entity::player::Player;
use crate::entity::{
    Entity, EntityBase,
    mob::{
        Mob, MobEntity, crossbow_attack_mob::CrossbowAttackMob, equipment::RegionalDifficulty,
        piglin_ai::PiglinAi,
    },
};
use crate::world::World;
use crate::world::level_accessor::LevelAccessor;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PiglinActivity {
    Idle,
    AdmireItem,
    Fight,
    Celebrate,
    Avoid,
    Ride,
}

impl PiglinActivity {
    /// The brain's non-core activity, as sounds and other mobs read it.
    #[must_use]
    pub const fn from_brain(activity: Option<Activity>) -> Self {
        match activity {
            Some(Activity::AdmireItem) => Self::AdmireItem,
            Some(Activity::Fight) => Self::Fight,
            Some(Activity::Celebrate) => Self::Celebrate,
            Some(Activity::Avoid) => Self::Avoid,
            Some(Activity::Ride) => Self::Ride,
            _ => Self::Idle,
        }
    }
}

const MEMORIES: [MemoryModuleId; 28] = [
    types::UNIVERSAL_ANGER.id(),
    types::ATE_RECENTLY.id(),
    types::ANGRY_AT.id(),
    types::ADMIRING_ITEM.id(),
    types::ADMIRING_DISABLED.id(),
    types::DISABLE_WALK_TO_ADMIRE_ITEM.id(),
    types::TIME_TRYING_TO_REACH_ADMIRE_ITEM.id(),
    types::CELEBRATE_LOCATION.id(),
    types::DANCING.id(),
    types::HUNTED_RECENTLY.id(),
    types::AVOID_TARGET.id(),
    types::RIDE_TARGET.id(),
    types::ATTACK_TARGET.id(),
    types::ATTACK_COOLING_DOWN.id(),
    types::CANT_REACH_WALK_TARGET_SINCE.id(),
    types::NEAREST_REPELLENT.id(),
    types::NEAREST_VISIBLE_ZOMBIFIED.id(),
    types::NEAREST_VISIBLE_NEMESIS.id(),
    types::NEAREST_VISIBLE_HUNTABLE_HOGLIN.id(),
    types::NEAREST_VISIBLE_BABY_HOGLIN.id(),
    types::NEAREST_VISIBLE_ADULT_PIGLINS.id(),
    types::NEARBY_ADULT_PIGLINS.id(),
    types::VISIBLE_ADULT_PIGLIN_COUNT.id(),
    types::VISIBLE_ADULT_HOGLIN_COUNT.id(),
    types::SPEAR_FLEEING_TIME.id(),
    types::SPEAR_FLEEING_POSITION.id(),
    types::SPEAR_CHARGE_POSITION.id(),
    types::SPEAR_ENGAGE_TIME.id(),
];

pub struct PiglinEntity {
    pub mob_entity: MobEntity,
    pub immune_to_zombification: AtomicBool,
    pub time_in_overworld: AtomicI32,
    pub is_baby: AtomicBool,
    pub cannot_hunt: AtomicBool,
    pub is_charging_crossbow: AtomicBool,
    pub is_dancing: AtomicBool,
    pub inventory: Mutex<Vec<ItemStack>>,
    /// What others asked of its brain, for the core activity.
    pub(super) queued: Mutex<Vec<QueuedChange>>,
    /// Memories kept readable while the brain is locked.
    pub(super) admiring: AtomicBool,
    pub(super) admiring_disabled: AtomicBool,
    pub(super) ate_recently: AtomicBool,
    pub(super) hunted_recently: AtomicBool,
    pub(super) near_repellent: AtomicBool,
    pub activity: AtomicCell<PiglinActivity>,
    pub ambient_sound_time: AtomicI32,
}

impl PiglinEntity {
    pub const CONVERSION_TIME: i32 = 300;
    pub const INVENTORY_SIZE: usize = 8;
    pub const XP_REWARD: u32 = 5;
    pub const AMBIENT_SOUND_INTERVAL: i32 = 80;

    pub const ADULT_DIMENSIONS: EntityDimensions = EntityDimensions {
        width: 0.6,
        height: 1.95,
        eye_height: 1.79,
    };
    pub const BABY_DIMENSIONS: EntityDimensions = EntityDimensions {
        width: 0.49,
        height: 0.98,
        eye_height: 0.78,
    };

    pub fn new(entity: Entity) -> Arc<Self> {
        let mob_entity = MobEntity::new(entity);
        let piglin = Self {
            mob_entity,
            immune_to_zombification: AtomicBool::new(false),
            time_in_overworld: AtomicI32::new(0),
            is_baby: AtomicBool::new(false),
            cannot_hunt: AtomicBool::new(false),
            is_charging_crossbow: AtomicBool::new(false),
            is_dancing: AtomicBool::new(false),
            inventory: Mutex::new(Vec::new()),
            queued: Mutex::new(Vec::new()),
            admiring: AtomicBool::new(false),
            admiring_disabled: AtomicBool::new(false),
            ate_recently: AtomicBool::new(false),
            hunted_recently: AtomicBool::new(false),
            near_repellent: AtomicBool::new(false),
            activity: AtomicCell::new(PiglinActivity::Idle),
            ambient_sound_time: AtomicI32::new(0),
        };
        let mob_arc = Arc::new(piglin);
        mob_arc.mob_entity.set_can_pick_up_loot(true);
        // Vanilla `AbstractPiglin.applyOpenDoorsAbility`.
        mob_arc
            .mob_entity
            .steered()
            .navigator()
            .set_can_open_doors(true);

        mob_arc
    }

    #[must_use]
    pub fn is_immune_to_zombification(&self) -> bool {
        self.immune_to_zombification.load(Ordering::Relaxed)
    }

    pub fn set_immune_to_zombification(&self, immune: bool) {
        self.immune_to_zombification
            .store(immune, Ordering::Relaxed);
        self.mob_entity
            .living_entity
            .entity
            .set_synced_data(tracked_data::piglin::DATA_IMMUNE_TO_ZOMBIFICATION, immune);
    }

    #[must_use]
    pub fn is_converting(&self, world: &World) -> bool {
        !self.is_immune_to_zombification()
            && !self.mob_entity.is_no_ai()
            && world.dimension.piglins_zombify
    }

    #[must_use]
    pub fn is_baby(&self) -> bool {
        self.is_baby.load(Ordering::Relaxed)
    }

    #[must_use]
    pub fn is_adult(&self) -> bool {
        !self.is_baby()
    }

    pub fn set_baby(&self, baby: bool) {
        self.mob_entity
            .set_baby_flag(&self.is_baby, tracked_data::piglin::DATA_BABY_ID, baby);
        let entity = &self.mob_entity.living_entity.entity;
        entity.set_default_dimensions(if baby {
            Self::BABY_DIMENSIONS
        } else {
            Self::ADULT_DIMENSIONS
        });
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
            .set_synced_data(tracked_data::piglin::DATA_IS_CHARGING_CROSSBOW, is_charging);
    }

    #[must_use]
    pub fn is_dancing(&self) -> bool {
        self.is_dancing.load(Ordering::Relaxed)
    }

    pub fn set_dancing(&self, is_dancing: bool) {
        self.is_dancing.store(is_dancing, Ordering::Relaxed);
        self.mob_entity
            .living_entity
            .entity
            .set_synced_data(tracked_data::piglin::DATA_IS_DANCING, is_dancing);
    }

    #[must_use]
    pub fn can_hunt(&self) -> bool {
        !self.cannot_hunt.load(Ordering::Relaxed)
    }

    pub fn set_cannot_hunt(&self, cannot_hunt: bool) {
        self.cannot_hunt.store(cannot_hunt, Ordering::Relaxed);
    }

    #[must_use]
    pub fn is_admiring(&self) -> bool {
        self.admiring.load(Ordering::Relaxed)
    }

    #[must_use]
    pub fn is_admiring_disabled(&self) -> bool {
        self.admiring_disabled.load(Ordering::Relaxed)
    }

    #[must_use]
    pub fn has_eaten_recently(&self) -> bool {
        self.ate_recently.load(Ordering::Relaxed)
    }

    #[must_use]
    pub fn has_attack_target(&self) -> bool {
        self.mob_entity
            .target
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_some()
    }

    #[must_use]
    pub fn off_hand_item(&self) -> ItemStack {
        self.mob_entity
            .living_entity
            .entity_equipment
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&EquipmentSlot::OFF_HAND)
    }

    #[must_use]
    pub fn is_holding_item_in_off_hand(&self) -> bool {
        !self.off_hand_item().is_empty()
    }

    fn make_sound(&self, sound: Sound) {
        let entity = &self.mob_entity.living_entity.entity;
        let base_pitch = if self.is_baby() { 1.5 } else { 1.0 };
        let pitch = (rand::random::<f32>() - rand::random::<f32>()).mul_add(0.2, base_pitch);
        entity.world.load().play_sound_fine(
            sound,
            SoundCategory::Hostile,
            &entity.pos.load(),
            1.0,
            pitch,
        );
    }

    fn tick_ambient_sound(&self) {
        let living = &self.mob_entity.living_entity;
        let alive = living.entity.is_alive() && living.health.load() > 0.0;
        if alive
            && rand::random_range(0..1000) < self.ambient_sound_time.fetch_add(1, Ordering::Relaxed)
        {
            self.reset_ambient_sound_time();
            let sound = self.mob_entity.with_brain(self, |brain| {
                PiglinAi::get_sound_for_activity(self, self.activity.load(), brain)
            });
            self.make_sound(sound);
        }
    }

    fn reset_ambient_sound_time(&self) {
        self.ambient_sound_time
            .store(-Self::AMBIENT_SOUND_INTERVAL, Ordering::Relaxed);
    }

    #[must_use]
    pub fn check_piglin_spawn_rules(level: &dyn LevelAccessor, pos: &BlockPos) -> bool {
        let below = BlockPos::new(pos.0.x, pos.0.y - 1, pos.0.z);
        let state = level.get_block_state(&below);
        state.id != Block::NETHER_WART_BLOCK.default_state.id
    }

    fn convert_to_zombified(&self) {
        let entity = &self.mob_entity.living_entity.entity;
        let world = entity.world.load();
        let pos = entity.pos.load();

        if world.level_info.load().difficulty != pumpkin_util::Difficulty::Peaceful {
            world.play_sound(
                Sound::EntityPiglinConvertedToZombified,
                SoundCategory::Hostile,
                &pos,
            );
        }

        self.drop_inventory();

        // Vanilla `AbstractPiglin.finishConversion`: nausea for 10 seconds.
        let _ = crate::entity::mob::conversion::convert_to(
            self,
            &EntityType::ZOMBIFIED_PIGLIN,
            crate::entity::mob::conversion::ConversionParams::single(true, true),
            |converted| {
                converted
                    .get_mob_entity()
                    .living_entity
                    .add_effect(pumpkin_data::potion::Effect {
                        effect_type: &pumpkin_data::effect::StatusEffect::NAUSEA,
                        duration: 200,
                        amplifier: 0,
                        ambient: false,
                        show_particles: true,
                        show_icon: true,
                        blend: false,
                    });
            },
        );
    }
}

impl Mob for PiglinEntity {
    fn mob_play_step_sound(&self, _pos: &BlockPos, _state: &BlockState) {
        self.play_entity_sound(Sound::EntityPiglinStep, 0.15, 1.0);
    }

    fn mob_metadata(&self) -> Option<MobMetadata> {
        self.is_baby().then(|| MobMetadata::new().baby(true))
    }

    fn spawn_as_baby(&self) -> bool {
        self.set_baby(true);
        true
    }

    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    fn populate_default_equipment_slots(
        &self,
        _world: &Arc<World>,
        _difficulty: &RegionalDifficulty,
    ) {
        if !self.is_baby.load(Ordering::Relaxed) {
            let living = &self.mob_entity.living_entity;
            let mut equipment = living
                .entity_equipment
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);

            let weapon = if rand::random::<f32>() < 0.5 {
                &Item::CROSSBOW
            } else if rand::random_range(0..10) == 0 {
                &Item::GOLDEN_SPEAR
            } else {
                &Item::GOLDEN_SWORD
            };
            equipment.put(&EquipmentSlot::MAIN_HAND, ItemStack::new(1, weapon));

            if rand::random::<f32>() < 0.1 {
                equipment.put(
                    &EquipmentSlot::HEAD,
                    ItemStack::new(1, &Item::GOLDEN_HELMET),
                );
            }
            if rand::random::<f32>() < 0.1 {
                equipment.put(
                    &EquipmentSlot::CHEST,
                    ItemStack::new(1, &Item::GOLDEN_CHESTPLATE),
                );
            }
            if rand::random::<f32>() < 0.1 {
                equipment.put(
                    &EquipmentSlot::LEGS,
                    ItemStack::new(1, &Item::GOLDEN_LEGGINGS),
                );
            }
            if rand::random::<f32>() < 0.1 {
                equipment.put(&EquipmentSlot::FEET, ItemStack::new(1, &Item::GOLDEN_BOOTS));
            }
        }
    }

    fn mob_init_data_tracker(&self) {
        let entity = self.get_entity();
        if self.is_immune_to_zombification() {
            entity.set_synced_data(tracked_data::piglin::DATA_IMMUNE_TO_ZOMBIFICATION, true);
        }
        if self.is_baby() {
            entity.set_synced_data(tracked_data::piglin::DATA_BABY_ID, true);
        }
        if self.is_charging_crossbow() {
            entity.set_synced_data(tracked_data::piglin::DATA_IS_CHARGING_CROSSBOW, true);
        }
        if self.is_dancing() {
            entity.set_synced_data(tracked_data::piglin::DATA_IS_DANCING, true);
        }
    }

    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        if self.is_immune_to_zombification() {
            nbt.put_bool("IsImmuneToZombification", true);
        }
        let time_in_overworld = self.time_in_overworld.load(Ordering::Relaxed);
        if time_in_overworld > 0 {
            nbt.put_int("TimeInOverworld", time_in_overworld);
        }
        if self.is_baby() {
            nbt.put_bool("IsBaby", true);
        }
        if !self.can_hunt() {
            nbt.put_bool("CannotHunt", true);
        }
        if self.is_charging_crossbow() {
            nbt.put_bool("IsChargingCrossbow", true);
        }
        if self.is_dancing() {
            nbt.put_bool("IsDancing", true);
        }

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
                nbt.put_list("Inventory", items_tag);
            }
        }
    }

    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        // Vanilla `AbstractPiglin.readAdditionalSaveData`: they do unless told otherwise.
        self.mob_entity
            .set_can_pick_up_loot(nbt.get_bool("CanPickUpLoot").unwrap_or(true));
        if let Some(immune) = nbt.get_bool("IsImmuneToZombification") {
            self.set_immune_to_zombification(immune);
        }
        if let Some(time) = nbt.get_int("TimeInOverworld") {
            self.time_in_overworld.store(time, Ordering::Relaxed);
        }
        if let Some(baby) = nbt.get_bool("IsBaby") {
            self.set_baby(baby);
        }
        if let Some(cannot_hunt) = nbt.get_bool("CannotHunt") {
            self.set_cannot_hunt(cannot_hunt);
        }
        if let Some(charging) = nbt.get_bool("IsChargingCrossbow") {
            self.set_charging_crossbow(charging);
        }
        if let Some(dancing) = nbt.get_bool("IsDancing") {
            self.set_dancing(dancing);
        }
        if let Some(inv_list) = nbt.get_list("Inventory") {
            let mut inv = self
                .inventory
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            inv.clear();
            for tag in inv_list {
                if let Some(compound) = tag.extract_compound()
                    && let Some(stack) = ItemStack::read_item_stack(compound)
                {
                    inv.push(stack);
                }
            }
        }
    }

    fn mob_interact(&self, player: &Arc<Player>, item_stack: &mut ItemStack) -> bool {
        if PiglinAi::can_admire(self, item_stack) {
            let taken = item_stack.split_unless_creative(player.gamemode.load(), 1);
            self.start_admiring(taken);
            return true;
        }
        self.mob_entity
            .mob_interact(player, item_stack, self.can_be_leashed())
    }

    fn mob_tick(&self, _caller: &dyn EntityBase) {
        let entity = &self.mob_entity.living_entity.entity;
        if !entity.is_alive() {
            return;
        }
        self.tick_ambient_sound();

        let world = entity.world.load();
        if self.is_converting(&world) {
            let time = self.time_in_overworld.fetch_add(1, Ordering::Relaxed) + 1;
            if time > Self::CONVERSION_TIME {
                self.convert_to_zombified();
            }
        } else {
            self.time_in_overworld.store(0, Ordering::Relaxed);
        }
    }

    /// Vanilla `Piglin.customServerAiStep` and `PiglinAi.updateActivity`.
    fn custom_server_ai_step(&self, _caller: &dyn EntityBase) {
        self.mob_entity.tick_brain(self);
        let target = self.mob_entity.with_brain(self, |brain| {
            let old = brain.get_active_non_core_activity();
            piglin_brain::piglin_update_activity(brain);
            let new = brain.get_active_non_core_activity();
            self.activity.store(PiglinActivity::from_brain(new));
            if old != new {
                self.make_sound(PiglinAi::get_sound_for_activity(
                    self,
                    self.activity.load(),
                    brain,
                ));
            }
            self.mob_entity
                .set_attacking(brain.has_memory_value(types::ATTACK_TARGET.id()));
            if !brain.has_memory_value(types::CELEBRATE_LOCATION.id()) {
                brain.erase(types::DANCING.id());
            }
            self.set_dancing(brain.has_memory_value(types::DANCING.id()));
            self.admiring.store(
                brain.has_memory_value(types::ADMIRING_ITEM.id()),
                Ordering::Relaxed,
            );
            self.admiring_disabled.store(
                brain.has_memory_value(types::ADMIRING_DISABLED.id()),
                Ordering::Relaxed,
            );
            self.ate_recently.store(
                brain.has_memory_value(types::ATE_RECENTLY.id()),
                Ordering::Relaxed,
            );
            self.hunted_recently.store(
                brain.has_memory_value(types::HUNTED_RECENTLY.id()),
                Ordering::Relaxed,
            );
            self.near_repellent.store(
                brain.has_memory_value(types::NEAREST_REPELLENT.id()),
                Ordering::Relaxed,
            );
            brain.get(types::ATTACK_TARGET).cloned()
        });
        // Vanilla `getTarget` reads the brain.
        self.mob_entity.set_target(target);
    }

    /// Vanilla `Piglin.makeBrain`.
    fn make_brain(&self, packed: &PackedMemories) -> Brain {
        BrainProvider {
            memory_types: &MEMORIES,
            sensor_types: &[
                SensorType::NearestLivingEntities,
                SensorType::NearestPlayers,
                SensorType::NearestItems,
                SensorType::HurtBy,
                SensorType::PiglinSpecific,
            ],
            activities: piglin_brain::piglin_activities,
        }
        .make_brain(self, packed)
    }

    /// Vanilla `Piglin.finalizeSpawn`: it hunts only after a while.
    fn finalize_spawn(
        &self,
        _world: &Arc<World>,
        group_data: Option<SpawnGroupData>,
    ) -> Option<SpawnGroupData> {
        self.mob_entity
            .with_brain(self, |brain| piglin_brain::init_memories(brain));
        self.finalize_spawn_super(group_data)
    }

    /// Vanilla `Piglin.wantsToPickUp`.
    fn wants_to_pick_up(&self, stack: &ItemStack) -> bool {
        self.get_entity()
            .world
            .load()
            .level_info
            .load()
            .game_rules
            .mob_griefing
            && self.mob_entity.can_pick_up_loot()
            && PiglinAi::wants_to_pickup(self, stack)
    }

    fn mob_weapon_range(&self) -> Option<f64> {
        self.is_holding_crossbow().then_some(8.0)
    }

    /// Vanilla `isHolding(canUseNonMeleeWeapon)`.
    fn holds_non_melee_weapon(&self) -> bool {
        self.is_holding_crossbow() || self.is_holding_kinetic_weapon()
    }

    fn after_hurt(
        &self,
        _damage_type: pumpkin_data::damage::DamageType,
        source: Option<&dyn EntityBase>,
        cause: Option<&dyn EntityBase>,
    ) {
        let Some(id) = cause.or(source).map(|by| by.get_entity().entity_id) else {
            return;
        };
        self.queue(Box::new(move |tick| {
            if let Some(attacker) = tick
                .world
                .get_entity_by_id(id)
                .filter(|attacker| attacker.get_living_entity().is_some())
            {
                piglin_anger::piglin_was_hurt_by(tick, &attacker);
            }
        }));
    }

    fn post_tick(&self) {
        self.pick_up_nearby_items();
    }

    fn on_damage(
        &self,
        _damage_type: pumpkin_data::damage::DamageType,
        _source: Option<&dyn EntityBase>,
    ) {
        self.reset_ambient_sound_time();
        if self.mob_entity.living_entity.dead.load(Ordering::Relaxed) {
            self.drop_inventory();
        }
    }

    fn get_preferred_weapon_type(&self) -> Option<&'static tag::Tag> {
        (!self.is_baby()).then_some(&tag::Item::MINECRAFT_PIGLIN_PREFERRED_WEAPONS)
    }

    fn can_replace_current_item(
        &self,
        new_item: &ItemStack,
        current_item: &ItemStack,
        slot: &EquipmentSlot,
    ) -> bool {
        if current_item.get_enchantment_level(&Enchantment::BINDING_CURSE) > 0 {
            return false;
        }
        let preferred = self.get_preferred_weapon_type();
        let new_wanted = PiglinAi::is_loved_item(new_item)
            || preferred.is_some_and(|weapons| new_item.item.has_tag(weapons));
        let current_wanted = PiglinAi::is_loved_item(current_item)
            || preferred.is_some_and(|weapons| current_item.item.has_tag(weapons));
        new_wanted && !current_wanted
            || (new_wanted || !current_wanted)
                && mob_equipment::can_replace_current_item(
                    &self.mob_entity,
                    preferred,
                    new_item,
                    current_item,
                    slot,
                )
    }

    fn as_crossbow_attack_mob(&self) -> Option<&dyn CrossbowAttackMob> {
        Some(self)
    }

    fn get_base_experience_reward(&self) -> u32 {
        Self::XP_REWARD
    }
}

impl CrossbowAttackMob for PiglinEntity {
    fn set_charging_crossbow(&self, is_charging: bool) {
        self.set_charging_crossbow(is_charging);
    }

    /// Vanilla `Piglin.onCrossbowAttackPerformed`: a shot resets the idle time.
    fn on_crossbow_attack_performed(&self) {
        self.mob_entity
            .living_entity
            .no_action_time
            .store(0, Ordering::Relaxed);
    }

    fn is_charging_crossbow(&self) -> bool {
        self.is_charging_crossbow()
    }
}

impl AbstractPiglin for PiglinEntity {
    fn can_hunt(&self) -> bool {
        Self::can_hunt(self)
    }

    fn is_adult(&self) -> bool {
        Self::is_adult(self)
    }

    fn queued_changes(&self) -> &Mutex<Vec<QueuedChange>> {
        &self.queued
    }

    fn hunted_recently(&self) -> bool {
        self.hunted_recently.load(Ordering::Relaxed)
    }
}
