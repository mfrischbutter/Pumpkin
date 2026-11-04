//! Vanilla `FallingBlockEntity`.

use crossbeam::atomic::AtomicCell;
use pumpkin_data::damage::DamageType;
use pumpkin_data::entity::EntityType;
use pumpkin_data::fluid::Fluid;
use pumpkin_data::game_event::GameEvent;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::tag::{self, Taggable};
use pumpkin_data::world::WorldEvent;
use pumpkin_data::{Block, BlockState, BlockStateId, Property, PropertyKey};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_protocol::bedrock::client::CUpdateBlock;
use pumpkin_protocol::java::client::play::{CAddTransientBlock, CBlockUpdate};
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_world::clip::{ClipBlock, ClipContext, ClipFluid};
use pumpkin_world::world::BlockFlags;
use std::sync::{
    Arc, Mutex, PoisonError,
    atomic::{AtomicBool, AtomicI32, Ordering},
};

use crate::{
    block::{
        blocks::{anvil::AnvilBlock, falling::FallingBlock},
        entities::block_entity_from_nbt,
        registry::can_replace_with_other_block,
    },
    entity::{
        Entity, EntityBase,
        living::LivingEntity,
        mob::enderman::{block_state_from_nbt, block_state_to_nbt},
    },
    server::Server,
    world::World,
};

/// Vanilla `Entity.getAirDrag`.
const AIR_DRAG: f32 = 0.98;
/// Vanilla `DEFAULT_MAX_FALL_DAMAGE`.
const DEFAULT_MAX_FALL_DAMAGE: i32 = 40;
/// Vanilla `tick`: how long a falling block may stay out of the world.
const OUT_OF_WORLD_TIME: i32 = 100;
/// Vanilla `tick`: how long a falling block may fall at all.
const MAX_FALL_TIME: i32 = 600;
/// Vanilla `causeFallDamage`: the anvil's chance to take damage, base and per block fallen.
const ANVIL_BREAK_CHANCE: f32 = 0.05;

pub struct FallingEntity {
    entity: Entity,
    block_state_id: AtomicCell<BlockStateId>,
    time: AtomicI32,
    drop_item: AtomicBool,
    cancel_drop: AtomicBool,
    hurt_entities: AtomicBool,
    fall_damage_max: AtomicI32,
    fall_damage_per_distance: AtomicCell<f32>,
    /// Vanilla `Entity.fallDistance`, which only living entities keep here otherwise.
    fall_distance: AtomicCell<f64>,
    /// Vanilla `blockData`, what the block entity of the placed block is loaded with.
    block_data: Mutex<Option<NbtCompound>>,
}

/// The state with its `waterlogged` property set, or as it is when it has none.
fn with_waterlogged(block: &Block, state_id: BlockStateId, waterlogged: bool) -> BlockStateId {
    let Some(properties) = block.properties(state_id) else {
        return state_id;
    };
    let mut properties = properties.to_props();
    let Some(property) = properties
        .iter_mut()
        .find(|property| property.key() == PropertyKey::Waterlogged)
    else {
        return state_id;
    };
    *property = Property::Waterlogged(waterlogged);
    block.from_properties(&properties).to_state_id(block)
}

impl FallingEntity {
    #[must_use]
    pub fn new(entity: Entity, block_state_id: BlockStateId) -> Self {
        entity
            .data
            .store(i32::from(block_state_id.as_u16()), Ordering::Relaxed);
        Self {
            entity,
            block_state_id: AtomicCell::new(block_state_id),
            time: AtomicI32::new(0),
            drop_item: AtomicBool::new(true),
            cancel_drop: AtomicBool::new(false),
            hurt_entities: AtomicBool::new(false),
            fall_damage_max: AtomicI32::new(DEFAULT_MAX_FALL_DAMAGE),
            fall_damage_per_distance: AtomicCell::new(0.0),
            fall_distance: AtomicCell::new(0.0),
            block_data: Mutex::new(None),
        }
    }

    /// Vanilla `FallingBlockEntity.fall`: the block leaves what fluid it held behind and comes
    /// back as an entity. A plugin cancelling the spawn leaves the block where it is.
    pub fn replace_spawn(
        world: &Arc<World>,
        position: BlockPos,
        block_state: BlockStateId,
    ) -> Option<Arc<Self>> {
        let block = Block::from_state_id(block_state);
        let spawn_pos = position.0.to_f64().add_raw(0.5, 0.0, 0.5);
        let mut entity = Entity::new(world.clone(), spawn_pos, &EntityType::FALLING_BLOCK);
        entity.blocks_building = true;
        let falling = Arc::new(Self::new(
            entity,
            with_waterlogged(block, block_state, false),
        ));
        if !world.spawn_entity(falling.clone()) {
            return None;
        }

        let left_behind = if block.is_waterlogged(block_state) {
            Block::WATER.default_state.id
        } else {
            Block::AIR.default_state.id
        };
        world.set_block_state(&position, left_behind, BlockFlags::NOTIFY_ALL);
        Some(falling)
    }

    /// Vanilla `setHurtsEntities`.
    pub fn set_hurts_entities(&self, damage_per_distance: f32, damage_max: i32) {
        self.hurt_entities.store(true, Ordering::Relaxed);
        self.fall_damage_per_distance.store(damage_per_distance);
        self.fall_damage_max.store(damage_max, Ordering::Relaxed);
    }

    /// Vanilla `disableDrop`.
    pub fn disable_drop(&self) {
        self.cancel_drop.store(true, Ordering::Relaxed);
    }

    fn set_block_state(&self, state_id: BlockStateId) {
        self.block_state_id.store(state_id);
        self.entity
            .data
            .store(i32::from(state_id.as_u16()), Ordering::Relaxed);
    }

    /// The source `getFallDamageSource` of the block names.
    fn fall_damage_type(block: &Block) -> DamageType {
        if block.has_tag(&tag::Block::MINECRAFT_ANVIL) {
            DamageType::FALLING_ANVIL
        } else if block == &Block::POINTED_DRIPSTONE || block == &Block::SULFUR_SPIKE {
            DamageType::FALLING_STALACTITE
        } else {
            DamageType::FALLING_BLOCK
        }
    }

    /// Vanilla `Entity.checkFallDamage`. Vanilla's falling block has no base tick, so it never
    /// counts as in water; `Block.fallOn` always ends in `causeFallDamage` here.
    fn check_fall_damage(&self, caller: &dyn EntityBase, moved_y: f64) {
        if moved_y < 0.0 {
            self.fall_distance
                .store(self.fall_distance.load() - f64::from(moved_y as f32));
        }
        let entity = &self.entity;
        if entity.on_ground.load(Ordering::Relaxed) {
            let fall_distance = self.fall_distance.swap(0.0);
            if fall_distance > 0.0 {
                self.cause_fall_damage(caller, fall_distance);
                entity
                    .world
                    .load()
                    .emit_game_event(GameEvent::HitGround.name(), entity.pos.load());
            }
        }
    }

    /// Vanilla `causeFallDamage`: hurts what stands below, and an anvil may chip from it.
    fn cause_fall_damage(&self, caller: &dyn EntityBase, fall_distance: f64) {
        if !self.hurt_entities.load(Ordering::Relaxed) {
            return;
        }
        let fall_distance_int = (fall_distance - 1.0).ceil() as i32;
        if fall_distance_int < 0 {
            return;
        }
        let state_id = self.block_state_id.load();
        let block = Block::from_state_id(state_id);
        let damage_type = Self::fall_damage_type(block);
        let scaled = (fall_distance_int as f32 * self.fall_damage_per_distance.load()).floor();
        let damage = (scaled as i32).min(self.fall_damage_max.load(Ordering::Relaxed)) as f32;

        let entity = &self.entity;
        let world = entity.world.load();
        for target in world.get_all_at_box(&entity.bounding_box.load()) {
            // Vanilla `NO_CREATIVE_OR_SPECTATOR` and `LIVING_ENTITY_STILL_ALIVE`.
            let alive = target
                .get_living_entity()
                .is_some_and(|living| !living.is_dead_or_dying());
            let exempt = target.is_spectator()
                || target.get_player().is_some_and(|player| player.is_creative());
            if alive && !exempt && target.get_entity().entity_id != entity.entity_id {
                target.damage_with_context(
                    target.as_ref(),
                    damage,
                    damage_type,
                    None,
                    Some(caller),
                    Some(caller),
                );
            }
        }

        let chance = (fall_distance_int as f32).mul_add(ANVIL_BREAK_CHANCE, ANVIL_BREAK_CHANCE);
        if block.has_tag(&tag::Block::MINECRAFT_ANVIL)
            && damage > 0.0
            && rand::random::<f32>() < chance
        {
            match AnvilBlock::damage(state_id) {
                Some(damaged) => self.set_block_state(damaged),
                None => self.disable_drop(),
            }
        }
    }

    /// Vanilla `spawnAtLocation(level, block)`.
    fn drop_as_item(&self, block: &Block) {
        if let Some(item) = Item::from_id(block.item_id) {
            self.entity.spawn_at_location(ItemStack::new(1, item));
        }
    }

    /// Vanilla `Fallable.onLand` of the falling blocks that have one.
    fn on_land(
        &self,
        world: &Arc<World>,
        pos: &BlockPos,
        block: &Block,
        replaced_state: &BlockState,
    ) {
        if block.has_tag(&tag::Block::MINECRAFT_ANVIL) {
            if !self.entity.is_silent() {
                world.sync_world_event(WorldEvent::SoundAnvilLand, *pos, 0);
            }
        } else if block.has_tag(&tag::Block::MINECRAFT_CONCRETE_POWDERS)
            && (FallingBlock::can_solidify(replaced_state)
                || FallingBlock::touches_liquid(&**world, pos))
            && let Some(name) = block.name.strip_suffix("_powder")
            && let Some(concrete) = Block::from_name(name)
        {
            world.set_block_state(pos, concrete.default_state.id, BlockFlags::NOTIFY_ALL);
        }
    }

    /// Vanilla `callOnBrokenAfterFall`.
    fn on_broken_after_fall(&self, world: &Arc<World>, pos: &BlockPos, block: &Block) {
        let silent = self.entity.is_silent();
        if block.has_tag(&tag::Block::MINECRAFT_ANVIL) {
            if !silent {
                world.sync_world_event(WorldEvent::SoundAnvilBroken, *pos, 0);
            }
        } else if block == &Block::POINTED_DRIPSTONE {
            if !silent {
                world.sync_world_event(WorldEvent::SoundPointedDripstoneLand, *pos, 0);
            }
        } else if block == &Block::SULFUR_SPIKE {
            if !silent {
                world.sync_world_event(WorldEvent::SoundSulfurSpikeLand, *pos, 0);
            }
        } else if block == &Block::SUSPICIOUS_SAND || block == &Block::SUSPICIOUS_GRAVEL {
            // Vanilla `BrushableBlock.onBrokenAfterFall`.
            let bounding_box = self.entity.bounding_box.load();
            let center = (bounding_box.min + bounding_box.max) * 0.5;
            world.sync_world_event(
                WorldEvent::ParticlesAndSoundDestroyBlock,
                BlockPos::floored_v(center),
                i32::from(self.block_state_id.load().as_u16()),
            );
            world.emit_game_event(GameEvent::BlockDestroy.name(), center);
        }
    }

    /// Vanilla `tick`: `blockData` merged over the placed block's block entity.
    fn apply_block_data(&self, world: &Arc<World>, pos: &BlockPos) {
        let data = self
            .block_data
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        let Some(data) = data else {
            return;
        };
        let Some(mut merged) = world.get_block_entity_nbt(pos) else {
            return;
        };
        for (name, value) in &data.child_tags {
            // The block entity stays what and where it is, whatever the tag says.
            if !matches!(&**name, "id" | "x" | "y" | "z") {
                merged.put(name, value.clone());
            }
        }
        if let Some(block_entity) = block_entity_from_nbt(&merged) {
            world.add_block_entity(block_entity);
        }
    }

    /// Vanilla `tick`: places the block, or breaks it into its item.
    fn land(&self, world: &Arc<World>, pos: &BlockPos, block: &Block, stuck_in_water: bool) {
        let entity = &self.entity;
        let (current_block, current_state) = world.get_block_and_state(pos);
        // Vanilla waits until a piston has finished moving a block through the landing cell.
        if current_block == &Block::MOVING_PISTON {
            return;
        }
        if self.cancel_drop.load(Ordering::Relaxed) {
            entity.remove();
            self.on_broken_after_fall(world, pos, block);
            return;
        }

        let state_id = self.block_state_id.load();
        let is_concrete = block.has_tag(&tag::Block::MINECRAFT_CONCRETE_POWDERS);
        let (below_block, below_state) = world.get_block_and_state(&pos.down());
        let would_continue_falling = FallingBlock::can_fall_through(below_state, below_block)
            && (!is_concrete || !stuck_in_water);
        let would_survive = !would_continue_falling
            && world.block_registry.can_place_at(
                None,
                Some(&**world),
                &**world,
                None,
                block,
                state_id.to_state(),
                pos,
                None,
                None,
            );
        let drops = self.drop_item.load(Ordering::Relaxed)
            && world.level_info.load().game_rules.entity_drops;
        if !can_replace_with_other_block(current_block, current_state) || !would_survive {
            entity.remove();
            if drops {
                self.on_broken_after_fall(world, pos, block);
                self.drop_as_item(block);
            }
            return;
        }

        let state_id = if world.get_exact_fluid(pos).id == Fluid::WATER.id {
            with_waterlogged(block, state_id, true)
        } else {
            state_id
        };
        self.set_block_state(state_id);
        world.set_block_state(pos, state_id, BlockFlags::NOTIFY_ALL);
        let placed = world.get_block_state_id(pos);
        if placed == state_id {
            // The update reaches watchers before the despawn, else the block would be missing
            // for them until the tick flush.
            let network_id = i32::from(placed.as_u16());
            world.send_to_tracking_players_editioned(
                entity,
                &CBlockUpdate::new(*pos, network_id.into()),
                &CUpdateBlock::new(*pos, BlockState::to_be_network_id(placed)),
            );
            world.send_to_tracking_players(
                entity,
                &CAddTransientBlock::new(*pos, network_id.into()),
            );
            entity.remove();
            self.on_land(world, pos, block, current_state);
            self.apply_block_data(world, pos);
        } else if drops {
            entity.remove();
            self.on_broken_after_fall(world, pos, block);
            self.drop_as_item(block);
        }
    }

    /// The server half of vanilla `tick`, once the entity has moved.
    fn tick_landing(&self, block: &Block) {
        let entity = &self.entity;
        let world = entity.world.load_full();
        let is_water = |pos: &BlockPos| world.get_fluid(pos).matches_type(&Fluid::WATER);
        let mut pos = entity.block_pos.load();
        let is_concrete = block.has_tag(&tag::Block::MINECRAFT_CONCRETE_POWDERS);
        let mut stuck_in_water = is_concrete && is_water(&pos);
        if is_concrete && entity.velocity.load().length_squared() > 1.0 {
            // Fast powder can fly through a water block without ever being inside it.
            let clip = world.clip(&ClipContext::new(
                entity.last_pos.load(),
                entity.pos.load(),
                ClipBlock::Collider,
                ClipFluid::SourceOnly,
            ));
            if !clip.is_miss && is_water(&clip.block_pos) {
                pos = clip.block_pos;
                stuck_in_water = true;
            }
        }

        if entity.on_ground.load(Ordering::Relaxed) || stuck_in_water {
            let bounced = entity.velocity.load().multiply(0.7, -0.5, 0.7);
            entity.velocity.store(bounced);
            self.land(&world, &pos, block, stuck_in_water);
            return;
        }

        let time = self.time.load(Ordering::Relaxed);
        let out_of_world = pos.0.y <= world.get_bottom_y() || pos.0.y > world.get_top_y();
        if (time > OUT_OF_WORLD_TIME && out_of_world) || time > MAX_FALL_TIME {
            if self.drop_item.load(Ordering::Relaxed)
                && world.level_info.load().game_rules.entity_drops
            {
                self.drop_as_item(block);
            }
            entity.remove();
        }
    }
}

impl EntityBase for FallingEntity {
    /// Vanilla `FallingBlockEntity.tick`, which skips the base tick.
    fn tick(&self, caller: &dyn EntityBase, _server: &Server) {
        let entity = &self.entity;
        let state_id = self.block_state_id.load();
        if state_id.to_state().is_air() {
            entity.remove();
            return;
        }
        // Read before moving: an anvil that chips on landing still breaks and sounds as it fell.
        let block = Block::from_state_id(state_id);
        self.time.fetch_add(1, Ordering::Relaxed);
        entity.update_last_pos();

        let mut velo = entity.velocity.load();
        velo.y -= self.get_gravity();
        entity.velocity.store(velo);

        let y_before = entity.pos.load().y;
        entity.move_entity(caller, velo);
        self.check_fall_damage(caller, entity.pos.load().y - y_before);
        entity.tick_block_collisions(caller);
        entity.tick_portal(caller);

        if entity.is_alive() {
            self.tick_landing(block);
        }

        let drag = f64::from(AIR_DRAG);
        entity
            .velocity
            .store(entity.velocity.load().multiply(drag, drag, drag));
    }

    fn init_data_tracker(&self) {
        self.entity.set_synced_data(
            pumpkin_data::tracked_data::falling_block::START_POS,
            self.entity.block_pos.load(),
        );
    }

    /// Vanilla `addAdditionalSaveData`.
    fn write_custom_nbt(&self, nbt: &mut NbtCompound) {
        nbt.put("BlockState", block_state_to_nbt(self.block_state_id.load()));
        nbt.put_int("Time", self.time.load(Ordering::Relaxed));
        nbt.put_bool("DropItem", self.drop_item.load(Ordering::Relaxed));
        nbt.put_bool("HurtEntities", self.hurt_entities.load(Ordering::Relaxed));
        nbt.put_float("FallHurtAmount", self.fall_damage_per_distance.load());
        nbt.put_int("FallHurtMax", self.fall_damage_max.load(Ordering::Relaxed));
        if let Some(data) = &*self
            .block_data
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
        {
            nbt.put_compound("TileEntityData", data.clone());
        }
        nbt.put_bool("CancelDrop", self.cancel_drop.load(Ordering::Relaxed));
        nbt.put_double("fall_distance", self.fall_distance.load());
    }

    /// Vanilla `readAdditionalSaveData`. Missing keys keep their value, a fresh entity holds the
    /// defaults; without `HurtEntities` a new `BlockState` decides, so anvils hurt by default.
    fn read_custom_nbt(&self, nbt: &NbtCompound) {
        let block_state = nbt.get("BlockState").and_then(block_state_from_nbt);
        if let Some(state_id) = block_state {
            self.set_block_state(state_id);
        }
        if let Some(time) = nbt.get_int("Time") {
            self.time.store(time, Ordering::Relaxed);
        }
        if let Some(hurt_entities) = nbt.get_bool("HurtEntities") {
            self.hurt_entities.store(hurt_entities, Ordering::Relaxed);
        } else if let Some(state_id) = block_state {
            self.hurt_entities.store(
                Block::from_state_id(state_id).has_tag(&tag::Block::MINECRAFT_ANVIL),
                Ordering::Relaxed,
            );
        }
        if let Some(amount) = nbt.get_numeric_float("FallHurtAmount") {
            self.fall_damage_per_distance.store(amount);
        }
        if let Some(max) = nbt.get_int("FallHurtMax") {
            self.fall_damage_max.store(max, Ordering::Relaxed);
        }
        if let Some(drop_item) = nbt.get_bool("DropItem") {
            self.drop_item.store(drop_item, Ordering::Relaxed);
        }
        if let Some(data) = nbt.get_compound("TileEntityData") {
            *self
                .block_data
                .lock()
                .unwrap_or_else(PoisonError::into_inner) = Some(data.clone());
        }
        if let Some(cancel_drop) = nbt.get_bool("CancelDrop") {
            self.cancel_drop.store(cancel_drop, Ordering::Relaxed);
        }
        if let Some(fall_distance) = nbt.get_numeric_double("fall_distance") {
            self.fall_distance.store(fall_distance);
        }
    }

    fn remove_custom_nbt(&self, removed: &[Box<str>]) {
        if removed.iter().any(|key| &**key == "TileEntityData") {
            *self
                .block_data
                .lock()
                .unwrap_or_else(PoisonError::into_inner) = None;
        }
    }

    /// Vanilla `isPickable`.
    fn can_hit(&self) -> bool {
        !self.entity.is_removed()
    }

    fn get_entity(&self) -> &Entity {
        &self.entity
    }

    fn get_living_entity(&self) -> Option<&LivingEntity> {
        None
    }

    /// Vanilla `hurtServer` only resends the motion and never takes the damage.
    fn damage_with_context(
        &self,
        _caller: &dyn EntityBase,
        _amount: f32,
        damage_type: DamageType,
        _position: Option<Vector3<f64>>,
        _source: Option<&dyn EntityBase>,
        cause: Option<&dyn EntityBase>,
    ) -> bool {
        if !self.entity.is_invulnerable_to(&damage_type, cause) {
            self.entity.velocity_dirty.store(true, Ordering::SeqCst);
        }
        false
    }

    fn get_gravity(&self) -> f64 {
        if self.entity.has_no_gravity() {
            0.0
        } else {
            0.04
        }
    }

    fn entity_bedrock_spawn_metadata(
        &self,
    ) -> Option<pumpkin_protocol::bedrock::client::set_actor_data::SyncedActorDataList> {
        let mut meta =
            pumpkin_protocol::bedrock::client::set_actor_data::SyncedActorDataList::new();
        let runtime_id = BlockState::to_be_network_id(self.block_state_id.load());
        meta.set(
            pumpkin_protocol::bedrock::client::set_actor_data::entity_data_key::VARIANT,
            pumpkin_protocol::bedrock::client::set_actor_data::MetadataValue::Int(
                runtime_id as i32,
            ),
        );
        Some(meta)
    }

    fn bedrock_y_offset(&self) -> f64 {
        0.49
    }

    fn cast_any(&self) -> &dyn std::any::Any {
        self
    }
}
