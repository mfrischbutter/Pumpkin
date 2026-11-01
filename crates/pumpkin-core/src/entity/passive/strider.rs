use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};

use pumpkin_data::BlockState;
use pumpkin_data::attributes::Attributes;
use pumpkin_data::data_component_impl::EquipmentSlot;
use pumpkin_data::entity::EntityType;
use pumpkin_data::fluid::Fluid;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::Sound;
use pumpkin_data::tag::{self, Taggable};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::math::boundingbox::BoundingBox;
use pumpkin_util::math::position::BlockPos;
use rand::RngExt;
use uuid::Uuid;

use crate::entity::ai::goal::move_to_target_pos::{MoveToTargetPos, MoveToTargetPosGoal};
use crate::entity::ai::goal::{Controls, Goal, GoalReach, ParentHandle};
use crate::entity::ai::pathfinder::node::{PathComputationType, PathType};
use crate::entity::attributes::{Modifier, ModifierOperation};
use crate::entity::custom_sound::CustomSound;
use crate::entity::r#type::from_type;
use crate::entity::{
    Entity, EntityBase,
    ageable::{AgeableData, AgeableMob},
    ai::goal::{
        breed::BreedGoal, escape_danger::EscapeDangerGoal, follow_parent::FollowParentGoal,
        look_around::RandomLookAroundGoal, look_at_entity::LookAtEntityGoal, tempt::TemptGoal,
        wander_around::WanderAroundGoal,
    },
    item_steerable::{ItemBasedSteering, ItemSteerable},
    mob::{Mob, MobEntity, saddle, spawn::SpawnGroupData},
    passive::animal::Animal,
    player::Player,
};
use crate::world::World;
use crate::world::level_accessor::LevelAccessor;

/// Vanilla `Strider.SUFFOCATING_MODIFIER`: a cold strider walks a third slower.
const SUFFOCATING_MODIFIER: &str = "minecraft:suffocating";

/// Vanilla `Strider.getLiquidCollisionShape`: it stands on the lower half of a lava block.
const LIQUID_COLLISION_SHAPE: BoundingBox =
    BoundingBox::new_array([0.0, 0.0, 0.0], [1.0, 0.5, 1.0]);

const fn is_lava(fluid: &Fluid) -> bool {
    fluid.id == Fluid::LAVA.id || fluid.id == Fluid::FLOWING_LAVA.id
}

pub struct StriderEntity {
    pub mob_entity: MobEntity,
    pub ageable_data: AgeableData,
    pub steering: ItemBasedSteering,
    pub suffocating: AtomicBool,
    /// Vanilla `isBeingTempted`, kept by [`StriderTemptGoal`].
    tempted: Arc<AtomicBool>,
}

impl StriderEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let mut mob_entity = MobEntity::new(entity);
        Self::set_up_navigation(&mut mob_entity);
        let tempted = Arc::new(AtomicBool::new(false));
        let strider = Self {
            mob_entity,
            ageable_data: AgeableData::default(),
            steering: ItemBasedSteering::default(),
            suffocating: AtomicBool::new(false),
            tempted: tempted.clone(),
        };
        let mob_arc = Arc::new(strider);
        let mob_weak: Weak<dyn Mob> = {
            let mob_arc: Arc<dyn Mob> = mob_arc.clone();
            Arc::downgrade(&mob_arc)
        };

        {
            // Vanilla `Strider.registerGoals`.
            let goal_selector = &mob_arc.mob_entity.goals_selector;
            goal_selector.add_goal(1, EscapeDangerGoal::new(1.65));
            goal_selector.add_goal(2, BreedGoal::new(1.0));
            goal_selector.add_goal(
                3,
                Box::new(StriderTemptGoal {
                    inner: TemptGoal::with_tag(
                        1.4,
                        &tag::Item::MINECRAFT_STRIDER_TEMPT_ITEMS,
                        false,
                    ),
                    tempted,
                }),
            );
            goal_selector.add_goal(4, Box::new(StriderGoToLavaGoal::new()));
            goal_selector.add_goal(5, Box::new(FollowParentGoal::new(1.0)));
            goal_selector.add_goal(7, Box::new(WanderAroundGoal::with_interval(1.0, 60)));
            goal_selector.add_goal(
                8,
                LookAtEntityGoal::with_default(mob_weak.clone(), &EntityType::PLAYER, 8.0),
            );
            goal_selector.add_goal(8, Box::new(RandomLookAroundGoal::default()));
            goal_selector.add_goal(
                9,
                LookAtEntityGoal::with_default(mob_weak, &EntityType::STRIDER, 8.0),
            );
        };

        mob_arc
    }

    /// Vanilla `Strider` constructor: lava and fire are fine, water is not.
    fn set_up_navigation(mob_entity: &mut MobEntity) {
        let navigator = mob_entity
            .navigator
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        navigator.set_pathfinding_malus(PathType::Water, -1.0);
        navigator.set_pathfinding_malus(PathType::Lava, 0.0);
        navigator.set_pathfinding_malus(PathType::DangerFire, 0.0);
        navigator.set_pathfinding_malus(PathType::DamageFire, 0.0);
    }

    #[must_use]
    pub fn is_suffocating(&self) -> bool {
        self.suffocating.load(Ordering::Relaxed)
    }

    /// Vanilla `setSuffocating`, with the slowness of the cold.
    pub fn set_suffocating(&self, suffocating: bool) {
        if self.suffocating.swap(suffocating, Ordering::Relaxed) == suffocating {
            return;
        }
        self.mob_entity
            .living_entity
            .update_attribute(&Attributes::MOVEMENT_SPEED, |inst| {
                if suffocating {
                    inst.add_or_replace_modifier(Modifier {
                        id: SUFFOCATING_MODIFIER.to_string(),
                        amount: -0.34,
                        operation: ModifierOperation::MultiplyBase,
                    });
                } else {
                    inst.remove_modifier(SUFFOCATING_MODIFIER);
                }
            });
        let entity = self.get_entity();
        entity.set_synced_data(
            pumpkin_data::tracked_data::strider::DATA_SUFFOCATING,
            suffocating,
        );
    }

    /// Vanilla `Strider.tick`: sounds, the cold outside lava, and floating up in it.
    fn tick_strider(&self) {
        let living = &self.mob_entity.living_entity;
        let entity = &living.entity;
        let sound = {
            let mut rng = self.get_random();
            if self.tempted.load(Ordering::Relaxed) && rng.random_range(0..140) == 0 {
                Some(Sound::EntityStriderHappy)
            } else if self.is_panicking() && rng.random_range(0..60) == 0 {
                Some(Sound::EntityStriderRetreat)
            } else {
                None
            }
        };
        if let Some(sound) = sound {
            self.play_entity_sound(sound, 1.0, 1.0);
        }
        if !self.mob_entity.is_no_ai() {
            let world = entity.world.load();
            let pos = entity.pos.load();
            let on = BlockPos::floored(pos.x, pos.y - 0.2, pos.z);
            let warm = |pos: &BlockPos| {
                world
                    .get_block(pos)
                    .has_tag(&tag::Block::MINECRAFT_STRIDER_WARM_BLOCKS)
            };
            let in_warm_blocks =
                warm(&entity.block_pos.load()) || warm(&on) || entity.lava_height.load() > 0.0;
            let on_warm_strider = entity.get_vehicle().is_some_and(|vehicle| {
                vehicle
                    .cast_any()
                    .downcast_ref::<Self>()
                    .is_some_and(|strider| !strider.is_suffocating())
            });
            self.set_suffocating(!in_warm_blocks && !on_warm_strider);
        }
        if entity.touching_lava.load(Ordering::Relaxed) {
            // Vanilla `checkFallDamage`: lava breaks the fall.
            living.fall_distance.store(0.0);
            self.float_strider();
        }
    }

    /// Vanilla `floatStrider`: on the surface it stands, below it rises.
    fn float_strider(&self) {
        let entity = self.get_entity();
        let block_pos = entity.block_pos.load();
        let above_shape = entity.bounding_box.load().min.y
            > f64::from(block_pos.0.y) + LIQUID_COLLISION_SHAPE.max.y - 1.0e-5;
        let lava_above = is_lava(entity.world.load().get_fluid(&block_pos.up()));
        if above_shape && !lava_above {
            entity.on_ground.store(true, Ordering::Relaxed);
        } else {
            entity
                .velocity
                .store((entity.velocity.load() * 0.5).add_raw(0.0, 0.05, 0.0));
        }
    }

    /// Vanilla `spawnJockey`: the rider spawns finalized on top.
    fn spawn_jockey(&self, world: &Arc<World>, jockey: &Arc<dyn EntityBase>) {
        let entity = self.get_entity();
        jockey.get_entity().set_rotation(entity.yaw.load(), 0.0);
        crate::entity::mob::spawn::finalize_spawn(
            jockey,
            world,
            crate::entity::mob::spawn::EntitySpawnReason::Jockey,
            None,
        );
        self.mob_entity.add_pending_rider(jockey.clone());
    }
}

impl AgeableMob for StriderEntity {
    fn get_ageable_data(&self) -> &AgeableData {
        &self.ageable_data
    }

    fn default_baby_spawn_chance(&self) -> f32 {
        crate::entity::ageable::STRIDER_BABY_SPAWN_CHANCE
    }
}

impl Animal for StriderEntity {
    /// Vanilla `Strider.isFood`.
    fn is_food(&self, item_stack: &ItemStack) -> bool {
        item_stack.item.has_tag(&tag::Item::MINECRAFT_STRIDER_FOOD)
    }
}

impl CustomSound for StriderEntity {
    fn hurt_sound(&self) -> Option<Sound> {
        Some(Sound::EntityStriderHurt)
    }

    fn death_sound(&self) -> Option<Sound> {
        Some(Sound::EntityStriderDeath)
    }
}

impl Mob for StriderEntity {
    /// Vanilla `Strider.getWalkTargetValue`: lava is worth 10; otherwise nothing, or never
    /// while the strider is in lava.
    fn get_walk_target_value(&self, pos: &BlockPos, level: &dyn LevelAccessor) -> f32 {
        if level.get_fluid(pos).has_tag(&tag::Fluid::MINECRAFT_LAVA) {
            10.0
        } else if self
            .get_entity()
            .touching_lava
            .load(std::sync::atomic::Ordering::Relaxed)
        {
            f32::NEG_INFINITY
        } else {
            0.0
        }
    }

    /// Vanilla `Strider.checkSpawnObstruction`: only entities obstruct, the lava does not.
    fn check_spawn_obstruction(&self, level: &dyn LevelAccessor) -> bool {
        level.is_unobstructed(&self.get_mob_entity().living_entity.entity)
    }

    /// Vanilla `Strider.getAmbientSound`: silent while fleeing or tempted.
    fn get_ambient_sound(&self) -> Option<Sound> {
        (!self.is_panicking() && !self.tempted.load(Ordering::Relaxed))
            .then_some(Sound::EntityStriderAmbient)
    }

    fn as_custom_sound(&self) -> Option<&dyn CustomSound> {
        Some(self)
    }

    fn is_sensitive_to_water(&self) -> bool {
        true
    }

    /// Vanilla `Strider.canStandOnFluid`.
    fn mob_can_stand_on_fluid(&self, fluid: &Fluid) -> bool {
        is_lava(fluid)
    }

    fn mob_liquid_collision_shape(&self) -> Option<BoundingBox> {
        Some(LIQUID_COLLISION_SHAPE)
    }

    fn mob_tick(&self, _caller: &dyn EntityBase) {
        self.tick_strider();
    }

    /// Vanilla `Strider.finalizeSpawn`: 1 in 30 adults carry a zombified piglin with a warped
    /// fungus on a stick and a saddle, 1 in 10 of the rest a baby strider.
    fn finalize_spawn(
        &self,
        world: &Arc<World>,
        group_data: Option<SpawnGroupData>,
    ) -> Option<SpawnGroupData> {
        if self.is_baby() {
            return self.finalize_spawn_super(group_data);
        }
        let (piglin, baby) = {
            let mut rng = self.get_random();
            let piglin = rng.random_range(0..30) == 0 && world.should_spawn_monsters();
            (piglin, !piglin && rng.random_range(0..10) == 0)
        };
        let pos = self.get_entity().pos.load();
        let baby_spawn_chance = if piglin {
            let jockey = from_type(&EntityType::ZOMBIFIED_PIGLIN, pos, world, Uuid::new_v4());
            self.spawn_jockey(world, &jockey);
            if let Some(rider) = jockey.get_mob() {
                rider.get_mob_entity().set_item_slot(
                    &EquipmentSlot::MAIN_HAND,
                    ItemStack::new(1, &Item::WARPED_FUNGUS_ON_A_STICK),
                );
            }
            self.mob_entity.set_item_slot_and_drop_when_killed(
                &EquipmentSlot::SADDLE,
                ItemStack::new(1, &Item::SADDLE),
            );
            0.0
        } else if baby {
            let jockey = from_type(&EntityType::STRIDER, pos, world, Uuid::new_v4());
            jockey.get_entity().set_age(-24000);
            self.spawn_jockey(world, &jockey);
            0.0
        } else {
            crate::entity::ageable::STRIDER_BABY_SPAWN_CHANCE
        };
        self.finalize_spawn_super(group_data.or(Some(SpawnGroupData::Ageable {
            group_size: 0,
            should_spawn_baby: true,
            baby_spawn_chance,
        })))
    }

    fn mob_play_step_sound(&self, _pos: &BlockPos, _state: &BlockState) {
        let sound = if self.get_entity().touching_lava.load(Ordering::Relaxed) {
            Sound::EntityStriderStepLava
        } else {
            Sound::EntityStriderStep
        };
        self.play_entity_sound(sound, 1.0, 1.0);
    }

    fn as_ageable(&self) -> Option<&dyn AgeableMob> {
        Some(self)
    }

    fn as_animal(&self) -> Option<&dyn Animal> {
        Some(self)
    }

    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        self.write_ageable_nbt(nbt);
    }

    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        self.read_ageable_nbt(nbt);
        saddle::read_legacy_saddle(self, nbt);
    }

    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    fn get_item_steerable(&self) -> Option<&dyn ItemSteerable> {
        Some(self)
    }

    /// Vanilla `Strider.canUseSlot(SADDLE)`.
    fn can_be_saddled(&self) -> bool {
        self.mob_entity.living_entity.entity.is_alive() && !self.is_baby()
    }

    fn saddle_sound(&self) -> Sound {
        Sound::EntityStriderSaddle
    }

    /// Vanilla `Strider.getControllingPassenger`: a rider holding a warped fungus on a stick.
    fn mob_controlling_player(&self) -> Option<Arc<Player>> {
        if !self.is_saddled() {
            return None;
        }
        saddle::first_passenger_player(self)
            .filter(|player| saddle::is_holding(player, &Item::WARPED_FUNGUS_ON_A_STICK))
    }

    /// Vanilla `Strider.tickRidden`.
    fn mob_tick_ridden(&self, controller: &Player) {
        saddle::face_rider(self, controller, 0.5);
        self.steering.tick_boost();
    }

    fn mob_init_data_tracker(&self) {
        let entity = self.get_entity();
        let is_baby = entity.age.load(Ordering::Relaxed) < 0;
        if is_baby {
            entity.set_synced_data(pumpkin_data::tracked_data::strider::DATA_BABY_ID, true);
        }
        entity.set_synced_data(
            pumpkin_data::tracked_data::strider::DATA_SUFFOCATING,
            self.is_suffocating(),
        );
    }

    fn mob_interact(&self, player: &Arc<Player>, item_stack: &mut ItemStack) -> bool {
        let has_food = self.is_food(item_stack);
        if self.is_saddled() && !has_food {
            let world = player.world();
            let ent = &self.mob_entity.living_entity.entity;
            if let Some(vehicle) = world.get_entity_by_id(ent.entity_id)
                && let Some(passenger) = world.get_player_by_id(player.entity_id())
            {
                ent.add_passenger(vehicle, passenger as Arc<dyn EntityBase>);
                return true;
            }
        }

        let consumed = self.animal_interact(player, item_stack, Sound::EntityStriderAmbient);
        // Vanilla `Strider.mobInteract`: eating plays its own sound.
        if consumed && has_food && !self.get_entity().is_silent() {
            let pitch = {
                let mut rng = self.get_random();
                (rng.random::<f32>() - rng.random::<f32>()).mul_add(0.2, 1.0)
            };
            let entity = self.get_entity();
            entity.world.load().play_sound_raw(
                Sound::EntityStriderEat as u16,
                self.mob_sound_source(),
                &entity.pos.load(),
                1.0,
                pitch,
            );
        }
        consumed || saddle::try_equip_saddle(self, player, item_stack)
    }
}

impl ItemSteerable for StriderEntity {
    /// Vanilla `Strider.boost`: the boost time goes to the rider's client, which moves it.
    fn boost(&self) -> bool {
        let boosted = self.steering.boost();
        if boosted {
            self.get_entity().set_synced_data(
                pumpkin_data::tracked_data::strider::DATA_BOOST_TIME,
                pumpkin_protocol::codec::var_int::VarInt(
                    self.steering.boost_time_total.load(Ordering::Relaxed),
                ),
            );
        }
        boosted
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// Vanilla `TemptGoal` as `Strider.temptGoal`: the strider knows while it is tempted.
struct StriderTemptGoal {
    inner: TemptGoal,
    tempted: Arc<AtomicBool>,
}

impl Goal for StriderTemptGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        self.inner.can_start(mob)
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        self.inner.should_continue(mob)
    }

    fn start(&mut self, mob: &dyn Mob) {
        self.inner.start(mob);
        self.tempted.store(true, Ordering::Relaxed);
    }

    fn stop(&mut self, mob: &dyn Mob) {
        self.inner.stop(mob);
        self.tempted.store(false, Ordering::Relaxed);
    }

    fn tick(&mut self, mob: &dyn Mob) {
        self.inner.tick(mob);
    }

    fn should_run_every_tick(&self) -> bool {
        self.inner.should_run_every_tick()
    }

    fn controls(&self) -> Controls {
        self.inner.controls()
    }

    fn tick_reach(&self, mob: &dyn Mob, running: bool) -> Option<GoalReach> {
        self.inner.tick_reach(mob, running)
    }
}

/// Lava with room to stand above, vanilla `StriderGoToLavaGoal.isValidTarget`.
struct LavaTarget;

impl MoveToTargetPos for LavaTarget {
    fn is_target_pos(&self, world: Arc<World>, block_pos: BlockPos) -> bool {
        let above = block_pos.up();
        let (block, state) = world.get_block_and_state(&above);
        world.get_block(&block_pos) == &pumpkin_data::Block::LAVA
            && world
                .block_registry
                .is_pathfindable(block, state, PathComputationType::Land)
    }

    /// Vanilla `StriderGoToLavaGoal.getMoveToTarget`: into the lava itself.
    fn moves_above_target(&self) -> bool {
        false
    }

    fn path_recalculation_interval(&self) -> i32 {
        20
    }
}

/// Vanilla `Strider.StriderGoToLavaGoal`: out of lava, it walks to lava within 8 blocks.
struct StriderGoToLavaGoal {
    inner: MoveToTargetPosGoal<LavaTarget>,
    _target: Box<LavaTarget>,
}

impl StriderGoToLavaGoal {
    fn new() -> Self {
        let target = Box::new(LavaTarget);
        // SAFETY: the target has its own heap allocation owned by this goal, so it never moves
        // and outlives the handle, and it is only ever read through shared references.
        let handle: ParentHandle<LavaTarget> = unsafe { ParentHandle::new(&target) };
        Self {
            inner: MoveToTargetPosGoal::new(handle, 1.0, 8, 2),
            _target: target,
        }
    }
}

impl Goal for StriderGoToLavaGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        !mob.get_entity().touching_lava.load(Ordering::Relaxed) && self.inner.can_start(mob)
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        let entity = mob.get_entity();
        !entity.touching_lava.load(Ordering::Relaxed)
            && LavaTarget.is_target_pos(entity.world.load_full(), self.inner.target_pos)
    }

    fn start(&mut self, mob: &dyn Mob) {
        self.inner.start(mob);
    }

    fn stop(&mut self, mob: &dyn Mob) {
        self.inner.stop(mob);
    }

    fn tick(&mut self, mob: &dyn Mob) {
        self.inner.tick(mob);
    }

    fn should_run_every_tick(&self) -> bool {
        true
    }

    fn controls(&self) -> Controls {
        self.inner.controls()
    }
}
