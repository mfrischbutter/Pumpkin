use std::sync::Arc;

use pumpkin_data::{BlockState, BlockStateId, Mirror, Rotation, fluid::Fluid};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::{
    HeightMap,
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
        BlockPlacer, StructureProcessor, TemplatePlacement, get_template,
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

        // `StructureTemplate.placeInWorld` draws the palette from the feature's random; the
        // templates of template features have one palette, the one placed below.
        if template.palettes().is_empty() {
            return false;
        }
        random.next_bounded_i32(template.palettes().len() as i32);
        let size = template.size;
        if (template.blocks.is_empty() && template.entities.is_empty())
            || size.x < 1
            || size.y < 1
            || size.z < 1
        {
            return false;
        }

        let (offset_x, offset_z) = rotation.rotate_offset(-(size.x / 2), -(size.z / 2));
        let position = Vector3::new(origin.0.x + offset_x, origin.0.y, origin.0.z + offset_z);
        // Vanilla turns the template around `position`, the placement below lays the turned
        // template out from its lowest corner.
        let corner = template
            .get_bounding_box_with_pivot(position, rotation, Vector3::new(0, 0, 0), Mirror::None)
            .min;
        // Without a bounding box the template reaches into the whole generation region. Vanilla
        // would also place its entities; no template of a template feature has any.
        place_template_with_options(
            &mut RegionPlacer(chunk),
            &template,
            corner,
            (0, 0),
            rotation,
            false,
            true,
            &self.processors,
            None,
            true,
            random,
            TemplatePlacement {
                position,
                level_seed: block_registry.feature_seed() as i64,
            },
        );
        // Vanilla then updates the shapes of the placed blocks and their neighbours
        // (`knownShape` is false), which the shared placement does not do.
        true
    }
}

/// The generation region as a template sees it while a feature places one, like vanilla's
/// `WorldGenRegion`: blocks set with their post-processing marks and block entities.
struct RegionPlacer<'a, T>(&'a mut T);

impl<T: GenerationCache> BlockPlacer for RegionPlacer<'_, T> {
    fn get_block_state(&self, pos: &Vector3<i32>) -> BlockStateId {
        GenerationCache::get_block_state(self.0, pos)
    }

    fn set_block_state(&mut self, pos: &Vector3<i32>, state: &BlockState) {
        GenerationCache::set_block_state(self.0, pos, state);
    }

    fn add_block_entity(&mut self, nbt: NbtCompound) {
        if let (Some(x), Some(y), Some(z)) = (nbt.get_int("x"), nbt.get_int("y"), nbt.get_int("z"))
        {
            GenerationCache::add_block_entity(self.0, &Vector3::new(x, y, z), nbt);
        }
    }

    fn get_height(&self, heightmap: HeightMap, x: i32, z: i32) -> Option<i32> {
        Some(self.0.get_top_y(&heightmap, x, z))
    }

    fn schedule_fluid_tick(&mut self, pos: &Vector3<i32>, fluid: &'static Fluid, delay: u8) {
        self.0.schedule_fluid_tick_at(pos, fluid, delay);
    }
}

#[cfg(test)]
mod tests {
    use pumpkin_data::configured_feature::ConfiguredFeature as FeatureKey;
    use pumpkin_util::random::legacy_rand::LegacyRand;

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
