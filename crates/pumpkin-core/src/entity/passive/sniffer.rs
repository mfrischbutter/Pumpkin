use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicI32, Ordering},
};

use pumpkin_data::BlockState;
use pumpkin_data::entity::{EntityStatus, EntityType};
use pumpkin_data::game_event::GameEvent;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::tag::{self, Taggable};
use pumpkin_protocol::bedrock::server::ActorEventID;
use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use rand::RngExt;

use super::sniffer_ai;
use crate::entity::ai::brain::behavior::poi::global_pos;
use crate::entity::ai::brain::memory::{MemoryModuleId, PackedMemories, types};
use crate::entity::ai::brain::sensing::SensorType;
use crate::entity::ai::brain::{Brain, BrainProvider, BrainTick};
use crate::entity::ai::pathfinder::node::PathType;
use crate::entity::ai::util::land_random_pos;
use crate::entity::{
    Entity, EntityBase,
    ageable::{AgeableData, AgeableMob},
    item::ItemEntity,
    mob::{Mob, MobEntity},
    passive::animal::Animal,
    player::Player,
};
use crate::world::loot::{LootContextParameters, generate_loot_from_handle};

pub const SNIFFER_BABY_START_AGE: i32 = -48000;
pub const DIGGING_DROP_SEED_OFFSET_TICKS: i32 = 120;
/// Vanilla `Sniffer.DIGGING_BB_HEIGHT_OFFSET`.
const DIGGING_HEIGHT_OFFSET: f32 = 0.4;
/// Vanilla `DIGGING_DIMENSIONS.withEyeHeight`.
const DIGGING_EYE_HEIGHT: f32 = 0.81;
/// Vanilla `storeExploredPosition` keeps these and adds the new one.
const MAX_EXPLORED_POSITIONS: usize = 20;
/// Vanilla `Sniffer.jumpFromGround`: below this it would not get over the step.
const JUMP_PUSH_BELOW_SPEED_SQR: f64 = 0.01;
const JUMP_PUSH_SPEED: f64 = 0.1;

const MEMORIES: [MemoryModuleId; 1] = [types::SNIFFER_EXPLORED_POSITIONS.id()];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(i32)]
pub enum SnifferState {
    #[default]
    Idling = 0,
    FeelingHappy = 1,
    Scenting = 2,
    Sniffing = 3,
    Searching = 4,
    Digging = 5,
    Rising = 6,
}

impl SnifferState {
    #[must_use]
    pub const fn from_id(id: i32) -> Self {
        match id {
            1 => Self::FeelingHappy,
            2 => Self::Scenting,
            3 => Self::Sniffing,
            4 => Self::Searching,
            5 => Self::Digging,
            6 => Self::Rising,
            _ => Self::Idling,
        }
    }

    #[must_use]
    pub const fn id(self) -> i32 {
        self as i32
    }
}

/// Represents a Sniffer, a large passive mob that digs up ancient seeds.
///
/// Wiki: <https://minecraft.wiki/w/Sniffer>
pub struct SnifferEntity {
    pub mob_entity: MobEntity,
    pub ageable_data: AgeableData,
    pub state: AtomicI32,
    pub drop_seed_at_tick: AtomicI32,
    /// Vanilla `onPathfindingStart`: on fire or in water it may path through water.
    water_allowed: AtomicBool,
}

impl SnifferEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let mut mob_entity = MobEntity::new(entity);
        let navigator = mob_entity
            .navigator
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        navigator.set_can_float(true);
        navigator.set_pathfinding_malus(PathType::Water, -1.0);
        navigator.set_pathfinding_malus(PathType::DangerPowderSnow, -1.0);
        navigator.set_pathfinding_malus(PathType::DamageCautious, -1.0);
        Arc::new(Self {
            mob_entity,
            ageable_data: AgeableData::default(),
            state: AtomicI32::new(SnifferState::Idling.id()),
            drop_seed_at_tick: AtomicI32::new(0),
            water_allowed: AtomicBool::new(false),
        })
    }

    #[must_use]
    pub fn get_state(&self) -> SnifferState {
        SnifferState::from_id(self.state.load(Ordering::Relaxed))
    }

    /// Vanilla `setState` with the part of `onSyncedDataUpdated` that resizes it.
    pub fn set_state(&self, state: SnifferState) {
        let was_digging = self.get_state() == SnifferState::Digging;
        self.state.store(state.id(), Ordering::Relaxed);
        let entity = self.get_entity();
        entity.set_synced_data(
            pumpkin_data::tracked_data::sniffer::STATE,
            VarInt(state.id()),
        );
        let digging = state == SnifferState::Digging;
        if digging != was_digging {
            let full = Entity::type_dimensions(&EntityType::SNIFFER);
            entity.set_default_dimensions(if digging {
                pumpkin_util::math::boundingbox::EntityDimensions::new(
                    full.width,
                    full.height - DIGGING_HEIGHT_OFFSET,
                    DIGGING_EYE_HEIGHT,
                )
            } else {
                full
            });
        }
    }

    /// Vanilla `transitionTo`.
    pub fn transition_to(&self, state: SnifferState) {
        let sound = match state {
            SnifferState::FeelingHappy => Some(Sound::EntitySnifferHappy),
            SnifferState::Sniffing => Some(Sound::EntitySnifferSniffing),
            SnifferState::Rising => Some(Sound::EntitySnifferDiggingStop),
            _ => None,
        };
        if let Some(sound) = sound {
            self.play_entity_sound(sound, 1.0, 1.0);
        }
        self.set_state(state);
        match state {
            SnifferState::Scenting => self.on_scenting_start(),
            SnifferState::Digging => self.on_digging_start(),
            _ => {}
        }
    }

    fn on_scenting_start(&self) {
        let pitch = if self.is_baby() { 1.3 } else { 1.0 };
        self.play_entity_sound(Sound::EntitySnifferScenting, 1.0, pitch);
    }

    fn on_digging_start(&self) {
        let entity = self.get_entity();
        let drop_tick = entity.tick_count.load(Ordering::Relaxed) + DIGGING_DROP_SEED_OFFSET_TICKS;
        self.drop_seed_at_tick.store(drop_tick, Ordering::Relaxed);
        entity.set_synced_data(
            pumpkin_data::tracked_data::sniffer::DROP_SEED_AT_TICK,
            VarInt(drop_tick),
        );
        entity.world.load().send_entity_status(
            entity,
            EntityStatus::SnifferDiggingSound,
            ActorEventID::None,
        );
    }

    /// Vanilla `storeExploredPosition(getOnPos())`.
    pub(super) fn store_explored_position(&self, tick: &mut BrainTick<'_>) {
        let on_pos = self
            .get_entity()
            .get_block_pos_below_that_affects_my_movement();
        let mut explored = tick
            .state
            .get(types::SNIFFER_EXPLORED_POSITIONS)
            .cloned()
            .unwrap_or_default();
        explored.truncate(MAX_EXPLORED_POSITIONS);
        explored.insert(0, global_pos(tick.world, on_pos));
        tick.state.set(types::SNIFFER_EXPLORED_POSITIONS, explored);
    }

    /// Vanilla `getHeadPosition`.
    #[must_use]
    pub fn get_head_position(&self) -> Vector3<f64> {
        let entity = self.get_entity();
        let yaw_rad = f64::from(entity.yaw.load()).to_radians();
        let forward = Vector3::new(-yaw_rad.sin(), 0.0, yaw_rad.cos());
        entity.pos.load() + forward * 2.25
    }

    /// Vanilla `getHeadBlock`.
    #[must_use]
    pub fn get_head_block(&self) -> BlockPos {
        let head_pos = self.get_head_position();
        let y = self.get_entity().pos.load().y + 0.2;
        BlockPos::floored(head_pos.x, y, head_pos.z)
    }

    /// Vanilla `canDig()`, with the brain at hand.
    pub(super) fn can_dig(&self, tick: &BrainTick<'_>) -> bool {
        let entity = self.get_entity();
        !tick.state.has_memory_value(types::IS_PANICKING.id())
            && !sniffer_ai::is_tempted(tick.state)
            && !self.is_baby()
            && !entity.is_in_water()
            && entity.on_ground.load(Ordering::Relaxed)
            && !entity.has_vehicle()
            && self.can_dig_at(tick, self.get_head_block().down())
    }

    /// Vanilla `canDig(BlockPos)`.
    fn can_dig_at(&self, tick: &BrainTick<'_>, pos: BlockPos) -> bool {
        if !tick
            .world
            .get_block(&pos)
            .has_tag(&tag::Block::MINECRAFT_SNIFFER_DIGGABLE_BLOCK)
        {
            return false;
        }
        let spot = global_pos(tick.world, pos);
        let explored = tick
            .state
            .get(types::SNIFFER_EXPLORED_POSITIONS)
            .is_some_and(|explored| {
                explored
                    .iter()
                    .any(|seen| seen.dimension.id == spot.dimension.id && seen.pos == spot.pos)
            });
        if explored {
            return false;
        }
        self.mob_entity
            .navigator
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .create_path(&self.mob_entity.living_entity, pos.to_f64(), 1)
            .is_some_and(|path| path.can_reach())
    }

    /// Vanilla `calculateDigPosition`.
    pub(super) fn calculate_dig_position(&self, tick: &BrainTick<'_>) -> Option<BlockPos> {
        (0..5).find_map(|idx| {
            let found = land_random_pos::get_pos(self, 10 + 2 * idx, 3)?;
            let pos = BlockPos::floored(found.x, found.y, found.z);
            if !tick
                .world
                .worldborder
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .contains_block(pos.0.x, pos.0.z)
            {
                return None;
            }
            let below = pos.down();
            self.can_dig_at(tick, below).then_some(below)
        })
    }

    /// Vanilla `dropSeed`.
    fn drop_seed(&self) {
        let entity = self.get_entity();
        if self.drop_seed_at_tick.load(Ordering::Relaxed)
            != entity.tick_count.load(Ordering::Relaxed)
        {
            return;
        }
        let world = entity.world.load();
        let Some(table) = world.get_loot_table("gameplay/sniffer_digging") else {
            return;
        };
        let this: &dyn EntityBase = self;
        let params = LootContextParameters {
            world: Some(&world),
            position: Some(entity.pos.load()),
            this_entity: Some(this),
            ..Default::default()
        };
        let head = self.get_head_block();
        for stack in generate_loot_from_handle(&table, rand::random(), &params) {
            let item = Entity::new(world.clone(), head.to_f64(), &EntityType::ITEM);
            world.spawn_entity(Arc::new(ItemEntity::new(item, stack)));
        }
        self.play_entity_sound(Sound::EntitySnifferDropSeed, 1.0, 1.0);
    }

    /// The server's part of vanilla `emitDiggingParticles`: the client draws the particles.
    fn emit_digging_game_event(&self) {
        let entity = self.get_entity();
        if entity.tick_count.load(Ordering::Relaxed) % 10 == 0 {
            entity.world.load().emit_game_event(
                GameEvent::EntityAction.name(),
                self.get_head_block().to_f64(),
            );
        }
    }

    /// Vanilla `onPathfindingStart` and `onPathfindingDone`.
    fn update_water_malus(&self) {
        let entity = self.get_entity();
        let allowed = entity.is_on_fire() || entity.is_in_water();
        if self.water_allowed.swap(allowed, Ordering::Relaxed) != allowed {
            self.mob_entity
                .navigator
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .set_pathfinding_malus(PathType::Water, if allowed { 0.0 } else { -1.0 });
        }
    }
}

impl AgeableMob for SnifferEntity {
    fn get_ageable_data(&self) -> &AgeableData {
        &self.ageable_data
    }

    fn get_baby_start_age(&self) -> i32 {
        SNIFFER_BABY_START_AGE
    }
}

/// Vanilla `Sniffer.canMate`: not while it is busy with something else.
const fn is_free_to_mate(state: SnifferState) -> bool {
    matches!(
        state,
        SnifferState::Idling | SnifferState::Scenting | SnifferState::FeelingHappy
    )
}

impl Animal for SnifferEntity {
    fn is_food(&self, item_stack: &ItemStack) -> bool {
        item_stack.item.has_tag(&tag::Item::MINECRAFT_SNIFFER_FOOD)
    }

    fn can_mate(&self, partner: &dyn EntityBase) -> bool {
        is_free_to_mate(self.get_state())
            && partner
                .cast_any()
                .downcast_ref::<Self>()
                .is_some_and(|partner| is_free_to_mate(partner.get_state()))
    }

    /// Vanilla `Sniffer.spawnChildFromBreeding`: no baby, an egg on the ground.
    fn breeds_eggs(&self) -> bool {
        true
    }

    fn on_bred_egg(&self) {
        let entity = self.get_entity();
        let world = entity.world.load();
        let pos = entity.pos.load();
        let egg = Entity::new(world.clone(), pos, &EntityType::ITEM);
        world.spawn_entity(Arc::new(ItemEntity::new(
            egg,
            ItemStack::new(1, &Item::SNIFFER_EGG),
        )));
        let mut random = self.get_random();
        let spread = random.random::<f32>() - random.random::<f32>();
        world.play_sound_fine(
            Sound::BlockSnifferEggPlop,
            SoundCategory::Neutral,
            &pos,
            1.0,
            spread * 0.2 + 0.5,
        );
    }
}

impl Mob for SnifferEntity {
    /// Vanilla `supportQuadLeash`.
    fn mob_support_quad_leash(&self) -> bool {
        true
    }

    fn make_brain(&self, packed: &PackedMemories) -> Brain {
        BrainProvider {
            memory_types: &MEMORIES,
            sensor_types: &[
                SensorType::NearestLivingEntities,
                SensorType::HurtBy,
                SensorType::NearestPlayers,
                SensorType::FoodTemptations,
            ],
            activities: sniffer_ai::activities,
        }
        .make_brain(self, packed)
    }

    /// Vanilla `Sniffer.customServerAiStep`.
    fn custom_server_ai_step(&self, _caller: &dyn EntityBase) {
        self.update_water_malus();
        self.mob_entity.tick_brain(self);
        self.mob_entity
            .with_brain(self, |brain| sniffer_ai::update_activity(brain));
    }

    /// Vanilla `Sniffer.jumpFromGround`.
    fn on_jump_from_ground(&self) {
        let speed = self
            .mob_entity
            .move_control
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get_speed_modifier();
        let entity = self.get_entity();
        if speed > 0.0
            && entity.velocity.load().horizontal_length_squared() < JUMP_PUSH_BELOW_SPEED_SQR
        {
            entity.update_velocity_from_input(Vector3::new(0.0, 0.0, 1.0), JUMP_PUSH_SPEED);
        }
    }

    /// Vanilla `Sniffer.die`.
    fn mob_on_die(&self, _killer: Option<&dyn EntityBase>) {
        self.transition_to(SnifferState::Idling);
    }

    /// Vanilla `Sniffer.getMaxHeadYRot`.
    fn get_max_head_rotation(&self) -> f32 {
        50.0
    }

    /// Vanilla `Sniffer.getAmbientSound`: quiet at work.
    fn get_ambient_sound(&self) -> Option<Sound> {
        (!matches!(
            self.get_state(),
            SnifferState::Digging | SnifferState::Searching
        ))
        .then_some(Sound::EntitySnifferIdle)
    }

    fn mob_play_step_sound(&self, _pos: &BlockPos, _state: &BlockState) {
        self.play_entity_sound(Sound::EntitySnifferStep, 0.15, 1.0);
    }

    fn as_ageable(&self) -> Option<&dyn AgeableMob> {
        Some(self)
    }

    fn as_animal(&self) -> Option<&dyn Animal> {
        Some(self)
    }

    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    /// Vanilla `Sniffer.tick`; the searching sound is the client's own.
    fn mob_tick(&self, _caller: &dyn EntityBase) {
        if self.get_state() == SnifferState::Digging {
            self.emit_digging_game_event();
            self.drop_seed();
        }
    }

    fn mob_init_data_tracker(&self) {
        let entity = self.get_entity();
        let is_baby = entity.age.load(Ordering::Relaxed) < 0;
        if is_baby {
            entity.set_synced_data(pumpkin_data::tracked_data::sniffer::BABY_ID, true);
        }
        entity.set_synced_data(
            pumpkin_data::tracked_data::sniffer::STATE,
            VarInt(self.get_state().id()),
        );
        entity.set_synced_data(
            pumpkin_data::tracked_data::sniffer::DROP_SEED_AT_TICK,
            VarInt(self.drop_seed_at_tick.load(Ordering::Relaxed)),
        );
    }

    fn mob_interact(&self, player: &Arc<Player>, item_stack: &mut ItemStack) -> bool {
        self.animal_interact(player, item_stack, Sound::EntitySnifferEat)
    }
}
