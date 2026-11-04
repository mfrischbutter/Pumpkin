use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};

use crate::block::blocks::fire::FireBlockBase;
use crate::block::blocks::fire::fire::FireBlock;
use crate::entity::ai::brain::memory::codec::{block_pos_from_tag, block_pos_to_tag};
use crate::entity::boss::ender_dragon::EnderDragonEntity;
use crate::entity::decoration::block_attached::emit_game_event;
use crate::entity::{Entity, EntityBase, living::LivingEntity};
use crate::server::Server;
use pumpkin_data::{
    Block,
    damage::DamageType,
    entity::{EntityType, MovementEmission},
    game_event::GameEvent,
    tag::{self, Taggable},
    tracked_data::end_crystal,
};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::math::{position::BlockPos, vector3::Vector3};
use pumpkin_world::world::BlockFlags;

/// Vanilla `EndCrystal`'s random start of `time`, bounded at this value.
const TIME_START_BOUND: i32 = 100_000;
/// Vanilla `hurtServer`: the power of the explosion of a crystal that was hit.
const EXPLOSION_POWER: f32 = 6.0;

pub struct EndCrystalEntity {
    entity: Entity,
    beam_target: Mutex<Option<BlockPos>>,
    show_bottom: AtomicBool,
    /// Vanilla `EndCrystal.time`.
    time: AtomicI32,
}

impl EndCrystalEntity {
    #[must_use]
    pub fn new(mut entity: Entity) -> Self {
        entity.blocks_building = true;
        entity
            .synched_data
            .define(end_crystal::BEAM_TARGET, None::<BlockPos>);
        // Vanilla `DEFAULT_SHOW_BOTTOM`.
        entity.synched_data.define(end_crystal::SHOW_BOTTOM, true);
        Self {
            entity,
            beam_target: Mutex::new(None),
            show_bottom: AtomicBool::new(true),
            time: AtomicI32::new(rand::random_range(0..TIME_START_BOUND)),
        }
    }

    pub fn set_show_bottom(&self, show_bottom: bool) {
        self.show_bottom.store(show_bottom, Ordering::Relaxed);
        self.entity
            .set_synced_data(end_crystal::SHOW_BOTTOM, show_bottom);
    }

    #[must_use]
    pub fn show_bottom(&self) -> bool {
        self.show_bottom.load(Ordering::Relaxed)
    }

    pub fn set_beam_target(&self, beam_target: Option<BlockPos>) {
        *self
            .beam_target
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = beam_target;
        self.entity
            .set_synced_data(end_crystal::BEAM_TARGET, beam_target);
    }

    #[must_use]
    pub fn beam_target(&self) -> Option<BlockPos> {
        *self
            .beam_target
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The vanilla `Invulnerable` tag every entity has; the dragon fight sets it on the
    /// crystals of a respawn.
    pub fn set_invulnerable(&self, invulnerable: bool) {
        self.entity.set_invulnerable(invulnerable);
    }

    #[must_use]
    pub fn is_invulnerable(&self) -> bool {
        self.entity.invulnerable.load(Ordering::Relaxed)
    }

    /// Vanilla `onDestroyedBy`: tells the dragon fight, and through it the dragon.
    fn on_destroyed_by(&self, cause: Option<&dyn EntityBase>) {
        let world = self.entity.world.load();
        let dragon = world.dragon_fight.as_ref().and_then(|fight_mutex| {
            let mut fight = fight_mutex
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let for_respawn = fight.is_respawning();
            fight.on_crystal_destroyed(&world, self.entity.entity_uuid);
            (!for_respawn).then_some(fight.dragon_uuid).flatten()
        });
        if let Some(dragon) = dragon.and_then(|uuid| {
            world
                .entities
                .load()
                .iter()
                .find(|entity| entity.get_entity().entity_uuid == uuid)
                .cloned()
        }) && let Some(dragon) = dragon.cast_any().downcast_ref::<EnderDragonEntity>()
        {
            dragon.on_crystal_destroyed(self, cause);
        }
    }

    /// The fire vanilla `tick` keeps below a crystal in a level with a dragon fight.
    fn keep_fire_lit(&self) {
        let world = self.entity.world.load();
        if world.dragon_fight.is_none() {
            return;
        }
        let pos = self.entity.block_pos.load();
        if !world.get_block_state(&pos).is_air() {
            return;
        }
        // Vanilla `BaseFireBlock.getState`.
        let fire_block = FireBlockBase::get_fire_type(&world, &pos);
        let state = if fire_block == Block::SOUL_FIRE {
            fire_block.default_state.id
        } else {
            FireBlock.get_state_for_position(&world, &fire_block, &pos)
        };
        world.set_block_state(&pos, state, BlockFlags::NOTIFY_ALL);
    }
}

impl EntityBase for EndCrystalEntity {
    fn get_entity(&self) -> &Entity {
        &self.entity
    }

    fn get_living_entity(&self) -> Option<&LivingEntity> {
        None
    }

    /// Vanilla `EndCrystal.tick`, which does none of the base entity's tick but these.
    fn tick(&self, caller: &dyn EntityBase, _server: &Server) {
        self.time.fetch_add(1, Ordering::Relaxed);
        self.entity.tick_block_collisions(caller);
        self.entity.tick_portal(caller);
        self.keep_fire_lit();
    }

    fn write_custom_nbt(&self, nbt: &mut NbtCompound) {
        if let Some(beam_target) = self.beam_target() {
            nbt.put("beam_target", block_pos_to_tag(beam_target));
        }
        nbt.put_bool("ShowBottom", self.show_bottom());
    }

    fn read_custom_nbt(&self, nbt: &NbtCompound) {
        self.set_beam_target(nbt.get("beam_target").and_then(block_pos_from_tag));
        self.set_show_bottom(nbt.get_bool("ShowBottom").unwrap_or(true));
    }

    /// Vanilla `getMovementEmission`.
    fn movement_emission(&self) -> MovementEmission {
        MovementEmission::None
    }

    /// Vanilla `isPickable`.
    fn can_hit(&self) -> bool {
        self.entity.is_alive()
    }

    /// Vanilla `EndCrystal.hurtServer`.
    fn damage_with_context(
        &self,
        _caller: &dyn EntityBase,
        _amount: f32,
        damage_type: DamageType,
        _position: Option<Vector3<f64>>,
        source: Option<&dyn EntityBase>,
        cause: Option<&dyn EntityBase>,
    ) -> bool {
        if self.entity.is_invulnerable_to(&damage_type, cause) {
            return false;
        }
        // Vanilla: the dragon cannot blow up its own crystals.
        if cause
            .or(source)
            .is_some_and(|by| by.get_entity().entity_type == &EntityType::ENDER_DRAGON)
        {
            return false;
        }

        self.entity.remove();
        if !damage_type.has_tag(&tag::DamageType::MINECRAFT_IS_EXPLOSION) {
            self.entity.world.load().explode(
                Some(&self.entity),
                self.entity.pos.load(),
                EXPLOSION_POWER,
                crate::world::ExplosionInteraction::Block,
            );
        }
        self.on_destroyed_by(cause.or(source));
        true
    }

    /// Vanilla `EndCrystal.kill`.
    fn kill(&self, _caller: &dyn EntityBase) {
        self.on_destroyed_by(None);
        self.entity.remove();
        emit_game_event(&self.entity, GameEvent::EntityDie, Some(self));
    }

    fn cast_any(&self) -> &dyn std::any::Any {
        self
    }
}
