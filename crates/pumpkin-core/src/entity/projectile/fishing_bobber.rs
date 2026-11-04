//! Vanilla `FishingHook`.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};

use crossbeam::atomic::AtomicCell;
use pumpkin_data::Block;
use pumpkin_data::attributes::Attributes;
use pumpkin_data::entity::{EntityStatus, EntityType};
use pumpkin_data::fluid::Fluid;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::particle::Particle;
use pumpkin_data::sound::Sound;
use pumpkin_data::tag::Taggable;
use pumpkin_protocol::bedrock::server::ActorEventID;
use pumpkin_util::Hand;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_util::random::RandomImpl;
use pumpkin_util::random::legacy_rand::LegacyRand;
use rand::RngExt;

use super::hit_result::{self, MoveHit};
use super::projectile_item::triangle;
use crate::entity::experience_orb::ExperienceOrbEntity;
use crate::entity::item::ItemEntity;
use crate::entity::projectile::ProjectileHit;
use crate::entity::projectile::owner::ProjectileOwner;
use crate::entity::projectile_deflection::ProjectileDeflectionType;
use crate::plugin::api::events::player::fish::{PlayerFishEvent, PlayerFishState};
use crate::world::World;
use crate::world::loot::{LootContextParameters, LootTableHandle, generate_loot_from_handle};
use crate::{
    entity::{Entity, EntityBase, living::LivingEntity, player::Player},
    server::Server,
};

/// Vanilla `FishingHook.MAX_OUT_OF_WATER_TIME`.
const MAX_OUT_OF_WATER_TIME: i32 = 10;
/// Vanilla `FishingHook.GRAVITY`.
const GRAVITY: f64 = 0.03;
/// What vanilla's `this.life >= 1200` lets a hook lie on the ground.
const MAX_LIFE_ON_GROUND: i32 = 1200;
/// The square of the 32 blocks a hook stays out from its owner.
const MAX_OWNER_DISTANCE_SQUARED: f64 = 1024.0;
/// The drag of vanilla's `double inertia = 0.92`.
const INERTIA: f64 = 0.92;

/// Vanilla `FishingHook.FishHookState`.
#[derive(Clone, Copy, PartialEq, Eq)]
enum FishHookState {
    Flying,
    HookedInEntity,
    Bobbing,
}

/// Vanilla `FishingHook.OpenWaterType`.
#[derive(Clone, Copy, PartialEq, Eq)]
enum OpenWaterType {
    AboveWater,
    InsideWater,
    Invalid,
}

pub struct FishingBobberEntity {
    pub entity: Entity,
    pub owner: ProjectileOwner,
    /// Vanilla `hookedIn`, by entity id.
    hooked_in: AtomicCell<Option<i32>>,
    state: AtomicCell<FishHookState>,
    biting: AtomicBool,
    out_of_water_time: AtomicI32,
    life: AtomicI32,
    nibble: AtomicI32,
    time_until_lured: AtomicI32,
    time_until_hooked: AtomicI32,
    fish_angle: AtomicCell<f32>,
    open_water: AtomicBool,
    has_been_shot: AtomicBool,
    luck: i32,
    lure_speed: i32,
}

impl FishingBobberEntity {
    pub fn new(entity: Entity, owner: &Player) -> Self {
        Self::new_with_rod(entity, owner, 0, 0)
    }

    /// Vanilla's `FishingHook(player, level, luck, lureSpeed)`; the fishing rod aims it.
    pub fn new_with_rod(entity: Entity, owner: &Player, luck: i32, lure_speed: i32) -> Self {
        Self {
            entity,
            owner: ProjectileOwner::new(Some(&owner.living_entity.entity)),
            hooked_in: AtomicCell::new(None),
            state: AtomicCell::new(FishHookState::Flying),
            biting: AtomicBool::new(false),
            out_of_water_time: AtomicI32::new(0),
            life: AtomicI32::new(0),
            nibble: AtomicI32::new(0),
            time_until_lured: AtomicI32::new(0),
            time_until_hooked: AtomicI32::new(0),
            fish_angle: AtomicCell::new(0.0),
            open_water: AtomicBool::new(true),
            has_been_shot: AtomicBool::new(false),
            luck: luck.max(0),
            lure_speed: lure_speed.max(0),
        }
    }

    /// Vanilla `isOpenWaterFishing`.
    #[must_use]
    pub fn is_open_water_fishing(&self) -> bool {
        self.open_water.load(Ordering::Relaxed)
    }

    /// Vanilla `discard` with `updateOwnerInfo(null)`.
    fn discard(&self) {
        if let Some(owner) = self.owner.get_player(&self.entity.world.load()) {
            let _ = owner.fishing_bobber.compare_exchange(
                self.entity.entity_id,
                -1,
                Ordering::Relaxed,
                Ordering::Relaxed,
            );
        }
        self.entity.remove();
    }

    /// Vanilla `retrieve`. The rod takes the returned damage and removes the hook.
    pub fn reel_in(&self, player: &Player, rod: &ItemStack, hand: Hand) -> i32 {
        if self.should_stop_fishing(player) {
            return 0;
        }
        let world = self.entity.world.load();
        let hooked = self
            .hooked_in
            .load()
            .and_then(|id| world.get_entity_by_id(id));
        let dmg = if let Some(hooked) = hooked {
            match self.reel_in_entity(player, hand, &*hooked) {
                Some(dmg) => dmg,
                None => return 0,
            }
        } else if self.nibble.load(Ordering::Relaxed) > 0 {
            match self.reel_in_loot(player, rod, hand) {
                Some(dmg) => dmg,
                None => return 0,
            }
        } else {
            0
        };
        if self.entity.on_ground.load(Ordering::Relaxed) {
            2
        } else {
            dmg
        }
    }

    /// The hooked branch of vanilla `retrieve`. `None` if a plugin cancelled it.
    fn reel_in_entity(&self, player: &Player, hand: Hand, hooked: &dyn EntityBase) -> Option<i32> {
        let hooked_entity = hooked.get_entity();
        self.fire_fish_event(
            player,
            hand,
            PlayerFishState::CaughtEntity,
            Some(hooked_entity),
            Vec::new(),
            0,
        )?;
        self.pull_entity(player, hooked);
        self.entity.world.load().send_entity_status(
            &self.entity,
            EntityStatus::FishingRodReelIn,
            ActorEventID::FishhookHookTime,
        );
        Some(if hooked_entity.entity_type == &EntityType::ITEM {
            3
        } else {
            5
        })
    }

    /// Vanilla `pullEntity`. A hooked player pulls itself: the client does it on the entity
    /// event, as vanilla's `canSimulateMovement` is false for players on the server.
    fn pull_entity(&self, owner: &Player, hooked: &dyn EntityBase) {
        if hooked.get_player().is_some() {
            return;
        }
        let delta = owner
            .get_entity()
            .pos
            .load()
            .sub(&self.entity.pos.load())
            .multiply(0.1, 0.1, 0.1);
        hooked.get_entity().add_velocity(delta);
    }

    /// The bite branch of vanilla `retrieve`. `None` if a plugin cancelled it.
    fn reel_in_loot(&self, player: &Player, rod: &ItemStack, hand: Hand) -> Option<i32> {
        let world = self.entity.world.load();
        let items = self.generate_fishing_loot(player, rod);
        let (items, xp_amount) = self.fire_fish_event(
            player,
            hand,
            PlayerFishState::CaughtFish,
            None,
            items,
            Self::roll_experience(),
        )?;

        let hook_pos = self.entity.pos.load();
        let player_pos = player.get_entity().pos.load();
        let orb_pos = player_pos.add_raw(0.0, 0.5, 0.5);
        let delta = player_pos.sub(&hook_pos);
        let velocity = Vector3::new(
            delta.x * 0.1,
            delta.y * 0.1 + delta.length().sqrt() * 0.08,
            delta.z * 0.1,
        );
        let mut experience = xp_amount;
        for item_stack in items {
            if item_stack.is_empty() {
                continue;
            }
            if item_stack
                .item
                .has_tag(&pumpkin_data::tag::Item::MINECRAFT_FISHES)
            {
                player.increment_stat(
                    pumpkin_data::statistic::StatisticCategory::Custom,
                    pumpkin_data::statistic::CustomStatistic::FishCaught as i32,
                    1,
                );
            }
            player.trigger_advancement(
                crate::entity::player::advancement::trigger::AdvancementTrigger::FishedItem {
                    item_id: format!("minecraft:{}", item_stack.item.registry_key),
                },
            );
            let item_entity = ItemEntity::new_with_velocity(
                Entity::new(world.clone(), hook_pos, &EntityType::ITEM),
                item_stack,
                velocity,
                0,
            );
            world.spawn_entity(Arc::new(item_entity));
            // One orb per item, the event's experience for the first.
            if experience > 0 {
                ExperienceOrbEntity::spawn(&world, orb_pos, experience as u32);
            }
            experience = Self::roll_experience();
        }
        Some(1)
    }

    /// Vanilla's `random.nextInt(6) + 1`.
    fn roll_experience() -> i32 {
        rand::rng().random_range(1..=6)
    }

    /// Vanilla's `BuiltInLootTables.FISHING` with the hook's luck and the owner's.
    fn generate_fishing_loot(&self, player: &Player, rod: &ItemStack) -> Vec<ItemStack> {
        let world = self.entity.world.load();
        let luck =
            self.luck as f32 + player.living_entity.get_attribute_value(&Attributes::LUCK) as f32;
        let params = LootContextParameters {
            position: Some(self.entity.pos.load()),
            tool: Some(rod.clone()),
            this_entity: Some(self),
            world: Some(&world),
            luck,
            ..Default::default()
        };
        let handle = world
            .server
            .upgrade()
            .and_then(|s| {
                s.datapack_manager
                    .get_loot_table("minecraft:gameplay/fishing")
            })
            .unwrap_or(LootTableHandle::Static(
                &pumpkin_data::loot_table::GAMEPLAY_FISHING,
            ));
        generate_loot_from_handle(&handle, 0, &params)
            .into_iter()
            .filter(|item_stack| !item_stack.is_empty())
            .collect()
    }

    /// Fires a `PlayerFishEvent` for what the hook caught. Returns the caught items and the
    /// experience to drop, or `None` if a plugin cancelled the event.
    fn fire_fish_event(
        &self,
        player: &Player,
        hand: Hand,
        state: PlayerFishState,
        caught: Option<&Entity>,
        caught_items: Vec<ItemStack>,
        exp_to_drop: i32,
    ) -> Option<(Vec<ItemStack>, i32)> {
        let world = self.entity.world.load();
        let Some(server) = world
            .server
            .upgrade()
            .filter(|server| server.plugin_manager.has_handlers::<PlayerFishEvent>())
        else {
            return Some((caught_items, exp_to_drop));
        };
        let Some(player) = world.get_player_by_uuid(player.gameprofile.id) else {
            return Some((caught_items, exp_to_drop));
        };
        let mut event = PlayerFishEvent::new(
            player,
            caught.map(|entity| entity.entity_uuid),
            self.entity.entity_uuid,
            caught.map_or_else(String::new, |entity| {
                format!("minecraft:{}", entity.entity_type.resource_name)
            }),
            state,
            hand,
            exp_to_drop,
        )
        .with_caught_items(caught_items);
        server.plugin_manager.fire_blocking(&server, &mut event);
        (!event.cancelled).then_some((event.caught_items, event.exp_to_drop))
    }

    /// Vanilla `shouldStopFishing`.
    fn should_stop_fishing(&self, owner: &Player) -> bool {
        if owner.living_entity.is_alive() && !owner.is_spectator() {
            let inventory = owner.inventory();
            let holds_rod = inventory.held_item().item.id == Item::FISHING_ROD.id
                || inventory.off_hand_item().item.id == Item::FISHING_ROD.id;
            let distance = self
                .entity
                .pos
                .load()
                .squared_distance_to_vec(&owner.get_entity().pos.load());
            if holds_rod && distance <= MAX_OWNER_DISTANCE_SQUARED {
                return false;
            }
        }
        self.discard();
        true
    }

    /// Vanilla `setHookedEntity`.
    fn set_hooked_entity(&self, hooked: Option<&Entity>) {
        let id = hooked.map(|entity| entity.entity_id);
        self.hooked_in.store(id);
        self.entity.set_synced_data(
            pumpkin_data::tracked_data::fishing_bobber::DATA_HOOKED_ENTITY,
            id.map_or(0, |id| id + 1),
        );
    }

    /// The hook's synchronized random of vanilla, seeded alike on both sides each tick so that
    /// client and server jerk the hook down by the same amount.
    fn synchronized_random(&self, world: &World) -> LegacyRand {
        let least_significant_bits = self.entity.entity_uuid.as_u128() as u64;
        LegacyRand::from_seed(least_significant_bits ^ world.get_world_age() as u64)
    }

    /// Vanilla `setBiting` by `DATA_BITING` and `onSyncedDataUpdated`.
    fn set_biting(&self, biting: bool, synchronized: &mut LegacyRand) {
        self.biting.store(biting, Ordering::Relaxed);
        self.entity.set_synced_data(
            pumpkin_data::tracked_data::fishing_bobber::DATA_BITING,
            biting,
        );
        if biting {
            let velocity = self.entity.velocity.load();
            let jerk = -0.4 * synchronized.next_f32().mul_add(0.4, 0.6);
            self.entity
                .velocity
                .store(Vector3::new(velocity.x, f64::from(jerk), velocity.z));
        }
    }

    /// Vanilla `FishingHook.tick`.
    fn tick_hook(&self, caller: &dyn EntityBase, server: &Server) {
        let entity = &self.entity;
        let world = entity.world.load();
        let mut synchronized = self.synchronized_random(&world);

        hit_result::emit_shoot_event(caller, &self.has_been_shot);
        self.owner.check_left_owner(entity);
        entity.tick(caller, server);

        let Some(owner) = self.owner.get_player(&world) else {
            entity.remove();
            return;
        };
        if self.should_stop_fishing(&owner) || !self.tick_life_on_ground() {
            return;
        }

        let block_pos = entity.block_pos.load();
        let (fluid, fluid_state) = world.get_fluid_and_fluid_state(&block_pos);
        let is_water = fluid.matches_type(&Fluid::WATER);
        let liquid_height = if is_water {
            world.get_fluid_height(&block_pos, fluid, &fluid_state)
        } else {
            0.0
        };

        match self.state.load() {
            FishHookState::Flying => {
                if self.hooked_in.load().is_some() {
                    entity.velocity.store(Vector3::new(0.0, 0.0, 0.0));
                    self.state.store(FishHookState::HookedInEntity);
                    return;
                }
                if liquid_height > 0.0 {
                    entity
                        .velocity
                        .store(entity.velocity.load().multiply(0.3, 0.2, 0.3));
                    self.state.store(FishHookState::Bobbing);
                    return;
                }
                self.check_collision(caller);
            }
            FishHookState::HookedInEntity => {
                self.tick_hooked_in_entity(&world);
                return;
            }
            FishHookState::Bobbing => {
                self.tick_bobbing(&world, block_pos, liquid_height, &mut synchronized);
            }
        }

        if !is_water && !entity.on_ground.load(Ordering::Relaxed) && self.hooked_in.load().is_none()
        {
            entity
                .velocity
                .store(entity.velocity.load().add_raw(0.0, -GRAVITY, 0.0));
        }
        entity.move_entity(caller, entity.velocity.load());
        entity.tick_block_collisions(caller);
        hit_result::update_rotation(entity);
        if self.state.load() == FishHookState::Flying
            && (entity.on_ground.load(Ordering::Relaxed)
                || entity.horizontal_collision.load(Ordering::Relaxed))
        {
            entity.velocity.store(Vector3::new(0.0, 0.0, 0.0));
        }
        entity
            .velocity
            .store(entity.velocity.load().multiply(INERTIA, INERTIA, INERTIA));
    }

    /// The life count of vanilla's tick. Returns `false` when the hook lay around too long.
    fn tick_life_on_ground(&self) -> bool {
        if !self.entity.on_ground.load(Ordering::Relaxed) {
            self.life.store(0, Ordering::Relaxed);
            return true;
        }
        if self.life.fetch_add(1, Ordering::Relaxed) + 1 >= MAX_LIFE_ON_GROUND {
            self.discard();
            return false;
        }
        true
    }

    /// Vanilla's `HOOKED_IN_ENTITY` state: at 80 % of the hooked entity's height.
    fn tick_hooked_in_entity(&self, world: &World) {
        let Some(id) = self.hooked_in.load() else {
            return;
        };
        let hooked = world.get_entity_by_id(id).filter(|hooked| {
            let alive = hooked
                .get_living_entity()
                .map_or_else(|| hooked.get_entity().is_alive(), LivingEntity::is_alive);
            alive && !hooked.is_spectator()
        });
        if let Some(hooked) = hooked {
            let hooked = hooked.get_entity();
            let height = f64::from(hooked.entity_dimension.load().height);
            self.entity
                .set_pos(hooked.pos.load().add_raw(0.0, height * 0.8, 0.0));
        } else {
            self.set_hooked_entity(None);
            self.state.store(FishHookState::Flying);
        }
    }

    /// Vanilla's `BOBBING` state.
    fn tick_bobbing(
        &self,
        world: &World,
        block_pos: BlockPos,
        liquid_height: f32,
        synchronized: &mut LegacyRand,
    ) {
        let entity = &self.entity;
        let movement = entity.velocity.load();
        let mut force =
            entity.pos.load().y + movement.y - f64::from(block_pos.0.y) - f64::from(liquid_height);
        if force.abs() < 0.01 && force != 0.0 {
            force += force.signum() * 0.1;
        }
        entity.velocity.store(Vector3::new(
            movement.x * 0.9,
            movement.y - force * f64::from(rand::random::<f32>()) * 0.2,
            movement.z * 0.9,
        ));

        let out_of_water_time = self.out_of_water_time.load(Ordering::Relaxed);
        let open_water = (self.nibble.load(Ordering::Relaxed) <= 0
            && self.time_until_hooked.load(Ordering::Relaxed) <= 0)
            || (self.open_water.load(Ordering::Relaxed)
                && out_of_water_time < MAX_OUT_OF_WATER_TIME
                && Self::calculate_open_water(world, block_pos));
        self.open_water.store(open_water, Ordering::Relaxed);

        if liquid_height > 0.0 {
            self.out_of_water_time
                .store((out_of_water_time - 1).max(0), Ordering::Relaxed);
            if self.biting.load(Ordering::Relaxed) {
                let pull =
                    -0.1 * f64::from(synchronized.next_f32()) * f64::from(synchronized.next_f32());
                entity
                    .velocity
                    .store(entity.velocity.load().add_raw(0.0, pull, 0.0));
            }
            self.catching_fish(world, block_pos, synchronized);
        } else {
            self.out_of_water_time.store(
                (out_of_water_time + 1).min(MAX_OUT_OF_WATER_TIME),
                Ordering::Relaxed,
            );
        }
    }

    /// Vanilla `checkCollision`.
    fn check_collision(&self, caller: &dyn EntityBase) {
        let hit = hit_result::hit_result_on_move_vector(&self.entity, &|target| {
            self.can_hit_entity(target)
        });
        match hit {
            MoveHit::Miss => {}
            MoveHit::WorldBorder => {
                ProjectileDeflectionType::reverse(caller, Vector3::new(0.2, 0.2, 0.2));
            }
            MoveHit::Hit(hit) => {
                hit_result::hit_target_or_deflect_self(caller, hit);
            }
        }
    }

    /// Vanilla `FishingHook.canHitEntity`: items too.
    fn can_hit_entity(&self, target: &dyn EntityBase) -> bool {
        let entity = target.get_entity();
        (hit_result::can_be_hit(target) && self.owner.can_hit(&self.entity.world.load(), entity))
            || (entity.is_alive() && entity.entity_type == &EntityType::ITEM)
    }

    /// Vanilla `catchingFish`: first a fish is lured, then it swims up, then it bites.
    fn catching_fish(&self, world: &World, block_pos: BlockPos, synchronized: &mut LegacyRand) {
        let above = block_pos.up();
        let mut fishing_speed = 1;
        if rand::random::<f32>() < 0.25 && world.is_raining_at(&above) {
            fishing_speed += 1;
        }
        if rand::random::<f32>() < 0.5 && !world.can_see_sky(&above) {
            fishing_speed -= 1;
        }

        let nibble = self.nibble.load(Ordering::Relaxed);
        if nibble > 0 {
            self.nibble.store(nibble - 1, Ordering::Relaxed);
            if nibble - 1 <= 0 {
                self.time_until_lured.store(0, Ordering::Relaxed);
                self.time_until_hooked.store(0, Ordering::Relaxed);
                self.set_biting(false, synchronized);
            }
        } else if self.time_until_hooked.load(Ordering::Relaxed) > 0 {
            self.tick_fish_swimming(world, fishing_speed, synchronized);
        } else if self.time_until_lured.load(Ordering::Relaxed) > 0 {
            self.tick_fish_lured(world, fishing_speed);
        } else {
            let lured = rand::rng().random_range(100..=600) - self.lure_speed;
            self.time_until_lured.store(lured, Ordering::Relaxed);
        }
    }

    /// The `timeUntilHooked` branch of `catchingFish`: the wake, then the bite.
    fn tick_fish_swimming(&self, world: &World, speed: i32, synchronized: &mut LegacyRand) {
        let entity = &self.entity;
        let time = self.time_until_hooked.load(Ordering::Relaxed) - speed;
        self.time_until_hooked.store(time, Ordering::Relaxed);
        let pos = entity.pos.load();
        if time > 0 {
            let angle_degrees =
                self.fish_angle.load() + triangle(&mut rand::rng(), 0.0, 9.188) as f32;
            self.fish_angle.store(angle_degrees);
            let angle = angle_degrees.to_radians();
            let (sin, cos) = (angle.sin(), angle.cos());
            let reach = time as f32 * 0.1;
            let fish = Vector3::new(
                pos.x + f64::from(sin * reach),
                pos.y.floor() + 1.0,
                pos.z + f64::from(cos * reach),
            );
            if world.get_block(&BlockPos::containing(fish.x, fish.y - 1.0, fish.z)) != &Block::WATER
            {
                return;
            }
            if rand::random::<f32>() < 0.15 {
                world.spawn_particle(
                    fish.add_raw(0.0, -f64::from(0.1f32), 0.0),
                    Vector3::new(sin, 0.1, cos),
                    0.0,
                    1,
                    Particle::Bubble,
                );
            }
            let (x, z) = (sin * 0.04, cos * 0.04);
            world.spawn_particle(fish, Vector3::new(z, 0.01, -x), 1.0, 0, Particle::Fishing);
            world.spawn_particle(fish, Vector3::new(-z, 0.01, x), 1.0, 0, Particle::Fishing);
        } else {
            self.play_entity_sound(
                Sound::EntityFishingBobberSplash,
                0.25,
                (rand::random::<f32>() - rand::random::<f32>()).mul_add(0.4, 1.0),
            );
            let width = entity.width();
            let count = (1.0 + width * 20.0) as i32;
            let offset = Vector3::new(width, 0.0, width);
            let at = pos.add_raw(0.0, 0.5, 0.0);
            world.spawn_particle(at, offset, 0.2, count, Particle::Bubble);
            world.spawn_particle(at, offset, 0.2, count, Particle::Fishing);
            self.nibble
                .store(rand::rng().random_range(20..=40), Ordering::Relaxed);
            self.set_biting(true, synchronized);
        }
    }

    /// The `timeUntilLured` branch of `catchingFish`: splashes around, more of them near the end.
    fn tick_fish_lured(&self, world: &World, speed: i32) {
        let time = self.time_until_lured.load(Ordering::Relaxed) - speed;
        self.time_until_lured.store(time, Ordering::Relaxed);
        let mut tease_chance = 0.15f32;
        if time < 20 {
            tease_chance += (20 - time) as f32 * 0.05;
        } else if time < 40 {
            tease_chance += (40 - time) as f32 * 0.02;
        } else if time < 60 {
            tease_chance += (60 - time) as f32 * 0.01;
        }

        if rand::random::<f32>() < tease_chance {
            let mut rng = rand::rng();
            let angle = rng.random_range(0.0f32..360.0).to_radians();
            let distance = rng.random_range(25.0f32..60.0);
            let pos = self.entity.pos.load();
            let splash = Vector3::new(
                pos.x + f64::from(angle.sin() * distance) * 0.1,
                pos.y.floor() + 1.0,
                pos.z + f64::from(angle.cos() * distance) * 0.1,
            );
            if world.get_block(&BlockPos::containing(splash.x, splash.y - 1.0, splash.z))
                == &Block::WATER
            {
                world.spawn_particle(
                    splash,
                    Vector3::new(0.1, 0.0, 0.1),
                    0.0,
                    2 + rng.random_range(0..2),
                    Particle::Splash,
                );
            }
        }

        if time <= 0 {
            let mut rng = rand::rng();
            self.fish_angle.store(rng.random_range(0.0f32..360.0));
            self.time_until_hooked
                .store(rng.random_range(20..=80), Ordering::Relaxed);
        }
    }

    /// Vanilla `calculateOpenWater`: the layers around the hook are water, then air above it.
    fn calculate_open_water(world: &World, block_pos: BlockPos) -> bool {
        let mut previous = OpenWaterType::Invalid;
        for y in -1..=2 {
            let layer = Self::open_water_type_for_layer(world, block_pos, y);
            match layer {
                OpenWaterType::AboveWater if previous == OpenWaterType::Invalid => return false,
                OpenWaterType::InsideWater if previous == OpenWaterType::AboveWater => {
                    return false;
                }
                OpenWaterType::Invalid => return false,
                OpenWaterType::AboveWater | OpenWaterType::InsideWater => {}
            }
            previous = layer;
        }
        true
    }

    /// Vanilla `getOpenWaterTypeForArea` over the five by five blocks of a layer.
    fn open_water_type_for_layer(world: &World, center: BlockPos, y: i32) -> OpenWaterType {
        let from = center.offset(Vector3::new(-2, y, -2));
        let to = center.offset(Vector3::new(2, y, 2));
        BlockPos::iterate(from, to)
            .map(|pos| Self::open_water_type_for_block(world, &pos))
            .reduce(|a, b| if a == b { a } else { OpenWaterType::Invalid })
            .unwrap_or(OpenWaterType::Invalid)
    }

    /// Vanilla `getOpenWaterTypeForBlock`.
    fn open_water_type_for_block(world: &World, pos: &BlockPos) -> OpenWaterType {
        let (block, state) = world.get_block_and_state(pos);
        if state.is_air() || block == &Block::LILY_PAD {
            return OpenWaterType::AboveWater;
        }
        let (fluid, fluid_state) = world.get_fluid_and_fluid_state(pos);
        if fluid.matches_type(&Fluid::WATER)
            && fluid_state.is_source
            && state.get_block_collision_shapes_at(pos).next().is_none()
        {
            OpenWaterType::InsideWater
        } else {
            OpenWaterType::Invalid
        }
    }
}

impl EntityBase for FishingBobberEntity {
    fn get_projectile_owner(&self) -> Option<&ProjectileOwner> {
        Some(&self.owner)
    }

    fn entity_bedrock_spawn_metadata(
        &self,
    ) -> Option<pumpkin_protocol::bedrock::client::set_actor_data::SyncedActorDataList> {
        let mut meta =
            pumpkin_protocol::bedrock::client::set_actor_data::SyncedActorDataList::new();
        meta.set(
            pumpkin_protocol::bedrock::client::set_actor_data::entity_data_key::OWNER,
            pumpkin_protocol::bedrock::client::set_actor_data::MetadataValue::Int64(i64::from(
                self.owner
                    .get(&self.entity.world.load())
                    .map_or(0, |owner| owner.get_entity().entity_id),
            )),
        );
        Some(meta)
    }

    fn get_entity(&self) -> &Entity {
        &self.entity
    }

    fn get_living_entity(&self) -> Option<&LivingEntity> {
        None
    }

    fn cast_any(&self) -> &dyn std::any::Any {
        self
    }

    /// Vanilla `FishingHook.onHitEntity` and `onHitBlock`.
    fn on_hit(&self, hit: ProjectileHit) {
        match hit {
            ProjectileHit::Entity { entity: target, .. } => {
                self.set_hooked_entity(Some(target.get_entity()));
            }
            ProjectileHit::Block { pos, hit_pos, .. } => {
                let entity = &self.entity;
                let world = entity.world.load();
                let (block, state) = world.get_block_and_state(&pos);
                if let Some(server) = world.server.upgrade() {
                    world
                        .block_registry
                        .on_projectile_hit(block, &world, self, &pos, state, &hit_pos, &server);
                }
                let distance = hit_pos.sub(&entity.pos.load()).length();
                entity.velocity.store(
                    entity
                        .velocity
                        .load()
                        .normalize()
                        .multiply(distance, distance, distance),
                );
            }
        }
    }

    fn tick(&self, caller: &dyn EntityBase, server: &Server) {
        self.tick_hook(caller, server);
    }
}
