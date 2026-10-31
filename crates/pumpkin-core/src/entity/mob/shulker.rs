use std::sync::atomic::AtomicU8;
use std::sync::atomic::Ordering::Relaxed;
use std::sync::{Arc, Weak};

use crossbeam::atomic::AtomicCell;
use pumpkin_data::attributes::Attributes;
use pumpkin_data::block_properties::Axis;
use pumpkin_data::damage::DamageType;
use pumpkin_data::data_component::DataComponent;
use pumpkin_data::data_component_impl::{DataComponentImpl, ShulkerColorImpl};
use pumpkin_data::dye_color::DyeColor;
use pumpkin_data::entity::EntityType;
use pumpkin_data::game_event::GameEvent;
use pumpkin_data::sound::Sound;
use pumpkin_data::tag::{self, Taggable};
use pumpkin_data::tracked_data;
use pumpkin_data::world::WorldEvent;
use pumpkin_data::{Block, BlockDirection};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_util::Difficulty;
use pumpkin_util::math::boundingbox::BoundingBox;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use rand::RngExt;

use crate::entity::ai::behavior::hunting::{self, Hunt};
use crate::entity::ai::control::{Control, LookControlTrait};
use crate::entity::ai::goal::look_around::RandomLookAroundGoal;
use crate::entity::ai::goal::look_at_entity::LookAtEntityGoal;
use crate::entity::ai::goal::revenge::RevengeGoal;
use crate::entity::ai::goal::{Controls, Goal, to_goal_ticks};
use crate::entity::ai::target_match::TargetMatch;
use crate::entity::attributes::{Modifier, ModifierOperation};
use crate::entity::combat::CombatRules;
use crate::entity::custom_sound::CustomSound;
use crate::entity::living::LivingEntity;
use crate::entity::mob::spawn::SpawnGroupData;
use crate::entity::mob::{Mob, MobEntity};
use crate::entity::projectile::shulker_bullet::ShulkerBulletEntity;
use crate::entity::{Entity, EntityBase};
use crate::plugin::api::events::entity::entity_target::TargetReason;
use crate::world::World;
use crate::world::game_event::GameEventContext;

const COVERED_ARMOR_MODIFIER_ID: &str = "minecraft:covered";
/// Armor of a closed shulker, vanilla `COVERED_ARMOR_MODIFIER`.
const COVERED_ARMOR: f64 = 20.0;
const NO_COLOR: u8 = 16;
const MAX_TELEPORT_DISTANCE: i32 = 8;
const TELEPORT_ATTEMPTS: usize = 5;
const OTHER_SHULKER_SCAN_RADIUS: f64 = 8.0;
const OTHER_SHULKER_LIMIT: f32 = 5.0;
const PEEK_PER_TICK: f32 = 0.05;
const DEFAULT_ATTACH_FACE: BlockDirection = BlockDirection::Down;
const MAX_SCALE: f64 = 3.0;
/// Raw peek amounts: fully open to attack, a crack to peek out.
const ATTACK_PEEK: u8 = 100;
const PEEK_OUT: u8 = 30;
/// Vanilla `reducedTickDelay(40)`: one in this many ticks an idle shulker peeks out.
const PEEK_CHANCE: i32 = 40;
const ATTACK_INTERVAL: i32 = 20;
/// A target farther than 20 blocks is given up.
const ATTACK_RANGE_SQ: f64 = 400.0;
/// Vanilla `getMaxHeadXRot` and `getMaxHeadYRot`.
const MAX_HEAD_ROTATION: f32 = 180.0;
const LOOK_AT_PLAYER_RANGE: f32 = 8.0;
const LOOK_AT_PLAYER_CHANCE: f32 = 0.02;
/// Along the attach axis the target search reaches only this far.
const SEARCH_THICKNESS: f64 = 4.0;
/// Vanilla `deflate(1.0E-6)` before the collision checks.
const COLLISION_MARGIN: f64 = 1.0E-6;

const SHULKER_HUNTS: &[Hunt] = &[
    Hunt::new(2, TargetMatch::Type(&EntityType::PLAYER), true)
        .gated_by(not_peaceful)
        .searching_in(attach_search_area),
    Hunt::new(3, TargetMatch::Enemy, true)
        .gated_by(has_team)
        .searching_in(attach_search_area),
];

/// Represents a Shulker, the boxed End mob that shoots homing bullets.
///
/// Wiki: <https://minecraft.wiki/w/Shulker>
pub struct ShulkerEntity {
    pub mob_entity: MobEntity,
    attach_face: AtomicCell<BlockDirection>,
    /// Vanilla `DATA_PEEK_ID`: how far the lid should open, 0 to 100.
    raw_peek: AtomicU8,
    color: AtomicU8,
    /// Vanilla `currentPeekAmount`: how far the lid is open, following the raw amount.
    current_peek: AtomicCell<f32>,
}

impl ShulkerEntity {
    #[must_use]
    pub fn new(entity: Entity) -> Arc<Self> {
        let mut mob_entity = MobEntity::new(entity);
        *mob_entity
            .look_control
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            Box::new(ShulkerLookControl::default());
        let shulker = Arc::new(Self {
            mob_entity,
            attach_face: AtomicCell::new(DEFAULT_ATTACH_FACE),
            raw_peek: AtomicU8::new(0),
            color: AtomicU8::new(NO_COLOR),
            current_peek: AtomicCell::new(0.0),
        });
        let mob_weak: Weak<dyn Mob> = {
            let mob_arc: Arc<dyn Mob> = shulker.clone();
            Arc::downgrade(&mob_arc)
        };

        let goal_selector = &shulker.mob_entity.goals_selector;
        goal_selector.add_goal(
            1,
            Box::new(LookAtEntityGoal::new(
                mob_weak,
                &EntityType::PLAYER,
                LOOK_AT_PLAYER_RANGE,
                LOOK_AT_PLAYER_CHANCE,
                true,
            )),
        );
        goal_selector.add_goal(4, Box::new(ShulkerAttackGoal::default()));
        goal_selector.add_goal(7, Box::new(ShulkerPeekGoal::default()));
        goal_selector.add_goal(8, Box::new(RandomLookAroundGoal::default()));

        let target_selector = &shulker.mob_entity.target_selector;
        target_selector.add_goal(
            1,
            Box::new(
                RevengeGoal::new(true)
                    .ignoring(|entity_type| entity_type == &EntityType::SHULKER)
                    .alerting_others(),
            ),
        );
        hunting::apply(target_selector, &shulker.mob_entity, SHULKER_HUNTS);

        shulker
    }

    #[must_use]
    pub fn get_attach_face(&self) -> BlockDirection {
        self.attach_face.load()
    }

    pub fn set_attach_face(&self, face: BlockDirection) {
        self.attach_face.store(face);
        self.get_entity()
            .set_synced_data(tracked_data::shulker::ATTACH_FACE_ID, VarInt(face as i32));
        self.refresh_bounding_box();
    }

    /// The dye colour id, or `None` for the default purple (plugin API).
    #[must_use]
    pub fn get_color(&self) -> Option<u8> {
        let color = self.color.load(Relaxed);
        (color != NO_COLOR).then_some(color)
    }

    /// Vanilla `getColor`: the dye colour, or `None` for the default purple.
    fn dye_color(&self) -> Option<DyeColor> {
        DyeColor::by_id(self.color.load(Relaxed))
    }

    /// Sets the dye colour, `None` for the default purple (plugin API).
    pub fn set_color(&self, color: Option<u8>) {
        self.set_color_id(color.unwrap_or(NO_COLOR));
    }

    fn set_color_id(&self, color: u8) {
        self.color.store(color, Relaxed);
        self.get_entity()
            .set_synced_data(tracked_data::shulker::COLOR, color as i8);
    }

    fn raw_peek(&self) -> u8 {
        self.raw_peek.load(Relaxed)
    }

    /// The raw peek amount, 0 closed to 100 open (plugin API).
    #[must_use]
    pub fn get_raw_peek(&self) -> u8 {
        self.raw_peek()
    }

    /// Vanilla `setRawPeekAmount` (plugin API).
    pub fn set_raw_peek(&self, amount: u8) {
        self.set_raw_peek_amount(amount);
    }

    /// Vanilla `isClosed`.
    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.raw_peek() == 0
    }

    /// Sets the synced peek amount alone, as vanilla does on load and teleport.
    fn store_raw_peek(&self, amount: u8) {
        self.raw_peek.store(amount, Relaxed);
        self.get_entity()
            .set_synced_data(tracked_data::shulker::PEEK_ID, amount);
    }

    /// Vanilla `setRawPeekAmount`: closing adds the covered armor, and either way sounds.
    fn set_raw_peek_amount(&self, amount: u8) {
        let living = &self.mob_entity.living_entity;
        let entity = &living.entity;
        living.update_attribute(&Attributes::ARMOR, |armor| {
            armor.remove_modifier(COVERED_ARMOR_MODIFIER_ID);
            if amount == 0 {
                armor.add_or_replace_permanent_modifier(Modifier {
                    id: COVERED_ARMOR_MODIFIER_ID.to_string(),
                    amount: COVERED_ARMOR,
                    operation: ModifierOperation::Add,
                });
            }
        });
        let (sound, event) = if amount == 0 {
            (Sound::EntityShulkerClose, GameEvent::ContainerClose)
        } else {
            (Sound::EntityShulkerOpen, GameEvent::ContainerOpen)
        };
        self.play_entity_sound(sound, 1.0, 1.0);
        let world = entity.world.load();
        world.emit_game_event_with_context(
            event.name(),
            entity.pos.load(),
            &GameEventContext {
                source_entity: Some(self),
            },
        );
        self.store_raw_peek(amount);
    }

    /// Vanilla `updatePeekAmount`: moves the lid one step towards the raw amount.
    fn update_peek_amount(&self) -> bool {
        let current = self.current_peek.load();
        let target = f32::from(self.raw_peek()) * 0.01;
        if current == target {
            return false;
        }
        let next = if current > target {
            // Vanilla `Mth.clamp(lowered, target, 1.0)`, which favours the lower bound.
            let lowered = current - PEEK_PER_TICK;
            if lowered < target {
                target
            } else {
                lowered.min(1.0)
            }
        } else {
            (current + PEEK_PER_TICK).clamp(0.0, target)
        };
        self.current_peek.store(next);
        true
    }

    /// Vanilla `getScale`, capped like `sanitizeScale`.
    fn scale(&self) -> f64 {
        self.mob_entity
            .living_entity
            .get_attribute_value(&Attributes::SCALE)
            .min(MAX_SCALE)
    }

    /// Vanilla `makeBoundingBox`: the shell grows out of its attach face as the lid opens.
    fn refresh_bounding_box(&self) {
        let entity = self.get_entity();
        let peek = physical_peek(self.current_peek.load());
        let direction = self.get_attach_face().opposite();
        let bounds = progress_aabb(self.scale(), direction, peek, entity.pos.load());
        entity.bounding_box.store(bounds);
    }

    /// Vanilla `canStayAt`: a free block with a sturdy face to hold on to and room to open.
    fn can_stay_at(&self, target: BlockPos, face: BlockDirection) -> bool {
        if self.is_position_blocked(target) {
            return false;
        }
        let world = self.get_entity().world.load();
        let opposite = face.opposite();
        let support = target.offset(face.to_offset());
        let sturdy = world
            .get_block_state_if_loaded(&support)
            .is_some_and(|state| state.is_side_solid(opposite));
        if !sturdy {
            return false;
        }
        let opened = progress_aabb(self.scale(), opposite, 1.0, bottom_center(target));
        world.is_space_empty(opened.contract_all(COLLISION_MARGIN))
    }

    fn can_stay_where_attached(&self) -> bool {
        self.can_stay_at(self.get_entity().block_pos.load(), self.get_attach_face())
    }

    /// Vanilla `isPositionBlocked`: not air, unless it is the piston moving this shulker.
    fn is_position_blocked(&self, target: BlockPos) -> bool {
        let world = self.get_entity().world.load();
        if world.get_block_state(&target).is_air() {
            return false;
        }
        let own_piston = world.get_block(&target) == &Block::MOVING_PISTON
            && target == self.get_entity().block_pos.load();
        !own_piston
    }

    /// Vanilla `findAttachableSurface`.
    fn find_attachable_surface(&self, target: BlockPos) -> Option<BlockDirection> {
        BlockDirection::all()
            .into_iter()
            .find(|&face| self.can_stay_at(target, face))
    }

    /// Vanilla `findNewAttachment`.
    fn find_new_attachment(&self) {
        let pos = self.get_entity().block_pos.load();
        if let Some(face) = self.find_attachable_surface(pos) {
            self.set_attach_face(face);
        } else {
            self.teleport_somewhere();
        }
    }

    /// Vanilla `teleportSomewhere`: up to five tries at a free spot within 8 blocks.
    fn teleport_somewhere(&self) -> bool {
        if self.mob_entity.is_no_ai() || !self.mob_entity.living_entity.is_alive() {
            return false;
        }
        let entity = self.get_entity();
        let world = entity.world.load_full();
        let old_pos = entity.block_pos.load();
        for _ in 0..TELEPORT_ATTEMPTS {
            let target = {
                let mut rng = self.get_random();
                let range = -MAX_TELEPORT_DISTANCE..=MAX_TELEPORT_DISTANCE;
                old_pos.add(
                    rng.random_range(range.clone()),
                    rng.random_range(range.clone()),
                    rng.random_range(range),
                )
            };
            if let Some(face) = self.teleport_face(&world, target) {
                self.teleport_to(&world, old_pos, target, face);
                return true;
            }
        }
        false
    }

    /// The face to attach to at `target`, if the shulker may teleport there.
    fn teleport_face(&self, world: &World, target: BlockPos) -> Option<BlockDirection> {
        let cell = BoundingBox::from_block(&target);
        let free = target.0.y > world.min_y
            && world.get_block_state(&target).is_air()
            && within_border(world, target)
            && world.is_space_empty(cell.contract_all(COLLISION_MARGIN));
        if !free {
            return None;
        }
        let face = self.find_attachable_surface(target)?;
        let support = world.get_block(&target.offset(face.to_offset()));
        (!support.has_tag(&tag::Block::MINECRAFT_SHULKER_DOES_NOT_TELEPORT_TO)).then_some(face)
    }

    fn teleport_to(
        &self,
        world: &Arc<World>,
        old_pos: BlockPos,
        target: BlockPos,
        face: BlockDirection,
    ) {
        let entity = self.get_entity();
        self.un_ride();
        self.set_attach_face(face);
        self.play_entity_sound(Sound::EntityShulkerTeleport, 1.0, 1.0);
        entity.teleport(bottom_center(target), None, None, world);
        world.emit_game_event_with_context(
            GameEvent::Teleport.name(),
            old_pos.to_centered_f64(),
            &GameEventContext {
                source_entity: Some(self),
            },
        );
        let packed = old_pos.pack_difference_in_position(
            &target,
            MAX_TELEPORT_DISTANCE,
            MAX_TELEPORT_DISTANCE,
            MAX_TELEPORT_DISTANCE,
        );
        world.sync_world_event(WorldEvent::ParticlesShulkerTeleport, old_pos, packed);
        self.store_raw_peek(0);
        self.set_mob_target(None, TargetReason::Unknown);
        self.refresh_bounding_box();
    }

    /// Vanilla `unRide`: off its vehicle and rid of its passengers.
    fn un_ride(&self) {
        let entity = self.get_entity();
        if let Some(vehicle) = entity.get_vehicle() {
            vehicle
                .get_entity()
                .remove_passenger_before_teleport(entity.entity_id);
        }
        let passengers = entity
            .passengers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        for passenger in passengers {
            entity.remove_passenger_before_teleport(passenger.get_entity().entity_id);
        }
    }

    /// Vanilla `hitByShulkerBullet`: an open shulker teleports away and may leave a new one.
    fn hit_by_shulker_bullet(&self) {
        let entity = self.get_entity();
        let old_pos = entity.pos.load();
        let old_bounds = entity.bounding_box.load();
        if self.is_closed() || !self.teleport_somewhere() {
            return;
        }
        let world = entity.world.load_full();
        let search = old_bounds.expand_all(OTHER_SHULKER_SCAN_RADIUS);
        let shulkers = world
            .get_entities_at_box(&search)
            .into_iter()
            .filter(is_live_shulker)
            .count();
        let failure_chance = (shulkers as f32 - 1.0) / OTHER_SHULKER_LIMIT;
        if rand::random::<f32>() < failure_chance {
            return;
        }
        let baby = Self::new(Entity::new(world.clone(), old_pos, &EntityType::SHULKER));
        baby.set_color_id(self.color.load(Relaxed));
        world.spawn_entity(baby);
    }

    /// Vanilla `ShulkerLookControl.getYRotD`: the head yaw towards `wanted`, measured in the
    /// plane the shulker opens out of.
    fn head_yaw_towards(&self, wanted: Vector3<f64>) -> Option<f32> {
        let entity = self.get_entity();
        let pos = entity.pos.load();
        let out = Vector3::new(
            (wanted.x - pos.x) as f32,
            (wanted.y - entity.get_eye_y()) as f32,
            (wanted.z - pos.z) as f32,
        );
        let up = self.get_attach_face().opposite();
        let forward = shell_forward(up);
        let normal = up.to_offset();
        let normal = Vector3::new(normal.x as f32, normal.y as f32, normal.z as f32);
        let right = normal.cross(&forward);
        let delta_right = right.dot(&out);
        let delta_forward = forward.dot(&out);
        if delta_right.abs() <= 1.0E-5 && delta_forward.abs() <= 1.0E-5 {
            return None;
        }
        Some((-delta_right).atan2(delta_forward).to_degrees())
    }
}

/// Vanilla `getPhysicalPeek`: how far the shell sticks out for a lid opening.
fn physical_peek(amount: f32) -> f64 {
    f64::from(0.5 - ((0.5 + amount) * std::f32::consts::PI).sin() * 0.5)
}

/// Vanilla `getProgressAabb`: a cube of `size` standing on `position`, stretched by
/// `progress` cubes towards `direction`.
fn progress_aabb(
    size: f64,
    direction: BlockDirection,
    progress: f64,
    position: Vector3<f64>,
) -> BoundingBox {
    let half = size * 0.5;
    let cube = BoundingBox::new(
        Vector3::new(-half, 0.0, -half),
        Vector3::new(half, size, half),
    );
    let offset = direction.to_offset();
    let stretch = Vector3::new(
        f64::from(offset.x) * progress * size,
        f64::from(offset.y) * progress * size,
        f64::from(offset.z) * progress * size,
    );
    cube.stretch(stretch).shift(position)
}

/// Vanilla `Vec3.atBottomCenterOf`.
fn bottom_center(pos: BlockPos) -> Vector3<f64> {
    Vector3::new(
        f64::from(pos.0.x) + 0.5,
        f64::from(pos.0.y),
        f64::from(pos.0.z) + 0.5,
    )
}

/// Vanilla `getRotation().transform(FORWARD)` for the face the shulker opens towards.
const fn shell_forward(up: BlockDirection) -> Vector3<f32> {
    match up {
        BlockDirection::Up => Vector3::new(0.0, 0.0, 1.0),
        BlockDirection::Down => Vector3::new(0.0, 0.0, -1.0),
        BlockDirection::North
        | BlockDirection::South
        | BlockDirection::West
        | BlockDirection::East => Vector3::new(0.0, -1.0, 0.0),
    }
}

fn within_border(world: &World, pos: BlockPos) -> bool {
    world
        .worldborder
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .contains_block(pos.0.x, pos.0.z)
}

fn as_shulker(mob: &dyn Mob) -> Option<&ShulkerEntity> {
    mob.cast_any().downcast_ref::<ShulkerEntity>()
}

fn not_peaceful(mob: &dyn Mob) -> bool {
    mob.get_entity().world.load().level_info.load().difficulty != Difficulty::Peaceful
}

/// Vanilla `ShulkerDefenseAttackGoal.canUse`: only a shulker on a team defends.
fn has_team(mob: &dyn Mob) -> bool {
    mob.get_team_name().is_some()
}

/// Vanilla `getTargetSearchArea` of the shulker's target goals: thin along the attach axis.
fn attach_search_area(mob: &dyn Mob, follow_range: f64) -> BoundingBox {
    let axis = as_shulker(mob).map_or(Axis::Y, |shulker| shulker.get_attach_face().to_axis());
    let bounds = mob.get_entity().bounding_box.load();
    match axis {
        Axis::X => bounds.expand(SEARCH_THICKNESS, follow_range, follow_range),
        Axis::Y => bounds.expand(follow_range, SEARCH_THICKNESS, follow_range),
        Axis::Z => bounds.expand(follow_range, follow_range, SEARCH_THICKNESS),
    }
}

fn is_alive(entity: &dyn EntityBase) -> bool {
    entity
        .get_living_entity()
        .is_some_and(LivingEntity::is_alive)
}

fn is_live_shulker(entity: &Arc<dyn EntityBase>) -> bool {
    entity.get_entity().entity_type == &EntityType::SHULKER && is_alive(entity.as_ref())
}

fn is_shulker_bullet(entity: &dyn EntityBase) -> bool {
    entity.get_entity().entity_type == &EntityType::SHULKER_BULLET
}

fn is_arrow(entity: &dyn EntityBase) -> bool {
    let entity_type = entity.get_entity().entity_type;
    entity_type == &EntityType::ARROW
        || entity_type == &EntityType::SPECTRAL_ARROW
        || entity_type == &EntityType::TRIDENT
}

impl Mob for ShulkerEntity {
    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    /// Vanilla `getDeltaMovement` is always zero, so a shulker never falls.
    fn get_mob_gravity(&self) -> f64 {
        0.0
    }

    fn get_max_look_pitch_change(&self) -> f32 {
        MAX_HEAD_ROTATION
    }

    fn get_max_head_rotation(&self) -> f32 {
        MAX_HEAD_ROTATION
    }

    fn get_ambient_sound(&self) -> Option<Sound> {
        Some(Sound::EntityShulkerAmbient)
    }

    /// Vanilla `playAmbientSound`: only while open.
    fn play_ambient_sound(&self) {
        if !self.is_closed() {
            self.make_sound(Sound::EntityShulkerAmbient);
        }
    }

    fn as_custom_sound(&self) -> Option<&dyn CustomSound> {
        Some(self)
    }

    fn mob_tick(&self, _caller: &dyn EntityBase) {
        self.get_entity()
            .velocity
            .store(Vector3::new(0.0, 0.0, 0.0));
    }

    /// The part of vanilla `Shulker.tick` after `super.tick()`.
    fn post_tick(&self) {
        let entity = self.get_entity();
        if !entity.has_vehicle() && !self.can_stay_where_attached() {
            self.find_new_attachment();
        }
        if self.update_peek_amount() {
            self.refresh_bounding_box();
        }
    }

    fn finalize_spawn(
        &self,
        _world: &Arc<World>,
        group_data: Option<SpawnGroupData>,
    ) -> Option<SpawnGroupData> {
        let entity = self.get_entity();
        entity.yaw.store(0.0);
        entity.head_yaw.store(0.0);
        self.finalize_spawn_super(group_data)
    }

    fn mob_init_data_tracker(&self) {
        let entity = self.get_entity();
        let face = self.get_attach_face();
        entity.set_synced_data(tracked_data::shulker::ATTACH_FACE_ID, VarInt(face as i32));
        entity.set_synced_data(tracked_data::shulker::PEEK_ID, self.raw_peek());
        entity.set_synced_data(tracked_data::shulker::COLOR, self.color.load(Relaxed) as i8);
    }

    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        nbt.put_byte("AttachFace", self.get_attach_face().to_index() as i8);
        nbt.put_byte("Peek", self.raw_peek() as i8);
        nbt.put_byte("Color", self.color.load(Relaxed) as i8);
    }

    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        if let Some(face) = nbt.get_byte("AttachFace") {
            // Vanilla `Direction.from3DDataValue` wraps any byte onto the six faces.
            let index = (i32::from(face) % 6).unsigned_abs() as u8;
            self.set_attach_face(BlockDirection::from_index(index).unwrap_or(DEFAULT_ATTACH_FACE));
        }
        if let Some(peek) = nbt.get_byte("Peek") {
            self.store_raw_peek(peek as u8);
        }
        if let Some(color) = nbt.get_byte("Color") {
            self.set_color_id(color as u8);
        }
    }

    fn mob_get_component(&self, component: DataComponent) -> Option<Box<dyn DataComponentImpl>> {
        if component != DataComponent::ShulkerColor {
            return None;
        }
        let value = self.dye_color()?.name().into();
        Some(Box::new(ShulkerColorImpl { value }))
    }

    fn mob_apply_implicit_component(&self, component: DataComponent, value: &str) {
        if component == DataComponent::ShulkerColor
            && let Some(color) = DyeColor::by_name(value)
        {
            self.set_color_id(color.id());
        }
    }

    /// Vanilla `hurtServer`: a closed shell turns arrows away.
    fn pre_damage_with_context(
        &self,
        _damage_type: DamageType,
        source: Option<&dyn EntityBase>,
        _cause: Option<&dyn EntityBase>,
    ) -> bool {
        let deflects = self.is_closed() && source.is_some_and(is_arrow);
        !deflects
    }

    /// Pumpkin's armor absorption reads only the equipment, so the shulker applies its armor
    /// attribute, which holds the covered bonus while closed, itself.
    fn modify_incoming_damage(&self, amount: f32, damage_type: DamageType) -> f32 {
        if crate::entity::living::bypasses_armor_durability(&damage_type) {
            return amount;
        }
        let living = &self.mob_entity.living_entity;
        let armor = living.get_attribute_value(&Attributes::ARMOR) as f32;
        let toughness = living.get_attribute_value(&Attributes::ARMOR_TOUGHNESS) as f32;
        CombatRules::get_damage_after_absorb(amount, armor, toughness, 0)
    }

    /// Vanilla `hurtServer` after the damage: a hurt shulker may teleport, an open one hit by
    /// a shulker bullet may duplicate.
    fn on_damage_with_context(
        &self,
        damage_type: DamageType,
        source: Option<&dyn EntityBase>,
        _cause: Option<&dyn EntityBase>,
    ) -> bool {
        let living = &self.mob_entity.living_entity;
        let low_health = living.health.load() < living.get_max_health() * 0.5;
        let bullet_hit = damage_type.has_tag(&tag::DamageType::MINECRAFT_IS_PROJECTILE)
            && source.is_some_and(is_shulker_bullet);
        if low_health && self.get_random().random_range(0..4) == 0 {
            self.teleport_somewhere();
        } else if bullet_hit {
            self.hit_by_shulker_bullet();
        }
        true
    }
}

impl CustomSound for ShulkerEntity {
    fn death_sound(&self) -> Option<Sound> {
        None
    }

    fn hurt_sound(&self) -> Option<Sound> {
        Some(if self.is_closed() {
            Sound::EntityShulkerHurtClosed
        } else {
            Sound::EntityShulkerHurt
        })
    }
}

/// Vanilla `Shulker.ShulkerLookControl`: turns the head in the plane of its shell and keeps
/// it level, never held back by the body.
#[derive(Default)]
struct ShulkerLookControl {
    max_yaw_change: f32,
    look_at_cooldown: i32,
    wanted: Vector3<f64>,
}

impl Control for ShulkerLookControl {}

impl LookControlTrait for ShulkerLookControl {
    fn tick(&mut self, mob: &dyn Mob) {
        let entity = mob.get_entity();
        // Vanilla `resetXRotOnTick`; `getXRotD` then asks for level, so the pitch stays 0.
        entity.set_pitch(0.0);
        let head_yaw = entity.head_yaw.load();
        if self.look_at_cooldown > 0 {
            self.look_at_cooldown -= 1;
            let yaw = as_shulker(mob).and_then(|shulker| shulker.head_yaw_towards(self.wanted));
            if let Some(yaw) = yaw {
                let turned = self.change_angle(head_yaw, yaw, self.max_yaw_change);
                entity.head_yaw.store(turned);
            }
        } else {
            let turned = self.change_angle(head_yaw, entity.body_yaw.load(), 10.0);
            entity.head_yaw.store(turned);
        }
    }

    fn look_at_with_range(
        &mut self,
        x: f64,
        y: f64,
        z: f64,
        max_yaw_change: f32,
        _max_pitch_change: f32,
    ) {
        self.wanted = Vector3::new(x, y, z);
        self.max_yaw_change = max_yaw_change;
        self.look_at_cooldown = 2;
    }

    fn wanted_position(&self) -> Vector3<f64> {
        self.wanted
    }

    fn is_looking_at_target(&self) -> bool {
        self.look_at_cooldown > 0
    }
}

/// Vanilla `Shulker.ShulkerAttackGoal`: opens up and shoots a bullet every 1 to 5.5 seconds.
#[derive(Default)]
struct ShulkerAttackGoal {
    attack_time: i32,
}

impl Goal for ShulkerAttackGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        let target = mob.get_mob_entity().get_target();
        target.is_some_and(|target| is_alive(target.as_ref())) && not_peaceful(mob)
    }

    fn start(&mut self, mob: &dyn Mob) {
        self.attack_time = ATTACK_INTERVAL;
        if let Some(shulker) = as_shulker(mob) {
            shulker.set_raw_peek_amount(ATTACK_PEEK);
        }
    }

    fn stop(&mut self, mob: &dyn Mob) {
        if let Some(shulker) = as_shulker(mob) {
            shulker.set_raw_peek_amount(0);
        }
    }

    fn should_run_every_tick(&self) -> bool {
        true
    }

    fn tick(&mut self, mob: &dyn Mob) {
        if !not_peaceful(mob) {
            return;
        }
        self.attack_time -= 1;
        let Some(target) = mob.get_mob_entity().get_target() else {
            return;
        };
        mob.get_mob_entity()
            .look_control
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .look_at_entity_with_range(&target, MAX_HEAD_ROTATION, MAX_HEAD_ROTATION);
        let target_entity = target.get_entity();
        let distance_sq = mob
            .get_entity()
            .pos
            .load()
            .squared_distance_to_vec(&target_entity.pos.load());
        if distance_sq >= ATTACK_RANGE_SQ {
            mob.set_mob_target(None, TargetReason::ForgotTarget);
        } else if self.attack_time <= 0 {
            let mut rng = mob.get_random();
            self.attack_time = ATTACK_INTERVAL + rng.random_range(0..10) * ATTACK_INTERVAL / 2;
            shoot_bullet(mob, target_entity);
            let pitch = (rng.random::<f32>() - rng.random::<f32>()) * 0.2 + 1.0;
            mob.play_entity_sound(Sound::EntityShulkerShoot, 2.0, pitch);
        }
    }

    fn controls(&self) -> Controls {
        Controls::MOVE | Controls::LOOK
    }
}

fn shoot_bullet(mob: &dyn Mob, target: &Entity) {
    let axis = as_shulker(mob).map_or(Axis::Y, |shulker| shulker.get_attach_face().to_axis());
    let entity = mob.get_entity();
    let bullet = ShulkerBulletEntity::new(entity, target, axis);
    entity.world.load_full().spawn_entity(Arc::new(bullet));
}

/// Vanilla `Shulker.ShulkerPeekGoal`: an idle shulker opens a crack for one to three seconds.
#[derive(Default)]
struct ShulkerPeekGoal {
    peek_time: i32,
}

impl Goal for ShulkerPeekGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        mob.get_mob_entity().get_target().is_none()
            && mob.get_random().random_range(0..to_goal_ticks(PEEK_CHANCE)) == 0
            && as_shulker(mob).is_some_and(ShulkerEntity::can_stay_where_attached)
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        mob.get_mob_entity().get_target().is_none() && self.peek_time > 0
    }

    fn start(&mut self, mob: &dyn Mob) {
        let seconds = 1 + mob.get_random().random_range(0..3);
        self.peek_time = self.get_tick_count(20 * seconds);
        if let Some(shulker) = as_shulker(mob) {
            shulker.set_raw_peek_amount(PEEK_OUT);
        }
    }

    fn stop(&mut self, mob: &dyn Mob) {
        if mob.get_mob_entity().get_target().is_none()
            && let Some(shulker) = as_shulker(mob)
        {
            shulker.set_raw_peek_amount(0);
        }
    }

    fn tick(&mut self, _mob: &dyn Mob) {
        self.peek_time -= 1;
    }
}
