use std::sync::{
    Arc, PoisonError, Weak,
    atomic::{AtomicBool, AtomicI32, Ordering},
};

use crate::entity::ai::behavior::hunting::{self, Hunt};
use crate::entity::ai::target_match::TargetMatch;
use crate::entity::attributes::Modifier;
use crate::entity::attributes::ModifierOperation;
use crate::entity::mob::requires_custom_persistence_base;
use crate::entity::mob::sun_burn::passes_burn_roll;
use crate::plugin::api::events::entity::entity_target::TargetReason;
use crate::plugin::api::events::entity::entity_teleport::EntityTeleportEvent;
use crate::world::World;
use crate::world::game_event::GameEventContext;
use crate::world::loot::{LootContextParameters, generate_loot_from_handle};

use crossbeam::atomic::AtomicCell;
use pumpkin_data::{
    Block, BlockStateId,
    attributes::Attributes,
    damage::DamageType,
    data_component_impl::EquipmentSlot,
    enchantment_provider::EnchantmentProvider,
    entity::{EntityStatus, EntityType},
    game_event::GameEvent,
    item::Item,
    item_stack::ItemStack,
    sound::Sound,
    tag,
    tag::Taggable,
    world::WorldEvent,
};
use pumpkin_nbt::{compound::NbtCompound, tag::NbtTag};
use pumpkin_protocol::bedrock::server::actor_event::ActorEventID;
use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_util::math::{boundingbox::BoundingBox, position::BlockPos, vector3::Vector3};
use pumpkin_world::generation::structure::template::PaletteEntry;
use rand::RngExt;

use crate::entity::{
    Entity, EntityBase,
    ai::{
        goal::{
            chase_player::ChasePlayerGoal, look_around::RandomLookAroundGoal,
            look_at_entity::LookAtEntityGoal, melee_attack::MeleeAttackGoal,
            pick_up_block::PickUpBlockGoal, place_block::PlaceBlockGoal,
            reset_universal_anger::ResetUniversalAngerGoal, revenge::RevengeGoal, swim::SwimGoal,
            teleport_towards_player::TeleportTowardsPlayerGoal, wander_around::WanderAroundGoal,
        },
        pathfinder::node::PathType,
    },
    mob::{
        Mob, MobEntity,
        neutral::{NeutralData, NeutralMob},
    },
    player::Player,
};

const SPEED_BOOST: f64 = 0.15;
const ENDERMAN_SPEED_BOOST_ID: &str = "minecraft:attacking";

/// Vanilla `MAX_TELEPORT_RADIUS`.
const MAX_TELEPORT_RADIUS: i32 = 32;
/// Vanilla `MAX_TELEPORT_DIAMETER`.
const MAX_TELEPORT_DIAMETER: i32 = 64;
/// Vanilla `MIN_DEAGGRESSION_TIME`: ticks a new target is safe from the daylight teleport.
const MIN_DEAGGRESSION_TIME: i32 = 600;
/// Tries of vanilla `repeatedlyTryToTeleport`.
const TELEPORT_ATTEMPTS: usize = 64;
/// Horizontal spread of vanilla `teleportTowards`.
const TELEPORT_TOWARDS_SPREAD: f64 = 8.0;
/// Vertical spread of vanilla `teleportTowards`.
const TELEPORT_TOWARDS_SPREAD_Y: i32 = 16;
/// How far vanilla `teleportTowards` jumps towards the target.
const TELEPORT_TOWARDS_DISTANCE: f64 = 16.0;
/// Radius of the clamped position difference sent with the teleport particles.
const TELEPORT_PARTICLE_RADIUS: i32 = 127;
/// Vanilla `WorldBorder.clampVec3ToBound` keeps the position this far inside the border.
const BORDER_MARGIN: f64 = 1.0E-5;
/// Cone of vanilla `isBeingStaredBy`, narrowed by the distance.
const STARE_CONE_SIZE: f64 = 0.025;
/// Root key of the carried block in the entity NBT.
const CARRIED_BLOCK_STATE_KEY: &str = "carriedBlockState";

fn is_projectile_damage(dt: DamageType) -> bool {
    let (names, _) = pumpkin_data::tag::DamageType::MINECRAFT_IS_PROJECTILE;
    names.contains(&dt.message_id)
}

const ENDERMAN_HUNTS: &[Hunt] = &[Hunt::new(
    3,
    TargetMatch::Type(&EntityType::ENDERMITE),
    true,
)];

/// Vanilla `Mth.clamp`, which never panics on an inverted range.
const fn clamp_to(value: f64, min: f64, max: f64) -> f64 {
    if value < min { min } else { value.min(max) }
}

/// Vanilla `BlockUtil.clampedPackDifferenceInPosition` for one radius on every axis.
fn clamped_pack_difference(from: &BlockPos, to: &BlockPos, radius: i32) -> i32 {
    let clamped = BlockPos::new(
        from.0.x + (to.0.x - from.0.x).clamp(-radius, radius),
        from.0.y + (to.0.y - from.0.y).clamp(-radius, radius),
        from.0.z + (to.0.z - from.0.z).clamp(-radius, radius),
    );
    from.pack_difference_in_position(&clamped, radius, radius, radius)
}

/// Vanilla `BlockState.CODEC`: the block id for a default state, else the id and its properties.
fn block_state_to_nbt(state_id: BlockStateId) -> NbtTag {
    let block = Block::from_state_id(state_id);
    let id = format!("minecraft:{}", block.name);
    if state_id == block.default_state.id {
        return NbtTag::String(id.into());
    }
    let properties = block
        .properties(state_id)
        .map(|properties| properties.to_props())
        .unwrap_or_default();
    let entry = PaletteEntry::with_properties(id, properties);
    NbtTag::Compound(entry.to_nbt_compound())
}

/// Reads [`block_state_to_nbt`], and the raw state id Pumpkin saved before.
fn block_state_from_nbt(tag: &NbtTag) -> Option<BlockStateId> {
    match tag {
        NbtTag::String(id) => Block::from_name(id).map(|block| block.default_state.id),
        NbtTag::Compound(compound) => {
            let entry = PaletteEntry::from_nbt_compound(compound).ok()?;
            let block = Block::from_name(&entry.name)?;
            // Blocks without properties have no properties type to build a state from.
            if entry.properties.is_empty() || block.properties(block.default_state.id).is_none() {
                return Some(block.default_state.id);
            }
            Some(block.from_properties(&entry.properties).to_state_id(block))
        }
        NbtTag::Int(id) => u16::try_from(*id).ok().and_then(BlockStateId::new),
        _ => None,
    }
}

pub struct EndermanEntity {
    pub mob_entity: MobEntity,
    neutral_data: NeutralData,
    carried_block: AtomicCell<Option<BlockStateId>>,
    /// Synced CREEPY flag, the open-mouth pose. Anger lives in `NeutralMob`.
    creepy: AtomicBool,
    provoked: AtomicBool,
    speed_boosted: AtomicBool,
    /// Tick count of the last target change, vanilla `targetChangeTime`.
    target_change_time: AtomicI32,
}

impl EndermanEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let mob_entity = MobEntity::new(entity);
        let entity = Self {
            mob_entity,
            neutral_data: NeutralData::default(),
            carried_block: AtomicCell::new(None),
            creepy: AtomicBool::new(false),
            provoked: AtomicBool::new(false),
            speed_boosted: AtomicBool::new(false),
            target_change_time: AtomicI32::new(0),
        };
        let mob_arc = Arc::new(entity);
        let mob_weak: Weak<dyn Mob> = {
            let mob_arc: Arc<dyn Mob> = mob_arc.clone();
            Arc::downgrade(&mob_arc)
        };

        let mut navigator = mob_arc
            .mob_entity
            .navigator
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        navigator.set_pathfinding_malus(PathType::Water, -1.0);
        drop(navigator);

        {
            let goal_selector = &mob_arc.mob_entity.goals_selector;
            let target_selector = &mob_arc.mob_entity.target_selector;

            goal_selector.add_goal(0, Box::new(SwimGoal::default()));
            goal_selector.add_goal(1, Box::new(ChasePlayerGoal::new(mob_arc.clone())));
            goal_selector.add_goal(2, Box::new(MeleeAttackGoal::new(1.0, false)));
            goal_selector.add_goal(
                7,
                Box::new(WanderAroundGoal::water_avoiding_with_probability(1.0, 0.0)),
            );
            goal_selector.add_goal(
                8,
                LookAtEntityGoal::with_default(mob_weak, &EntityType::PLAYER, 8.0),
            );
            goal_selector.add_goal(8, Box::new(RandomLookAroundGoal::default()));
            goal_selector.add_goal(10, Box::new(PlaceBlockGoal::new(mob_arc.clone())));
            goal_selector.add_goal(11, Box::new(PickUpBlockGoal::new(mob_arc.clone())));

            target_selector.add_goal(1, Box::new(TeleportTowardsPlayerGoal::new(mob_arc.clone())));
            target_selector.add_goal(2, Box::new(RevengeGoal::new(true)));
            hunting::apply(target_selector, &mob_arc.mob_entity, ENDERMAN_HUNTS);
            target_selector.add_goal(4, ResetUniversalAngerGoal::new(false));
        };

        mob_arc
    }

    /// Vanilla `teleport()`: a random spot up to 32 blocks away on each axis.
    pub fn teleport_randomly(&self) -> bool {
        let living = &self.mob_entity.living_entity;
        if !living.is_alive() {
            return false;
        }
        let pos = living.entity.pos.load();
        let (x, y, z) = {
            let mut rng = self.get_random();
            let diameter = f64::from(MAX_TELEPORT_DIAMETER);
            let x = pos.x + (rng.random::<f64>() - 0.5) * diameter;
            let offset_y = rng.random_range(0..MAX_TELEPORT_DIAMETER) - MAX_TELEPORT_RADIUS;
            let z = pos.z + (rng.random::<f64>() - 0.5) * diameter;
            (x, pos.y + f64::from(offset_y), z)
        };

        self.teleport_to(x, y, z)
    }

    /// Vanilla `teleportTowards`: a jump of 16 blocks towards `target`, with some spread.
    pub fn teleport_towards(&self, target: &dyn EntityBase) -> bool {
        let entity = &self.mob_entity.living_entity.entity;
        let pos = entity.pos.load();
        let target_entity = target.get_entity();
        let target_pos = target_entity.pos.load();

        let dir = Vector3::new(
            pos.x - target_pos.x,
            pos.y + f64::from(entity.height()) * 0.5 - target_entity.get_eye_y(),
            pos.z - target_pos.z,
        )
        .normalize();
        let jump = dir * TELEPORT_TOWARDS_DISTANCE;
        let (x, y, z) = {
            let mut rng = self.get_random();
            let spread_y = TELEPORT_TOWARDS_SPREAD_Y;
            let x = pos.x + (rng.random::<f64>() - 0.5) * TELEPORT_TOWARDS_SPREAD - jump.x;
            let offset_y = rng.random_range(0..spread_y) - spread_y / 2;
            let z = pos.z + (rng.random::<f64>() - 0.5) * TELEPORT_TOWARDS_SPREAD - jump.z;
            (x, pos.y + f64::from(offset_y) - jump.y, z)
        };

        self.teleport_to(x, y, z)
    }

    /// Vanilla `teleport(x, y, z)` over `LivingEntity.randomTeleport`, with particles and sounds.
    pub fn teleport_to(&self, x: f64, y: f64, z: f64) -> bool {
        let entity = &self.mob_entity.living_entity.entity;
        if entity.has_vehicle() {
            return false;
        }
        let origin = entity.pos.load();
        let world = entity.world.load_full();
        let Some(destination) = self.find_teleport_destination(&world, x, y, z) else {
            return false;
        };

        if let Some(server) = world.server.upgrade() {
            let mut event = EntityTeleportEvent::new(entity.entity_id, origin, destination);
            server.plugin_manager.fire_blocking(&server, &mut event);
            if event.cancelled {
                return false;
            }
        }

        entity.teleport(destination, None, None, &world);
        // Entity event 46: the client draws the teleport particles.
        world.send_entity_status(entity, EntityStatus::Teleport, ActorEventID::None);
        self.mob_entity.steered().navigator().stop();

        let source: &dyn EntityBase = self;
        world.emit_game_event_with_context(
            GameEvent::Teleport.name(),
            origin,
            &GameEventContext {
                source_entity: Some(source),
            },
        );
        if !entity.is_silent() {
            let from = BlockPos::floored_v(origin);
            let to = entity.block_pos.load();
            world.sync_world_event(
                WorldEvent::ParticlesEndermanTeleport,
                from,
                clamped_pack_difference(&from, &to, TELEPORT_PARTICLE_RADIUS),
            );
            // Vanilla plays the sound twice: at the old position fields, which the teleport
            // already moved to the destination, and at the position.
            let category = self.mob_sound_source();
            world.play_sound(Sound::EntityEndermanTeleport, category, &destination);
            world.play_sound(Sound::EntityEndermanTeleport, category, &destination);
        }

        true
    }

    /// Vanilla `randomTeleport`: drops from the spot to the first block entities can teleport to.
    fn find_teleport_destination(
        &self,
        world: &World,
        x: f64,
        y: f64,
        z: f64,
    ) -> Option<Vector3<f64>> {
        let (x, z) = {
            let border = world
                .worldborder
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            let half = border.new_diameter / 2.0;
            (
                clamp_to(
                    x,
                    border.center_x - half,
                    border.center_x + half - BORDER_MARGIN,
                ),
                clamp_to(
                    z,
                    border.center_z - half,
                    border.center_z + half - BORDER_MARGIN,
                ),
            )
        };

        let mut pos = BlockPos::floored(x, y, z);
        if !world.has_chunks_at(pos.0.x, pos.0.z, pos.0.x, pos.0.z) {
            return None;
        }
        let mut target_y = y;
        while pos.0.y > world.dimension.min_y {
            pos = pos.down();
            let block = world.get_block(&pos);
            if block.has_tag(&tag::Block::MINECRAFT_ENTITIES_CAN_TELEPORT_TO) {
                let destination = Vector3::new(x, target_y, z);
                return self.check_teleport_destination(world, block, destination);
            }
            target_y -= 1.0;
        }
        None
    }

    /// Vanilla `checkPositionAndTeleport` with `canRandomlyTeleportTo`, without the move.
    fn check_teleport_destination(
        &self,
        world: &World,
        ground: &Block,
        target: Vector3<f64>,
    ) -> Option<Vector3<f64>> {
        let avoided = &tag::Block::MINECRAFT_ENDERMAN_DOES_NOT_TELEPORT_TO;
        if ground.has_tag(avoided) {
            return None;
        }
        let entity = &self.mob_entity.living_entity.entity;
        let dimensions = entity.entity_dimension.load();
        let bb = BoundingBox::new_from_pos(target.x, target.y, target.z, &dimensions);
        if !world.is_space_empty(bb) || world.contains_any_liquid(bb) {
            return None;
        }
        if BlockPos::iterate(bb.min_block_pos(), bb.max_block_pos())
            .any(|pos| world.get_block(&pos).has_tag(avoided))
        {
            return None;
        }
        // Vanilla `Enderman.canRandomlyTeleportTo`: never onto water.
        let below = BlockPos::floored_v(target).down();
        if world
            .get_fluid(&below)
            .has_tag(&tag::Fluid::MINECRAFT_WATER)
        {
            return None;
        }
        Some(target)
    }

    /// Vanilla `repeatedlyTryToTeleport`.
    fn repeatedly_try_to_teleport(&self) {
        if self.mob_entity.living_entity.entity.has_vehicle() {
            return;
        }
        for _ in 0..TELEPORT_ATTEMPTS {
            if self.teleport_randomly() {
                return;
            }
        }
    }

    fn set_speed_boost(&self, boosted: bool) {
        if self.speed_boosted.swap(boosted, Ordering::Relaxed) == boosted {
            return;
        }
        let living = &self.mob_entity.living_entity;
        living.update_attribute(&Attributes::MOVEMENT_SPEED, |inst| {
            if boosted {
                inst.add_or_replace_modifier(Modifier {
                    id: ENDERMAN_SPEED_BOOST_ID.to_string(),
                    amount: SPEED_BOOST,
                    operation: ModifierOperation::Add,
                });
            } else {
                inst.remove_modifier(ENDERMAN_SPEED_BOOST_ID);
            }
        });
        crate::entity::attributes::send_attribute_updates_for_living(
            living,
            vec![Attributes::MOVEMENT_SPEED],
        );
    }

    pub fn set_creepy(&self, creepy: bool) {
        self.creepy.store(creepy, Ordering::Relaxed);
        self.mob_entity
            .living_entity
            .entity
            .set_synced_data(pumpkin_data::tracked_data::enderman::CREEPY, creepy);
    }

    pub fn is_creepy(&self) -> bool {
        self.creepy.load(Ordering::Relaxed)
    }

    pub fn set_provoked(&self, provoked: bool) {
        self.provoked.store(provoked, Ordering::Relaxed);
        self.mob_entity
            .living_entity
            .entity
            .set_synced_data(pumpkin_data::tracked_data::enderman::STARED_AT, provoked);
    }

    pub fn set_carried_block(&self, block_state: Option<BlockStateId>) {
        self.carried_block.store(block_state);
        let value = block_state.map_or(VarInt(0), |id| VarInt(i32::from(id.as_u16())));
        self.mob_entity
            .living_entity
            .entity
            .set_synced_data(pumpkin_data::tracked_data::enderman::CARRY_STATE, value);
    }

    pub fn get_carried_block(&self) -> Option<BlockStateId> {
        self.carried_block.load()
    }

    /// Vanilla `isBeingStaredBy`: the player looks at the eyes, with no gaze disguise worn.
    pub fn is_player_staring(&self, player: &Player) -> bool {
        let head = player
            .living_entity
            .entity_equipment
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&EquipmentSlot::HEAD);
        if head
            .item
            .has_tag(&tag::Item::MINECRAFT_GAZE_DISGUISE_EQUIPMENT)
        {
            return false;
        }

        // Vanilla `isLookingAtMe` with the enderman's eye height as the only gaze height.
        let entity = &self.mob_entity.living_entity.entity;
        let viewer = player.get_entity();
        let pos = entity.pos.load();
        let viewer_pos = viewer.pos.load();
        let to_eyes = Vector3::new(
            pos.x - viewer_pos.x,
            entity.get_eye_y() - viewer.get_eye_y(),
            pos.z - viewer_pos.z,
        );
        let distance = to_eyes.length();
        let look =
            Vector3::rotation_vector(f64::from(viewer.pitch.load()), f64::from(viewer.yaw.load()));
        look.dot(&to_eyes.normalize()) > 1.0 - STARE_CONE_SIZE / distance
            && viewer.has_line_of_sight(entity)
    }
}

impl NeutralMob for EndermanEntity {
    fn get_neutral_data(&self) -> &NeutralData {
        &self.neutral_data
    }

    /// Vanilla updates the anger at the start of `aiStep`.
    fn updates_anger_in_ai_step(&self) -> bool {
        true
    }
}

impl Mob for EndermanEntity {
    /// Vanilla `Enderman.requiresCustomPersistence`: an enderman carrying a block stays.
    fn requires_custom_persistence(&self) -> bool {
        requires_custom_persistence_base(self) || self.carried_block.load().is_some()
    }

    fn as_neutral(&self) -> Option<&dyn NeutralMob> {
        Some(self)
    }

    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        if let Some(block_state) = self.carried_block.load() {
            nbt.put(CARRIED_BLOCK_STATE_KEY, block_state_to_nbt(block_state));
        }
    }

    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        let carried = nbt
            .get(CARRIED_BLOCK_STATE_KEY)
            .and_then(block_state_from_nbt)
            .filter(|id| !id.to_state().is_air());
        self.set_carried_block(carried);
    }

    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    fn mob_metadata(&self) -> Option<crate::entity::mob::MobMetadata> {
        self.is_creepy()
            .then(|| crate::entity::mob::MobMetadata::new().screaming(true))
    }

    /// Vanilla `setTarget`: a target opens the mouth and speeds the enderman up.
    fn apply_mob_target(&self, target: Option<Arc<dyn EntityBase>>) {
        let has_target = target.is_some();
        self.mob_entity.set_target(target);

        if has_target {
            let tick_count = self.get_entity().tick_count.load(Ordering::Relaxed);
            self.target_change_time.store(tick_count, Ordering::Relaxed);
            self.set_creepy(true);
            self.set_speed_boost(true);
        } else {
            self.target_change_time.store(0, Ordering::Relaxed);
            self.set_creepy(false);
            self.set_provoked(false);
            self.set_speed_boost(false);
        }
    }

    fn before_mob_ai_step(&self) {
        self.update_persistent_anger();
    }

    /// Vanilla `customServerAiStep`: daylight under the open sky ends the target.
    ///
    /// The enderman then teleports away.
    fn custom_server_ai_step(&self, _caller: &dyn EntityBase) {
        let entity = self.get_entity();
        let world = entity.world.load();
        let calm_since = self.target_change_time.load(Ordering::Relaxed) + MIN_DEAGGRESSION_TIME;
        if !world.is_bright_outside() || entity.tick_count.load(Ordering::Relaxed) < calm_since {
            return;
        }
        let eye = BlockPos::floored_v(entity.get_eye_pos());
        let brightness = world.get_light_level_dependent_magic_value(&eye);
        if passes_burn_roll(brightness, self.get_random().random())
            && world.can_see_sky(&entity.block_pos.load())
        {
            self.set_mob_target(None, TargetReason::ForgotTarget);
            self.teleport_randomly();
        }
    }

    /// Vanilla `getWalkTargetValue`: every stroll target is worth the same.
    fn get_walk_target_value(&self, _pos: &BlockPos) -> f32 {
        0.0
    }

    fn get_ambient_sound(&self) -> Option<Sound> {
        Some(if self.is_creepy() {
            Sound::EntityEndermanScream
        } else {
            Sound::EntityEndermanAmbient
        })
    }

    fn is_sensitive_to_water(&self) -> bool {
        true
    }

    /// Vanilla `dropCustomDeathLoot`: the carried block drops as if mined with silk touch.
    fn custom_death_loot(&self) -> Vec<ItemStack> {
        let Some(state_id) = self.carried_block.load() else {
            return Vec::new();
        };
        let entity = self.get_entity();
        let world = entity.world.load();
        let Some(loot_table) = Block::from_state_id(state_id)
            .loot_table
            .and_then(|key| world.get_loot_table(key))
        else {
            return Vec::new();
        };
        let mut tool = ItemStack::new(1, &Item::DIAMOND_AXE);
        // A single fixed enchantment: the difficulty does not change it.
        EnchantmentProvider::ENDERMAN_LOOT_DROP.apply_to_stack(&mut tool, 0.0);
        let this: &dyn EntityBase = self;
        let params = LootContextParameters {
            world: Some(&world),
            block_state: Some(state_id.to_state()),
            position: Some(entity.pos.load()),
            tool: Some(tool),
            this_entity: Some(this),
            ..Default::default()
        };
        generate_loot_from_handle(&loot_table, rand::random(), &params)
    }

    /// Vanilla `hurtServer` before `super`: projectiles never hit, the enderman teleports away.
    fn pre_damage_with_context(
        &self,
        damage_type: DamageType,
        _source: Option<&dyn EntityBase>,
        cause: Option<&dyn EntityBase>,
    ) -> bool {
        let entity = self.get_entity();
        if entity.is_invulnerable_to(&damage_type, cause) {
            return false;
        }
        if is_projectile_damage(damage_type) && !entity.has_vehicle() {
            self.repeatedly_try_to_teleport();
            return false;
        }
        true
    }

    /// Vanilla `hurtServer` after `super`: hurt without an attacker, it mostly teleports away.
    fn after_hurt(
        &self,
        _damage_type: DamageType,
        source: Option<&dyn EntityBase>,
        cause: Option<&dyn EntityBase>,
    ) {
        let by_living = cause
            .or(source)
            .is_some_and(|attacker| attacker.get_living_entity().is_some());
        if !by_living && self.get_random().random_range(0..10) != 0 {
            self.teleport_randomly();
        }
    }
}
