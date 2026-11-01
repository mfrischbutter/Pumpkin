use std::sync::{
    Arc, Weak,
    atomic::{AtomicBool, AtomicI32, Ordering},
};

use crate::entity::bucketable::{
    Bucketable, bucket_mob_pickup, load_age_from_bucket_tag, save_age_to_bucket_tag,
};
use crate::entity::mob::requires_custom_persistence_base;
use crate::world::level_accessor::LevelAccessor;
use pumpkin_data::entity::EntityType;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::Sound;
use pumpkin_data::tag::{self, Taggable};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_protocol::codec::var_int::VarInt;
use rand::RngExt;

use crate::entity::{
    Entity, EntityBase,
    ageable::{AgeableData, AgeableMob},
    ai::goal::{
        active_target::ActiveTargetGoal, breed::BreedGoal, escape_danger::EscapeDangerGoal,
        follow_parent::FollowParentGoal, look_around::RandomLookAroundGoal,
        look_at_entity::LookAtEntityGoal, melee_attack::MeleeAttackGoal, revenge::RevengeGoal,
        swim::SwimGoal, tempt::TemptGoal, try_find_water::TryFindWaterGoal,
        wander_around::WanderAroundGoal,
    },
    mob::{Mob, MobEntity},
    passive::animal::Animal,
    player::Player,
};

const TEMPT_ITEMS: &[&Item] = &[&Item::TROPICAL_FISH_BUCKET];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(i32)]
pub enum AxolotlVariant {
    #[default]
    Lucy = 0,
    Wild = 1,
    Gold = 2,
    Cyan = 3,
    Blue = 4,
}

impl AxolotlVariant {
    #[must_use]
    pub const fn from_id(id: i32) -> Self {
        match id {
            1 => Self::Wild,
            2 => Self::Gold,
            3 => Self::Cyan,
            4 => Self::Blue,
            _ => Self::Lucy,
        }
    }

    #[must_use]
    pub const fn id(self) -> i32 {
        self as i32
    }

    #[must_use]
    pub fn random_variant() -> Self {
        let mut rng = rand::rng();
        if rng.random_range(0..1200) == 0 {
            Self::Blue
        } else {
            match rng.random_range(0..4) {
                1 => Self::Wild,
                2 => Self::Gold,
                3 => Self::Cyan,
                _ => Self::Lucy,
            }
        }
    }
}

pub struct AxolotlEntity {
    pub mob_entity: MobEntity,
    pub ageable_data: AgeableData,
    pub variant: AtomicI32,
    pub playing_dead: AtomicBool,
    pub from_bucket: AtomicBool,
    pub play_dead_ticks: AtomicI32,
}

impl AxolotlEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let mob_entity = MobEntity::new(entity);
        let variant = AxolotlVariant::random_variant();
        let axolotl = Self {
            mob_entity,
            ageable_data: AgeableData::default(),
            variant: AtomicI32::new(variant.id()),
            playing_dead: AtomicBool::new(false),
            from_bucket: AtomicBool::new(false),
            play_dead_ticks: AtomicI32::new(0),
        };
        let mob_arc = Arc::new(axolotl);
        let mob_weak: Weak<dyn Mob> = {
            let mob_arc: Arc<dyn Mob> = mob_arc.clone();
            Arc::downgrade(&mob_arc)
        };

        {
            let goal_selector = &mob_arc.mob_entity.goals_selector;

            goal_selector.add_goal(0, Box::new(TryFindWaterGoal));
            goal_selector.add_goal(0, Box::new(SwimGoal::default()));
            goal_selector.add_goal(1, EscapeDangerGoal::new(1.5));
            goal_selector.add_goal(2, BreedGoal::new(1.0));
            goal_selector.add_goal(3, Box::new(TemptGoal::new(1.25, TEMPT_ITEMS, false)));
            goal_selector.add_goal(4, Box::new(FollowParentGoal::new(1.25)));
            goal_selector.add_goal(5, Box::new(MeleeAttackGoal::new(1.2, false)));
            goal_selector.add_goal(6, Box::new(WanderAroundGoal::new(1.0)));
            goal_selector.add_goal(
                7,
                LookAtEntityGoal::with_default(mob_weak, &EntityType::PLAYER, 6.0),
            );
            goal_selector.add_goal(8, Box::new(RandomLookAroundGoal::default()));
        };

        {
            let target_selector = &mob_arc.mob_entity.target_selector;

            target_selector.add_goal(1, Box::new(RevengeGoal::new(true)));
            target_selector.add_goal(
                2,
                ActiveTargetGoal::with_default(&mob_arc.mob_entity, &EntityType::DROWNED, false),
            );
            target_selector.add_goal(
                2,
                ActiveTargetGoal::with_default(&mob_arc.mob_entity, &EntityType::GUARDIAN, false),
            );
            target_selector.add_goal(
                2,
                ActiveTargetGoal::with_default(
                    &mob_arc.mob_entity,
                    &EntityType::ELDER_GUARDIAN,
                    false,
                ),
            );
            target_selector.add_goal(
                3,
                ActiveTargetGoal::with_default(&mob_arc.mob_entity, &EntityType::SQUID, false),
            );
            target_selector.add_goal(
                3,
                ActiveTargetGoal::with_default(&mob_arc.mob_entity, &EntityType::GLOW_SQUID, false),
            );
            target_selector.add_goal(
                3,
                ActiveTargetGoal::with_default(&mob_arc.mob_entity, &EntityType::COD, false),
            );
            target_selector.add_goal(
                3,
                ActiveTargetGoal::with_default(&mob_arc.mob_entity, &EntityType::SALMON, false),
            );
            target_selector.add_goal(
                3,
                ActiveTargetGoal::with_default(
                    &mob_arc.mob_entity,
                    &EntityType::TROPICAL_FISH,
                    false,
                ),
            );
        };

        mob_arc
    }

    #[must_use]
    pub fn get_variant(&self) -> AxolotlVariant {
        AxolotlVariant::from_id(self.variant.load(Ordering::Relaxed))
    }

    pub fn set_variant(&self, variant: AxolotlVariant) {
        self.variant.store(variant.id(), Ordering::Relaxed);
        let entity = self.get_entity();
        entity.set_synced_data(
            pumpkin_data::tracked_data::axolotl::DATA_VARIANT,
            VarInt(variant.id()),
        );
    }

    #[must_use]
    pub fn is_playing_dead(&self) -> bool {
        self.playing_dead.load(Ordering::Relaxed)
    }

    pub fn set_playing_dead(&self, playing_dead: bool) {
        self.playing_dead.store(playing_dead, Ordering::Relaxed);
        let entity = self.get_entity();
        entity.set_synced_data(
            pumpkin_data::tracked_data::axolotl::DATA_PLAYING_DEAD,
            playing_dead,
        );
    }

    #[must_use]
    pub fn is_from_bucket(&self) -> bool {
        self.from_bucket.load(Ordering::Relaxed)
    }

    pub fn set_from_bucket(&self, from_bucket: bool) {
        self.from_bucket.store(from_bucket, Ordering::Relaxed);
        let entity = self.get_entity();
        entity.set_synced_data(
            pumpkin_data::tracked_data::axolotl::FROM_BUCKET,
            from_bucket,
        );
    }
}

/// Vanilla `Axolotl.useRareVariant`: one bred baby in this many takes the rare variant.
const RARE_VARIANT_ODDS: i32 = 1200;

impl Bucketable for AxolotlEntity {
    fn is_from_bucket(&self) -> bool {
        self.from_bucket.load(Ordering::Relaxed)
    }

    fn set_from_bucket(&self, from_bucket: bool) {
        Self::set_from_bucket(self, from_bucket);
    }

    fn bucket_item(&self) -> &'static Item {
        &Item::AXOLOTL_BUCKET
    }

    /// Vanilla `Axolotl.getPickupSound`.
    fn pickup_sound(&self) -> Sound {
        Sound::ItemBucketFillAxolotl
    }

    /// Vanilla `Axolotl.saveToBucketTag`: the age and its lock go along (the hunting cooldown
    /// is the brain's).
    fn save_to_bucket_tag(&self, bucket: &mut ItemStack) {
        self.save_default_data_to_bucket_tag(bucket);
        save_age_to_bucket_tag(self, bucket);
    }

    /// Vanilla `Axolotl.loadFromBucketTag`.
    fn load_from_bucket_tag(&self, nbt: &NbtCompound) {
        self.load_default_data_from_bucket_tag(nbt);
        load_age_from_bucket_tag(self, nbt);
    }
}

impl AgeableMob for AxolotlEntity {
    fn get_ageable_data(&self) -> &AgeableData {
        &self.ageable_data
    }
}

impl Animal for AxolotlEntity {
    fn is_food(&self, item_stack: &ItemStack) -> bool {
        item_stack.item.has_tag(&tag::Item::MINECRAFT_AXOLOTL_FOOD)
            || item_stack.item == &Item::TROPICAL_FISH_BUCKET
            || item_stack.item == &Item::TROPICAL_FISH
    }

    /// Vanilla `Axolotl.getBreedOffspring`: one baby in 1200 takes the rare blue variant, the
    /// others one parent's; a bred baby stays loaded.
    fn get_breed_offspring(&self, partner: &dyn EntityBase) -> Arc<dyn EntityBase> {
        let entity = self.get_entity();
        let baby = Self::new(Entity::new(
            entity.world.load_full(),
            entity.pos.load(),
            &EntityType::AXOLOTL,
        ));
        let mut rng = rand::rng();
        let variant = if rng.random_range(0..RARE_VARIANT_ODDS) == 0 {
            AxolotlVariant::Blue
        } else if rng.random::<bool>() {
            self.get_variant()
        } else {
            partner
                .cast_any()
                .downcast_ref::<Self>()
                .map_or_else(|| self.get_variant(), Self::get_variant)
        };
        baby.set_variant(variant);
        baby.get_mob_entity()
            .persistence_required
            .store(true, Ordering::Relaxed);
        baby
    }
}

impl Mob for AxolotlEntity {
    /// Vanilla `Axolotl.getAmbientSound`.
    fn get_ambient_sound(&self) -> Option<pumpkin_data::sound::Sound> {
        Some(if self.get_entity().is_in_water() {
            pumpkin_data::sound::Sound::EntityAxolotlIdleWater
        } else {
            pumpkin_data::sound::Sound::EntityAxolotlIdleAir
        })
    }

    /// Vanilla `Axolotl.checkSpawnObstruction`: only entities obstruct, the water does not.
    fn check_spawn_obstruction(&self, level: &dyn LevelAccessor) -> bool {
        level.is_unobstructed(&self.get_mob_entity().living_entity.entity)
    }

    /// Vanilla `Axolotl.removeWhenFarAway`: only wild, unnamed axolotls despawn.
    fn remove_when_far_away(&self, _distance_sq: f64) -> bool {
        !self.is_from_bucket() && self.get_entity().custom_name.load().is_none()
    }

    /// Vanilla `Axolotl.requiresCustomPersistence`: a bucketed axolotl stays.
    fn requires_custom_persistence(&self) -> bool {
        requires_custom_persistence_base(self) || self.is_from_bucket()
    }

    fn as_ageable(&self) -> Option<&dyn AgeableMob> {
        Some(self)
    }

    fn as_animal(&self) -> Option<&dyn Animal> {
        Some(self)
    }

    fn as_bucketable(&self) -> Option<&dyn Bucketable> {
        Some(self)
    }

    /// Vanilla `Axolotl.mobInteract`: the water bucket picks it up first.
    fn mob_interact_with_hand(
        &self,
        player: &Arc<Player>,
        item_stack: &mut ItemStack,
        _hand: pumpkin_util::Hand,
    ) -> Option<bool> {
        bucket_mob_pickup(player, item_stack, self)
    }

    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        nbt.put_int("Variant", self.get_variant().id());
        nbt.put_bool("FromBucket", Self::is_from_bucket(self));
    }

    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        if let Some(variant) = nbt.get_int("Variant") {
            self.set_variant(AxolotlVariant::from_id(variant));
        }
        if let Some(from_bucket) = nbt.get_bool("FromBucket") {
            self.set_from_bucket(from_bucket);
        }
    }

    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    fn custom_server_ai_step(&self, _caller: &dyn EntityBase) {
        let play_dead_ticks = self.play_dead_ticks.load(Ordering::Relaxed);
        if play_dead_ticks > 0 {
            let remaining = play_dead_ticks - 1;
            self.play_dead_ticks.store(remaining, Ordering::Relaxed);
            if remaining == 0 {
                self.set_playing_dead(false);
            }
        }
    }

    fn mob_init_data_tracker(&self) {
        let entity = self.get_entity();
        let is_baby = entity.age.load(Ordering::Relaxed) < 0;
        if is_baby {
            entity.set_synced_data(pumpkin_data::tracked_data::axolotl::DATA_BABY_ID, true);
        }
        entity.set_synced_data(
            pumpkin_data::tracked_data::axolotl::DATA_VARIANT,
            VarInt(self.get_variant().id()),
        );
        entity.set_synced_data(
            pumpkin_data::tracked_data::axolotl::DATA_PLAYING_DEAD,
            self.is_playing_dead(),
        );
        entity.set_synced_data(
            pumpkin_data::tracked_data::axolotl::FROM_BUCKET,
            self.is_from_bucket(),
        );
    }

    fn mob_interact(&self, player: &Arc<Player>, item_stack: &mut ItemStack) -> bool {
        self.animal_interact(player, item_stack, Sound::EntityAxolotlIdleAir)
    }
}
