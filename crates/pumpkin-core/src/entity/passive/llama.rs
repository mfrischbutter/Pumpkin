use std::sync::{
    Arc, Weak,
    atomic::{AtomicBool, AtomicI32, AtomicU8, Ordering},
};

use crossbeam::atomic::AtomicCell;
use pumpkin_data::BlockState;
use pumpkin_data::entity::EntityType;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::tag::{self, Taggable};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_util::math::position::BlockPos;
use rand::RngExt;
use uuid::Uuid;

use crate::entity::ai::behavior::hunting::{self, Hunt};
use crate::entity::ai::target_match::TargetMatch;
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
        animal::{Animal, get_carpet_color_from_item},
        trader_llama,
    },
    player::Player,
    projectile::llama_spit::LlamaSpitEntity,
};
use crate::world::World;

/// Vanilla `Llama.setStrength`: strength stays within 1 to 5.
const MAX_STRENGTH: i32 = 5;

pub const FLAG_TAME: u8 = 2;
pub const FLAG_BRED: u8 = 8;
pub const FLAG_EATING: u8 = 16;
pub const FLAG_STANDING: u8 = 32;
pub const FLAG_OPEN_MOUTH: u8 = 64;

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
    pub carpet_color: AtomicCell<Option<u8>>,
    pub flags: AtomicU8,
    pub has_chest: AtomicBool,
    pub temper: AtomicI32,
    pub owner: AtomicCell<Option<Uuid>>,
    /// Vanilla `TraderLlama.despawnDelay`.
    pub despawn_delay: AtomicI32,
}

impl LlamaEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let mob_entity = MobEntity::new(entity);
        let variant = LlamaVariant::random_variant();
        let strength = random_strength();

        let llama = Self {
            mob_entity,
            ageable_data: AgeableData::default(),
            variant: AtomicI32::new(variant.id()),
            strength: AtomicI32::new(strength),
            carpet_color: AtomicCell::new(None),
            flags: AtomicU8::new(0),
            has_chest: AtomicBool::new(false),
            temper: AtomicI32::new(0),
            owner: AtomicCell::new(None),
            despawn_delay: AtomicI32::new(trader_llama::DEFAULT_DESPAWN_DELAY),
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

            // Vanilla `Llama.registerGoals`, without RunAroundLikeCrazyGoal and
            // LlamaFollowCaravanGoal yet.
            goal_selector.add_goal(0, Box::new(SwimGoal::default()));
            goal_selector.add_goal(
                3,
                Box::new(RangedAttackGoal::new(ranged_weak, 1.25, 40, 20.0)),
            );
            goal_selector.add_goal(3, EscapeDangerGoal::new(1.2));
            goal_selector.add_goal(4, BreedGoal::new(1.0));
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

            target_selector.add_goal(1, Box::new(RevengeGoal::new(true)));
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
    pub fn has_flag(&self, flag: u8) -> bool {
        (self.flags.load(Ordering::Relaxed) & flag) != 0
    }

    pub fn set_flag(&self, flag: u8, val: bool) {
        let current = self.flags.load(Ordering::Relaxed);
        let new_flags = if val { current | flag } else { current & !flag };
        self.flags.store(new_flags, Ordering::Relaxed);
        let entity = self.get_entity();
        entity.set_synced_data(
            pumpkin_data::tracked_data::llama::DATA_ID_FLAGS,
            new_flags as i8,
        );
    }

    #[must_use]
    pub fn is_tame(&self) -> bool {
        self.has_flag(FLAG_TAME)
    }

    pub fn set_tame(&self, val: bool) {
        self.set_flag(FLAG_TAME, val);
    }

    #[must_use]
    pub fn has_chest(&self) -> bool {
        self.has_chest.load(Ordering::Relaxed)
    }

    pub fn set_has_chest(&self, val: bool) {
        self.has_chest.store(val, Ordering::Relaxed);
        let entity = self.get_entity();
        entity.set_synced_data(pumpkin_data::tracked_data::llama::DATA_ID_CHEST, val);
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
    }
}

impl AgeableMob for LlamaEntity {
    fn get_ageable_data(&self) -> &AgeableData {
        &self.ageable_data
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
            .is_some_and(|llama| self.can_parent() && llama.can_parent())
    }

    /// Vanilla `Llama.getBreedOffspring`: up to the stronger parent's strength (one more
    /// with a 3% chance) and a parent's variant.
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

impl LlamaEntity {
    /// Vanilla `AbstractHorse.canParent`.
    fn can_parent(&self) -> bool {
        let entity = self.get_entity();
        let living = &self.mob_entity.living_entity;
        !entity.has_passengers()
            && !entity.has_vehicle()
            && self.is_tame()
            && !self.is_baby()
            && living.health.load() >= living.get_max_health()
            && self.mob_entity.is_in_love()
    }
}

impl Mob for LlamaEntity {
    fn is_tamed(&self) -> bool {
        self.is_tame()
    }

    /// Vanilla `Llama.finalizeSpawn`: a random strength and the group's variant.
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

    fn mob_play_step_sound(&self, _pos: &BlockPos, _state: &BlockState) {
        self.play_entity_sound(Sound::EntityLlamaStep, 0.15, 1.0);
    }

    fn as_ageable(&self) -> Option<&dyn AgeableMob> {
        Some(self)
    }

    fn as_animal(&self) -> Option<&dyn Animal> {
        Some(self)
    }

    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        self.write_ageable_nbt(nbt);
        nbt.put_int("Variant", self.get_variant().id());
        nbt.put_int("Strength", self.get_strength());
        nbt.put_bool("ChestedHorse", self.has_chest());
        nbt.put_bool("Tame", self.is_tame());
        nbt.put_int("Temper", self.temper.load(Ordering::Relaxed));
        if self.is_trader() {
            nbt.put_int("DespawnDelay", self.despawn_delay.load(Ordering::Relaxed));
        }
        if let Some(owner) = self.owner.load() {
            nbt.put_uuid("Owner", owner);
        }
    }

    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        self.read_ageable_nbt(nbt);
        if let Some(variant) = nbt.get_int("Variant") {
            self.set_variant(LlamaVariant::from_id(variant));
        }
        if let Some(strength) = nbt.get_int("Strength") {
            self.set_strength(strength);
        }
        if let Some(chested) = nbt.get_bool("ChestedHorse") {
            self.set_has_chest(chested);
        }
        if let Some(tame) = nbt.get_bool("Tame") {
            self.set_tame(tame);
        }
        if let Some(temper) = nbt.get_int("Temper") {
            self.temper.store(temper, Ordering::Relaxed);
        }
        if let Some(delay) = nbt.get_int("DespawnDelay") {
            self.despawn_delay.store(delay, Ordering::Relaxed);
        }
        if let Some(owner) = nbt.get_uuid("Owner") {
            self.owner.store(Some(owner));
        }
    }

    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    /// Vanilla `TraderLlama.aiStep`.
    fn mob_tick(&self, _caller: &dyn EntityBase) {
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
            self.flags.load(Ordering::Relaxed) as i8,
        );
    }

    fn mob_interact(&self, player: &Arc<Player>, item_stack: &mut ItemStack) -> bool {
        let item = item_stack.get_item();

        if self.is_tame() && item == &Item::CHEST && !self.has_chest() && !self.is_baby() {
            self.set_has_chest(true);
            item_stack.decrement_unless_creative(player.gamemode.load(), 1);
            let entity = self.get_entity();
            let world = entity.world.load();
            world.play_sound(
                Sound::EntityDonkeyChest,
                SoundCategory::Neutral,
                &entity.pos.load(),
            );
            return true;
        }

        if self.is_tame()
            && !self.is_baby()
            && let Some(color) = get_carpet_color_from_item(item)
        {
            self.carpet_color.store(Some(color));
            item_stack.decrement_unless_creative(player.gamemode.load(), 1);
            let entity = self.get_entity();
            let world = entity.world.load();
            world.play_sound(
                Sound::EntityLlamaSwag,
                SoundCategory::Neutral,
                &entity.pos.load(),
            );
            return true;
        }

        if !self.is_baby() && !self.is_food(item_stack) {
            // Vanilla `TraderLlama.doPlayerRide` refuses a llama leashed to its trader.
            if trader_llama::leashing_trader(self.get_entity()).is_some() {
                return true;
            }
            let world = player.world();
            let ent = &self.mob_entity.living_entity.entity;
            if let Some(vehicle) = world.get_entity_by_id(ent.entity_id)
                && let Some(passenger) = world.get_player_by_id(player.entity_id())
            {
                ent.add_passenger(vehicle, passenger as Arc<dyn EntityBase>);
                return true;
            }
        }

        self.animal_interact(player, item_stack, Sound::EntityLlamaAmbient)
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
