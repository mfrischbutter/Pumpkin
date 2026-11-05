use std::sync::{
    Arc, Weak,
    atomic::{AtomicBool, AtomicI32, Ordering},
};

use crossbeam::atomic::AtomicCell;
use pumpkin_data::BlockState;
use pumpkin_data::data_component::DataComponent;
use pumpkin_data::data_component_impl::{DataComponentImpl, LlamaVariantImpl};
use pumpkin_data::entity::EntityType;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::Sound;
use pumpkin_data::tag::{self, Taggable};
use pumpkin_inventory::mount_screen_handler::MountScreenHandler;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_util::math::position::BlockPos;
use rand::RngExt;
use uuid::Uuid;

use crate::entity::ai::behavior::hunting::{self, Hunt};
use crate::entity::ai::goal::llama_follow_caravan::LlamaFollowCaravanGoal;
use crate::entity::ai::goal::run_around_like_crazy::RunAroundLikeCrazyGoal;
use crate::entity::ai::target_match::TargetMatch;
use crate::entity::custom_sound::CustomSound;
use crate::entity::passive::tamable::is_untamed;
use crate::entity::{
    Entity, EntityBase,
    ageable::{AgeableData, AgeableMob},
    ai::goal::{
        breed::BreedGoal, escape_danger::EscapeDangerGoal, follow_parent::FollowParentGoal,
        look_around::RandomLookAroundGoal, look_at_entity::LookAtEntityGoal,
        ranged_attack::RangedAttackGoal, revenge::RevengeGoal, swim::SwimGoal, tempt::TemptGoal,
        wander_around::WanderAroundGoal,
    },
    mob::{
        Mob, MobEntity, RangedAttackMob,
        spawn::{EntitySpawnReason, SpawnGroupData},
    },
    passive::{
        abstract_horse::{self as horse_base, AbstractHorse, HorseData},
        animal::Animal,
        horse_food::FoodEffect,
        horse_inventory, trader_llama,
    },
    player::Player,
    projectile::llama_spit::LlamaSpitEntity,
};
use crate::world::World;

/// Vanilla `Llama.setStrength`: strength stays within 1 to 5.
const MAX_STRENGTH: i32 = 5;
/// Vanilla `Llama.getMaxTemper`.
const LLAMA_MAX_TEMPER: i32 = 30;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(i32)]
pub enum LlamaVariant {
    #[default]
    Creamy = 0,
    White = 1,
    Brown = 2,
    Gray = 3,
}

impl LlamaVariant {
    #[must_use]
    pub const fn from_id(id: i32) -> Self {
        match id {
            1 => Self::White,
            2 => Self::Brown,
            3 => Self::Gray,
            _ => Self::Creamy,
        }
    }

    #[must_use]
    pub const fn id(self) -> i32 {
        self as i32
    }

    /// The name the `llama/variant` component uses.
    #[must_use]
    pub const fn name(self) -> &'static str {
        pumpkin_data::entity_variant::LlamaVariant::by_id(self.id()).name()
    }

    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        pumpkin_data::entity_variant::LlamaVariant::from_name(name)
            .map(|variant| Self::from_id(variant.id()))
    }

    #[must_use]
    pub fn random_variant() -> Self {
        let mut rng = rand::rng();
        match rng.random_range(0..4) {
            1 => Self::White,
            2 => Self::Brown,
            3 => Self::Gray,
            _ => Self::Creamy,
        }
    }
}

/// Vanilla `Llama.registerGoals`: the speed in a caravan.
const FOLLOW_CARAVAN_SPEED: f64 = 2.1;
/// Vanilla `Llama.getFollowLeashSpeed`.
const FOLLOW_LEASH_SPEED: f64 = 2.0;
/// Vanilla `Llama`'s `setRequiredPathLength`.
const REQUIRED_PATH_LENGTH: f32 = 40.0;

/// Vanilla `Llama.setRandomStrength`: 1 to 3, or 1 to 5 with a 4% chance.
fn random_strength() -> i32 {
    let mut rng = rand::rng();
    let max_strength = if rng.random::<f32>() < 0.04 { 5 } else { 3 };
    1 + rng.random_range(0..max_strength)
}

/// Vanilla `Wolf.avoidLlama`: the stronger the llama, the likelier a wolf flees.
pub fn scares_wolves(threat: &dyn EntityBase) -> bool {
    let strength = threat
        .cast_any()
        .downcast_ref::<LlamaEntity>()
        .map(LlamaEntity::get_strength);
    strength.is_some_and(|strength| strength >= rand::rng().random_range(0..5))
}

/// Vanilla `LlamaAttackWolfGoal`: untamed wolves within a quarter of the follow range.
pub const LLAMA_HUNTS: &[Hunt] = &[Hunt::new(2, TargetMatch::Type(&EntityType::WOLF), false)
    .every(16)
    .only_mobs(is_untamed)
    .range_scale(0.25)];

pub struct LlamaEntity {
    pub mob_entity: MobEntity,
    pub ageable_data: AgeableData,
    pub variant: AtomicI32,
    pub strength: AtomicI32,
    pub has_chest: AtomicBool,
    horse: HorseData,
    /// Vanilla `TraderLlama.despawnDelay`.
    pub despawn_delay: AtomicI32,
    /// Vanilla `Llama.didSpit`: a spit drops the revenge target once.
    did_spit: AtomicBool,
    /// Vanilla `Llama.caravanHead`, by entity id.
    caravan_head: AtomicCell<Option<i32>>,
    /// Vanilla `Llama.caravanTail`, by entity id.
    caravan_tail: AtomicCell<Option<i32>>,
}

/// Vanilla `Llama`, also as a trader llama: this one entity type stands for both.
#[must_use]
pub fn as_llama(entity: &dyn EntityBase) -> Option<&LlamaEntity> {
    entity.cast_any().downcast_ref::<LlamaEntity>()
}

impl LlamaEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let mut mob_entity = MobEntity::new(entity);
        // Vanilla `Llama`: a caravan follows its head over longer paths.
        mob_entity
            .navigator
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .set_required_path_length(REQUIRED_PATH_LENGTH);
        let variant = LlamaVariant::random_variant();
        let strength = random_strength();

        let llama = Self {
            mob_entity,
            ageable_data: AgeableData::default(),
            variant: AtomicI32::new(variant.id()),
            strength: AtomicI32::new(strength),
            has_chest: AtomicBool::new(false),
            horse: HorseData::default(),
            despawn_delay: AtomicI32::new(trader_llama::DEFAULT_DESPAWN_DELAY),
            did_spit: AtomicBool::new(false),
            caravan_head: AtomicCell::new(None),
            caravan_tail: AtomicCell::new(None),
        };
        let mob_arc = Arc::new(llama);
        let mob_weak: Weak<dyn Mob> = {
            let mob_arc: Arc<dyn Mob> = mob_arc.clone();
            Arc::downgrade(&mob_arc)
        };
        let ranged_weak: Weak<dyn RangedAttackMob> = {
            let ranged_arc: Arc<dyn RangedAttackMob> = mob_arc.clone();
            Arc::downgrade(&ranged_arc)
        };

        {
            let goal_selector = &mob_arc.mob_entity.goals_selector;
            let target_selector = &mob_arc.mob_entity.target_selector;

            // Vanilla `Llama.registerGoals`.
            goal_selector.add_goal(0, Box::new(SwimGoal::default()));
            goal_selector.add_goal(1, Box::new(RunAroundLikeCrazyGoal::new(1.2)));
            goal_selector.add_goal(
                2,
                Box::new(LlamaFollowCaravanGoal::new(FOLLOW_CARAVAN_SPEED)),
            );
            goal_selector.add_goal(
                3,
                Box::new(RangedAttackGoal::new(ranged_weak, 1.25, 40, 20.0)),
            );
            goal_selector.add_goal(3, EscapeDangerGoal::new(1.2));
            // Vanilla `BreedGoal` looks for its own class: a llama also finds trader llamas.
            let breed = if mob_arc.is_trader() {
                BreedGoal::new(1.0)
            } else {
                BreedGoal::with_partners(1.0, TargetMatch::LLAMA_LIKE)
            };
            goal_selector.add_goal(4, breed);
            goal_selector.add_goal(
                5,
                Box::new(TemptGoal::with_tag(
                    1.25,
                    &tag::Item::MINECRAFT_LLAMA_TEMPT_ITEMS,
                    false,
                )),
            );
            goal_selector.add_goal(6, Box::new(FollowParentGoal::new(1.0)));
            goal_selector.add_goal(7, Box::new(WanderAroundGoal::water_avoiding(0.7)));
            goal_selector.add_goal(
                8,
                LookAtEntityGoal::with_default(mob_weak, &EntityType::PLAYER, 6.0),
            );
            goal_selector.add_goal(9, Box::new(RandomLookAroundGoal::default()));

            target_selector.add_goal(
                1,
                Box::new(RevengeGoal::new(true).continuing_while(|mob| {
                    as_llama(mob).is_none_or(|llama| !llama.did_spit.swap(false, Ordering::Relaxed))
                })),
            );
            hunting::apply(target_selector, &mob_arc.mob_entity, LLAMA_HUNTS);
            if mob_arc.is_trader() {
                trader_llama::register_goals(&mob_arc.mob_entity);
            }
        };

        mob_arc
    }

    /// Vanilla `isTraderLlama`.
    #[must_use]
    pub fn is_trader(&self) -> bool {
        self.get_entity().entity_type == &EntityType::TRADER_LLAMA
    }

    #[must_use]
    pub fn get_variant(&self) -> LlamaVariant {
        LlamaVariant::from_id(self.variant.load(Ordering::Relaxed))
    }

    pub fn set_variant(&self, variant: LlamaVariant) {
        self.variant.store(variant.id(), Ordering::Relaxed);
        let entity = self.get_entity();
        entity.set_synced_data(
            pumpkin_data::tracked_data::llama::DATA_VARIANT_ID,
            VarInt(variant.id()),
        );
    }

    #[must_use]
    pub fn get_strength(&self) -> i32 {
        self.strength.load(Ordering::Relaxed)
    }

    pub fn set_strength(&self, strength: i32) {
        let strength = strength.clamp(1, MAX_STRENGTH);
        self.strength.store(strength, Ordering::Relaxed);
        let entity = self.get_entity();
        entity.set_synced_data(
            pumpkin_data::tracked_data::llama::DATA_STRENGTH_ID,
            VarInt(strength),
        );
    }

    #[must_use]
    pub fn is_tame(&self) -> bool {
        horse_base::is_tamed(self)
    }

    pub fn set_tame(&self, val: bool) {
        horse_base::set_flag(self, horse_base::FLAG_TAME, val);
    }

    pub fn set_has_chest(&self, val: bool) {
        self.has_chest.store(val, Ordering::Relaxed);
        let entity = self.get_entity();
        entity.set_synced_data(pumpkin_data::tracked_data::llama::DATA_ID_CHEST, val);
    }

    /// Vanilla `equipChest` with `Llama.playChestEquipsSound`.
    fn equip_chest(&self, player: &Player, stack: &mut ItemStack) {
        self.set_has_chest(true);
        let pitch = {
            let mut rng = self.get_random();
            (rng.random::<f32>() - rng.random::<f32>()).mul_add(0.2, 1.0)
        };
        self.play_entity_sound(Sound::EntityLlamaChest, 1.0, pitch);
        stack.decrement_unless_creative(player.gamemode.load(), 1);
        horse_inventory::create_inventory(self);
    }

    /// Vanilla `inCaravan`.
    pub fn in_caravan(&self) -> bool {
        self.caravan_head.load().is_some()
    }

    /// Vanilla `hasCaravanTail`.
    #[must_use]
    pub fn has_caravan_tail(&self) -> bool {
        self.caravan_tail.load().is_some()
    }

    /// Vanilla `getCaravanHead`.
    #[must_use]
    pub fn get_caravan_head(&self) -> Option<Arc<dyn EntityBase>> {
        let head = self.caravan_head.load()?;
        self.get_entity().world.load().get_entity_by_id(head)
    }

    /// Vanilla `joinCaravan`.
    pub fn join_caravan(&self, head: &Self) {
        self.caravan_head.store(Some(head.get_entity().entity_id));
        head.caravan_tail.store(Some(self.get_entity().entity_id));
    }

    /// Vanilla `leaveCaravan`.
    pub fn leave_caravan(&self) {
        if let Some(head) = self.get_caravan_head()
            && let Some(head) = as_llama(head.as_ref())
        {
            head.caravan_tail.store(None);
        }
        self.caravan_head.store(None);
    }

    pub fn spit(&self, target: &Arc<dyn EntityBase>) {
        let entity = self.get_entity();
        let world = entity.world.load_full();

        let spit_entity = Entity::new(world.clone(), entity.pos.load(), &EntityType::LLAMA_SPIT);
        let spit = LlamaSpitEntity::new_shot(spit_entity, entity);

        let mob_pos = entity.pos.load();
        let target_entity = target.get_entity();
        let target_pos = target_entity.pos.load();
        let target_height = f64::from(target_entity.entity_dimension.load().height);

        let dx = target_pos.x - mob_pos.x;
        let dy = (target_pos.y + target_height / 3.0) - spit.get_entity().pos.load().y;
        let dz = target_pos.z - mob_pos.z;
        let horizontal_distance = dx.hypot(dz);
        let yo = horizontal_distance * 0.2;

        spit.thrown.set_velocity(dx, dy + yo, dz, 1.5, 10.0);

        let spit_arc: Arc<dyn EntityBase> = Arc::new(spit);
        world.spawn_entity(spit_arc);

        let mut rng = self.get_random();
        let pitch = (rng.random::<f32>() - rng.random::<f32>()).mul_add(0.2, 1.0);
        self.play_entity_sound(Sound::EntityLlamaSpit, 1.0, pitch);
        self.did_spit.store(true, Ordering::Relaxed);
    }
}

impl AgeableMob for LlamaEntity {
    fn get_ageable_data(&self) -> &AgeableData {
        &self.ageable_data
    }

    fn age_boundary_reached(&self) {
        horse_base::refresh_age_dimensions(self);
    }
}

impl Animal for LlamaEntity {
    fn is_food(&self, item_stack: &ItemStack) -> bool {
        item_stack.item.has_tag(&tag::Item::MINECRAFT_LLAMA_FOOD)
    }

    /// Vanilla `Llama.canMate` with `AbstractHorse.canParent`: two tame, grown, unmounted
    /// llamas at full health.
    fn can_mate(&self, partner: &dyn EntityBase) -> bool {
        partner
            .cast_any()
            .downcast_ref::<Self>()
            .is_some_and(|llama| horse_base::can_parent(self) && horse_base::can_parent(llama))
    }

    /// Vanilla `Llama.getBreedOffspring`: health, jump and speed from both parents, up to the
    /// stronger parent's strength (one more with a 3% chance) and a parent's variant.
    fn get_breed_offspring(&self, partner: &dyn EntityBase) -> Arc<dyn EntityBase> {
        let entity = self.get_entity();
        let baby = Self::new(Entity::new(
            entity.world.load_full(),
            entity.pos.load(),
            entity.entity_type,
        ));
        // Vanilla `TraderLlama.makeNewLlama`: a bred trader llama stays loaded.
        if self.is_trader() {
            baby.mob_entity
                .persistence_required
                .store(true, Ordering::Relaxed);
        }
        horse_base::set_offspring_attributes(self, partner, baby.as_ref());
        if let Some(partner) = partner.cast_any().downcast_ref::<Self>() {
            let mut rng = self.get_random();
            let stronger = self.get_strength().max(partner.get_strength());
            let mut strength = rng.random_range(0..stronger.max(1)) + 1;
            if rng.random::<f32>() < 0.03 {
                strength += 1;
            }
            baby.set_strength(strength);
            let parent = if rng.random::<bool>() { self } else { partner };
            baby.set_variant(parent.get_variant());
        }
        baby
    }
}

impl AbstractHorse for LlamaEntity {
    fn horse_data(&self) -> &HorseData {
        &self.horse
    }

    fn horse_eating_sound(&self) -> Option<Sound> {
        Some(Sound::EntityLlamaEat)
    }

    fn horse_angry_sound(&self) -> Option<Sound> {
        Some(Sound::EntityLlamaAngry)
    }

    fn can_perform_rearing(&self) -> bool {
        false
    }

    fn max_temper(&self) -> i32 {
        LLAMA_MAX_TEMPER
    }

    fn can_eat_grass(&self) -> bool {
        false
    }

    /// Vanilla `Llama.handleEating`'s table.
    fn food_effect(&self, item: &Item) -> FoodEffect {
        if item == &Item::WHEAT {
            FoodEffect {
                heal: 2.0,
                age_up: 10,
                temper: 3,
                love: false,
            }
        } else if item == &Item::HAY_BLOCK {
            FoodEffect {
                heal: 10.0,
                age_up: 90,
                temper: 6,
                love: true,
            }
        } else {
            FoodEffect::default()
        }
    }

    fn opens_mouth_to_eat(&self) -> bool {
        false
    }

    /// Vanilla `Llama.followMommy`.
    fn can_follow_mommy(&self) -> bool {
        !self.in_caravan()
    }

    /// Vanilla `TraderLlama.doPlayerRide` refuses a llama leashed to its trader.
    fn can_player_ride(&self) -> bool {
        !self.is_trader() || trader_llama::leashing_trader(self.get_entity()).is_none()
    }

    fn has_chest(&self) -> bool {
        self.has_chest.load(Ordering::Relaxed)
    }

    /// Vanilla `Llama.getInventoryColumns`.
    fn inventory_columns(&self) -> i32 {
        if self.has_chest() {
            self.get_strength()
        } else {
            0
        }
    }

    /// A llama's slot is the carpet's; it takes no saddle.
    fn bedrock_equip_slot(&self) -> usize {
        MountScreenHandler::SLOT_BODY_ARMOR
    }
}

impl CustomSound for LlamaEntity {
    fn hurt_sound(&self) -> Option<Sound> {
        Some(Sound::EntityLlamaHurt)
    }

    fn death_sound(&self) -> Option<Sound> {
        Some(Sound::EntityLlamaDeath)
    }

    fn sound_volume(&self) -> f32 {
        horse_base::SOUND_VOLUME
    }
}

impl Mob for LlamaEntity {
    /// Vanilla `AbstractHorse.isPushable`.
    fn mob_is_pushable(&self) -> bool {
        !self.get_entity().has_passengers()
    }

    /// Vanilla `Llama.getFollowLeashSpeed`.
    fn follow_leash_speed(&self) -> f64 {
        FOLLOW_LEASH_SPEED
    }

    /// Vanilla `Llama.tickLeash`: the caravan head stays in the same island.
    fn tick_links(&self) -> Vec<i32> {
        self.caravan_head.load().into_iter().collect()
    }

    fn is_tamed(&self) -> bool {
        self.is_tame()
    }

    /// Vanilla `AbstractHorse.getOwnerReference`.
    fn get_owner_uuid(&self) -> Option<Uuid> {
        horse_base::owner(self)
    }

    /// Vanilla `Llama.isImmobile`: no rearing, so only eating or dying holds it still.
    fn is_mob_immobile(&self) -> bool {
        self.mob_entity.living_entity.is_dead_or_dying() || horse_base::is_eating(self)
    }

    fn on_elastic_leash_pull(&self) {
        horse_base::on_leash_pull(self);
    }

    /// Vanilla `Llama.finalizeSpawn` with `AbstractHorse.finalizeSpawn`: a random strength,
    /// the group's variant and, as for every chested horse, a random health.
    fn finalize_spawn(
        &self,
        _world: &Arc<World>,
        group_data: Option<SpawnGroupData>,
    ) -> Option<SpawnGroupData> {
        self.set_strength(random_strength());
        let (variant, group_size) = match group_data {
            Some(SpawnGroupData::Llama {
                variant,
                group_size,
            }) => (variant, group_size),
            _ => (LlamaVariant::random_variant(), 0),
        };
        self.set_variant(variant);
        horse_base::randomize_max_health(self);
        // `LlamaGroupData` is ageable group data that spawns babies after the first llama.
        self.finalize_ageable_spawn(Some(SpawnGroupData::Ageable {
            group_size,
            // Vanilla `TraderLlama.finalizeSpawn`: no babies with a trader.
            should_spawn_baby: !self.is_trader(),
            baby_spawn_chance: self.default_baby_spawn_chance(),
        }));
        Some(SpawnGroupData::Llama {
            variant,
            group_size: group_size + 1,
        })
    }

    fn get_ambient_sound(&self) -> Option<Sound> {
        Some(Sound::EntityLlamaAmbient)
    }

    fn get_ambient_sound_interval(&self) -> i32 {
        horse_base::AMBIENT_SOUND_INTERVAL
    }

    fn as_custom_sound(&self) -> Option<&dyn CustomSound> {
        Some(self)
    }

    /// Vanilla `Llama.get(LLAMA_VARIANT)`.
    fn mob_get_component(&self, component: DataComponent) -> Option<Box<dyn DataComponentImpl>> {
        (component == DataComponent::LlamaVariant).then(|| {
            Box::new(LlamaVariantImpl {
                value: self.get_variant().name().into(),
            }) as Box<dyn DataComponentImpl>
        })
    }

    /// Vanilla `Llama.applyImplicitComponent(LLAMA_VARIANT)`.
    fn mob_set_variant_name(&self, name: &str) {
        if let Some(variant) = LlamaVariant::from_name(name) {
            self.set_variant(variant);
        }
    }

    fn mob_play_step_sound(&self, _pos: &BlockPos, _state: &BlockState) {
        self.play_entity_sound(Sound::EntityLlamaStep, 0.15, 1.0);
    }

    fn as_ageable(&self) -> Option<&dyn AgeableMob> {
        Some(self)
    }

    fn as_animal(&self) -> Option<&dyn Animal> {
        Some(self)
    }

    fn as_abstract_horse(&self) -> Option<&dyn AbstractHorse> {
        Some(self)
    }

    /// Vanilla `AbstractChestedHorse.dropEquipment`.
    fn custom_death_loot(&self) -> Vec<ItemStack> {
        horse_inventory::inventory_drops(self)
    }

    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        self.write_ageable_nbt(nbt);
        nbt.put_int("Variant", self.get_variant().id());
        nbt.put_int("Strength", self.get_strength());
        horse_base::write_horse_nbt(self, nbt);
        horse_inventory::write_chest_nbt(self, nbt);
        if self.is_trader() {
            nbt.put_int("DespawnDelay", self.despawn_delay.load(Ordering::Relaxed));
        }
    }

    /// Vanilla `Llama.readAdditionalSaveData`: the strength first, so the chest gets its
    /// columns; a missing one is 1 and a missing variant creamy.
    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        self.read_ageable_nbt(nbt);
        self.set_strength(nbt.get_int("Strength").unwrap_or(0));
        self.set_variant(LlamaVariant::from_id(nbt.get_int("Variant").unwrap_or(0)));
        horse_base::read_horse_nbt(self, nbt);
        self.set_has_chest(nbt.get_bool("ChestedHorse").unwrap_or(false));
        horse_inventory::read_chest_items(self, nbt);
        if let Some(delay) = nbt.get_int("DespawnDelay") {
            self.despawn_delay.store(delay, Ordering::Relaxed);
        }
    }

    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    /// Vanilla `AbstractHorse.tick` and `aiStep` with `TraderLlama.aiStep`.
    fn mob_tick(&self, _caller: &dyn EntityBase) {
        horse_base::horse_tick(self);
        horse_base::horse_ai_step(self);
        if self.is_trader() {
            trader_llama::maybe_despawn(self);
        }
    }

    /// Vanilla `TraderLlama.finalizeSpawn`: one that comes with a trader is grown.
    fn after_finalize_spawn(
        &self,
        _world: &Arc<World>,
        reason: EntitySpawnReason,
        group_data: Option<SpawnGroupData>,
    ) -> Option<SpawnGroupData> {
        if self.is_trader() && reason == EntitySpawnReason::Event {
            self.set_age(0);
        }
        group_data
    }

    fn mob_init_data_tracker(&self) {
        let entity = self.get_entity();
        let is_baby = entity.age.load(Ordering::Relaxed) < 0;
        if is_baby {
            entity.set_synced_data(pumpkin_data::tracked_data::llama::DATA_BABY_ID, true);
        }
        entity.set_synced_data(
            pumpkin_data::tracked_data::llama::DATA_VARIANT_ID,
            VarInt(self.get_variant().id()),
        );
        entity.set_synced_data(
            pumpkin_data::tracked_data::llama::DATA_STRENGTH_ID,
            VarInt(self.get_strength()),
        );
        entity.set_synced_data(
            pumpkin_data::tracked_data::llama::DATA_ID_CHEST,
            self.has_chest(),
        );
        entity.set_synced_data(
            pumpkin_data::tracked_data::llama::DATA_ID_FLAGS,
            horse_base::flags(self) as i8,
        );
    }

    /// Vanilla `AbstractChestedHorse.mobInteract` with `AbstractHorse.mobInteract`: a wild
    /// llama can be mounted to tame it, a tame one takes a chest and a carpet.
    fn mob_interact(&self, player: &Arc<Player>, item_stack: &mut ItemStack) -> bool {
        horse_base::horse_interact(self, player, item_stack, |stack| {
            if !self.has_chest() && stack.item == &Item::CHEST {
                self.equip_chest(player, stack);
                return true;
            }
            false
        })
    }
}

impl RangedAttackMob for LlamaEntity {
    fn perform_ranged_attack(&self, target: &Arc<dyn EntityBase>, _power: f32) {
        self.spit(target);
    }
}

#[cfg(test)]
mod tests {
    use super::random_strength;

    #[test]
    fn random_strength_stays_within_vanilla_bounds() {
        for _ in 0..1000 {
            assert!((1..=5).contains(&random_strength()));
        }
    }
}
