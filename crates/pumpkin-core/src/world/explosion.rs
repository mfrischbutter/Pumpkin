use std::sync::Arc;

use pumpkin_data::{
    Block, BlockState, BlockStateId,
    entity::EntityType,
    fluid::Fluid,
    game_event::GameEvent,
    particle::Particle,
    sound::Sound,
    tag::{Tag, Taggable},
};
use pumpkin_util::math::{position::BlockPos, vector3::Vector3};
use pumpkin_world::chunk::ChunkData;
use rand::{RngExt, rng};
use rustc_hash::{FxHashMap, FxHashSet};

use crate::{
    block::{ExplodeArgs, blocks::fire::FireBlockBase, drop_loot},
    entity::{Entity, EntityBase},
    plugin::api::events::{
        block::block_explode::BlockExplodeEvent, entity::entity_explode::EntityExplodeEvent,
    },
    server::Server,
    world::{game_event::GameEventContext, loot::LootContextParameters},
};

use super::{BlockFlags, World};

mod entities;

/// Vanilla `SimpleExplosionDamageCalculator`: the resistance of its immune blocks.
const IMMUNE_BLOCK_RESISTANCE: f32 = 3_600_000.0;

/// Defines the type of explosion interaction with the world.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExplosionInteraction {
    None,
    Block,
    Mob,
    Tnt,
    Trigger,
}

/// Defines how an explosion interacts with blocks in the world.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockInteraction {
    /// Keeps blocks intact (no block damage, no drops).
    Keep,
    /// Destroys blocks and drops 100% of items without decay.
    Destroy,
    /// Destroys blocks and applies loot decay based on explosion radius.
    DestroyWithDecay,
    /// Triggers block effects without destroying them.
    TriggerBlock,
}

/// Vanilla `ExplosionDamageCalculator.getBlockExplosionResistance`.
#[must_use]
pub(crate) const fn default_block_explosion_resistance(
    block: &Block,
    fluid: &pumpkin_data::fluid::FluidState,
) -> Option<f32> {
    if block.default_state.is_air() && fluid.is_empty {
        None
    } else {
        Some(fluid.blast_resistance.max(block.blast_resistance))
    }
}

/// Vanilla `PrimedTnt.USED_PORTAL_DAMAGE_CALCULATOR`: what came through a nether portal leaves
/// the portal standing.
pub struct UsedPortalDamageCalculator;

impl ExplosionDamageCalculator for UsedPortalDamageCalculator {
    fn get_block_explosion_resistance(
        &self,
        _explosion: &Explosion,
        _world: &World,
        _pos: &BlockPos,
        block: &Block,
        fluid: &pumpkin_data::fluid::FluidState,
    ) -> Option<f32> {
        if block == &Block::NETHER_PORTAL {
            None
        } else {
            default_block_explosion_resistance(block, fluid)
        }
    }

    fn should_block_explode(
        &self,
        _explosion: &Explosion,
        _world: &World,
        _pos: &BlockPos,
        block: &Block,
        _power: f32,
    ) -> bool {
        block != &Block::NETHER_PORTAL
    }
}

/// Defines how damage and block destruction are calculated for an explosion.
pub trait ExplosionDamageCalculator: Send + Sync {
    /// Returns the block's explosion resistance. If None, the block is treated as air/empty.
    fn get_block_explosion_resistance(
        &self,
        _explosion: &Explosion,
        _world: &World,
        _pos: &BlockPos,
        block: &Block,
        fluid: &pumpkin_data::fluid::FluidState,
    ) -> Option<f32> {
        default_block_explosion_resistance(block, fluid)
    }

    /// Returns whether this block should be destroyed / affected by the explosion.
    fn should_block_explode(
        &self,
        _explosion: &Explosion,
        _world: &World,
        _pos: &BlockPos,
        _block: &Block,
        _power: f32,
    ) -> bool {
        true
    }

    /// Returns whether the entity should take damage from the explosion.
    fn should_damage_entity(&self, _explosion: &Explosion, _entity: &dyn EntityBase) -> bool {
        true
    }

    /// Returns knockback multiplier for the given entity (default 1.0).
    fn get_knockback_multiplier(&self, _entity: &dyn EntityBase) -> f32 {
        1.0
    }

    /// Calculates the damage amount to deal to the entity given the exposure.
    fn get_entity_damage_amount(
        &self,
        explosion: &Explosion,
        entity: &dyn EntityBase,
        exposure: f32,
    ) -> f32 {
        // Vanilla `doubleRadius`.
        let radius = explosion.power as f64 * 2.0;
        let distance = (entity
            .get_entity()
            .pos
            .load()
            .squared_distance_to_vec(&explosion.pos))
        .sqrt()
            / radius;
        let damage_multiplier = (1.0 - distance) * exposure as f64;
        (f64::midpoint(damage_multiplier * damage_multiplier, damage_multiplier)
            * 7.0
            * radius
            + 1.0) as f32
    }
}

/// Default explosion damage calculator implementing vanilla standard explosion rules.
pub struct DefaultExplosionDamageCalculator;

impl ExplosionDamageCalculator for DefaultExplosionDamageCalculator {}

/// A configurable explosion damage calculator (e.g. for wind charges, mace wind bursts).
pub struct SimpleExplosionDamageCalculator {
    pub damages_entities: bool,
    pub damages_blocks: bool,
    pub knockback_multiplier: Option<f32>,
    pub immune_blocks: Option<&'static Tag>,
}

impl SimpleExplosionDamageCalculator {
    #[must_use]
    pub const fn new(
        damages_entities: bool,
        damages_blocks: bool,
        knockback_multiplier: Option<f32>,
        immune_blocks: Option<&'static Tag>,
    ) -> Self {
        Self {
            damages_entities,
            damages_blocks,
            knockback_multiplier,
            immune_blocks,
        }
    }
}

impl ExplosionDamageCalculator for SimpleExplosionDamageCalculator {
    fn get_block_explosion_resistance(
        &self,
        _explosion: &Explosion,
        _world: &World,
        _pos: &BlockPos,
        block: &Block,
        fluid: &pumpkin_data::fluid::FluidState,
    ) -> Option<f32> {
        // Vanilla: with immune blocks nothing else resists at all, so a burst reaches through
        // walls until an immune block stops the ray.
        if let Some(immune_tag) = self.immune_blocks {
            return block.has_tag(immune_tag).then_some(IMMUNE_BLOCK_RESISTANCE);
        }
        default_block_explosion_resistance(block, fluid)
    }

    fn should_block_explode(
        &self,
        _explosion: &Explosion,
        _world: &World,
        _pos: &BlockPos,
        _block: &Block,
        _power: f32,
    ) -> bool {
        self.damages_blocks
    }

    fn should_damage_entity(&self, _explosion: &Explosion, _entity: &dyn EntityBase) -> bool {
        self.damages_entities
    }

    fn get_knockback_multiplier(&self, _entity: &dyn EntityBase) -> f32 {
        self.knockback_multiplier.unwrap_or(1.0)
    }
}

/// Vanilla `Level.explode`'s settings beyond source, place, power and interaction.
pub struct ExplosionSettings {
    pub calculator: Option<Arc<dyn ExplosionDamageCalculator>>,
    pub fire: bool,
    pub small_particle: Particle,
    pub large_particle: Particle,
    pub sound: Sound,
}

impl Default for ExplosionSettings {
    /// Vanilla `Level.explode` without particles and sound of its own.
    fn default() -> Self {
        Self {
            calculator: None,
            fire: false,
            small_particle: Particle::Explosion,
            large_particle: Particle::ExplosionEmitter,
            sound: Sound::EntityGenericExplode,
        }
    }
}

/// What an explosion that happened means for the packet to the players.
pub struct ExplosionOutcome {
    pub block_count: u32,
    /// Vanilla `ServerExplosion.hitPlayers`, by entity id.
    pub hit_players: FxHashMap<i32, Vector3<f64>>,
}

type ExplodedBlocks = FxHashMap<BlockPos, (&'static Block, &'static BlockState)>;

pub struct Explosion {
    power: f32,
    pos: Vector3<f64>,
    block_interaction: BlockInteraction,
    damage_calculator: Option<Arc<dyn ExplosionDamageCalculator>>,
    preserve_rails: bool,
    source_entity_id: Option<i32>,
    source_type: Option<&'static EntityType>,
    plays_sound: bool,
    fire: bool,
}

impl Explosion {
    #[must_use]
    pub const fn new(power: f32, pos: Vector3<f64>, block_interaction: BlockInteraction) -> Self {
        Self {
            power,
            pos,
            block_interaction,
            damage_calculator: None,
            preserve_rails: false,
            source_entity_id: None,
            source_type: None,
            plays_sound: true,
            fire: false,
        }
    }

    /// Sets the entity that caused the explosion, if there is one.
    #[must_use]
    pub fn with_source(mut self, source: Option<&Entity>) -> Self {
        self.source_entity_id = source.map(|entity| entity.entity_id);
        self.source_type = source.map(|entity| entity.entity_type);
        self.plays_sound = source.is_none_or(|entity| !entity.is_silent());
        self
    }

    /// Vanilla `ServerExplosion.fire`.
    #[must_use]
    pub const fn with_fire(mut self, fire: bool) -> Self {
        self.fire = fire;
        self
    }

    /// Vanilla `ServerExplosion.isSmall`.
    #[must_use]
    pub fn is_small(&self) -> bool {
        self.power < 2.0 || self.block_interaction == BlockInteraction::Keep
    }

    /// Vanilla `Level.explode`: no sound for a silent source.
    #[must_use]
    pub const fn plays_sound(&self) -> bool {
        self.plays_sound
    }

    fn source_is(&self, kind: &EntityType) -> bool {
        self.source_type.is_some_and(|source| source == kind)
    }

    /// Vanilla `ServerExplosion.canTriggerBlocks`, for an explosion that triggers blocks.
    fn can_trigger_blocks(&self, world: &World) -> bool {
        !self.source_is(&EntityType::BREEZE_WIND_CHARGE)
            || world.level_info.load().game_rules.mob_griefing
    }

    /// Vanilla `ServerExplosion.interactWithBlocks` for `TRIGGER_BLOCK`: nothing breaks, only
    /// the blocks overriding `onExplosionHit` react.
    fn trigger_blocks(&self, world: &Arc<World>, blocks: &ExplodedBlocks) {
        if !self.can_trigger_blocks(world) {
            return;
        }
        for (pos, (block, _state)) in blocks {
            if let Some(pumpkin_block) = world.block_registry.get_pumpkin_block(block.id) {
                pumpkin_block.on_explosion_trigger(ExplodeArgs {
                    world,
                    block,
                    position: pos,
                });
            }
        }
    }

    /// Vanilla `ServerExplosion.createFire`.
    fn create_fire<'a>(world: &Arc<World>, targets: impl Iterator<Item = &'a BlockPos>) {
        let mut random = rng();
        for pos in targets {
            if random.random_range(0..3) == 0
                && world.get_block_state(pos).is_air()
                && world.get_block_state(&pos.down()).is_solid_render()
            {
                let state = FireBlockBase::get_state(world, pos);
                world.set_block_state(pos, state, BlockFlags::NOTIFY_ALL);
            }
        }
    }

    #[must_use]
    pub fn with_damage_calculator(
        mut self,
        calculator: Arc<dyn ExplosionDamageCalculator>,
    ) -> Self {
        self.damage_calculator = Some(calculator);
        self
    }

    #[must_use]
    pub const fn preserving_rails(mut self) -> Self {
        self.preserve_rails = true;
        self
    }

    fn protects_rail(&self, world: &World, pos: &BlockPos, block: &Block) -> bool {
        self.preserve_rails && (Self::is_rail(block) || Self::is_rail(world.get_block(&pos.up())))
    }

    fn is_rail(block: &Block) -> bool {
        block.id == Block::RAIL.id
            || block.id == Block::POWERED_RAIL.id
            || block.id == Block::DETECTOR_RAIL.id
            || block.id == Block::ACTIVATOR_RAIL.id
    }

    #[allow(clippy::too_many_lines)]
    fn get_blocks_to_destroy(
        &self,
        world: &World,
        air_reached: &mut FxHashSet<BlockPos>,
    ) -> ExplodedBlocks {
        let mut map = FxHashMap::default();

        let mut chunk_cache: FxHashMap<
            pumpkin_util::math::vector2::Vector2<i32>,
            Option<Arc<ChunkData>>,
        > = FxHashMap::default();

        let default_calc = DefaultExplosionDamageCalculator;
        let calc: &dyn ExplosionDamageCalculator = match &self.damage_calculator {
            Some(c) => c.as_ref(),
            None => &default_calc,
        };

        for x in 0..16 {
            for y in 0..16 {
                for z in 0..16 {
                    if x > 0 && x < 15 && y > 0 && y < 15 && z > 0 && z < 15 {
                        continue;
                    }

                    let mut dir_x = f64::from(x) / 7.5 - 1.0;
                    let mut dir_y = f64::from(y) / 7.5 - 1.0;
                    let mut dir_z = f64::from(z) / 7.5 - 1.0;

                    let length = (dir_x * dir_x + dir_y * dir_y + dir_z * dir_z).sqrt();
                    dir_x /= length;
                    dir_y /= length;
                    dir_z /= length;

                    let mut pos_x = self.pos.x;
                    let mut pos_y = self.pos.y;
                    let mut pos_z = self.pos.z;

                    let random_val = rand::random::<f32>();
                    let mut h = self.power * random_val.mul_add(0.6, 0.7);

                    while h > 0.0 {
                        let block_pos = BlockPos::floored(pos_x, pos_y, pos_z);

                        if !world.is_in_build_limit(block_pos) {
                            break;
                        }

                        let (chunk_pos, relative) = block_pos.chunk_and_chunk_relative_position();

                        let chunk_opt = chunk_cache.entry(chunk_pos).or_insert_with(|| {
                            world
                                .level
                                .read_chunk_sync(&chunk_pos, std::clone::Clone::clone)
                        });

                        let state_id = if let Some(chunk) = chunk_opt {
                            chunk
                                .section
                                .get_block_absolute_y(
                                    relative.x as usize,
                                    block_pos.0.y,
                                    relative.z as usize,
                                )
                                .unwrap_or(Block::AIR.default_state.id)
                        } else {
                            Block::AIR.default_state.id
                        };

                        let (block, state) = BlockState::from_id_with_block(state_id);

                        let (_fluid, fluid_state) = Fluid::from_state_id(state_id).map_or_else(
                            || {
                                if block.is_waterlogged(state_id) {
                                    (&Fluid::FLOWING_WATER, &Fluid::FLOWING_WATER.states[0])
                                } else {
                                    (&Fluid::EMPTY, &Fluid::EMPTY.states[0])
                                }
                            },
                            |raw_fluid| {
                                let f = raw_fluid.to_flowing();
                                (f, &f.states[0])
                            },
                        );

                        if !state.is_air() || !fluid_state.is_empty {
                            let protects_rail = self.protects_rail(world, &block_pos, block);
                            let resistance = if protects_rail {
                                Some(0.0)
                            } else {
                                calc.get_block_explosion_resistance(
                                    self,
                                    world,
                                    &block_pos,
                                    block,
                                    fluid_state,
                                )
                            };

                            if let Some(resistance) = resistance {
                                h -= (resistance + 0.3) * 0.3;
                            }

                            if h > 0.0
                                && !protects_rail
                                && calc.should_block_explode(self, world, &block_pos, block, h)
                            {
                                map.insert(block_pos, (block, state));
                            }
                        } else if self.fire {
                            air_reached.insert(block_pos);
                        }

                        pos_x += dir_x * 0.3;
                        pos_y += dir_y * 0.3;
                        pos_z += dir_z * 0.3;
                        h -= 0.225_000_01;
                    }
                }
            }
        }
        map
    }

    /// The share of destroyed blocks that drop loot, as the explode events report it.
    fn default_yield_rate(&self) -> f32 {
        if self.block_interaction == BlockInteraction::DestroyWithDecay && self.power > 0.0 {
            1.0 / self.power
        } else {
            1.0
        }
    }

    /// Fires the explode event for this explosion. Returns the blocks and yield rate the
    /// plugins left, or `None` when a plugin cancelled the explosion.
    fn fire_explode_event(
        &self,
        server: &Arc<Server>,
        world: &Arc<World>,
        mut blocks: ExplodedBlocks,
        yield_rate: f32,
    ) -> Option<(ExplodedBlocks, f32)> {
        let (cancelled, returned_blocks, yield_rate) =
            if let Some(entity_id) = self.source_entity_id {
                if !server.plugin_manager.has_handlers::<EntityExplodeEvent>() {
                    return Some((blocks, yield_rate));
                }
                let mut event = EntityExplodeEvent::new(
                    world.clone(),
                    entity_id,
                    self.pos,
                    blocks.keys().copied().collect(),
                    yield_rate,
                );
                server.plugin_manager.fire_blocking(server, &mut event);
                (event.cancelled, event.blocks, event.yield_rate)
            } else {
                if !server.plugin_manager.has_handlers::<BlockExplodeEvent>() {
                    return Some((blocks, yield_rate));
                }
                let mut event = BlockExplodeEvent::new(
                    world.clone(),
                    BlockPos::floored(self.pos.x, self.pos.y, self.pos.z),
                    blocks.keys().copied().collect(),
                    yield_rate,
                );
                server.plugin_manager.fire_blocking(server, &mut event);
                (event.cancelled, event.blocks, event.yield_rate)
            };
        if cancelled {
            return None;
        }

        // Plugins may add positions the rays did not reach; those use the block there now.
        let blocks = returned_blocks
            .into_iter()
            .filter_map(|pos| {
                let entry = blocks.remove(&pos).or_else(|| {
                    let (block, state) = world.get_block_and_state(&pos);
                    (!state.is_air()).then_some((block, state))
                })?;
                Some((pos, entry))
            })
            .collect();
        Some((blocks, yield_rate))
    }

    /// Vanilla `BlockBehaviour.onExplosionHit` of an explosion that breaks blocks. Returns how
    /// many it broke.
    fn destroy_blocks(
        &self,
        world: &Arc<World>,
        blocks: &ExplodedBlocks,
        yield_rate: f32,
        default_yield_rate: f32,
    ) -> u32 {
        let decay_drops = self.block_interaction == BlockInteraction::DestroyWithDecay;
        let explosion_radius = if yield_rate == default_yield_rate {
            decay_drops.then_some(self.power)
        } else {
            // Loot survives an explosion with a chance of 1 / radius.
            (yield_rate < 1.0).then_some(1.0 / yield_rate)
        };

        for (pos, (block, state)) in blocks {
            let pumpkin_block = world.block_registry.get_pumpkin_block(block.id);

            if pumpkin_block.is_none_or(|s| s.should_drop_items_on_explosion()) {
                let is_raining = world.is_raining();
                let is_thundering = world.is_thundering();
                let params = LootContextParameters {
                    block_state: Some(state),
                    explosion_radius,
                    position: Some(pumpkin_util::math::vector3::Vector3::new(
                        pos.0.x as f64,
                        pos.0.y as f64,
                        pos.0.z as f64,
                    )),
                    world_time: world.level_info.load().day_time as u64,
                    is_raining: Some(is_raining),
                    is_thundering: Some(is_thundering),
                    ..Default::default()
                };
                drop_loot(world, block, pos, false, &params);
            }

            world.set_block_state(pos, BlockStateId::AIR, BlockFlags::NOTIFY_ALL);
            world.close_container_screens_at(pos);

            if let Some(pumpkin_block) = pumpkin_block {
                pumpkin_block.explode(ExplodeArgs {
                    world,
                    block,
                    position: pos,
                });
            }
        }
        blocks.len() as u32
    }

    /// Returns what happened, or `None` when a plugin cancelled the explosion.
    pub fn explode(&self, world: &Arc<World>) -> Option<ExplosionOutcome> {
        let mut air_reached = FxHashSet::default();
        let mut blocks = if self.block_interaction == BlockInteraction::Keep {
            ExplodedBlocks::default()
        } else {
            self.get_blocks_to_destroy(world, &mut air_reached)
        };
        let default_yield_rate = self.default_yield_rate();
        let mut yield_rate = default_yield_rate;
        if let Some(server) = world.server.upgrade() {
            (blocks, yield_rate) = self.fire_explode_event(&server, world, blocks, yield_rate)?;
        }

        let source = self
            .source_entity_id
            .and_then(|id| world.get_entity_by_id(id));
        world.emit_game_event_with_context(
            GameEvent::Explode.name(),
            self.pos,
            &GameEventContext {
                source_entity: source.as_deref(),
            },
        );
        let hit_players = self.damage_entities(world, source.as_ref());

        let block_count = match self.block_interaction {
            BlockInteraction::Keep => 0,
            BlockInteraction::TriggerBlock => {
                self.trigger_blocks(world, &blocks);
                0
            }
            BlockInteraction::Destroy | BlockInteraction::DestroyWithDecay => {
                self.destroy_blocks(world, &blocks, yield_rate, default_yield_rate)
            }
        };

        if self.fire {
            Self::create_fire(world, air_reached.iter().chain(blocks.keys()));
        }
        Some(ExplosionOutcome {
            block_count,
            hit_players,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::Explosion;
    use pumpkin_data::Block;

    #[test]
    fn tnt_minecart_rail_protection_covers_every_rail_type() {
        for rail in [
            &Block::RAIL,
            &Block::POWERED_RAIL,
            &Block::DETECTOR_RAIL,
            &Block::ACTIVATOR_RAIL,
        ] {
            assert!(Explosion::is_rail(rail));
        }
        assert!(!Explosion::is_rail(&Block::STONE));
    }
}
