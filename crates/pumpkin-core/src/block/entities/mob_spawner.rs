//! Vanilla `BaseSpawner` behind the spawner block: the spawn data with its potentials, the
//! delay roll, the player and crowd tests, the spawn rules and the spawn itself.

use std::sync::{
    Arc, Mutex, PoisonError,
    atomic::{AtomicI32, Ordering},
};

use pumpkin_data::data_component_impl::EquipmentSlot;
use pumpkin_data::entity::{EntityStatus, EntityType};
use pumpkin_data::game_event::GameEvent;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::world::WorldEvent;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_nbt::tag::NbtTag;
use pumpkin_protocol::bedrock::server::actor_event::ActorEventID;
use pumpkin_util::Difficulty;
use pumpkin_util::math::boundingbox::BoundingBox;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use rand::seq::IndexedRandom;
use rand::{RngExt, rng};

use crate::block::entities::BlockEntity;
use crate::entity::EntityBase;
use crate::entity::mob::Mob;
use crate::entity::mob::equipment::get_equipment_slot_for_item;
use crate::entity::mob::spawn::{EntitySpawnReason, finalize_spawn};
use crate::entity::r#type::{
    check_spawn_rules, entity_type_from_nbt, load_entity, self_and_passengers,
};
use crate::plugin::api::events::entity::spawner_spawn::SpawnerSpawnEvent;
use crate::world::World;
use crate::world::loot::{LootContextParameters, generate_loot_from_handle};
use crate::world::natural_spawner::mob_may_spawn;

/// Vanilla `EVENT_SPAWN`: the block event clients reset their delay on.
const EVENT_SPAWN: u8 = 1;
const DEFAULT_SPAWN_DELAY: i32 = 20;
const DEFAULT_MIN_SPAWN_DELAY: i32 = 200;
const DEFAULT_MAX_SPAWN_DELAY: i32 = 800;
const DEFAULT_SPAWN_COUNT: i32 = 4;
const DEFAULT_MAX_NEARBY_ENTITIES: i32 = 6;
const DEFAULT_REQUIRED_PLAYER_RANGE: i32 = 16;
const DEFAULT_SPAWN_RANGE: i32 = 4;
/// Vanilla `CustomSpawnRules.LIGHT_RANGE`: the limits stay within it, and it is their default.
const LIGHT_RANGE: (i32, i32) = (0, 15);
/// Vanilla `Weighted`'s default weight.
const DEFAULT_WEIGHT: i32 = 1;
/// The slots an equipment table's single drop chance covers (vanilla `EquipmentSlot.values()`).
const EQUIPMENT_SLOTS: [EquipmentSlot; 8] = [
    EquipmentSlot::MAIN_HAND,
    EquipmentSlot::OFF_HAND,
    EquipmentSlot::FEET,
    EquipmentSlot::LEGS,
    EquipmentSlot::CHEST,
    EquipmentSlot::HEAD,
    EquipmentSlot::BODY,
    EquipmentSlot::SADDLE,
];

/// Vanilla `SpawnData.CustomSpawnRules`: light limits that stand in for the type's placement
/// rules.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CustomSpawnRules {
    pub block_light_limit: (i32, i32),
    pub sky_light_limit: (i32, i32),
}

impl CustomSpawnRules {
    fn from_nbt(nbt: &NbtCompound) -> Self {
        Self {
            block_light_limit: light_limit(nbt, "block_light_limit"),
            sky_light_limit: light_limit(nbt, "sky_light_limit"),
        }
    }

    fn write_nbt(&self) -> NbtCompound {
        let mut nbt = NbtCompound::new();
        nbt.put_compound("block_light_limit", range_nbt(self.block_light_limit));
        nbt.put_compound("sky_light_limit", range_nbt(self.sky_light_limit));
        nbt
    }

    /// Vanilla `CustomSpawnRules.isValidPosition`.
    fn is_valid_position(&self, world: &World, pos: &BlockPos) -> bool {
        let block_light = i32::from(world.get_block_light_level(pos).unwrap_or(0));
        within(self.block_light_limit, block_light)
            && within(
                self.sky_light_limit,
                world.get_effective_sky_brightness(pos),
            )
    }
}

const fn within((min, max): (i32, i32), value: i32) -> bool {
    min <= value && value <= max
}

/// Vanilla `InclusiveRange.INT.lenientOptionalFieldOf(name, LIGHT_RANGE)`: a compound of
/// `min_inclusive` and `max_inclusive` or a list of the two; anything else, or a range
/// outside the light range, is the whole light range.
fn light_limit(nbt: &NbtCompound, name: &str) -> (i32, i32) {
    let range = match nbt.get(name) {
        Some(NbtTag::Compound(range)) => range
            .get_int("min_inclusive")
            .zip(range.get_int("max_inclusive")),
        Some(NbtTag::List(list)) if list.len() == 2 => {
            list[0].extract_int().zip(list[1].extract_int())
        }
        _ => None,
    };
    range
        .filter(|(min, max)| LIGHT_RANGE.0 <= *min && min <= max && *max <= LIGHT_RANGE.1)
        .unwrap_or(LIGHT_RANGE)
}

fn range_nbt((min, max): (i32, i32)) -> NbtCompound {
    let mut nbt = NbtCompound::new();
    nbt.put_int("min_inclusive", min);
    nbt.put_int("max_inclusive", max);
    nbt
}

/// Vanilla `EquipmentTable`: the loot table that equips the spawned mob and the drop chance
/// per slot.
#[derive(Clone, PartialEq)]
pub struct EquipmentTable {
    pub loot_table: String,
    pub slot_drop_chances: Vec<(EquipmentSlot, f32)>,
}

impl EquipmentTable {
    fn from_nbt(nbt: &NbtCompound) -> Option<Self> {
        let loot_table = nbt.get_string("loot_table")?.to_string();
        let slot_drop_chances = match nbt.get("slot_drop_chances") {
            Some(NbtTag::Float(chance)) => EQUIPMENT_SLOTS
                .iter()
                .map(|slot| (slot.clone(), *chance))
                .collect(),
            Some(NbtTag::Compound(chances)) => chances
                .child_tags
                .iter()
                .filter_map(|(name, tag)| {
                    Some((
                        EquipmentSlot::get_from_name(name.as_ref())?.clone(),
                        tag.extract_float()?,
                    ))
                })
                .collect(),
            _ => Vec::new(),
        };
        Some(Self {
            loot_table,
            slot_drop_chances,
        })
    }

    /// Vanilla `DROP_CHANCES_CODEC`: one number when every slot shares the chance, else the
    /// map, nothing when there is none.
    fn write_nbt(&self) -> NbtCompound {
        let mut nbt = NbtCompound::new();
        nbt.put_string("loot_table", self.loot_table.clone());
        let Some((_, first)) = self.slot_drop_chances.first() else {
            return nbt;
        };
        let all_the_same = self.slot_drop_chances.len() == EQUIPMENT_SLOTS.len()
            && self
                .slot_drop_chances
                .iter()
                .all(|(_, chance)| chance == first);
        if all_the_same {
            nbt.put_float("slot_drop_chances", *first);
        } else {
            let mut chances = NbtCompound::new();
            for (slot, chance) in &self.slot_drop_chances {
                chances.put_float(slot_name(slot), *chance);
            }
            nbt.put_compound("slot_drop_chances", chances);
        }
        nbt
    }
}

/// The serialized name of a slot (vanilla `EquipmentSlot.getName`).
fn slot_name(slot: &EquipmentSlot) -> &str {
    match slot {
        EquipmentSlot::MainHand(data)
        | EquipmentSlot::OffHand(data)
        | EquipmentSlot::Feet(data)
        | EquipmentSlot::Legs(data)
        | EquipmentSlot::Chest(data)
        | EquipmentSlot::Head(data)
        | EquipmentSlot::Body(data)
        | EquipmentSlot::Saddle(data) => &data.name,
    }
}

/// Vanilla `SpawnData`: the entity to spawn with its optional spawn rules and equipment.
#[derive(Clone, Default, PartialEq)]
pub struct SpawnData {
    pub entity: NbtCompound,
    pub custom_spawn_rules: Option<CustomSpawnRules>,
    pub equipment: Option<EquipmentTable>,
}

impl SpawnData {
    fn of_type(entity_type: &EntityType) -> Self {
        let mut data = Self::default();
        data.set_entity_id(entity_type);
        data
    }

    /// Vanilla `BaseSpawner.setEntityId`'s write into the entity data.
    fn set_entity_id(&mut self, entity_type: &EntityType) {
        self.entity
            .put_string("id", format!("minecraft:{}", entity_type.resource_name));
    }

    fn from_nbt(nbt: &NbtCompound) -> Self {
        Self {
            entity: nbt.get_compound("entity").cloned().unwrap_or_default(),
            custom_spawn_rules: nbt
                .get_compound("custom_spawn_rules")
                .map(CustomSpawnRules::from_nbt),
            equipment: nbt
                .get_compound("equipment")
                .and_then(EquipmentTable::from_nbt),
        }
    }

    fn write_nbt(&self) -> NbtCompound {
        let mut nbt = NbtCompound::new();
        nbt.put_compound("entity", self.entity.clone());
        if let Some(rules) = &self.custom_spawn_rules {
            nbt.put_compound("custom_spawn_rules", rules.write_nbt());
        }
        if let Some(equipment) = &self.equipment {
            nbt.put_compound("equipment", equipment.write_nbt());
        }
        nbt
    }

    /// Vanilla `hasNoConfiguration`: nothing but the id, so the mob gets its spawn
    /// finalization.
    fn has_no_configuration(&self) -> bool {
        self.entity.child_tags.len() == 1 && self.entity.get_string("id").is_some()
    }

    /// The `Pos` the entity data may carry (vanilla reads it with `Vec3.CODEC`).
    fn position(&self) -> Option<Vector3<f64>> {
        let pos = self.entity.get_list("Pos")?;
        if pos.len() != 3 {
            return None;
        }
        Some(Vector3::new(
            pos[0].extract_double()?,
            pos[1].extract_double()?,
            pos[2].extract_double()?,
        ))
    }
}

/// How one spawn attempt of vanilla `serverTick` ends.
enum Attempt {
    Spawned,
    /// Vanilla `continue`: on to the next attempt.
    Skipped,
    /// Vanilla `delay(); return`: the round ends with a new delay.
    Reset,
}

pub struct MobSpawnerBlockEntity {
    pub position: BlockPos,
    pub spawn_delay: AtomicI32,
    /// Vanilla `spawnPotentials`: weights and spawn data the next one is drawn from.
    pub spawn_potentials: Mutex<Vec<(i32, SpawnData)>>,
    pub next_spawn_data: Mutex<Option<SpawnData>>,
    pub min_spawn_delay: i32,
    pub max_spawn_delay: i32,
    pub spawn_count: i32,
    pub max_nearby_entities: i32,
    pub required_player_range: i32,
    pub spawn_range: i32,
}

impl MobSpawnerBlockEntity {
    pub const ID: &'static str = "minecraft:mob_spawner";

    #[must_use]
    pub fn new(position: BlockPos, entity_type: Option<&'static EntityType>) -> Self {
        Self {
            position,
            spawn_delay: AtomicI32::new(DEFAULT_SPAWN_DELAY),
            spawn_potentials: Mutex::new(Vec::new()),
            next_spawn_data: Mutex::new(entity_type.map(SpawnData::of_type)),
            min_spawn_delay: DEFAULT_MIN_SPAWN_DELAY,
            max_spawn_delay: DEFAULT_MAX_SPAWN_DELAY,
            spawn_count: DEFAULT_SPAWN_COUNT,
            max_nearby_entities: DEFAULT_MAX_NEARBY_ENTITIES,
            required_player_range: DEFAULT_REQUIRED_PLAYER_RANGE,
            spawn_range: DEFAULT_SPAWN_RANGE,
        }
    }

    /// Vanilla `BaseSpawner.setEntityId` (a spawn egg on the block): the next spawn data's
    /// entity becomes this type.
    pub fn set_entity_id(&self, entity_type: &'static EntityType) {
        let drawn = self.draw_potential().unwrap_or_default();
        self.next_spawn_data
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get_or_insert(drawn)
            .set_entity_id(entity_type);
    }

    /// Vanilla `getOrCreateNextSpawnData`.
    fn get_or_create_next_spawn_data(&self) -> SpawnData {
        if let Some(data) = &*self
            .next_spawn_data
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
        {
            return data.clone();
        }
        let drawn = self.draw_potential().unwrap_or_default();
        self.next_spawn_data
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get_or_insert(drawn)
            .clone()
    }

    /// Vanilla `spawnPotentials.getRandom(random)`.
    fn draw_potential(&self) -> Option<SpawnData> {
        self.spawn_potentials
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .choose_weighted(&mut rng(), |(weight, _)| *weight)
            .ok()
            .map(|(_, data)| data.clone())
    }

    /// Vanilla `isNearPlayer`.
    fn is_near_player(&self, world: &World) -> bool {
        world.has_nearby_alive_player(
            self.position.to_centered_f64(),
            f64::from(self.required_player_range),
        )
    }

    /// Vanilla `BaseSpawner.delay`: the next delay, the next spawn data from the potentials,
    /// and the block event clients animate on.
    fn delay(&self, world: &World) {
        let delay = if self.max_spawn_delay <= self.min_spawn_delay {
            self.min_spawn_delay
        } else {
            self.min_spawn_delay
                + rng().random_range(0..self.max_spawn_delay - self.min_spawn_delay)
        };
        self.spawn_delay.store(delay, Ordering::Relaxed);
        if let Some(data) = self.draw_potential() {
            *self
                .next_spawn_data
                .lock()
                .unwrap_or_else(PoisonError::into_inner) = Some(data);
        }
        world.add_synced_block_event(self.position, EVENT_SPAWN, 0);
    }

    /// The spawn attempts of vanilla `serverTick` once the delay ran out.
    fn spawn_round(&self, world: &Arc<World>) {
        let data = self.get_or_create_next_spawn_data();
        let mut delay = false;
        for _ in 0..self.spawn_count {
            match self.spawn_attempt(world, &data) {
                Attempt::Spawned => delay = true,
                Attempt::Skipped => {}
                Attempt::Reset => {
                    self.delay(world);
                    return;
                }
            }
        }
        if delay {
            self.delay(world);
        }
    }

    /// Vanilla's random spot: within the spawn range sideways, one block up or down.
    fn random_spawn_pos(&self) -> Vector3<f64> {
        let pos = self.position.0;
        let range = f64::from(self.spawn_range);
        Vector3::new(
            f64::from(pos.x) + (rng().random::<f64>() - rng().random::<f64>()) * range + 0.5,
            f64::from(pos.y + rng().random_range(0..3) - 1),
            f64::from(pos.z) + (rng().random::<f64>() - rng().random::<f64>()) * range + 0.5,
        )
    }

    /// Whether the spot passes the custom spawn rules, or without them the type's placement
    /// rules with the spawner reason.
    fn spot_allowed(
        world: &World,
        data: &SpawnData,
        entity_type: &'static EntityType,
        pos: &BlockPos,
    ) -> bool {
        data.custom_spawn_rules.as_ref().map_or_else(
            || {
                check_spawn_rules(
                    entity_type,
                    world,
                    EntitySpawnReason::Spawner,
                    pos,
                    world.is_thundering(),
                )
            },
            |rules| {
                (entity_type.category.is_friendly
                    || world.level_info.load().difficulty != Difficulty::Peaceful)
                    && rules.is_valid_position(world, pos)
            },
        )
    }

    /// Vanilla `maxNearbyEntities`: entities of the type whose box meets the block's box
    /// grown by the spawn range.
    fn is_crowded(&self, world: &World, entity_type: &'static EntityType) -> bool {
        let search =
            BoundingBox::from_block(&self.position).expand_all(f64::from(self.spawn_range));
        let nearby = world
            .get_entities_at_box(&search)
            .iter()
            .filter(|entity| entity.get_entity().entity_type == entity_type)
            .count();
        nearby >= usize::try_from(self.max_nearby_entities).unwrap_or(0)
    }

    /// One attempt of vanilla `serverTick`.
    fn spawn_attempt(&self, world: &Arc<World>, data: &SpawnData) -> Attempt {
        let Some(entity_type) = entity_type_from_nbt(&data.entity) else {
            return Attempt::Reset;
        };
        let spawn_pos = data.position().unwrap_or_else(|| self.random_spawn_pos());
        if !world.is_space_empty(entity_type.get_spawn_bounding_box(
            spawn_pos.x,
            spawn_pos.y,
            spawn_pos.z,
        )) {
            return Attempt::Skipped;
        }
        let spawn_block_pos = BlockPos::floored_v(spawn_pos);
        if !Self::spot_allowed(world, data, entity_type, &spawn_block_pos) {
            return Attempt::Skipped;
        }
        let entity = load_entity(entity_type, spawn_pos, &data.entity, world);
        for passenger in self_and_passengers(&entity) {
            passenger.get_entity().set_pos(spawn_pos);
        }
        if self.is_crowded(world, entity_type) {
            return Attempt::Reset;
        }
        entity
            .get_entity()
            .set_rotation(rng().random::<f32>() * 360.0, 0.0);
        if let Some(mob) = entity.get_mob() {
            let allowed = if data.custom_spawn_rules.is_some() {
                mob.check_spawn_obstruction(world)
            } else {
                mob_may_spawn(world, entity.as_ref())
            };
            if !allowed {
                return Attempt::Skipped;
            }
            if data.has_no_configuration() {
                finalize_spawn(&entity, world, None);
            }
            if let Some(table) = &data.equipment {
                equip(world, mob, &entity, table);
            }
        }
        let mut event = SpawnerSpawnEvent::new(entity.get_entity().entity_id, self.position);
        if let Some(server) = world.server.upgrade() {
            server.plugin_manager.fire_blocking(&server, &mut event);
        }
        if event.cancelled {
            // A cancelled spawn counts as an attempt, so the spawner goes on its cooldown
            // instead of retrying every tick while a plugin keeps cancelling.
            return Attempt::Spawned;
        }
        if !world.spawn_entity(entity.clone()) {
            return Attempt::Reset;
        }
        for passenger in self_and_passengers(&entity).into_iter().skip(1) {
            passenger.init_data_tracker();
            world.add_entity_silent(passenger);
        }
        world.sync_world_event(WorldEvent::ParticlesMobblockSpawn, self.position, 0);
        world.emit_game_event(
            GameEvent::EntityPlace.name(),
            spawn_block_pos.to_centered_f64(),
        );
        if entity.get_mob().is_some() {
            // Vanilla `Mob.spawnAnim`.
            world.send_entity_status(
                entity.get_entity(),
                EntityStatus::Poof,
                ActorEventID::GroundDust,
            );
        }
        Attempt::Spawned
    }

    /// Everything but the potentials: what vanilla sends clients as the update tag.
    fn write_spawner_nbt(&self, nbt: &mut NbtCompound) {
        nbt.put_short("Delay", self.spawn_delay.load(Ordering::Relaxed) as i16);
        nbt.put_short("MinSpawnDelay", self.min_spawn_delay as i16);
        nbt.put_short("MaxSpawnDelay", self.max_spawn_delay as i16);
        nbt.put_short("SpawnCount", self.spawn_count as i16);
        nbt.put_short("MaxNearbyEntities", self.max_nearby_entities as i16);
        nbt.put_short("RequiredPlayerRange", self.required_player_range as i16);
        nbt.put_short("SpawnRange", self.spawn_range as i16);
        if let Some(data) = &*self
            .next_spawn_data
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
        {
            nbt.put_compound("SpawnData", data.write_nbt());
        }
    }
}

/// Vanilla `EquipmentUser.equip(EquipmentTable, LootParams)`: the table's items go into the
/// slots they fit, one item per slot, with the table's drop chances.
fn equip(world: &Arc<World>, mob: &dyn Mob, entity: &Arc<dyn EntityBase>, table: &EquipmentTable) {
    let Some(handle) = world.get_loot_table(&table.loot_table) else {
        return;
    };
    let params = LootContextParameters {
        world: Some(world.as_ref()),
        this_entity: Some(entity.as_ref()),
        position: Some(entity.get_entity().pos.load()),
        ..Default::default()
    };
    let mut inserted: Vec<EquipmentSlot> = Vec::new();
    for mut stack in generate_loot_from_handle(&handle, rand::random(), &params) {
        if stack.is_empty() {
            continue;
        }
        let slot = get_equipment_slot_for_item(&stack);
        if inserted.contains(&slot) {
            continue;
        }
        // Vanilla `EquipmentSlot.limit`: one item in every slot but the hands.
        let equipped: ItemStack =
            if matches!(slot, EquipmentSlot::MainHand(_) | EquipmentSlot::OffHand(_)) {
                stack
            } else {
                stack.split(1)
            };
        mob.get_mob_entity().set_item_slot(&slot, equipped);
        if let Some((_, chance)) = table
            .slot_drop_chances
            .iter()
            .find(|(candidate, _)| *candidate == slot)
        {
            mob.get_mob_entity()
                .living_entity
                .equipment_drop_chances
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .insert(slot.clone(), *chance);
        }
        inserted.push(slot);
    }
}

impl BlockEntity for MobSpawnerBlockEntity {
    fn resource_location(&self) -> &'static str {
        Self::ID
    }

    fn get_position(&self) -> BlockPos {
        self.position
    }

    /// Vanilla `BaseSpawner.serverTick`.
    fn tick(&self, world: &Arc<World>) {
        if !self.is_near_player(world) || !world.level_info.load().game_rules.spawner_blocks_work {
            return;
        }
        if self.spawn_delay.load(Ordering::Relaxed) == -1 {
            self.delay(world);
        }
        if self.spawn_delay.load(Ordering::Relaxed) > 0 {
            self.spawn_delay.fetch_sub(1, Ordering::Relaxed);
            return;
        }
        self.spawn_round(world);
    }

    fn from_nbt(nbt: &NbtCompound, position: BlockPos) -> Self
    where
        Self: Sized,
    {
        let get_num = |name: &str| {
            nbt.get_short(name)
                .map(i32::from)
                .or_else(|| nbt.get_int(name))
                .or_else(|| nbt.get_byte(name).map(i32::from))
        };
        let next_spawn_data = nbt.get_compound("SpawnData").map(SpawnData::from_nbt);
        // Vanilla: without potentials, the spawn data (or an empty one) is the only entry.
        let spawn_potentials = nbt.get_list("SpawnPotentials").map_or_else(
            || vec![(DEFAULT_WEIGHT, next_spawn_data.clone().unwrap_or_default())],
            |entries| {
                entries
                    .iter()
                    .filter_map(NbtTag::extract_compound)
                    .map(|entry| {
                        (
                            entry.get_int("weight").unwrap_or(DEFAULT_WEIGHT),
                            entry
                                .get_compound("data")
                                .map(SpawnData::from_nbt)
                                .unwrap_or_default(),
                        )
                    })
                    .collect()
            },
        );
        Self {
            position,
            spawn_delay: AtomicI32::new(get_num("Delay").unwrap_or(DEFAULT_SPAWN_DELAY)),
            spawn_potentials: Mutex::new(spawn_potentials),
            next_spawn_data: Mutex::new(next_spawn_data),
            min_spawn_delay: get_num("MinSpawnDelay").unwrap_or(DEFAULT_MIN_SPAWN_DELAY),
            max_spawn_delay: get_num("MaxSpawnDelay").unwrap_or(DEFAULT_MAX_SPAWN_DELAY),
            spawn_count: get_num("SpawnCount").unwrap_or(DEFAULT_SPAWN_COUNT),
            max_nearby_entities: get_num("MaxNearbyEntities")
                .unwrap_or(DEFAULT_MAX_NEARBY_ENTITIES),
            required_player_range: get_num("RequiredPlayerRange")
                .unwrap_or(DEFAULT_REQUIRED_PLAYER_RANGE),
            spawn_range: get_num("SpawnRange").unwrap_or(DEFAULT_SPAWN_RANGE),
        }
    }

    fn write_nbt(&self, nbt: &mut NbtCompound) {
        self.write_spawner_nbt(nbt);
        let potentials = self
            .spawn_potentials
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        nbt.put_list(
            "SpawnPotentials",
            potentials
                .iter()
                .map(|(weight, data)| {
                    let mut entry = NbtCompound::new();
                    entry.put_int("weight", *weight);
                    entry.put_compound("data", data.write_nbt());
                    NbtTag::Compound(entry)
                })
                .collect(),
        );
    }

    fn chunk_data_nbt(&self) -> Option<NbtCompound> {
        let mut nbt = NbtCompound::new();
        self.write_spawner_nbt(&mut nbt);
        Some(nbt)
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}
