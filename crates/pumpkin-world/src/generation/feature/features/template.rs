use std::sync::Arc;

use pumpkin_data::Rotation;
use pumpkin_util::{
    math::{
        pool::{Pool, Weighted},
        position::BlockPos,
        vector3::Vector3,
    },
    random::{RandomGenerator, RandomImpl},
};

use crate::generation::{
    proto_chunk::GenerationCache,
    structure::template::{
        RegionPlacer, StructurePlaceSettings, StructureProcessor, TemplatePlacement, get_template,
        place_template_with_options,
    },
};
use crate::world::WorldPortalExt;

/// Vanilla's `TemplateFeature`: one of several structure templates, turned at random and placed
/// with its middle on the origin.
pub struct TemplateFeature {
    pub templates: Vec<Weighted<TemplateEntry>>,
    pub processors: Arc<[StructureProcessor]>,
}

/// A template and the ways the feature may turn it.
pub struct TemplateEntry {
    pub template: &'static str,
    pub rotations: &'static [Rotation],
}

impl TemplateFeature {
    pub fn generate<T: GenerationCache>(
        &self,
        chunk: &mut T,
        block_registry: &dyn WorldPortalExt,
        random: &mut RandomGenerator,
        origin: BlockPos,
    ) -> bool {
        let Some(entry) = Pool::get(&self.templates, random) else {
            return false;
        };
        let rotation_index = random.next_bounded_i32(entry.rotations.len() as i32) as usize;
        let Some(&rotation) = entry.rotations.get(rotation_index) else {
            return false;
        };
        let Some(template) = get_template(entry.template) else {
            return false;
        };
        if template.palettes.is_empty() {
            return false;
        }

        let (offset_x, offset_z) =
            rotation.rotate_offset(-(template.size.x / 2), -(template.size.z / 2));
        let position = Vector3::new(origin.0.x + offset_x, origin.0.y, origin.0.z + offset_z);
        // Vanilla places with `setRandom(random)`, so the palette comes from the feature's random.
        // Its processors are rule processors, which seed from the block's position in vanilla too.
        let palette_index = random.next_bounded_i32(template.palettes.len() as i32) as usize;
        let settings = StructurePlaceSettings {
            rotation,
            apply_waterlogging: true,
            processors: self.processors.clone(),
            palette_index: Some(palette_index),
            ..StructurePlaceSettings::default()
        };
        // Without a bounding box the template reaches into the whole generation region.
        place_template_with_options(
            &mut RegionPlacer(chunk),
            &template,
            position,
            &settings,
            random,
            TemplatePlacement {
                reference: position,
                level_seed: block_registry.feature_seed() as i64,
                skip_air: false,
                keep_jigsaws: true,
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use pumpkin_data::configured_feature::ConfiguredFeature as FeatureKey;
    use pumpkin_util::random::legacy_rand::LegacyRand;

    use pumpkin_data::Mirror;

    use super::*;
    use crate::generation::feature::configured_features::{CONFIGURED_FEATURES, ConfiguredFeature};
    use crate::generation::feature::features::sculk::test_utils::empty_region;
    use crate::generation::feature::placed_features::Feature;
    use crate::generation::proto_chunk_test::stage_parity::TestBlockRegistry;
    use crate::generation::structure::template::{BlockStateResolver, StructureTemplate};

    static ROTATIONS: [Rotation; 4] = Rotation::values();

    #[test]
    fn templates_turn_around_their_middle_like_vanilla() {
        // 12 by 11 blocks, so the halves of its width and depth differ.
        const TEMPLATE: &str = "minecraft:spring/sulfur_spring_medium_2";
        let template = get_template(TEMPLATE);
        assert!(template.is_some(), "{TEMPLATE} is embedded");
        let Some(template) = template else {
            return;
        };
        let origin = BlockPos::new(24, 64, 24);

        for rotation in &ROTATIONS {
            let feature = TemplateFeature {
                templates: vec![Weighted {
                    data: TemplateEntry {
                        template: TEMPLATE,
                        rotations: std::slice::from_ref(rotation),
                    },
                    weight: 1,
                }],
                processors: Arc::from([]),
            };
            let mut level = empty_region();
            let mut random = RandomGenerator::Legacy(LegacyRand::from_seed(7));
            assert!(feature.generate(&mut level, &TestBlockRegistry, &mut random, origin));

            // Vanilla's `getRotatedOffset`: the turned west and north by half the width and the
            // depth.
            let (half_x, half_z) = (template.size.x / 2, template.size.z / 2);
            let (offset_x, offset_z) = match rotation {
                Rotation::None => (-half_x, -half_z),
                Rotation::Clockwise90 => (half_z, -half_x),
                Rotation::Rotate180 => (half_x, half_z),
                Rotation::CounterClockwise90 => (-half_z, half_x),
            };
            let position = origin.0 + Vector3::new(offset_x, 0, offset_z);
            let mut placed = 0;
            for block in &template.blocks {
                let entry = &template.palette[block.state as usize];
                let expected = BlockStateResolver::resolve(entry, *rotation, Mirror::None);
                assert!(expected.is_some(), "{} resolves", entry.name);
                let Some(expected) = expected else {
                    return;
                };
                let pos = position
                    + StructureTemplate::transform_block_pos(
                        block.pos,
                        Mirror::None,
                        *rotation,
                        Vector3::new(0, 0, 0),
                    );
                assert_eq!(
                    GenerationCache::get_block_state(&level, &pos),
                    expected.id,
                    "{rotation:?} at {pos:?}"
                );
                if !expected.is_air() {
                    placed += 1;
                }
            }
            let mut found = 0;
            for x in 0..48 {
                for y in 60..80 {
                    for z in 0..48 {
                        let state =
                            GenerationCache::get_block_state(&level, &Vector3::new(x, y, z));
                        if !state.to_state().is_air() {
                            found += 1;
                        }
                    }
                }
            }
            assert_eq!(found, placed, "{rotation:?} placed nothing else");

            // Vanilla draws the template, its rotation and its palette.
            let mut expected_random = RandomGenerator::Legacy(LegacyRand::from_seed(7));
            for _ in 0..3 {
                expected_random.next_bounded_i32(1);
            }
            assert_eq!(random.next_i64(), expected_random.next_i64());
        }
    }

    #[test]
    fn desert_wells_hide_suspicious_sand_with_loot() {
        // One suspicious sand a block below the well's middle and one two blocks below, each on
        // one of five spots and with the well's archaeology loot.
        let well = CONFIGURED_FEATURES.get(&FeatureKey::DesertWell);
        assert!(well.is_some(), "the desert well is configured");
        let Some(well) = well else {
            return;
        };
        let origin = BlockPos::new(24, 64, 24);
        let mut level = empty_region();
        let mut random = RandomGenerator::Legacy(LegacyRand::from_seed(3));
        assert!(well.generate(
            &mut level,
            &TestBlockRegistry,
            -64,
            384,
            pumpkin_data::placed_feature::PlacedFeature::DesertWell,
            &mut random,
            origin,
        ));

        let mut sand = Vec::new();
        for x in 16..32 {
            for y in 56..72 {
                for z in 16..32 {
                    let pos = Vector3::new(x, y, z);
                    if GenerationCache::get_block_state(&level, &pos).to_block_id()
                        == pumpkin_data::BlockId::SUSPICIOUS_SAND
                    {
                        sand.push(pos - origin.0);
                    }
                }
            }
        }
        sand.sort_by_key(|offset| offset.y);
        assert_eq!(sand.len(), 2, "{sand:?}");
        for (offset, y) in sand.iter().zip([-2, -1]) {
            assert_eq!(offset.y, y, "{sand:?}");
            assert!(offset.x.abs() + offset.z.abs() <= 1, "{sand:?}");
        }

        let mut block_entities = Vec::new();
        for chunk in &mut level.chunks {
            if let crate::chunk_system::Chunk::Proto(chunk) = chunk {
                block_entities.extend(chunk.take_pending_block_entities());
            }
        }
        for offset in &sand {
            let pos = origin.0 + *offset;
            let loot_table = block_entities
                .iter()
                .rev()
                .find(|nbt| {
                    nbt.get_int("x") == Some(pos.x)
                        && nbt.get_int("y") == Some(pos.y)
                        && nbt.get_int("z") == Some(pos.z)
                })
                .and_then(|nbt| nbt.get_string("LootTable"));
            assert_eq!(loot_table, Some("minecraft:archaeology/desert_well"));
        }
    }

    #[test]
    fn written_out_processor_lists_are_read() {
        // The desert well gives its suspicious sand a loot table through such a list.
        let well = CONFIGURED_FEATURES.get(&FeatureKey::DesertWell);
        assert!(matches!(well, Some(ConfiguredFeature::Overlay(_))));
        let Some(ConfiguredFeature::Overlay(well)) = well else {
            return;
        };
        let processors: Vec<usize> = well
            .features
            .iter()
            .filter_map(|feature| match &feature.get()?.feature {
                Feature::Inlined(feature) => match feature.as_ref() {
                    ConfiguredFeature::Template(template) => Some(template.processors.len()),
                    _ => None,
                },
                Feature::Named(_) => None,
            })
            .collect();
        assert_eq!(processors, [0, 1, 1]);
    }
}
