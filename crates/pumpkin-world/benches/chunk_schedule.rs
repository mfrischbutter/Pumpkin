//! Benchmarks the chunk scheduler on the server's path: tickets on a `Level` until the wanted
//! chunks are loaded at full.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use criterion::{Criterion, criterion_group, criterion_main};
use pumpkin_config::world::LevelConfig;
use pumpkin_data::BlockStateId;
use pumpkin_data::dimension::Dimension;
use pumpkin_util::math::vector2::Vector2;
use pumpkin_world::chunk_system::ChunkLoading;
use pumpkin_world::generation::secret::GenerationSecret;
use pumpkin_world::level::Level;
use pumpkin_world::world::WorldPortalExt;
use std::sync::Arc;
use std::time::{Duration, Instant};

const SEED: i64 = 42;
const VIEW_DISTANCE: u8 = 8;
const FORCED_SIZE: i32 = 16;

// Stub portal — allows all block placements, skips mob spawning.
struct BlockRegistry;
impl WorldPortalExt for BlockRegistry {
    fn can_place_at(
        &self,
        _block: &pumpkin_data::Block,
        _state: &pumpkin_data::BlockState,
        _block_accessor: &dyn pumpkin_world::world::BlockAccessor,
        _block_pos: &pumpkin_util::math::position::BlockPos,
    ) -> bool {
        true
    }

    fn mirror(
        &self,
        block: &pumpkin_data::Block,
        state_id: BlockStateId,
        mirror: pumpkin_data::Mirror,
    ) -> &'static pumpkin_data::BlockState {
        block.mirror(state_id, mirror)
    }

    fn rotate(
        &self,
        block: &pumpkin_data::Block,
        state_id: BlockStateId,
        rotation: pumpkin_data::Rotation,
    ) -> &'static pumpkin_data::BlockState {
        block.rotate(state_id, rotation)
    }

    fn spawn_mobs_for_chunk_generation(
        &self,
        _cache: &mut dyn pumpkin_world::generation::proto_chunk::GenerationCache,
        _biome: &'static pumpkin_data::chunk::Biome,
        _chunk_x: i32,
        _chunk_z: i32,
    ) {
    }

    fn feature_seed(&self) -> u64 {
        0
    }
}

/// Times a fresh level from adding the tickets until every wanted chunk is loaded at full.
fn generate(
    rt: &tokio::runtime::Runtime,
    add_tickets: impl Fn(&mut ChunkLoading),
    wanted: &[Vector2<i32>],
) -> Duration {
    rt.block_on(async {
        let dir = tempfile::TempDir::new().unwrap();
        let level = Level::from_root_folder(
            &LevelConfig::default(),
            dir.path().to_path_buf(),
            SEED,
            GenerationSecret::NONE,
            Dimension::OVERWORLD,
        );
        level.world_portal.store(Arc::new(Some(
            Arc::new(BlockRegistry) as Arc<dyn WorldPortalExt>
        )));

        let start = Instant::now();
        {
            let mut chunk_loading = level.chunk_loading.lock().unwrap();
            add_tickets(&mut chunk_loading);
            chunk_loading.send_change();
        };
        while !wanted
            .iter()
            .all(|pos| level.loaded_chunks.contains_key(pos))
        {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let elapsed = start.elapsed();

        level.shutdown().await;
        elapsed
    })
}

fn bench_chunk_schedule(c: &mut Criterion) {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    let view = i32::from(VIEW_DISTANCE);
    let player_view: Vec<_> = (-view..=view)
        .flat_map(|x| (-view..=view).map(move |z| Vector2::new(x, z)))
        .collect();
    let forced_area: Vec<_> = (0..FORCED_SIZE)
        .flat_map(|x| (0..FORCED_SIZE).map(move |z| Vector2::new(x, z)))
        .collect();

    let mut group = c.benchmark_group("chunk_schedule");
    group.sample_size(10);
    group.measurement_time(Duration::from_secs(120));

    group.bench_function("player_view_8", |b| {
        b.iter_custom(|iters| {
            (0..iters)
                .map(|_| {
                    generate(
                        &rt,
                        |chunk_loading| {
                            chunk_loading.add_ticket(
                                Vector2::new(0, 0),
                                ChunkLoading::get_level_from_view_distance(VIEW_DISTANCE),
                            );
                        },
                        &player_view,
                    )
                })
                .sum()
        });
    });

    group.bench_function("forced_area_16", |b| {
        b.iter_custom(|iters| {
            (0..iters)
                .map(|_| {
                    generate(
                        &rt,
                        |chunk_loading| {
                            for pos in &forced_area {
                                chunk_loading.add_forced_chunk_ticket(*pos);
                            }
                        },
                        &forced_area,
                    )
                })
                .sum()
        });
    });

    group.finish();
}

criterion_group!(benches, bench_chunk_schedule);
criterion_main!(benches);
