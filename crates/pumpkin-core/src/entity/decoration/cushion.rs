use std::sync::{
    Arc,
    atomic::{AtomicI32, AtomicU8, Ordering},
};

use pumpkin_data::damage::DamageType;
use pumpkin_data::dye_color::DyeColor;
use pumpkin_data::entity::EntityPose;
use pumpkin_data::fluid::Fluid;
use pumpkin_data::game_event::GameEvent;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::particle::Particle;
use pumpkin_data::sound::Sound;
use pumpkin_data::tag::{self, Taggable};
use pumpkin_data::{Block, BlockState};
use pumpkin_nbt::{compound::NbtCompound, tag::NbtTag};
use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_protocol::java::client::play::ParticleOptions;
use pumpkin_util::math::boundingbox::BoundingBox;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;

use crate::entity::lightning::LightningBoltEntity;
use crate::entity::player::Player;
use crate::entity::{Entity, EntityBase, living::LivingEntity};
use crate::server::Server;
use crate::world::World;
use crate::world::game_event::GameEventContext;

/// Vanilla `BlockAttachedEntity.CHECK_INTERVAL`.
const CHECK_INTERVAL: i32 = 100;
/// Vanilla `Cushion.hasAnchorBelow`: how far below the cushion a block still holds it.
const ANCHOR_THICKNESS: f64 = 0.015625;
const ANCHOR_SEARCH_DEPTH: f64 = 0.125;
/// Vanilla `Entity.lavaHurt`.
const LAVA_DAMAGE: f32 = 4.0;
const PARTICLE_COUNT: i32 = 10;
const PARTICLE_SPEED: f32 = 0.05;

/// Vanilla `AABB.nextDeflated`.
const fn next_deflated(bounding_box: &BoundingBox) -> BoundingBox {
    let (min, max) = (bounding_box.min, bounding_box.max);
    BoundingBox::new(
        Vector3::new(min.x.next_up(), min.y.next_up(), min.z.next_up()),
        Vector3::new(max.x.next_down(), max.y.next_down(), max.z.next_down()),
    )
}

/// Vanilla `VoxelShape.bounds`, `None` for an empty shape.
fn shape_bounds(state: &BlockState, pos: &BlockPos) -> Option<BoundingBox> {
    state.get_block_outline_shapes_at(pos).reduce(|a, b| {
        BoundingBox::new(
            Vector3::new(
                a.min.x.min(b.min.x),
                a.min.y.min(b.min.y),
                a.min.z.min(b.min.z),
            ),
            Vector3::new(
                a.max.x.max(b.max.x),
                a.max.y.max(b.max.y),
                a.max.z.max(b.max.z),
            ),
        )
    })
}

/// Vanilla `Cushion.hasAnchorBelow`.
fn has_anchor_below(world: &World, bounding_box: &BoundingBox) -> bool {
    let (min, max) = (bounding_box.min, bounding_box.max);
    let anchor = BoundingBox::new(
        Vector3::new(min.x, min.y - ANCHOR_THICKNESS, min.z),
        Vector3::new(max.x.next_down(), min.y, max.z.next_down()),
    );
    let search = anchor.expand_towards(0.0, -ANCHOR_SEARCH_DEPTH, 0.0);
    let mut positions = BlockPos::iterate(search.min_block_pos(), search.max_block_pos());
    positions.any(|pos| {
        shape_bounds(world.get_block_state(&pos), &pos)
            .is_some_and(|bounds| bounds.at_pos(pos).intersects(&anchor))
    })
}

/// Vanilla `Cushion.isAnchorBuried`: colliders leave nothing of the slice the cushion rests in.
fn is_anchor_buried(world: &World, bounding_box: &BoundingBox) -> bool {
    let (min, max) = (bounding_box.min, bounding_box.max);
    let slice = next_deflated(&BoundingBox::new(
        min,
        Vector3::new(max.x, min.y + ANCHOR_THICKNESS, max.z),
    ));
    let mut exposed = vec![slice];
    for pos in BlockPos::iterate(slice.min_block_pos(), slice.max_block_pos()) {
        for shape in world
            .get_block_state(&pos)
            .get_block_collision_shapes_at(&pos)
        {
            let collider = shape.at_pos(pos);
            exposed = exposed
                .iter()
                .flat_map(|piece| piece.subtract(&collider))
                .collect();
            if exposed.is_empty() {
                return true;
            }
        }
    }
    false
}

/// Vanilla `Cushion.isCoveredBySuffocatingBlocks`.
fn is_covered_by_suffocating_blocks(world: &World, bounding_box: &BoundingBox) -> bool {
    let deflated = next_deflated(bounding_box);
    let mut positions = BlockPos::iterate(deflated.min_block_pos(), deflated.max_block_pos());
    positions.all(|pos| world.get_block_state(&pos).is_suffocating())
}

/// Vanilla `Cushion`, a `BlockAttachedEntity` a player can sit on.
pub struct CushionEntity {
    pub entity: Entity,
    color: AtomicU8,
    ticks_since_last_check: AtomicI32,
}

impl CushionEntity {
    pub fn new(entity: Entity, color: u8) -> Self {
        entity
            .synched_data
            .define(pumpkin_data::tracked_data::cushion::COLOR, VarInt(0));
        let cushion = Self {
            entity,
            color: AtomicU8::new(color),
            ticks_since_last_check: AtomicI32::new(0),
        };
        cushion.sync_color();
        cushion
    }

    #[must_use]
    pub fn color(&self) -> u8 {
        self.color.load(Ordering::Relaxed)
    }

    pub fn set_color(&self, color: u8) {
        self.color.store(color, Ordering::Relaxed);
        self.sync_color();
    }

    fn sync_color(&self) {
        self.entity.set_synced_data(
            pumpkin_data::tracked_data::cushion::COLOR,
            VarInt(i32::from(self.color())),
        );
    }

    #[must_use]
    pub const fn item_for_color(color: u8) -> &'static Item {
        match color {
            0 => &Item::WHITE_CUSHION,
            1 => &Item::ORANGE_CUSHION,
            2 => &Item::MAGENTA_CUSHION,
            3 => &Item::LIGHT_BLUE_CUSHION,
            4 => &Item::YELLOW_CUSHION,
            5 => &Item::LIME_CUSHION,
            6 => &Item::PINK_CUSHION,
            7 => &Item::GRAY_CUSHION,
            8 => &Item::LIGHT_GRAY_CUSHION,
            9 => &Item::CYAN_CUSHION,
            10 => &Item::PURPLE_CUSHION,
            11 => &Item::BLUE_CUSHION,
            12 => &Item::BROWN_CUSHION,
            13 => &Item::GREEN_CUSHION,
            14 => &Item::RED_CUSHION,
            _ => &Item::BLACK_CUSHION,
        }
    }

    #[must_use]
    pub fn color_from_item(item: &Item) -> u8 {
        for (id, comp) in item.components {
            if *id == pumpkin_data::data_component::DataComponent::CushionColor {
                return pumpkin_data::data_component_impl::get::<
                    pumpkin_data::data_component_impl::CushionColorImpl,
                >(*comp)
                .color
                .id();
            }
        }
        // Vanilla's `Items` gives each cushion its `CUSHION_COLOR`, which the extracted item
        // components leave out; the item's name holds the color as well.
        item.registry_key
            .strip_suffix("_cushion")
            .and_then(DyeColor::by_name)
            .map_or_else(|| DyeColor::White.id(), |color| color.id())
    }

    #[must_use]
    pub fn color_from_stack(stack: &ItemStack) -> u8 {
        stack
            .get_data_component::<pumpkin_data::data_component_impl::CushionColorImpl>()
            .map_or_else(|| Self::color_from_item(stack.get_item()), |c| c.color.id())
    }

    /// Vanilla `Cushion.canBePlacedAt`.
    #[must_use]
    pub fn can_be_placed_at(world: &World, bounding_box: &BoundingBox) -> bool {
        Self::would_survive_at(world, bounding_box) && !is_anchor_buried(world, bounding_box)
    }

    /// Vanilla `Cushion.wouldSurviveAt`.
    #[must_use]
    pub fn would_survive_at(world: &World, bounding_box: &BoundingBox) -> bool {
        has_anchor_below(world, bounding_box)
            && !is_covered_by_suffocating_blocks(world, bounding_box)
    }

    /// Vanilla `Entity.playSound`: at the entity, unless it is silent.
    fn play_sound(&self, sound: Sound) {
        if !self.entity.is_silent() {
            self.entity.play_sound(sound);
        }
    }

    /// Vanilla `Entity.setRemoved` lets the riders off, before they could hear the get-up sound.
    fn remove_and_eject(&self) {
        self.entity.remove();
        let riders: Vec<i32> = self
            .entity
            .passengers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .rev()
            .map(|rider| rider.get_entity().entity_id)
            .collect();
        for rider in riders {
            self.entity.remove_passenger(rider);
        }
    }

    /// Vanilla `BlockAttachedEntity.kill`.
    fn kill_attributed(&self, attributed_to: Option<&dyn EntityBase>) {
        let pos = self.entity.pos.load();
        self.remove_and_eject();
        self.entity.world.load().emit_game_event_with_context(
            GameEvent::EntityDie.name(),
            pos,
            &GameEventContext {
                source_entity: Some(attributed_to.unwrap_or(self)),
            },
        );
    }

    /// Vanilla `Cushion.dropItem`: the item carries the custom name.
    fn drop_item(&self, caused_by: Option<&dyn EntityBase>) {
        let entity = &self.entity;
        self.play_sound(Sound::EntityCushionBreak);
        self.show_breaking_particles();
        if !entity
            .world
            .load()
            .level_info
            .load()
            .game_rules
            .entity_drops
        {
            return;
        }
        if caused_by
            .and_then(EntityBase::get_player)
            .is_some_and(Player::is_creative)
        {
            return;
        }
        let mut stack = ItemStack::new(1, Self::item_for_color(self.color()));
        if let Some(name) = entity.custom_name.load().as_ref() {
            stack.set_data_component(pumpkin_data::data_component_impl::CustomNameImpl {
                name: name.clone(),
            });
        }
        entity.spawn_at_location(stack);
    }

    /// Vanilla `Cushion.showBreakingParticles`: wool of the cushion's colour.
    fn show_breaking_particles(&self) {
        let entity = &self.entity;
        let color = DyeColor::by_id(self.color()).unwrap_or(DyeColor::White);
        let Some(wool) = Block::from_name(&format!("{}_wool", color.name())) else {
            return;
        };
        let pos = entity.pos.load();
        let (width, height) = (entity.width(), entity.height());
        entity.world.load().spawn_particle_with_options(
            Vector3::new(pos.x, pos.y + f64::from(height) * (2.0 / 3.0), pos.z),
            Vector3::new(width / 4.0, height / 4.0, width / 4.0),
            PARTICLE_SPEED,
            PARTICLE_COUNT,
            &ParticleOptions::Block {
                particle: Particle::Block,
                state: wool.default_state.id,
            },
        );
    }

    /// Vanilla `Cushion.hurtServer` and `BlockAttachedEntity.hurtServer`.
    fn hurt_server(&self, damage_type: DamageType, cause: Option<&dyn EntityBase>) -> bool {
        let entity = &self.entity;
        let world = entity.world.load();
        if let Some(player) = cause.and_then(EntityBase::get_player)
            && (!player.may_build()
                || world.is_in_spawn_protection(player, &entity.block_pos.load()))
        {
            return false;
        }
        if entity.is_invulnerable_to(&damage_type, cause) {
            return false;
        }
        if !world.level_info.load().game_rules.mob_griefing
            && cause.is_some_and(|source| source.get_mob().is_some())
        {
            return false;
        }
        if !entity.is_removed() {
            self.kill_attributed(cause);
            self.drop_item(cause);
        }
        true
    }

    /// Vanilla `Cushion.tickAtCheckInterval`: only lava does anything to a cushion in a fluid.
    fn tick_at_check_interval(&self) {
        let entity = &self.entity;
        let world = entity.world.load();
        let block_pos = entity.block_pos.load();
        let (fluid, state) = world.get_fluid_and_fluid_state(&block_pos);
        if fluid.matches_type(&Fluid::LAVA) {
            let height = world.get_fluid_height(&block_pos, fluid, &state);
            let fluid_box = BoundingBox::new(
                block_pos.0.to_f64(),
                block_pos.0.to_f64().add_raw(1.0, f64::from(height), 1.0),
            );
            if entity.bounding_box.load().intersects(&fluid_box)
                && self.damage_with_context(self, LAVA_DAMAGE, DamageType::LAVA, None, None, None)
                && !entity.is_silent()
            {
                world.play_sound_fine(
                    Sound::EntityGenericBurn,
                    entity.entity_type.sound_source,
                    &entity.pos.load(),
                    0.4,
                    2.0 + rand::random::<f32>() * 0.4,
                );
            }
        }
        self.destroy_if_in_fire();
    }

    /// Vanilla `Cushion.destroyIfInFire`.
    pub fn destroy_if_in_fire(&self) {
        if self.entity.is_removed() {
            return;
        }
        let deflated = next_deflated(&self.entity.bounding_box.load());
        let world = self.entity.world.load();
        let mut positions = BlockPos::iterate(deflated.min_block_pos(), deflated.max_block_pos());
        if positions.any(|pos| {
            Block::from_state_id(world.get_block_state(&pos).id)
                .has_tag(&tag::Block::MINECRAFT_FIRE)
        }) {
            self.damage_with_context(self, 1.0, DamageType::IN_FIRE, None, None, None);
        }
    }

    /// Vanilla `Entity.startRiding` with the cushion to ride.
    fn start_riding(&self, player: &Arc<Player>) -> bool {
        let entity = &self.entity;
        let rider = player.get_entity();
        let current = rider.get_vehicle();
        if current
            .as_ref()
            .is_some_and(|vehicle| vehicle.get_entity().entity_id == entity.entity_id)
            || rider.has_indirect_passenger(entity)
            || rider.riding_cooldown.load(Ordering::Relaxed) > 0
        {
            return false;
        }
        let world = entity.world.load();
        let Some(vehicle) = world.get_entity_by_id(entity.entity_id) else {
            return false;
        };

        if let Some(current) = current {
            current.get_entity().remove_passenger(rider.entity_id);
        }
        rider.set_pose(EntityPose::Standing);
        entity.add_passenger(vehicle, player.clone() as Arc<dyn EntityBase>);
        // A plugin can refuse the mount.
        if !entity.has_passenger(rider.entity_id) {
            return false;
        }
        let source: &dyn EntityBase = &**player;
        world.emit_game_event_with_context(
            GameEvent::EntityMount.name(),
            entity.pos.load(),
            &GameEventContext {
                source_entity: Some(source),
            },
        );
        true
    }
}

impl EntityBase for CushionEntity {
    fn get_entity(&self) -> &Entity {
        &self.entity
    }

    fn get_living_entity(&self) -> Option<&LivingEntity> {
        None
    }

    fn init_data_tracker(&self) {
        self.sync_color();
    }

    /// Vanilla `BlockAttachedEntity.tick`.
    fn tick(&self, caller: &dyn EntityBase, _server: &Server) {
        let entity = &self.entity;
        entity.check_out_of_world(caller);
        if self.ticks_since_last_check.fetch_add(1, Ordering::Relaxed) < CHECK_INTERVAL {
            return;
        }
        self.ticks_since_last_check.store(0, Ordering::Relaxed);
        self.tick_at_check_interval();
        if !entity.is_removed()
            && !Self::would_survive_at(&entity.world.load(), &entity.bounding_box.load())
        {
            self.remove_and_eject();
            self.drop_item(None);
        }
    }

    /// Vanilla `BlockAttachedEntity.isPickable`.
    fn can_hit(&self) -> bool {
        self.entity.is_alive()
    }

    fn write_custom_nbt(&self, nbt: &mut NbtCompound) {
        let block_pos = self.entity.block_pos.load().0;
        nbt.put(
            "block_pos",
            NbtTag::IntArray(vec![block_pos.x, block_pos.y, block_pos.z]),
        );
        let color = DyeColor::by_id(self.color()).unwrap_or(DyeColor::White);
        nbt.put_string("color", color.name().to_string());
    }

    /// Vanilla `Cushion.readAdditionalSaveData`, and the `Color` byte Pumpkin saved before.
    fn read_custom_nbt(&self, nbt: &NbtCompound) {
        let color = nbt
            .get_string("color")
            .and_then(DyeColor::by_name)
            .map(|color| color.id())
            .or_else(|| nbt.get_byte("Color").map(|color| color as u8))
            .unwrap_or(DyeColor::White.id());
        self.set_color(color);
    }

    /// Vanilla `Cushion.interact`: `false` is `PASS`, a refused ride still consumes the click.
    fn interact(&self, player: &Arc<Player>, _item_stack: &mut ItemStack) -> bool {
        if player.get_entity().is_sneaking() || self.entity.has_passengers() {
            return false;
        }
        if self.start_riding(player) {
            self.play_sound(Sound::EntityCushionSit);
        }
        true
    }

    /// Vanilla `BlockAttachedEntity.skipAttackInteraction`: a player's hit never goes on to
    /// `Player.attack`.
    fn skip_attack_interaction(&self, source: &Player) -> bool {
        let entity = &self.entity;
        if entity
            .world
            .load()
            .is_in_spawn_protection(source, &entity.block_pos.load())
        {
            return true;
        }
        self.hurt_server(DamageType::PLAYER_ATTACK, Some(source))
    }

    fn damage_with_context(
        &self,
        _caller: &dyn EntityBase,
        _amount: f32,
        damage_type: DamageType,
        _position: Option<Vector3<f64>>,
        source: Option<&dyn EntityBase>,
        cause: Option<&dyn EntityBase>,
    ) -> bool {
        self.hurt_server(damage_type, cause.or(source))
    }

    /// Vanilla `BlockAttachedEntity.kill`: no item.
    fn kill(&self, _caller: &dyn EntityBase) {
        self.kill_attributed(None);
    }

    /// Vanilla `Cushion.thunderHit`.
    fn on_lightning_strike(&self, _caller: &dyn EntityBase, lightning: &LightningBoltEntity) {
        if !self.entity.is_removed() {
            self.kill_attributed(Some(lightning));
            self.drop_item(Some(lightning));
        }
    }

    /// Vanilla `BlockAttachedEntity.move`: anything that moves a cushion breaks it.
    fn move_entity(&self, _caller: &dyn EntityBase, motion: Vector3<f64>) {
        if !self.entity.is_removed() && motion.length_squared() > 0.0 {
            self.kill_attributed(None);
            self.drop_item(None);
        }
    }

    fn cast_any(&self) -> &dyn std::any::Any {
        self
    }
}
