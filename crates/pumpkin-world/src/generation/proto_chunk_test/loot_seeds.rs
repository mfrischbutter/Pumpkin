//! Loot table seeds of structure chests against vanilla 26.3.

use pumpkin_data::dimension::Dimension;

use super::stage_parity::decorate_chunk;

/// Vanilla 26.3, seed 1657324430, no generation secret: the buried treasure starting in
/// chunk (0, 22) puts its chest at (9, 61, 361) with this loot table seed.
#[test]
fn buried_treasure_loot_seed_matches_vanilla() {
    let (cx, cz) = (0, 22);
    let mut grid = decorate_chunk(1_657_324_430, Dimension::OVERWORLD, cx, cz, |_, _| {});
    let center = grid.chunks.get_mut(&(cx, cz)).unwrap();
    let block_entities = center.take_pending_block_entities();

    let chest = block_entities
        .iter()
        .find(|nbt| {
            nbt.get_int("x") == Some(9)
                && nbt.get_int("y") == Some(61)
                && nbt.get_int("z") == Some(361)
        })
        .unwrap_or_else(|| panic!("no block entity at (9, 61, 361): {block_entities:?}"));

    assert_eq!(
        chest.get_string("LootTable"),
        Some("minecraft:chests/buried_treasure")
    );
    assert_eq!(
        chest.get_long("LootTableSeed"),
        Some(-8_064_810_552_006_593_108)
    );
}
