use std::sync::{
    Arc, Mutex,
    atomic::{AtomicI32, AtomicI64, Ordering},
};

use crossbeam::atomic::AtomicCell;
use pumpkin_data::block_properties::{CopperGolemPose, CopperGolemStatueLikeProperties};
use pumpkin_data::damage::DamageType;
use pumpkin_data::data_component_impl::EquipmentSlot;
use pumpkin_data::game_event::GameEvent;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::tag::{self, Taggable};
use pumpkin_data::world::WorldEvent;
use pumpkin_data::{Block, BlockState};
use pumpkin_inventory::Inventory;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::text::TextComponent;
use pumpkin_world::world::BlockFlags;
use rand::RngExt;
use uuid::Uuid;

use super::copper_golem_ai;
use crate::block::blocks::weathering_copper::WeatherState;
use crate::block::entities::copper_golem_statue::CopperGolemStatueBlockEntity;
use crate::entity::ai::brain::behavior::utils::throw_item;
use crate::entity::ai::brain::memory::{PackedMemories, types};
use crate::entity::ai::brain::sensing::SensorType;
use crate::entity::ai::brain::{Brain, BrainProvider};
use crate::entity::ai::pathfinder::node::PathType;
use crate::entity::mob::spawn::SpawnGroupData;
use crate::entity::{
    Entity, EntityBase,
    custom_sound::CustomSound,
    mob::{Mob, MobEntity},
    player::Player,
};
use crate::world::World;

/// Vanilla `CopperGolem.IGNORE_WEATHERING_TICK`: the golem is waxed.
const IGNORE_WEATHERING_TICK: i64 = -2;
/// Vanilla `CopperGolem.UNSET_WEATHERING_TICK`.
const UNSET_WEATHERING_TICK: i64 = -1;
/// Vanilla `CopperGolem.WEATHERING_TICK_FROM`.
const WEATHERING_TICK_FROM: i64 = 504_000;
/// Vanilla `CopperGolem.WEATHERING_TICK_TO`.
const WEATHERING_TICK_TO: i64 = 552_000;
/// Vanilla `CopperGolem.TURN_TO_STATUE_CHANCE`.
const TURN_TO_STATUE_CHANCE: f32 = 0.0058;
/// Vanilla `CopperGolem.SPAWN_COOLDOWN_MIN`.
const SPAWN_COOLDOWN_MIN: i32 = 60;
/// Vanilla `CopperGolem.SPAWN_COOLDOWN_MAX`.
const SPAWN_COOLDOWN_MAX: i32 = 100;
/// The next weathering tick of a fully oxidized golem in vanilla `updateWeathering`.
const OXIDIZED_WEATHERING_TICK: i64 = 0;
/// How high vanilla `CopperGolem.shear` drops the antenna item.
const SHEARED_ITEM_DROP_HEIGHT: f64 = 1.5;
/// Vanilla `CopperGolemStatueBlock.Pose.values`.
const STATUE_POSES: [CopperGolemPose; 4] = [
    CopperGolemPose::Standing,
    CopperGolemPose::Sitting,
    CopperGolemPose::Running,
    CopperGolemPose::Star,
];
/// Vanilla `CopperGolem.EQUIPMENT_SLOT_ANTENNA`.
pub const EQUIPMENT_SLOT_ANTENNA: EquipmentSlot = EquipmentSlot::SADDLE;

/// Vanilla `CopperGolemState`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(i32)]
pub enum CopperGolemState {
    #[default]
    Idle = 0,
    GettingItem = 1,
    GettingNoItem = 2,
    DroppingItem = 3,
    DroppingNoItem = 4,
}

impl CopperGolemState {
    #[must_use]
    pub const fn from_id(id: i32) -> Self {
        match id {
            1 => Self::GettingItem,
            2 => Self::GettingNoItem,
            3 => Self::DroppingItem,
            4 => Self::DroppingNoItem,
            _ => Self::Idle,
        }
    }

    #[must_use]
    pub const fn id(self) -> i32 {
        self as i32
    }
}

/// Represents a Copper Golem, a passive creation mob that sorts copper chests into chests.
///
/// Wiki: <https://minecraft.wiki/w/Copper_Golem>
pub struct CopperGolemEntity {
    pub mob_entity: MobEntity,
    pub weather_state: AtomicI32,
    pub state: AtomicI32,
    pub next_weathering_tick: AtomicI64,
    /// Vanilla `lastLightningBoltUUID`.
    last_lightning_bolt: AtomicCell<Option<Uuid>>,
    /// Vanilla `openedChestPos` with the container it opened. Vanilla's openers counter drops a
    /// golem that no longer has the chest open on its next recheck; here clearing closes it.
    opened_chest: Mutex<Option<Arc<dyn Inventory>>>,
}

impl CopperGolemEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let mut mob_entity = MobEntity::new(entity);
        Self::set_up_movement(&mut mob_entity);
        let golem = Arc::new(Self {
            mob_entity,
            weather_state: AtomicI32::new(WeatherState::Unaffected.ordinal() as i32),
            state: AtomicI32::new(CopperGolemState::Idle.id()),
            next_weathering_tick: AtomicI64::new(UNSET_WEATHERING_TICK),
            last_lightning_bolt: AtomicCell::new(None),
            opened_chest: Mutex::new(None),
        });
        // Vanilla `CopperGolem`'s constructor: a copper golem never counts as transient.
        golem
            .mob_entity
            .persistence_required
            .store(true, Ordering::Relaxed);
        let cooldown = golem
            .get_random()
            .random_range(SPAWN_COOLDOWN_MIN..SPAWN_COOLDOWN_MAX);
        golem.mob_entity.with_brain(&*golem, |brain| {
            brain.set(types::TRANSPORT_ITEMS_COOLDOWN_TICKS, cooldown);
        });
        golem
    }

    /// The navigation of vanilla `CopperGolem`'s constructor.
    fn set_up_movement(mob_entity: &mut MobEntity) {
        let navigator = mob_entity
            .navigator
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        navigator.set_required_path_length(48.0);
        navigator.set_can_open_doors(true);
        navigator.set_pathfinding_malus(PathType::DangerFire, 16.0);
        navigator.set_pathfinding_malus(PathType::DangerOther, 16.0);
        navigator.set_pathfinding_malus(PathType::DamageFire, -1.0);
    }

    #[must_use]
    pub fn get_weather_state(&self) -> WeatherState {
        WeatherState::from_ordinal(self.weather_state.load(Ordering::Relaxed) as usize)
            .unwrap_or(WeatherState::Unaffected)
    }

    pub fn set_weather_state(&self, state: WeatherState) {
        let id = state.ordinal() as i32;
        self.weather_state.store(id, Ordering::Relaxed);
        self.get_entity().set_synced_data(
            pumpkin_data::tracked_data::copper_golem::WEATHER_STATE,
            VarInt(id),
        );
    }

    #[must_use]
    pub fn get_state(&self) -> CopperGolemState {
        CopperGolemState::from_id(self.state.load(Ordering::Relaxed))
    }

    pub fn set_state(&self, state: CopperGolemState) {
        self.state.store(state.id(), Ordering::Relaxed);
        self.get_entity().set_synced_data(
            pumpkin_data::tracked_data::copper_golem::COPPER_GOLEM_STATE,
            VarInt(state.id()),
        );
    }

    /// Vanilla `Container.startOpen` with `setOpenedChestPos`.
    pub fn set_opened_chest(&self, container: Arc<dyn Inventory>) {
        container.on_open();
        let previous = self
            .opened_chest
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .replace(container);
        if let Some(previous) = previous {
            previous.on_close();
        }
    }

    /// Vanilla `clearOpenedChestPos`, closing the container as vanilla's `stopOpen` or the
    /// openers counter's recheck would.
    pub fn clear_opened_chest(&self) {
        let opened = self
            .opened_chest
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        if let Some(opened) = opened {
            opened.on_close();
        }
    }

    /// Vanilla `CopperGolem.spawn`, for a golem built from a pattern.
    pub fn spawn(&self, weather_state: WeatherState) {
        self.set_weather_state(weather_state);
        self.play_spawn_sound();
    }

    /// Vanilla `CopperGolem.playSpawnSound`.
    pub fn play_spawn_sound(&self) {
        self.play_entity_sound(Sound::EntityCopperGolemSpawn, 1.0, 1.0);
    }

    /// Vanilla `CopperGolem.readyForShearing`.
    #[must_use]
    pub fn ready_for_shearing(&self) -> bool {
        self.mob_entity
            .get_item_by_slot(&EQUIPMENT_SLOT_ANTENNA)
            .item
            .has_tag(&tag::Item::MINECRAFT_SHEARABLE_FROM_COPPER_GOLEM)
    }

    /// Vanilla `CopperGolem.shear`.
    pub fn shear(&self, category: SoundCategory) {
        let entity = self.get_entity();
        let world = entity.world.load_full();
        let pos = entity.pos.load();
        world.play_sound_fine(Sound::EntityCopperGolemShear, category, &pos, 1.0, 1.0);
        let antenna = self
            .mob_entity
            .set_item_slot(&EQUIPMENT_SLOT_ANTENNA, ItemStack::EMPTY.clone());
        world.drop_stack_at(pos.add_raw(0.0, SHEARED_ITEM_DROP_HEIGHT, 0.0), antenna);
    }

    /// Vanilla `CopperGolem.updateWeathering`.
    fn update_weathering(&self) {
        let next_weathering_tick = self.next_weathering_tick.load(Ordering::Relaxed);
        if next_weathering_tick == IGNORE_WEATHERING_TICK {
            return;
        }
        let world = self.get_entity().world.load_full();
        let game_time = world.get_world_age();
        if next_weathering_tick == UNSET_WEATHERING_TICK {
            self.next_weathering_tick.store(
                game_time + self.random_weathering_delay(),
                Ordering::Relaxed,
            );
            return;
        }
        let weather_state = self.get_weather_state();
        match weather_state.next() {
            None => {
                if self.can_turn_to_statue(&world) {
                    self.turn_to_statue(&world);
                }
            }
            Some(new_state) if game_time >= next_weathering_tick => {
                self.set_weather_state(new_state);
                let next = if new_state == WeatherState::Oxidized {
                    OXIDIZED_WEATHERING_TICK
                } else {
                    next_weathering_tick + self.random_weathering_delay()
                };
                self.next_weathering_tick.store(next, Ordering::Relaxed);
            }
            Some(_) => {}
        }
    }

    fn random_weathering_delay(&self) -> i64 {
        self.get_random()
            .random_range(WEATHERING_TICK_FROM..=WEATHERING_TICK_TO)
    }

    /// Vanilla `CopperGolem.canTurnToStatue`.
    fn can_turn_to_statue(&self, world: &World) -> bool {
        world
            .get_block_state(&self.get_entity().block_pos.load())
            .is_air()
            && self.get_random().random::<f32>() <= TURN_TO_STATUE_CHANCE
    }

    /// Vanilla `CopperGolem.turnToStatue`.
    fn turn_to_statue(&self, world: &Arc<World>) {
        let entity = self.get_entity();
        let pos = entity.block_pos.load();
        let block = &Block::OXIDIZED_COPPER_GOLEM_STATUE;
        let mut props = CopperGolemStatueLikeProperties::default(block);
        props.copper_golem_pose =
            STATUE_POSES[self.get_random().random_range(0..STATUE_POSES.len())];
        props.facing = entity.get_horizontal_facing();
        world.set_block_state(&pos, props.to_state_id(block), BlockFlags::NOTIFY_ALL);
        let Some(block_entity) = world.get_block_entity(&pos) else {
            return;
        };
        let Some(statue) = block_entity
            .as_any()
            .downcast_ref::<CopperGolemStatueBlockEntity>()
        else {
            return;
        };
        let custom_name = (*entity.custom_name.load_full()).clone();
        statue.create_statue(custom_name.map(TextComponent::get_text));
        self.mob_entity.drop_preserved_equipment(world);
        entity.remove();
        self.play_entity_sound(Sound::EntityCopperGolemBecomeStatue, 1.0, 1.0);
        if entity.is_leashed() {
            if world.level_info.load().game_rules.entity_drops {
                entity.unleash();
                world.drop_stack(&pos, ItemStack::new(1, &Item::LEAD));
            } else {
                entity.remove_leash();
            }
        }
    }

    /// The shears branch of vanilla `CopperGolem.mobInteract`, with Pumpkin's plugin event.
    fn try_shear(&self, player: &Arc<Player>, item_stack: &mut ItemStack) -> bool {
        let entity = self.get_entity();
        let world = entity.world.load();
        if let Some(server) = world.server.upgrade() {
            let mut event =
                crate::plugin::api::events::player::player_shear_entity::PlayerShearEntityEvent {
                    player: player.clone(),
                    entity_id: entity.entity_id,
                    hand: 0,
                    cancelled: false,
                };
            server.plugin_manager.fire_blocking(&server, &mut event);
            if event.cancelled {
                return false;
            }
        }
        self.shear(SoundCategory::Players);
        world.emit_game_event(GameEvent::Shear.name(), entity.pos.load());
        let _ = player.damage_item_stack(item_stack, 1);
        true
    }

    /// The axe branches of vanilla `CopperGolem.mobInteract`: unwaxes, else scrapes one state off.
    fn try_scrape(&self, player: &Arc<Player>, item_stack: &mut ItemStack) -> bool {
        let entity = self.get_entity();
        let world = entity.world.load();
        let pos = entity.block_pos.load();
        let previous = self.get_weather_state().previous();
        let waxed = self.next_weathering_tick.load(Ordering::Relaxed) == IGNORE_WEATHERING_TICK;
        if !waxed && previous.is_none() {
            return false;
        }
        world.play_sound_fine(
            Sound::ItemAxeScrape,
            self.sound_source(),
            &entity.pos.load(),
            1.0,
            1.0,
        );
        let event = if waxed {
            WorldEvent::ParticlesWaxOff
        } else {
            WorldEvent::ParticlesScrape
        };
        world.sync_world_event(event, pos, 0);
        self.next_weathering_tick
            .store(UNSET_WEATHERING_TICK, Ordering::Relaxed);
        if !waxed && let Some(previous) = previous {
            self.set_weather_state(previous);
        }
        let _ = player.damage_item_stack(item_stack, 1);
        true
    }

    #[must_use]
    pub fn step_sound(&self) -> Sound {
        match self.get_weather_state() {
            WeatherState::Unaffected | WeatherState::Exposed => Sound::EntityCopperGolemStep,
            WeatherState::Weathered => Sound::EntityCopperGolemWeatheredStep,
            WeatherState::Oxidized => Sound::EntityCopperGolemOxidizedStep,
        }
    }
}

impl CustomSound for CopperGolemEntity {
    fn hurt_sound(&self) -> Option<Sound> {
        Some(match self.get_weather_state() {
            WeatherState::Unaffected | WeatherState::Exposed => Sound::EntityCopperGolemHurt,
            WeatherState::Weathered => Sound::EntityCopperGolemWeatheredHurt,
            WeatherState::Oxidized => Sound::EntityCopperGolemOxidizedHurt,
        })
    }

    fn death_sound(&self) -> Option<Sound> {
        Some(match self.get_weather_state() {
            WeatherState::Unaffected | WeatherState::Exposed => Sound::EntityCopperGolemDeath,
            WeatherState::Weathered => Sound::EntityCopperGolemWeatheredDeath,
            WeatherState::Oxidized => Sound::EntityCopperGolemOxidizedDeath,
        })
    }
}

impl Mob for CopperGolemEntity {
    /// Vanilla `CopperGolem.BRAIN_PROVIDER`.
    fn make_brain(&self, packed: &PackedMemories) -> Brain {
        BrainProvider {
            memory_types: &[],
            sensor_types: &[SensorType::NearestLivingEntities, SensorType::HurtBy],
            activities: copper_golem_ai::activities,
        }
        .make_brain(self, packed)
    }

    /// Vanilla `CopperGolem.customServerAiStep`.
    fn custom_server_ai_step(&self, _caller: &dyn EntityBase) {
        self.mob_entity.tick_brain(self);
        self.mob_entity
            .with_brain(self, |brain| copper_golem_ai::update_activity(brain));
    }

    /// Vanilla `CopperGolem.tick`; vanilla weathers after `super.tick`, Pumpkin's hook is before.
    fn mob_tick(&self, _caller: &dyn EntityBase) {
        self.update_weathering();
    }

    /// Vanilla `CopperGolem.finalizeSpawn`.
    fn finalize_spawn(
        &self,
        _world: &Arc<World>,
        group_data: Option<SpawnGroupData>,
    ) -> Option<SpawnGroupData> {
        self.play_spawn_sound();
        self.finalize_spawn_super(group_data)
    }

    /// Vanilla `AbstractGolem.removeWhenFarAway`.
    fn remove_when_far_away(&self, _distance_sq: f64) -> bool {
        false
    }

    /// Vanilla `CopperGolem.actuallyHurt`.
    fn on_damage(&self, _damage_type: DamageType, _source: Option<&dyn EntityBase>) {
        self.set_state(CopperGolemState::Idle);
    }

    /// A golem removed at a chest no longer holds it open, as vanilla's recheck finds.
    fn on_removed(&self) {
        self.clear_opened_chest();
    }

    fn mob_play_step_sound(&self, _pos: &BlockPos, _state: &BlockState) {
        self.play_entity_sound(self.step_sound(), 1.0, 1.0);
    }

    fn as_custom_sound(&self) -> Option<&dyn crate::entity::custom_sound::CustomSound> {
        Some(self)
    }

    /// Vanilla `CopperGolem.addAdditionalSaveData`.
    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        nbt.put_long(
            "next_weather_age",
            self.next_weathering_tick.load(Ordering::Relaxed),
        );
        nbt.put_string(
            "weather_state",
            self.get_weather_state().as_str().to_string(),
        );
    }

    /// Vanilla `CopperGolem.readAdditionalSaveData`.
    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        self.next_weathering_tick.store(
            nbt.get_long("next_weather_age")
                .unwrap_or(UNSET_WEATHERING_TICK),
            Ordering::Relaxed,
        );
        self.set_weather_state(
            nbt.get_string("weather_state")
                .and_then(WeatherState::from_name)
                .unwrap_or(WeatherState::Unaffected),
        );
    }

    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    /// Vanilla `CopperGolem.thunderHit`.
    fn mob_on_lightning_strike(
        &self,
        caller: &dyn EntityBase,
        lightning: &crate::entity::lightning::LightningBoltEntity,
    ) {
        self.mob_entity
            .living_entity
            .on_lightning_strike(caller, lightning);
        let lightning_bolt = Some(lightning.get_entity().entity_uuid);
        if self.last_lightning_bolt.swap(lightning_bolt) != lightning_bolt
            && let Some(previous) = self.get_weather_state().previous()
        {
            self.next_weathering_tick
                .store(UNSET_WEATHERING_TICK, Ordering::Relaxed);
            self.set_weather_state(previous);
        }
    }

    fn mob_init_data_tracker(&self) {
        let entity = self.get_entity();
        entity.set_synced_data(
            pumpkin_data::tracked_data::copper_golem::WEATHER_STATE,
            VarInt(self.get_weather_state().ordinal() as i32),
        );
        entity.set_synced_data(
            pumpkin_data::tracked_data::copper_golem::COPPER_GOLEM_STATE,
            VarInt(self.get_state().id()),
        );
    }

    /// Vanilla `CopperGolem.mobInteract`, after the leash handling of vanilla `Entity.interact`.
    fn mob_interact(&self, player: &Arc<Player>, item_stack: &mut ItemStack) -> bool {
        if self
            .mob_entity
            .mob_interact(player, item_stack, self.can_be_leashed())
        {
            return true;
        }
        if item_stack.is_empty() {
            let equipped = self.mob_entity.get_item_by_slot(&EquipmentSlot::MAIN_HAND);
            if equipped.is_empty() {
                return false;
            }
            throw_item(self.get_entity(), equipped, player.get_entity().pos.load());
            self.mob_entity
                .set_item_slot(&EquipmentSlot::MAIN_HAND, ItemStack::EMPTY.clone());
            return true;
        }
        if item_stack.item.id == Item::SHEARS.id && self.ready_for_shearing() {
            return self.try_shear(player, item_stack);
        }
        if item_stack.item.id == Item::HONEYCOMB.id
            && self.next_weathering_tick.load(Ordering::Relaxed) != IGNORE_WEATHERING_TICK
        {
            let entity = self.get_entity();
            let world = entity.world.load();
            let pos = entity.block_pos.load();
            world.sync_world_event(WorldEvent::ParticlesWaxOn, pos, 0);
            world.play_block_sound(Sound::ItemHoneycombWaxOn, SoundCategory::Blocks, pos);
            self.next_weathering_tick
                .store(IGNORE_WEATHERING_TICK, Ordering::Relaxed);
            item_stack.decrement_unless_creative(player.gamemode.load(), 1);
            return true;
        }
        item_stack.item.has_tag(&tag::Item::MINECRAFT_AXES) && self.try_scrape(player, item_stack)
    }
}
