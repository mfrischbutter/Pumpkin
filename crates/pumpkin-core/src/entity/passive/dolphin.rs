use std::sync::{
    Arc, Weak,
    atomic::{AtomicBool, AtomicI32, Ordering},
};

use crate::entity::mob::spawn::SpawnGroupData;
use crate::world::World;
use pumpkin_data::damage::DamageType;
use pumpkin_data::data_component_impl::EquipmentSlot;
use pumpkin_data::effect::StatusEffect;
use pumpkin_data::entity::EntityType;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::potion::Effect;
use pumpkin_data::sound::Sound;
use pumpkin_data::tag::{self, Taggable};
use pumpkin_data::tracked_data;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use rand::RngExt;

use crate::entity::ai::behavior::fleeing::{self, Flee};
use crate::entity::item::ItemEntity;
use crate::entity::{
    Entity, EntityBase,
    ai::control::{
        smooth_swimming_look_control::SmoothSwimmingLookControl,
        smooth_swimming_move_control::SmoothSwimmingMoveControl,
    },
    ai::goal::{
        Controls, Goal, breath_air::BreathAirGoal, look_around::RandomLookAroundGoal,
        look_at_entity::LookAtEntityGoal, melee_attack::MeleeAttackGoal,
        random_swimming::RandomSwimmingGoal, revenge::RevengeGoal, to_goal_ticks,
        try_find_water::TryFindWaterGoal,
    },
    ai::pathfinder::{Navigator, NavigatorGoal},
    ai::target_match::TargetMatch,
    mob::{Mob, MobEntity},
    player::Player,
};
use crate::world::level_accessor::LevelAccessor;

const DOLPHIN_FLEES: &[Flee] = &[Flee::new(9, TargetMatch::GUARDIAN_LIKE, 8.0, 1.0, 1.0)];

/// Vanilla `Dolphin.TOTAL_AIR_SUPPLY` and `TOTAL_MOISTNESS_LEVEL`.
const TOTAL_AIR_SUPPLY: i32 = 4800;
const TOTAL_MOISTNESS_LEVEL: i32 = 2400;

pub struct DolphinEntity {
    pub mob_entity: MobEntity,
    pub got_fish: AtomicBool,
    pub moistness_level: AtomicI32,
}

impl DolphinEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let mut mob_entity = MobEntity::new(entity);
        // Vanilla `Dolphin` constructor and `createNavigation`.
        *mob_entity
            .move_control
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            Box::new(SmoothSwimmingMoveControl::new(85, 10, 0.02, 0.1, true));
        *mob_entity
            .look_control
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            Box::new(SmoothSwimmingLookControl::new(10));
        *mob_entity
            .navigator
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Navigator::water_bound(true);
        mob_entity.set_can_pick_up_loot(true);
        mob_entity
            .living_entity
            .entity
            .air_supply
            .store(TOTAL_AIR_SUPPLY, Ordering::Relaxed);
        let dolphin = Self {
            mob_entity,
            got_fish: AtomicBool::new(false),
            moistness_level: AtomicI32::new(TOTAL_MOISTNESS_LEVEL),
        };
        let mob_arc = Arc::new(dolphin);
        let mob_weak: Weak<dyn Mob> = {
            let mob_arc: Arc<dyn Mob> = mob_arc.clone();
            Arc::downgrade(&mob_arc)
        };

        {
            // Vanilla `Dolphin.registerGoals`.
            // TODO: DolphinSwimToTreasureGoal (1) needs structure search, and
            // FollowPlayerRiddenEntityGoal (8) needs a player's recent horizontal movement.
            let goal_selector = &mob_arc.mob_entity.goals_selector;
            goal_selector.add_goal(0, Box::new(BreathAirGoal));
            goal_selector.add_goal(0, Box::new(TryFindWaterGoal));
            goal_selector.add_goal(2, Box::new(DolphinSwimWithPlayerGoal { player: None }));
            goal_selector.add_goal(4, Box::new(RandomSwimmingGoal::new(1.0, 10)));
            goal_selector.add_goal(4, Box::new(RandomLookAroundGoal::default()));
            goal_selector.add_goal(
                5,
                LookAtEntityGoal::with_default(mob_weak, &EntityType::PLAYER, 6.0),
            );
            goal_selector.add_goal(5, Box::new(DolphinJumpGoal::new(10)));
            goal_selector.add_goal(6, Box::new(MeleeAttackGoal::new(1.2, true)));
            goal_selector.add_goal(7, Box::new(MoveToItemGoal { cooldown: 0 }));
            goal_selector.add_goal(8, Box::new(PlayWithItemsGoal));
            fleeing::apply(goal_selector, DOLPHIN_FLEES);

            mob_arc.mob_entity.target_selector.add_goal(
                1,
                Box::new(
                    RevengeGoal::new(true)
                        .ignoring(|entity_type| TargetMatch::GUARDIAN_LIKE.matches(entity_type))
                        .alerting_others(),
                ),
            );
        };

        mob_arc
    }

    #[must_use]
    pub fn got_fish(&self) -> bool {
        self.got_fish.load(Ordering::Relaxed)
    }

    pub fn set_got_fish(&self, got_fish: bool) {
        self.got_fish.store(got_fish, Ordering::Relaxed);
        self.get_entity()
            .set_synced_data(tracked_data::dolphin::GOT_FISH, got_fish);
    }

    fn set_moistness_level(&self, level: i32) {
        if self.moistness_level.swap(level, Ordering::Relaxed) != level {
            self.get_entity()
                .set_synced_data(tracked_data::dolphin::MOISTNESS_LEVEL, VarInt(level));
        }
    }

    /// The part of vanilla `Mob.aiStep` that picks up items, with `Dolphin.pickUpItem`: an item
    /// in reach goes into its empty mouth and always drops.
    fn pick_up_items(&self) {
        let mob_entity = &self.mob_entity;
        mob_entity.for_each_item_in_pickup_reach(|item| {
            if !held_item(self).is_empty() {
                return;
            }
            let stack = item
                .get_item_stack()
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone();
            if stack.is_empty() {
                return;
            }
            mob_entity
                .living_entity
                .send_pickup_animation(item.get_entity(), u32::from(stack.item_count));
            mob_entity.set_item_slot_and_drop_when_killed(&EquipmentSlot::MAIN_HAND, stack);
            item.get_entity().remove();
        });
    }
}

fn held_item(mob: &dyn Mob) -> ItemStack {
    mob.get_mob_entity()
        .living_entity
        .entity_equipment
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(&EquipmentSlot::MAIN_HAND)
}

/// Vanilla `Dolphin.ALLOWED_ITEMS`: items in water it may pick up now.
fn playable_items(mob: &dyn Mob) -> Vec<Arc<dyn EntityBase>> {
    let entity = mob.get_entity();
    let area = entity.bounding_box.load().expand(8.0, 8.0, 8.0);
    entity
        .world
        .load()
        .get_entities_at_box(&area)
        .into_iter()
        .filter(|candidate| {
            candidate
                .cast_any()
                .downcast_ref::<ItemEntity>()
                .is_some_and(|item| {
                    !item.has_pickup_delay()
                        && item.get_entity().is_alive()
                        && item.get_entity().is_in_water()
                })
        })
        .collect()
}

/// Vanilla `Dolphin.ItemGoal.dropItem`: throws the held item ahead, not to be picked up
/// for 2 seconds.
fn drop_item(mob: &dyn Mob) -> bool {
    let stack = held_item(mob);
    if stack.is_empty() {
        return false;
    }
    let entity = mob.get_entity();
    let world = entity.world.load_full();
    let pos = entity.pos.load();
    let item = ItemEntity::new(
        Entity::new(
            world.clone(),
            Vector3::new(pos.x, entity.get_eye_y() - 0.3, pos.z),
            &EntityType::ITEM,
        ),
        stack,
    );
    item.set_pickup_delay(40);
    item.set_thrower(entity.entity_uuid);
    let mut rng = mob.get_random();
    let dir = rng.random::<f32>() * std::f32::consts::TAU;
    let pow2 = 0.02 * rng.random::<f32>();
    let (yaw, pitch) = (
        entity.yaw.load().to_radians(),
        entity.pitch.load().to_radians(),
    );
    item.get_entity().velocity.store(Vector3::new(
        f64::from((0.3 * -yaw.sin()).mul_add(pitch.cos(), dir.cos() * pow2)),
        f64::from(0.3 * pitch.sin() * 1.5),
        f64::from((0.3 * yaw.cos()).mul_add(pitch.cos(), dir.sin() * pow2)),
    ));
    world.spawn_entity(Arc::new(item));
    mob.get_mob_entity()
        .set_item_slot(&EquipmentSlot::MAIN_HAND, ItemStack::EMPTY.clone());
    true
}

impl Mob for DolphinEntity {
    /// Vanilla `Dolphin.finalizeSpawn`: full air and a level pitch. The group's one in ten
    /// babies need an ageable dolphin, which this one is not yet.
    fn finalize_spawn(
        &self,
        _world: &Arc<World>,
        group_data: Option<SpawnGroupData>,
    ) -> Option<SpawnGroupData> {
        let entity = self.get_entity();
        entity.set_air_supply(TOTAL_AIR_SUPPLY);
        entity.set_rotation(entity.yaw.load(), 0.0);
        self.finalize_spawn_super(group_data)
    }

    /// Vanilla `AgeableWaterCreature.checkSpawnObstruction`: only entities obstruct, the
    /// water does not.
    fn check_spawn_obstruction(&self, level: &dyn LevelAccessor) -> bool {
        level.is_unobstructed(&self.get_mob_entity().living_entity.entity)
    }

    /// Vanilla `Dolphin.getAmbientSound`.
    fn get_ambient_sound(&self) -> Option<Sound> {
        Some(if self.get_entity().is_in_water() {
            Sound::EntityDolphinAmbientWater
        } else {
            Sound::EntityDolphinAmbient
        })
    }

    /// Vanilla `Dolphin.getMaxAirSupply`.
    fn mob_max_air_supply(&self) -> i32 {
        TOTAL_AIR_SUPPLY
    }

    /// Vanilla `Dolphin.increaseAirSupply`: one breath fills it.
    fn mob_increase_air_supply(&self, _current: i32) -> i32 {
        TOTAL_AIR_SUPPLY
    }

    fn travel_in_water(&self, caller: &dyn EntityBase, _gravity: f64, _falling: bool) {
        let living = &self.mob_entity.living_entity;
        let entity = &living.entity;
        entity.update_velocity_from_input(
            living.movement_input.load(),
            f64::from(living.get_speed()),
        );
        living.make_move(caller);
        let mut velocity = entity.velocity.load() * 0.9;
        if self.mob_entity.get_target().is_none() {
            velocity.y -= 0.005;
        }
        entity.velocity.store(velocity);
    }

    /// Vanilla `Dolphin.tick`: it dries out on land, wet again in water or rain, and flops
    /// about on the ground; and `Mob.aiStep`'s item pickup.
    fn mob_tick(&self, caller: &dyn EntityBase) {
        let entity = self.get_entity();
        if self.mob_entity.is_no_ai() {
            entity.set_air_supply(TOTAL_AIR_SUPPLY);
            return;
        }
        if entity.is_in_water_or_rain() {
            self.set_moistness_level(TOTAL_MOISTNESS_LEVEL);
        } else {
            let level = self.moistness_level.load(Ordering::Relaxed) - 1;
            self.set_moistness_level(level);
            if level <= 0 {
                caller.damage(caller, 1.0, DamageType::DRY_OUT);
            }
            if entity.on_ground.load(Ordering::Relaxed) {
                let mut rng = self.get_random();
                let x = (rng.random::<f32>() * 2.0 - 1.0) * 0.2;
                let z = (rng.random::<f32>() * 2.0 - 1.0) * 0.2;
                entity.velocity.store(entity.velocity.load().add_raw(
                    f64::from(x),
                    0.5,
                    f64::from(z),
                ));
                entity.yaw.store(rng.random::<f32>() * 360.0);
                entity.on_ground.store(false, Ordering::Relaxed);
                entity.velocity_dirty.store(true, Ordering::Relaxed);
            }
        }
        self.pick_up_items();
    }

    fn mob_init_data_tracker(&self) {
        let entity = self.get_entity();
        entity.set_synced_data(tracked_data::dolphin::GOT_FISH, self.got_fish());
        entity.set_synced_data(
            tracked_data::dolphin::MOISTNESS_LEVEL,
            VarInt(self.moistness_level.load(Ordering::Relaxed)),
        );
    }

    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        nbt.put_bool("GotFish", self.got_fish());
        nbt.put_int("Moistness", self.moistness_level.load(Ordering::Relaxed));
    }

    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        self.set_got_fish(nbt.get_bool("GotFish").unwrap_or(false));
        self.set_moistness_level(nbt.get_int("Moistness").unwrap_or(TOTAL_MOISTNESS_LEVEL));
    }

    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    /// Vanilla `Dolphin.mobInteract`: a fish is eaten, and the dolphin wants to show treasure.
    fn mob_interact(&self, player: &Arc<Player>, item_stack: &mut ItemStack) -> bool {
        if item_stack.is_empty() || !item_stack.get_item().has_tag(&tag::Item::MINECRAFT_FISHES) {
            return self
                .mob_entity
                .mob_interact(player, item_stack, self.can_be_leashed());
        }
        self.play_entity_sound(Sound::EntityDolphinEat, 1.0, 1.0);
        self.set_got_fish(true);
        item_stack.decrement_unless_creative(player.gamemode.load(), 1);
        true
    }
}

/// Vanilla `Dolphin.DolphinSwimWithPlayerGoal`: swims along with a swimming player within
/// 10 blocks and gives them Dolphin's Grace.
struct DolphinSwimWithPlayerGoal {
    player: Option<Arc<Player>>,
}

impl DolphinSwimWithPlayerGoal {
    const SPEED_MODIFIER: f64 = 4.0;

    fn grace(player: &Player) {
        player.living_entity.add_effect(Effect {
            effect_type: &StatusEffect::DOLPHINS_GRACE,
            duration: 100,
            amplifier: 0,
            ambient: false,
            show_particles: true,
            show_icon: true,
            blend: false,
        });
    }
}

impl Goal for DolphinSwimWithPlayerGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        let entity = mob.get_entity();
        let pos = entity.pos.load();
        self.player = entity
            .world
            .load()
            .get_nearby_players(pos, 10.0)
            .into_iter()
            .filter(|player| !player.is_spectator())
            .min_by(|a, b| {
                a.get_entity()
                    .pos
                    .load()
                    .squared_distance_to_vec(&pos)
                    .total_cmp(&b.get_entity().pos.load().squared_distance_to_vec(&pos))
            });
        let Some(player) = &self.player else {
            return false;
        };
        let is_target = mob
            .get_mob_entity()
            .get_target()
            .is_some_and(|target| target.get_entity().entity_id == player.get_entity().entity_id);
        player.get_entity().is_swimming() && !is_target
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        let pos = mob.get_entity().pos.load();
        self.player.as_ref().is_some_and(|player| {
            player.get_entity().is_swimming()
                && player.get_entity().pos.load().squared_distance_to_vec(&pos) < 256.0
        })
    }

    fn start(&mut self, _mob: &dyn Mob) {
        if let Some(player) = &self.player {
            Self::grace(player);
        }
    }

    fn stop(&mut self, mob: &dyn Mob) {
        self.player = None;
        mob.get_mob_entity().steered().navigator().stop();
    }

    fn tick(&mut self, mob: &dyn Mob) {
        let Some(player) = self.player.clone() else {
            return;
        };
        let target: Arc<dyn EntityBase> = player.clone();
        mob.get_mob_entity()
            .look_control
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .look_at_entity_with_range(&target, 21.0, 1.0);
        let pos = mob.get_entity().pos.load();
        let player_pos = player.get_entity().pos.load();
        let navigator_owner = mob.get_mob_entity().steered();
        let mut navigator = navigator_owner.navigator();
        if pos.squared_distance_to_vec(&player_pos) < 6.25 {
            navigator.stop();
        } else {
            navigator.set_progress(NavigatorGoal::new(pos, player_pos, Self::SPEED_MODIFIER));
        }
        drop(navigator);
        if player.get_entity().is_swimming() && rand::random_range(0..6) == 0 {
            Self::grace(&player);
        }
    }

    fn controls(&self) -> Controls {
        Controls::MOVE | Controls::LOOK
    }
}

/// Vanilla `DolphinJumpGoal`: now and then it leaps out of clear water with open air above.
struct DolphinJumpGoal {
    interval: i32,
    breached: bool,
}

impl DolphinJumpGoal {
    const STEPS_TO_CHECK: [i32; 6] = [0, 1, 4, 5, 6, 7];

    const fn new(interval: i32) -> Self {
        Self {
            interval: to_goal_ticks(interval),
            breached: false,
        }
    }

    /// Vanilla `getMotionDirection`, the horizontal direction of its yaw, as a step.
    fn motion_step(mob: &dyn Mob) -> (i32, i32) {
        let quarter = ((mob.get_entity().yaw.load() / 90.0 + 0.5).floor() as i32) & 3;
        match quarter {
            0 => (0, 1),
            1 => (-1, 0),
            2 => (0, -1),
            _ => (1, 0),
        }
    }

    fn clear_at(mob: &dyn Mob, base: BlockPos, step: (i32, i32), n: i32) -> bool {
        let world = mob.get_entity().world.load();
        let pos = BlockPos::new(base.0.x + step.0 * n, base.0.y, base.0.z + step.1 * n);
        let (block, _) = world.get_block_and_state(&pos);
        world
            .get_fluid(&pos)
            .matches_type(&pumpkin_data::fluid::Fluid::WATER)
            && !block.has_tag(&tag::Block::MINECRAFT_BLOCKS_DOLPHIN_JUMP)
            && world.get_block_state(&pos.up()).is_air()
            && world.get_block_state(&pos.up_height(2)).is_air()
    }
}

impl Goal for DolphinJumpGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        if mob.get_random().random_range(0..self.interval) != 0 {
            return false;
        }
        let step = Self::motion_step(mob);
        let base = mob.get_entity().block_pos.load();
        Self::STEPS_TO_CHECK
            .iter()
            .all(|&n| Self::clear_at(mob, base, step, n))
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        let entity = mob.get_entity();
        let yd = entity.velocity.load().y;
        let pitch = entity.pitch.load();
        (yd * yd >= 0.03 || pitch == 0.0 || pitch.abs() >= 10.0 || !entity.is_in_water())
            && !entity.on_ground.load(Ordering::Relaxed)
    }

    fn can_stop(&self) -> bool {
        false
    }

    fn start(&mut self, mob: &dyn Mob) {
        let (x, z) = Self::motion_step(mob);
        let entity = mob.get_entity();
        entity.velocity.store(entity.velocity.load().add_raw(
            f64::from(x) * 0.6,
            0.7,
            f64::from(z) * 0.6,
        ));
        entity.velocity_dirty.store(true, Ordering::Relaxed);
        mob.get_mob_entity().steered().navigator().stop();
    }

    fn stop(&mut self, mob: &dyn Mob) {
        mob.get_entity().pitch.store(0.0);
    }

    fn tick(&mut self, mob: &dyn Mob) {
        let entity = mob.get_entity();
        let already_breached = self.breached;
        if !already_breached {
            self.breached = entity
                .world
                .load()
                .get_fluid(&entity.block_pos.load())
                .matches_type(&pumpkin_data::fluid::Fluid::WATER);
        }
        if self.breached && !already_breached {
            mob.play_entity_sound(Sound::EntityDolphinJump, 1.0, 1.0);
        }
        let movement = entity.velocity.load();
        let pitch = entity.pitch.load();
        if movement.y * movement.y < 0.03 && pitch != 0.0 {
            entity
                .pitch
                .store(pitch + pumpkin_util::math::wrap_degrees(-pitch) * 0.2);
        } else if movement.length() > 1.0e-5 {
            let horizontal = movement.x.hypot(movement.z);
            entity
                .pitch
                .store((-movement.y).atan2(horizontal).to_degrees() as f32);
        }
    }

    fn should_run_every_tick(&self) -> bool {
        true
    }

    fn controls(&self) -> Controls {
        Controls::MOVE | Controls::JUMP
    }
}

/// Vanilla `Dolphin.MoveToItemGoal`: swims to items floating within 8 blocks, playing.
struct MoveToItemGoal {
    cooldown: i32,
}

impl MoveToItemGoal {
    fn swim_to_first(mob: &dyn Mob) -> bool {
        let Some(item) = playable_items(mob).into_iter().next() else {
            return false;
        };
        let pos = mob.get_entity().pos.load();
        mob.get_mob_entity()
            .steered()
            .navigator()
            .set_progress(NavigatorGoal::new(pos, item.get_entity().pos.load(), 1.2));
        true
    }
}

impl Goal for MoveToItemGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        self.cooldown <= mob.get_entity().tick_count.load(Ordering::Relaxed)
            && !playable_items(mob).is_empty()
    }

    fn start(&mut self, mob: &dyn Mob) {
        if Self::swim_to_first(mob) {
            mob.play_entity_sound(Sound::EntityDolphinPlay, 1.0, 1.0);
        }
        self.cooldown = 0;
    }

    fn stop(&mut self, mob: &dyn Mob) {
        if drop_item(mob) {
            self.cooldown = mob.get_entity().tick_count.load(Ordering::Relaxed)
                + mob.get_random().random_range(0..100);
        }
    }

    fn tick(&mut self, mob: &dyn Mob) {
        Self::swim_to_first(mob);
    }

    fn controls(&self) -> Controls {
        Controls::MOVE
    }
}

/// Vanilla `Dolphin.PlayWithItemsGoal`: an item in its mouth is tossed away again.
struct PlayWithItemsGoal;

impl Goal for PlayWithItemsGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        !held_item(mob).is_empty()
    }

    fn stop(&mut self, mob: &dyn Mob) {
        drop_item(mob);
    }

    fn tick(&mut self, mob: &dyn Mob) {
        drop_item(mob);
    }

    fn controls(&self) -> Controls {
        Controls::empty()
    }
}
