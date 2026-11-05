//! Vanilla `AbstractThrownPotion`: what splash and lingering potions share.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use pumpkin_data::block_properties::{
    CampfireLikeProperties, CandleCakeProperties, CandleLikeProperties,
};
use pumpkin_data::damage::DamageType;
use pumpkin_data::game_event::GameEvent;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::tag::{self, Taggable};
use pumpkin_data::world::WorldEvent;
use pumpkin_data::{Block, BlockDirection, BlockStateId};
use pumpkin_util::math::boundingbox::BoundingBox;
use pumpkin_util::math::position::BlockPos;
use pumpkin_world::world::BlockFlags;

use super::ProjectileHit;
use crate::entity::EntityBase;
use crate::item::potion::{PotionContents, PotionEffect};
use crate::world::World;
use crate::world::game_event::GameEventContext;

/// Vanilla `SPLASH_RANGE`, also the horizontal inflation of the search box.
pub const SPLASH_RANGE: f64 = 4.0;
/// Vanilla `SPLASH_RANGE_SQ`.
pub const SPLASH_RANGE_SQ: f64 = 16.0;
/// The vertical inflation of the search box.
pub const SPLASH_HEIGHT: f64 = 2.0;

/// Vanilla `getEntitiesOfClass(LivingEntity.class, box)`: the living entities, no spectators.
pub(crate) fn living_in_box(world: &World, area: &BoundingBox) -> Vec<Arc<dyn EntityBase>> {
    world
        .get_all_at_box(area)
        .into_iter()
        .filter(|entity| entity.get_living_entity().is_some() && !entity.is_spectator())
        .collect()
}

/// Vanilla `AABB.distanceToSqr(AABB)`.
#[must_use]
pub fn box_distance_sqr(a: &BoundingBox, b: &BoundingBox) -> f64 {
    let dx = (a.min.x - b.max.x).max(b.min.x - a.max.x).max(0.0);
    let dy = (a.min.y - b.max.y).max(b.min.y - a.max.y).max(0.0);
    let dz = (a.min.z - b.max.z).max(b.min.z - a.max.z).max(0.0);
    dx * dx + dy * dy + dz * dz
}

/// Vanilla `AbstractThrownPotion.onHit` with `onHitBlock`; `on_hit_as_potion` is the abstract
/// method of the same name, which only runs for a potion with effects.
pub(super) fn on_hit(
    potion: &dyn EntityBase,
    stack: &ItemStack,
    hit: &ProjectileHit,
    on_hit_as_potion: impl FnOnce(&[PotionEffect]),
) {
    let entity = potion.get_entity();
    let world = entity.world.load_full();
    // Vanilla `ThrowableProjectile.tick` has moved only as far as the hit: what follows measures
    // from here and the lingering cloud starts here.
    entity.set_pos(hit.hit_pos());
    if let ProjectileHit::Block { pos, face, .. } = hit {
        douse_around(&world, potion, stack, *pos, *face);
    }
    affect_entities_around(&world, potion, stack);

    let effects = PotionContents::read_potion_effects(stack);
    if !effects.is_empty() {
        on_hit_as_potion(&effects);
    }

    let position = BlockPos::floored_v(hit.hit_pos());
    let (particles, sound) = if PotionContents::has_instant_potion(stack) {
        (
            WorldEvent::ParticlesInstantPotionSplash,
            WorldEvent::SoundInstantPotionSplash,
        )
    } else {
        (
            WorldEvent::ParticlesSpellPotionSplash,
            WorldEvent::SoundSpellPotionSplash,
        )
    };
    world.sync_world_event(particles, position, PotionContents::stack_color(stack));
    if !entity.silent.load(Ordering::Relaxed) {
        world.sync_world_event(sound, position, 0);
    }
}

/// Vanilla `AbstractThrownPotion.affectEntitiesAround`: a water bottle hurts the water
/// sensitive, puts out what burns and wets axolotls.
fn affect_entities_around(world: &Arc<World>, potion: &dyn EntityBase, stack: &ItemStack) {
    let hurts = PotionContents::is_in_tag(
        stack,
        &tag::Potion::MINECRAFT_HURTS_WATER_SENSITIVE_ENTITIES,
    );
    let extinguishes =
        PotionContents::is_in_tag(stack, &tag::Potion::MINECRAFT_EXTINGUISHES_ENTITIES);
    let rehydrates = PotionContents::is_in_tag(stack, &tag::Potion::MINECRAFT_REHYDRATES_AXOLOTLS);
    if !(hurts || extinguishes || rehydrates) {
        return;
    }

    let entity = potion.get_entity();
    let position = entity.pos.load();
    let area = entity
        .bounding_box
        .load()
        .expand(SPLASH_RANGE, SPLASH_HEIGHT, SPLASH_RANGE);
    let nearby = world.get_all_at_box(&area);
    let owner = potion.get_owner_entity();
    for target in &nearby {
        let Some(living) = target.get_living_entity() else {
            continue;
        };
        let target_entity = target.get_entity();
        if position.squared_distance_to_vec(&target_entity.pos.load()) >= SPLASH_RANGE_SQ {
            continue;
        }
        let sensitive = target
            .get_mob()
            .is_some_and(crate::entity::mob::Mob::is_sensitive_to_water);
        if hurts && sensitive {
            target.damage_with_context(
                &**target,
                1.0,
                DamageType::INDIRECT_MAGIC,
                None,
                Some(potion),
                owner.as_deref(),
            );
        }
        if extinguishes && target_entity.is_on_fire() && living.is_alive() {
            target_entity.extinguish();
        }
    }

    if rehydrates {
        for target in &nearby {
            if let Some(axolotl) = target
                .cast_any()
                .downcast_ref::<crate::entity::passive::axolotl::AxolotlEntity>()
            {
                axolotl.rehydrate();
            }
        }
    }
}

/// The dousing of vanilla `AbstractThrownPotion.onHitBlock`.
fn douse_around(
    world: &Arc<World>,
    potion: &dyn EntityBase,
    stack: &ItemStack,
    pos: BlockPos,
    face: BlockDirection,
) {
    if !PotionContents::is_in_tag(stack, &tag::Potion::MINECRAFT_DOUSES_FIRE) {
        return;
    }
    let effect_pos = pos.offset(face.to_offset());
    douse_fire(world, potion, effect_pos);
    douse_fire(world, potion, pos);
    for direction in [
        BlockDirection::North,
        BlockDirection::East,
        BlockDirection::South,
        BlockDirection::West,
    ] {
        douse_fire(world, potion, effect_pos.offset(direction.to_offset()));
    }
}

/// Vanilla `AbstractThrownPotion.douseFire`.
fn douse_fire(world: &Arc<World>, potion: &dyn EntityBase, pos: BlockPos) {
    let (block, state_id) = world.get_block_and_state_id(&pos);
    if block.has_tag(&tag::Block::MINECRAFT_FIRE) {
        // `destroyBlock` plays no break effect for fire.
        world.set_block_state(&pos, Block::AIR.default_state.id, BlockFlags::NOTIFY_ALL);
        world.emit_game_event_with_context(
            GameEvent::BlockDestroy.name(),
            pos.to_centered_f64(),
            &GameEventContext {
                source_entity: Some(potion),
            },
        );
    } else if block.has_tag(&tag::Block::MINECRAFT_CANDLES) {
        let mut properties = CandleLikeProperties::from_state_id(state_id);
        if properties.lit {
            properties.lit = false;
            extinguish_candle(world, pos, properties.to_state_id(block));
        }
    } else if block.has_tag(&tag::Block::MINECRAFT_CANDLE_CAKES) {
        let mut properties = CandleCakeProperties::from_state_id(state_id);
        if properties.lit {
            properties.lit = false;
            extinguish_candle(world, pos, properties.to_state_id(block));
        }
    } else if block.has_tag(&tag::Block::MINECRAFT_CAMPFIRES) {
        let mut properties = CampfireLikeProperties::from_state_id(state_id);
        if properties.lit {
            properties.lit = false;
            world.sync_world_event(WorldEvent::SoundExtinguishFire, pos, 0);
            // Vanilla `CampfireBlock.douse`.
            let owner = potion.get_owner_entity();
            world.emit_game_event_with_context(
                GameEvent::BlockChange.name(),
                pos.to_centered_f64(),
                &GameEventContext {
                    source_entity: owner.as_deref(),
                },
            );
            world.set_block_state(&pos, properties.to_state_id(block), BlockFlags::NOTIFY_ALL);
        }
    }
}

/// Vanilla `AbstractCandleBlock.extinguish` without a player.
fn extinguish_candle(world: &Arc<World>, pos: BlockPos, unlit: BlockStateId) {
    world.set_block_state(&pos, unlit, BlockFlags::NOTIFY_ALL);
    world.play_sound(
        Sound::BlockCandleExtinguish,
        SoundCategory::Blocks,
        &pos.to_centered_f64(),
    );
    world.emit_game_event(GameEvent::BlockChange.name(), pos.to_centered_f64());
}
