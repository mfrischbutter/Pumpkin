use pumpkin_data::{Block, BlockDirection, BlockId, BlockState, Property};
use pumpkin_util::{
    DoublePerlinNoiseParametersCodec,
    math::{
        clamped_map,
        int_provider::IntProvider,
        pool::{Pool, Weighted},
        position::BlockPos,
    },
    random::{RandomGenerator, RandomImpl, legacy_rand::LegacyRand},
};

use super::noise::perlin::DoublePerlinNoiseSampler;
use crate::generation::block_predicate::BlockPredicate;
use crate::generation::proto_chunk::GenerationCache;
use crate::world::{BlockAccessor, WorldPortalExt};

pub enum BlockStateProvider {
    Simple(SimpleStateProvider),
    Weighted(WeightedBlockStateProvider),
    NoiseThreshold(NoiseThresholdBlockStateProvider),
    NoiseProvider(NoiseBlockStateProvider),
    DualNoise(DualNoiseBlockStateProvider),
    Rotated(RotatedBlockStateProvider),
    RandomBlock(RandomBlockStateProvider),
    RandomizedInt(RandomizedIntBlockStateProvider),
    Rule(RuleBasedBlockStateProvider),
}

impl BlockStateProvider {
    pub fn get_for_bonemeal(
        &self,
        random: &mut RandomGenerator,
        pos: BlockPos,
    ) -> Option<&'static BlockState> {
        match self {
            Self::NoiseThreshold(provider) => Some(provider.get(random, pos)),
            Self::NoiseProvider(provider) => Some(provider.get(pos)),
            Self::Simple(provider) => Some(provider.get(pos)),
            Self::Weighted(provider) => Some(provider.get(random)),
            Self::DualNoise(provider) => Some(provider.get(pos)),
            Self::Rotated(provider) => provider.get_for_bonemeal(random, pos),
            Self::RandomBlock(provider) => provider.get(random),
            Self::RandomizedInt(provider) => provider.get_for_bonemeal(random, pos),
            Self::Rule(_) => None,
        }
    }

    pub fn get_state_for_world(
        &self,
        world: &dyn BlockAccessor,
        random: &mut RandomGenerator,
        pos: BlockPos,
    ) -> &'static BlockState {
        match self {
            Self::Simple(provider) => provider.get(pos),
            Self::Weighted(provider) => provider.get(random),
            Self::NoiseThreshold(provider) => provider.get(random, pos),
            Self::NoiseProvider(provider) => provider.get(pos),
            Self::DualNoise(provider) => provider.get(pos),
            Self::Rotated(provider) => provider.get_state_for_world(world, random, pos),
            Self::RandomBlock(provider) => provider
                .get(random)
                .unwrap_or_else(|| world.get_block_state(&pos)),
            Self::RandomizedInt(provider) => provider.get_state_for_world(world, random, pos),
            Self::Rule(provider) => {
                for rule in &provider.rules {
                    if rule.if_true.test_world(world, None, &pos) {
                        return rule.then.get_state_for_world(world, random, pos);
                    }
                }
                provider.fallback.as_ref().map_or_else(
                    || world.get_block_state(&pos),
                    |f| f.get_state_for_world(world, random, pos),
                )
            }
        }
    }

    pub fn get<T: GenerationCache>(
        &self,
        random: &mut RandomGenerator,
        pos: BlockPos,
        chunk: &T,
        block_registry: &dyn WorldPortalExt,
    ) -> &'static BlockState {
        match self {
            Self::NoiseThreshold(provider) => provider.get(random, pos),
            Self::NoiseProvider(provider) => provider.get(pos),
            Self::Simple(provider) => provider.get(pos),
            Self::Weighted(provider) => provider.get(random),
            Self::DualNoise(provider) => provider.get(pos),
            Self::Rotated(provider) => provider.get(random, pos, chunk, block_registry),
            Self::RandomBlock(provider) => provider
                .get(random)
                .unwrap_or_else(|| GenerationCache::get_block_state(chunk, &pos.0).to_state()),
            Self::RandomizedInt(provider) => provider.get(random, pos, chunk, block_registry),
            Self::Rule(provider) => provider.get(block_registry, chunk, random, pos),
        }
    }

    pub fn get_with_context<T: GenerationCache>(
        &self,
        block_registry: &dyn WorldPortalExt,
        chunk: &T,
        random: &mut RandomGenerator,
        pos: BlockPos,
    ) -> &'static BlockState {
        match self {
            Self::Rule(provider) => provider.get(block_registry, chunk, random, pos),
            _ => self.get(random, pos, chunk, block_registry),
        }
    }

    pub fn get_optional<T: GenerationCache>(
        &self,
        block_registry: &dyn WorldPortalExt,
        chunk: &T,
        random: &mut RandomGenerator,
        pos: BlockPos,
    ) -> Option<&'static BlockState> {
        match self {
            Self::Rule(provider) => provider.get_optional(block_registry, chunk, random, pos),
            Self::RandomBlock(provider) => provider.get(random),
            _ => Some(self.get(random, pos, chunk, block_registry)),
        }
    }
}

pub struct RuleBasedBlockStateProvider {
    pub fallback: Option<Box<BlockStateProvider>>,
    pub rules: Vec<BlockStateRule>,
}

impl RuleBasedBlockStateProvider {
    pub fn get<T: GenerationCache>(
        &self,
        block_registry: &dyn WorldPortalExt,
        chunk: &T,
        random: &mut RandomGenerator,
        pos: BlockPos,
    ) -> &'static BlockState {
        if let Some(optional) = self.get_optional(block_registry, chunk, random, pos) {
            return optional;
        }
        GenerationCache::get_block_state(chunk, &pos.0).to_state()
    }
    pub fn get_optional<T: GenerationCache>(
        &self,
        block_registry: &dyn WorldPortalExt,
        chunk: &T,
        random: &mut RandomGenerator,
        pos: BlockPos,
    ) -> Option<&'static BlockState> {
        for rule in &self.rules {
            if rule.if_true.test(block_registry, chunk, &pos)
                && let Some(state) = rule.then.get_optional(block_registry, chunk, random, pos)
            {
                return Some(state);
            }
        }
        self.fallback
            .as_ref()
            .and_then(|f| f.get_optional(block_registry, chunk, random, pos))
    }
}

pub struct BlockStateRule {
    pub if_true: BlockPredicate,
    pub then: BlockStateProvider,
}

pub struct RandomizedIntBlockStateProvider {
    pub source: Box<BlockStateProvider>,
    pub property: pumpkin_data::PropertyKey,
    pub values: IntProvider,
}

impl RandomizedIntBlockStateProvider {
    fn apply_property(
        &self,
        base_state: &'static BlockState,
        random: &mut RandomGenerator,
    ) -> &'static BlockState {
        let block = Block::from_id(base_state.id.to_block_id());
        let Some(props_source) = block.properties(base_state.id) else {
            return base_state;
        };
        let mut prop_list = props_source.to_props();
        let Some(target_prop) = prop_list.iter_mut().find(|p| p.key() == self.property) else {
            return base_state;
        };
        let value = self.values.get(random);
        if let Ok(val_u8) = u8::try_from(value)
            && let Some(new_prop) = pumpkin_data::Property::from_key_and_int(self.property, val_u8)
        {
            *target_prop = new_prop;
        }
        let new_state_id = block.from_properties(&prop_list).to_state_id(block);
        BlockState::from_id(new_state_id)
    }

    pub fn get_for_bonemeal(
        &self,
        random: &mut RandomGenerator,
        pos: BlockPos,
    ) -> Option<&'static BlockState> {
        self.source
            .get_for_bonemeal(random, pos)
            .map(|state| self.apply_property(state, random))
    }

    pub fn get_state_for_world(
        &self,
        world: &dyn BlockAccessor,
        random: &mut RandomGenerator,
        pos: BlockPos,
    ) -> &'static BlockState {
        let state = self.source.get_state_for_world(world, random, pos);
        self.apply_property(state, random)
    }

    pub fn get<T: GenerationCache>(
        &self,
        random: &mut RandomGenerator,
        pos: BlockPos,
        chunk: &T,
        block_registry: &dyn WorldPortalExt,
    ) -> &'static BlockState {
        let state = self.source.get(random, pos, chunk, block_registry);
        self.apply_property(state, random)
    }
}

pub struct RotatedBlockStateProvider {
    pub state: Box<BlockStateProvider>,
    pub direction: Option<BlockDirection>,
}

impl RotatedBlockStateProvider {
    /// Returns the given direction or a random one, drawn before the inner provider like vanilla.
    fn direction(&self, random: &mut RandomGenerator) -> BlockDirection {
        self.direction
            .unwrap_or_else(|| BlockDirection::random(random))
    }

    /// Sets the axis and the facing like vanilla's `trySetValue` calls.
    fn rotate(state: &'static BlockState, direction: BlockDirection) -> &'static BlockState {
        let block = Block::from_id(state.id.to_block_id());
        let Some(properties) = block.properties(state.id) else {
            return state;
        };
        let mut props = properties.to_props();
        if !props
            .iter()
            .any(|p| matches!(p, Property::Axis(_) | Property::Facing(_)))
        {
            return state;
        }
        // `from_props` applies the entries in order and skips a value the block's property does
        // not have, so a horizontal facing keeps its value for up and down.
        props.extend([
            Property::Axis(direction.to_axis()),
            Property::Facing(direction.to_facing()),
        ]);
        BlockState::from_id(block.from_properties(&props).to_state_id(block))
    }

    pub fn get_for_bonemeal(
        &self,
        random: &mut RandomGenerator,
        pos: BlockPos,
    ) -> Option<&'static BlockState> {
        let direction = self.direction(random);
        self.state
            .get_for_bonemeal(random, pos)
            .map(|state| Self::rotate(state, direction))
    }

    pub fn get_state_for_world(
        &self,
        world: &dyn BlockAccessor,
        random: &mut RandomGenerator,
        pos: BlockPos,
    ) -> &'static BlockState {
        let direction = self.direction(random);
        let state = self.state.get_state_for_world(world, random, pos);
        Self::rotate(state, direction)
    }

    pub fn get<T: GenerationCache>(
        &self,
        random: &mut RandomGenerator,
        pos: BlockPos,
        chunk: &T,
        block_registry: &dyn WorldPortalExt,
    ) -> &'static BlockState {
        let direction = self.direction(random);
        let state = self.state.get(random, pos, chunk, block_registry);
        Self::rotate(state, direction)
    }
}

pub struct RandomBlockStateProvider {
    pub blocks: &'static [u16],
}

impl RandomBlockStateProvider {
    /// Picks a block's default state like vanilla's `HolderSet.getRandomElement`, which draws
    /// nothing from an empty set.
    pub fn get(&self, random: &mut RandomGenerator) -> Option<&'static BlockState> {
        if self.blocks.is_empty() {
            return None;
        }
        let index = random.next_bounded_i32(self.blocks.len() as i32) as usize;
        self.blocks
            .get(index)
            .and_then(|&id| BlockId::new(id))
            .map(|id| id.to_block().default_state)
    }
}

pub struct DualNoiseBlockStateProvider {
    pub base: NoiseBlockStateProvider,
    pub variety: [u32; 2],
    pub slow_noise: DoublePerlinNoiseParametersCodec,
    pub slow_scale: f64,
}

impl DualNoiseBlockStateProvider {
    #[must_use]
    pub fn get(&self, pos: BlockPos) -> &'static BlockState {
        let sampler = DoublePerlinNoiseSampler::new(
            &mut RandomGenerator::Legacy(LegacyRand::from_seed(self.base.base.seed as u64)),
            self.slow_noise.first_octave,
            &self.slow_noise.amplitudes,
            false,
        );
        let slow_noise =
            self.get_slow_noise(pos.0.x as f64, pos.0.y as f64, pos.0.z as f64, &sampler);
        let mapped = clamped_map(
            slow_noise,
            -1.0,
            1.0,
            self.variety[0] as f64,
            self.variety[1] as f64 + 1.0,
        ) as i32;
        let mut list = Vec::with_capacity(mapped as usize);
        for i in 0..mapped {
            let value = self.get_slow_noise(
                (pos.0.x + i * 54545) as f64,
                pos.0.y as f64,
                (pos.0.z + i * 34234) as f64,
                &sampler,
            );
            list.push(NoiseBlockStateProvider::get_state_by_value(
                &self.base.states,
                value,
            ));
        }
        let value = self.base.base.get_noise(pos);
        NoiseBlockStateProvider::get_state_by_value(&list, value)
    }

    fn get_slow_noise(&self, x: f64, y: f64, z: f64, sampler: &DoublePerlinNoiseSampler) -> f64 {
        f64::from(sampler.sample(
            x * self.slow_scale,
            y * self.slow_scale,
            z * self.slow_scale,
        ))
    }
}

pub struct WeightedBlockStateProvider {
    pub entries: Vec<Weighted<&'static BlockState>>,
}

impl WeightedBlockStateProvider {
    pub fn get(&self, random: &mut RandomGenerator) -> &'static BlockState {
        Pool::get(&self.entries, random)
            .copied()
            .unwrap_or(Block::AIR.default_state)
    }
}

pub struct SimpleStateProvider {
    pub state: &'static BlockState,
}

impl SimpleStateProvider {
    #[must_use]
    pub const fn get(&self, _pos: BlockPos) -> &'static BlockState {
        self.state
    }
}

pub struct NoiseBlockStateProviderBase {
    pub seed: i64,
    pub noise: DoublePerlinNoiseParametersCodec,
    pub scale: f32,
}

impl NoiseBlockStateProviderBase {
    #[must_use]
    pub fn get_noise(&self, pos: BlockPos) -> f64 {
        let sampler = DoublePerlinNoiseSampler::new(
            &mut RandomGenerator::Legacy(LegacyRand::from_seed(self.seed as u64)),
            self.noise.first_octave,
            &self.noise.amplitudes,
            false,
        );
        f64::from(sampler.sample(
            pos.0.x as f64 * f64::from(self.scale),
            pos.0.y as f64 * f64::from(self.scale),
            pos.0.z as f64 * f64::from(self.scale),
        ))
    }
}

pub struct NoiseBlockStateProvider {
    pub base: NoiseBlockStateProviderBase,
    pub states: Vec<&'static BlockState>,
}

impl NoiseBlockStateProvider {
    #[must_use]
    pub fn get(&self, pos: BlockPos) -> &'static BlockState {
        let value = self.base.get_noise(pos);
        Self::get_state_by_value(&self.states, value)
    }

    fn get_state_by_value(states: &[&'static BlockState], value: f64) -> &'static BlockState {
        let val = f64::midpoint(1.0, value).clamp(0.0, 0.9999);
        states[(val * states.len() as f64) as usize]
    }
}

pub struct NoiseThresholdBlockStateProvider {
    pub base: NoiseBlockStateProviderBase,
    pub threshold: f32,
    pub high_chance: f32,
    pub default_state: &'static BlockState,
    pub low_states: Vec<&'static BlockState>,
    pub high_states: Vec<&'static BlockState>,
}

impl NoiseThresholdBlockStateProvider {
    pub fn get(&self, random: &mut RandomGenerator, pos: BlockPos) -> &'static BlockState {
        let value = self.base.get_noise(pos);
        if value < self.threshold as f64 {
            return self.low_states[random.next_bounded_i32(self.low_states.len() as i32) as usize];
        }
        if random.next_f32() < self.high_chance {
            return self.high_states
                [random.next_bounded_i32(self.high_states.len() as i32) as usize];
        }
        self.default_state
    }
}
