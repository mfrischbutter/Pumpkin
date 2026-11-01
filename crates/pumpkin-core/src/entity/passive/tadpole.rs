use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};

use pumpkin_data::BlockState;
use pumpkin_data::entity::EntityType;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::particle::Particle;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::tag::{self, Taggable};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;

use crate::entity::bucketable::{
    Bucketable, bucket_mob_pickup, load_age_from_bucket_tag, save_age_to_bucket_tag,
};
use crate::entity::mob::requires_custom_persistence_base;
use crate::entity::{
    Entity, EntityBase,
    ageable::{AgeableData, AgeableMob},
    ai::goal::{
        escape_danger::EscapeDangerGoal, look_around::RandomLookAroundGoal,
        look_at_entity::LookAtEntityGoal, swim::SwimGoal, tempt::TemptGoal,
        try_find_water::TryFindWaterGoal, wander_around::WanderAroundGoal,
    },
    mob::{Mob, MobEntity, spawn::SpawnGroupData},
    player::Player,
};
use crate::world::World;
use crate::world::level_accessor::LevelAccessor;

const TEMPT_ITEMS: &[&Item] = &[&Item::SLIME_BALL];

pub struct TadpoleEntity {
    pub mob_entity: MobEntity,
    pub ageable_data: AgeableData,
    /// Vanilla `AbstractFish.fromBucket`: a released tadpole stays.
    from_bucket: AtomicBool,
}

impl TadpoleEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let mob_entity = MobEntity::new(entity);
        let tadpole = Self {
            mob_entity,
            ageable_data: AgeableData::default(),
            from_bucket: AtomicBool::new(false),
        };
        let mob_arc = Arc::new(tadpole);
        let mob_weak: Weak<dyn Mob> = {
            let mob_arc: Arc<dyn Mob> = mob_arc.clone();
            Arc::downgrade(&mob_arc)
        };

        {
            let goal_selector = &mob_arc.mob_entity.goals_selector;

            goal_selector.add_goal(0, Box::new(TryFindWaterGoal));
            goal_selector.add_goal(0, Box::new(SwimGoal::default()));
            goal_selector.add_goal(1, EscapeDangerGoal::new(1.5));
            goal_selector.add_goal(2, Box::new(TemptGoal::new(1.25, TEMPT_ITEMS, false)));
            goal_selector.add_goal(3, Box::new(WanderAroundGoal::new(1.0)));
            goal_selector.add_goal(
                4,
                LookAtEntityGoal::with_default(mob_weak, &EntityType::PLAYER, 6.0),
            );
            goal_selector.add_goal(5, Box::new(RandomLookAroundGoal::default()));
        };

        mob_arc
    }

    fn is_food(item_stack: &ItemStack) -> bool {
        item_stack.item.has_tag(&tag::Item::MINECRAFT_FROG_FOOD)
            || item_stack.item == &Item::SLIME_BALL
    }
}

impl Bucketable for TadpoleEntity {
    fn is_from_bucket(&self) -> bool {
        self.from_bucket.load(Ordering::Relaxed)
    }

    fn set_from_bucket(&self, from_bucket: bool) {
        self.from_bucket.store(from_bucket, Ordering::Relaxed);
        self.get_entity().set_synced_data(
            pumpkin_data::tracked_data::abstract_fish::FROM_BUCKET,
            from_bucket,
        );
    }

    fn bucket_item(&self) -> &'static Item {
        &Item::TADPOLE_BUCKET
    }

    /// Vanilla `Tadpole.getPickupSound`.
    fn pickup_sound(&self) -> Sound {
        Sound::ItemBucketFillTadpole
    }

    /// Vanilla `Tadpole.saveToBucketTag`: the age and its lock go along.
    fn save_to_bucket_tag(&self, bucket: &mut ItemStack) {
        self.save_default_data_to_bucket_tag(bucket);
        save_age_to_bucket_tag(self, bucket);
    }

    /// Vanilla `Tadpole.loadFromBucketTag`.
    fn load_from_bucket_tag(&self, nbt: &NbtCompound) {
        self.load_default_data_from_bucket_tag(nbt);
        load_age_from_bucket_tag(self, nbt);
    }
}

impl AgeableMob for TadpoleEntity {
    fn get_ageable_data(&self) -> &AgeableData {
        &self.ageable_data
    }

    fn ageable_interact(&self, _player: &Arc<Player>, _stack: &mut ItemStack) -> bool {
        false
    }
}

impl Mob for TadpoleEntity {
    /// Vanilla `WaterAnimal.checkSpawnObstruction`: only entities obstruct, the water does
    /// not.
    fn check_spawn_obstruction(&self, level: &dyn LevelAccessor) -> bool {
        level.is_unobstructed(&self.get_mob_entity().living_entity.entity)
    }

    /// Vanilla `AbstractFish` steps silently.
    fn mob_play_step_sound(&self, _pos: &BlockPos, _state: &BlockState) {}

    fn as_ageable(&self) -> Option<&dyn AgeableMob> {
        Some(self)
    }

    fn as_bucketable(&self) -> Option<&dyn Bucketable> {
        Some(self)
    }

    /// Vanilla `Tadpole.mobInteract`: the water bucket picks it up first.
    fn mob_interact_with_hand(
        &self,
        player: &Arc<Player>,
        item_stack: &mut ItemStack,
        _hand: pumpkin_util::Hand,
    ) -> Option<bool> {
        bucket_mob_pickup(player, item_stack, self)
    }

    /// Vanilla `AbstractFish.removeWhenFarAway`: a bucketed or named tadpole stays.
    fn remove_when_far_away(&self, _distance_sq: f64) -> bool {
        !self.is_from_bucket() && self.get_entity().custom_name.load().is_none()
    }

    /// Vanilla `AbstractFish.requiresCustomPersistence`.
    fn requires_custom_persistence(&self) -> bool {
        requires_custom_persistence_base(self) || self.is_from_bucket()
    }

    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        self.write_ageable_nbt(nbt);
        nbt.put_bool("FromBucket", self.is_from_bucket());
    }

    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        self.read_ageable_nbt(nbt);
        if let Some(from_bucket) = nbt.get_bool("FromBucket") {
            self.set_from_bucket(from_bucket);
        }
    }

    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    fn finalize_spawn(
        &self,
        _world: &Arc<World>,
        group_data: Option<SpawnGroupData>,
    ) -> Option<SpawnGroupData> {
        self.mob_entity.finalize_spawn_base();
        group_data
    }

    fn should_drop_loot(&self) -> bool {
        self.mob_entity
            .living_entity
            .entity
            .world
            .load()
            .level_info
            .load()
            .game_rules
            .mob_drops
    }

    fn should_drop_experience(&self) -> bool {
        false
    }

    fn mob_interact(&self, player: &Arc<Player>, item_stack: &mut ItemStack) -> bool {
        if Self::is_food(item_stack) {
            item_stack.decrement_unless_creative(player.gamemode.load(), 1);
            let age = self
                .get_mob_entity()
                .living_entity
                .entity
                .age
                .load(std::sync::atomic::Ordering::Relaxed);
            let speedup = (-age / 10).max(1);
            self.get_mob_entity()
                .living_entity
                .entity
                .age
                .fetch_add(speedup, std::sync::atomic::Ordering::Relaxed);

            let entity = self.get_entity();
            let world = entity.world.load();
            let pos = entity.pos.load();
            world.spawn_particle(
                pos + Vector3::new(0.0, f64::from(entity.height()), 0.0),
                Vector3::new(0.5, 0.5, 0.5),
                1.0,
                7,
                Particle::HappyVillager,
            );
            world.play_sound(Sound::EntityTadpoleGrowUp, SoundCategory::Neutral, &pos);
            return true;
        }

        if item_stack.get_item() == &Item::WATER_BUCKET {
            let entity = self.get_entity();
            let world = entity.world.load();
            if let Some(server) = world.server.upgrade() {
                let mut event = crate::plugin::api::events::player::player_bucket_entity::PlayerBucketEntityEvent {
                    player: player.clone(),
                    entity_id: entity.entity_id,
                    bucket_item: "tadpole_bucket".to_string(),
                    cancelled: false,
                };
                server.plugin_manager.fire_blocking(&server, &mut event);
                if event.cancelled {
                    return false;
                }
            }
            item_stack.decrement_unless_creative(player.gamemode.load(), 1);
            let pos = entity.pos.load();
            world.play_sound(Sound::ItemBucketFillTadpole, SoundCategory::Neutral, &pos);
            entity.remove();
            return true;
        }

        false
    }
}
