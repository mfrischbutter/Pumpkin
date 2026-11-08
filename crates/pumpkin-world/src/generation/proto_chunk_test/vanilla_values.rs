//! Single values read from vanilla 26.3 that a past difference got wrong.

use pumpkin_data::Block;
use pumpkin_data::dimension::Dimension;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_util::world_seed::Seed;

use super::surface_biomes;
use crate::generation::generator::WorldGenerator;
use crate::generation::get_world_gen;
use crate::generation::proto_chunk::ProtoChunk;

const SEED: u64 = 1_657_324_430;

/// Generates the overworld chunk holding block (`x`, `z`) to the terrain stage (noise, surface,
/// carvers), the stage vanilla saves as `terrain`.
fn terrain_chunk(world_gen: &WorldGenerator, x: i32, z: i32) -> ProtoChunk {
    let WorldGenerator::Noise(generator) = world_gen else {
        unreachable!()
    };
    let (cx, cz) = (x >> 4, z >> 4);
    let mut chunk = ProtoChunk::new(cx, cz, world_gen);
    chunk.step_to_biomes(generator);
    chunk.set_structure_starts(generator);
    chunk.set_structure_references(generator);
    chunk.step_to_noise(generator);
    chunk.step_to_surface(generator, &surface_biomes(world_gen, cx, cz));
    chunk.step_to_carvers(generator);
    chunk
}

/// Vanilla 26.3's saved terrain-stage chunks of seed 1657324430: these blocks at aquifer borders
/// are stone or deepslate. Picking the nearest aquifers in another order on equal distance, or
/// weighing them in f32 instead of vanilla's double, put water or lava sources there.
#[test]
fn aquifer_borders_stay_solid() {
    let world_gen = get_world_gen(
        Seed(SEED),
        Dimension::OVERWORLD,
        false,
        Vec::new(),
        String::new(),
    );
    for (x, y, z, expected) in [
        (1883, 48, 3337, &Block::STONE),
        (-1315, 49, -4690, &Block::STONE),
        (-1125, -25, -499, &Block::DEEPSLATE),
        (351, 5, -780, &Block::DEEPSLATE),
    ] {
        let chunk = terrain_chunk(&world_gen, x, z);
        let block = Block::from_state_id(chunk.get_block_state(&Vector3::new(x, y, z)));
        assert_eq!(
            block.id, expected.id,
            "expected {} at ({x}, {y}, {z}), got {}",
            expected.name, block.name
        );
    }
}
