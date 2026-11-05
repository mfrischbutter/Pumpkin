use std::{
    collections::{HashMap, HashSet},
    sync::LazyLock,
};

use pumpkin_util::{math::position::BlockPos, random::RandomGenerator};

use super::features::{
    bamboo::BambooFeature,
    basalt_columns::BasaltColumnsFeature,
    basalt_pillar::BasaltPillarFeature,
    block_column::BlockColumnFeature,
    block_pile::BlockPileFeature,
    blue_ice::BlueIceFeature,
    bonus_chest::BonusChestFeature,
    chorus_plant::ChorusPlantFeature,
    coral::{
        coral_claw::CoralClawFeature, coral_mushroom::CoralMushroomFeature,
        coral_tree::CoralTreeFeature,
    },
    delta_feature::DeltaFeatureFeature,
    desert_well::DesertWellFeature,
    disk::DiskFeature,
    drip_stone::{
        cluster::SpeleothemClusterFeature, large::LargeDripstoneFeature, small::SpeleothemFeature,
    },
    end_gateway::EndGatewayFeature,
    end_island::EndIslandFeature,
    end_platform::EndPlatformFeature,
    end_podium::EndPodiumFeature,
    end_spike::EndSpikeFeature,
    fallen_tree::FallenTreeFeature,
    fill_layer::FillLayerFeature,
    forest_rock::ForestRockFeature,
    fossil::FossilFeature,
    freeze_top_layer::FreezeTopLayerFeature,
    geode::GeodeFeature,
    glowstone_blob::GlowstoneBlobFeature,
    huge_brown_mushroom::HugeBrownMushroomFeature,
    huge_fungus::HugeFungusFeature,
    huge_red_mushroom::HugeRedMushroomFeature,
    ice_spike::IceSpikeFeature,
    iceberg::IcebergFeature,
    kelp::KelpFeature,
    lake::LakeFeature,
    monster_room::DungeonFeature,
    multiface_growth::MultifaceGrowthFeature,
    nether_forest_vegetation::NetherForestVegetationFeature,
    netherrack_replace_blobs::ReplaceBlobsFeature,
    ore::OreFeature,
    overlay::OverlayFeature,
    projected_random_patchy_square::ProjectedRandomPatchySquareFeature,
    random_boolean_selector::RandomBooleanFeature,
    random_neighbor_spread::RandomNeighborSpreadFeature,
    random_patch::RandomPatchFeature,
    random_selector::RandomFeature,
    replace_single_block::ReplaceSingleBlockFeature,
    root_system::RootSystemFeature,
    scattered_ore::ScatteredOreFeature,
    sculk_patch::SculkPatchFeature,
    sea_pickle::SeaPickleFeature,
    seagrass::SeagrassFeature,
    sequence::SequenceFeature,
    simple_block::SimpleBlockFeature,
    simple_random_selector::SimpleRandomFeature,
    single_block_pillar::SingleBlockPillarFeature,
    spring_feature::SpringFeatureFeature,
    stepped_column_cluster::SteppedColumnClusterFeature,
    template::TemplateFeature,
    tree::TreeFeature,
    twisting_vines::TwistingVinesFeature,
    underwater_magma::UnderwaterMagmaFeature,
    vegetation_patch,
    vegetation_patch::VegetationPatchFeature,
    vines::VinesFeature,
    void_start_platform::VoidStartPlatformFeature,
    waterlogged_vegetation_patch,
    waterlogged_vegetation_patch::WaterloggedVegetationPatchFeature,
    weeping_vines::WeepingVinesFeature,
    weighted_random_selector::WeightedRandomFeature,
};
use crate::generation::proto_chunk::GenerationCache;
use crate::world::WorldPortalExt;

pub static CONFIGURED_FEATURES: LazyLock<
    HashMap<pumpkin_data::configured_feature::ConfiguredFeature, ConfiguredFeature>,
> = LazyLock::new(build_configured_features);

pub static BONE_MEAL_FEATURES: LazyLock<
    HashSet<pumpkin_data::configured_feature::ConfiguredFeature>,
> = LazyLock::new(|| {
    pumpkin_data::tag::get_tag_values(
        pumpkin_data::tag::RegistryKey::WorldgenConfiguredFeature,
        "minecraft:can_spawn_from_bone_meal",
    )
    .into_iter()
    .flatten()
    .filter_map(|name| pumpkin_data::configured_feature::ConfiguredFeature::from_name(name))
    .collect()
});

pub enum ConfiguredFeature {
    NoOp,
    Tree(Box<TreeFeature>),
    FallenTree(FallenTreeFeature),
    Flower(RandomPatchFeature),
    NoBonemealFlower(RandomPatchFeature),
    RandomPatch(RandomPatchFeature),
    BlockPile(BlockPileFeature),
    SpringFeature(SpringFeatureFeature),
    ChorusPlant(ChorusPlantFeature),
    ReplaceSingleBlock(ReplaceSingleBlockFeature),
    VoidStartPlatform(VoidStartPlatformFeature),
    DesertWell(DesertWellFeature),
    Fossil(FossilFeature),
    HugeRedMushroom(HugeRedMushroomFeature),
    HugeBrownMushroom(HugeBrownMushroomFeature),
    IceSpike(IceSpikeFeature),
    GlowstoneBlob(GlowstoneBlobFeature),
    FreezeTopLayer(FreezeTopLayerFeature),
    Vines(VinesFeature),
    BlockColumn(BlockColumnFeature),
    VegetationPatch(VegetationPatchFeature),
    WaterloggedVegetationPatch(WaterloggedVegetationPatchFeature),
    RootSystem(RootSystemFeature),
    MultifaceGrowth(MultifaceGrowthFeature),
    UnderwaterMagma(UnderwaterMagmaFeature),
    MonsterRoom(DungeonFeature),
    BlueIce(BlueIceFeature),
    Iceberg(IcebergFeature),
    ForestRock(ForestRockFeature),
    Disk(DiskFeature),
    Lake(LakeFeature),
    Ore(OreFeature),
    EndPlatform(EndPlatformFeature),
    EndPodium(EndPodiumFeature),
    EndSpike(EndSpikeFeature),
    EndIsland(EndIslandFeature),
    EndGateway(EndGatewayFeature),
    Seagrass(SeagrassFeature),
    Kelp(KelpFeature),
    CoralTree(CoralTreeFeature),
    CoralMushroom(CoralMushroomFeature),
    CoralClaw(CoralClawFeature),
    SeaPickle(SeaPickleFeature),
    SimpleBlock(SimpleBlockFeature),
    Bamboo(BambooFeature),
    HugeFungus(HugeFungusFeature),
    NetherForestVegetation(NetherForestVegetationFeature),
    WeepingVines(WeepingVinesFeature),
    TwistingVines(TwistingVinesFeature),
    BasaltColumns(BasaltColumnsFeature),
    DeltaFeature(DeltaFeatureFeature),
    NetherrackReplaceBlobs(ReplaceBlobsFeature),
    FillLayer(FillLayerFeature),
    BonusChest(BonusChestFeature),
    BasaltPillar(BasaltPillarFeature),
    ScatteredOre(ScatteredOreFeature),
    RandomSelector(RandomFeature),
    SimpleRandomSelector(SimpleRandomFeature),
    WeightedRandomSelector(WeightedRandomFeature),
    RandomBooleanSelector(RandomBooleanFeature),
    Geode(Box<GeodeFeature>),
    SpeleothemCluster(SpeleothemClusterFeature),
    LargeDripstone(LargeDripstoneFeature),
    Speleothem(SpeleothemFeature),
    SculkPatch(SculkPatchFeature),
    Overlay(OverlayFeature),
    Sequence(SequenceFeature),
    ProjectedRandomPatchySquare(ProjectedRandomPatchySquareFeature),
    RandomNeighborSpread(RandomNeighborSpreadFeature),
    SingleBlockPillar(SingleBlockPillarFeature),
    SteppedColumnCluster(SteppedColumnClusterFeature),
    Template(TemplateFeature),
}

// Yes this may look ugly and you wonder why this is hard coded, but it makes sense to hardcode since we have to add logic for these in code

impl ConfiguredFeature {
    #[expect(clippy::too_many_arguments)]
    #[expect(clippy::too_many_lines)]
    pub fn generate<T: GenerationCache>(
        &self,
        chunk: &mut T,
        block_registry: &dyn WorldPortalExt,
        min_y: i8,
        height: u16,
        feature_name: pumpkin_data::placed_feature::PlacedFeature, // This placed feature
        random: &mut RandomGenerator,
        pos: BlockPos,
    ) -> bool {
        match self {
            Self::NetherrackReplaceBlobs(feature) => feature.generate(
                chunk,
                block_registry,
                min_y,
                height,
                feature_name,
                random,
                pos,
            ),
            Self::NetherForestVegetation(feature) => feature.generate(
                chunk,
                block_registry,
                min_y,
                height,
                feature_name,
                random,
                pos,
            ),
            Self::VegetationPatch(feature) => feature.generate(
                chunk,
                block_registry,
                min_y,
                height,
                feature_name,
                random,
                pos,
            ),
            Self::WaterloggedVegetationPatch(feature) => feature.generate(
                chunk,
                block_registry,
                min_y,
                height,
                feature_name,
                random,
                pos,
            ),
            Self::Speleothem(feature) => feature.generate(chunk, random, pos),
            Self::CoralMushroom(_feature) => CoralMushroomFeature::generate(
                chunk,
                block_registry,
                min_y,
                height,
                feature_name,
                random,
                pos,
            ),
            Self::CoralTree(feature) => feature.generate(
                chunk,
                block_registry,
                min_y,
                height,
                feature_name,
                random,
                pos,
            ),
            Self::CoralClaw(feature) => feature.generate(
                chunk,
                block_registry,
                min_y,
                height,
                feature_name,
                random,
                pos,
            ),
            Self::EndPlatform(_feature) => EndPlatformFeature::generate(
                chunk,
                block_registry,
                min_y,
                height,
                feature_name,
                random,
                pos,
            ),
            Self::EndPodium(feature) => feature.generate(chunk, pos),
            Self::EndSpike(feature) => feature.generate(
                chunk,
                block_registry,
                min_y,
                height,
                feature_name,
                random,
                pos,
            ),
            Self::SpringFeature(feature) => feature.generate(block_registry, chunk, random, pos),
            Self::SimpleBlock(feature) => feature.generate(block_registry, chunk, random, pos),
            Self::RandomPatch(feature)
            | Self::Flower(feature)
            | Self::NoBonemealFlower(feature) => feature.generate(
                chunk,
                block_registry,
                min_y,
                height,
                feature_name,
                random,
                pos,
            ),
            Self::DesertWell(_feature) => {
                DesertWellFeature::generate(chunk, min_y, height, feature_name, random, pos)
            }
            Self::Fossil(feature) => feature.generate(chunk, min_y, height, random, pos),
            Self::Bamboo(feature) => feature.generate(
                chunk,
                block_registry,
                min_y,
                height,
                feature_name,
                random,
                pos,
            ),
            Self::BlockColumn(feature) => feature.generate(
                chunk,
                block_registry,
                min_y,
                height,
                feature_name,
                random,
                pos,
            ),
            Self::RandomBooleanSelector(feature) => feature.generate(
                chunk,
                block_registry,
                min_y,
                height,
                feature_name,
                random,
                pos,
            ),
            Self::Tree(feature) => feature.generate(block_registry, chunk, random, pos),
            Self::RandomSelector(feature) => feature.generate(
                chunk,
                block_registry,
                min_y,
                height,
                feature_name,
                random,
                pos,
            ),
            Self::SimpleRandomSelector(feature) => feature.generate(
                chunk,
                block_registry,
                min_y,
                height,
                feature_name,
                random,
                pos,
            ),
            Self::WeightedRandomSelector(feature) => feature.generate(
                chunk,
                block_registry,
                min_y,
                height,
                feature_name,
                random,
                pos,
            ),
            Self::Vines(_feature) => VinesFeature::generate(
                chunk,
                block_registry,
                min_y,
                height,
                feature_name,
                random,
                pos,
            ),
            Self::Seagrass(feature) => {
                feature.generate(chunk, min_y, height, feature_name, random, pos)
            }
            Self::TwistingVines(feature) => {
                feature.generate(chunk, min_y, height, feature_name, random, pos)
            }
            Self::UnderwaterMagma(feature) => {
                feature.generate(chunk, min_y, height, feature_name, random, pos)
            }
            Self::SeaPickle(feature) => {
                feature.generate(chunk, min_y, height, feature_name, random, pos)
            }
            Self::Geode(feature) => feature.generate(
                chunk,
                block_registry,
                min_y,
                height,
                feature_name,
                random,
                pos,
            ),
            Self::Kelp(_feature) => {
                KelpFeature::generate(chunk, min_y, height, feature_name, random, pos)
            }
            Self::Ore(feature) => feature.generate(
                chunk,
                block_registry,
                min_y,
                height,
                feature_name,
                random,
                pos,
            ),
            Self::ScatteredOre(feature) => feature.generate(
                chunk,
                block_registry,
                min_y,
                height,
                feature_name,
                random,
                pos,
            ),
            Self::MonsterRoom(_feature) => {
                DungeonFeature::generate(chunk, min_y, height, feature_name, random, pos)
            }
            Self::BlueIce(_feature) => BlueIceFeature::generate(chunk, random, pos),
            Self::GlowstoneBlob(_feature) => {
                GlowstoneBlobFeature::generate(chunk, min_y, height, feature_name, random, pos)
            }
            Self::Disk(feature) => feature.generate(
                chunk,
                block_registry,
                min_y,
                height,
                feature_name,
                random,
                pos,
            ),
            Self::Lake(feature) => feature.generate(block_registry, chunk, random, pos),
            Self::BasaltColumns(feature) => feature.generate(
                chunk,
                block_registry,
                min_y,
                height,
                feature_name,
                random,
                pos,
            ),
            Self::BasaltPillar(_feature) => BasaltPillarFeature::generate(chunk, random, pos),
            Self::ForestRock(feature) => feature.generate(chunk, block_registry, random, pos),
            Self::FreezeTopLayer(_feature) => {
                FreezeTopLayerFeature::generate(chunk, min_y, height, feature_name, random, pos)
            }
            Self::IceSpike(feature) => feature.generate(chunk, block_registry, random, pos),
            Self::Iceberg(feature) => {
                feature.generate(chunk, min_y, height, feature_name, random, pos)
            }
            Self::ChorusPlant(_feature) => {
                ChorusPlantFeature::generate(chunk, min_y, height, feature_name, random, pos)
            }
            Self::EndIsland(_feature) => {
                EndIslandFeature::generate(chunk, min_y, height, feature_name, random, pos)
            }
            Self::SculkPatch(feature) => feature.generate(chunk, random, pos),
            Self::RootSystem(feature) => feature.generate(
                chunk,
                block_registry,
                min_y,
                height,
                feature_name,
                random,
                pos,
            ),
            Self::BonusChest(_feature) => {
                BonusChestFeature::generate(chunk, min_y, height, feature_name, random, pos)
            }
            Self::DeltaFeature(feature) => {
                feature.generate(chunk, min_y, height, feature_name, random, pos)
            }
            Self::BlockPile(feature) => feature.generate(
                chunk,
                block_registry,
                min_y,
                height,
                feature_name,
                random,
                pos,
            ),
            Self::SpeleothemCluster(feature) => feature.generate(chunk, random, pos),
            Self::LargeDripstone(feature) => feature.generate(chunk, random, pos),
            Self::EndGateway(feature) => feature.generate(chunk, pos),
            Self::FillLayer(feature) => {
                feature.generate(chunk, min_y, height, feature_name, random, pos)
            }
            Self::FallenTree(feature) => feature.generate(
                chunk,
                block_registry,
                min_y,
                height,
                feature_name,
                random,
                pos,
            ),
            Self::HugeBrownMushroom(feature) => {
                feature.generate(chunk, min_y, height, feature_name, random, pos)
            }
            Self::HugeFungus(feature) => feature.generate(
                chunk,
                block_registry,
                min_y,
                height,
                feature_name,
                random,
                pos,
            ),
            Self::HugeRedMushroom(feature) => {
                feature.generate(chunk, min_y, height, feature_name, random, pos)
            }
            Self::MultifaceGrowth(feature) => {
                feature.generate(chunk, min_y, height, feature_name, random, pos)
            }
            Self::ReplaceSingleBlock(feature) => {
                feature.generate(chunk, min_y, height, feature_name, random, pos)
            }
            Self::VoidStartPlatform(feature) => {
                feature.generate(chunk, min_y, height, feature_name, random, pos)
            }
            Self::WeepingVines(feature) => {
                feature.generate(chunk, min_y, height, feature_name, random, pos)
            }
            Self::Overlay(feature) => feature.generate(
                chunk,
                block_registry,
                min_y,
                height,
                feature_name,
                random,
                pos,
            ),
            Self::Sequence(feature) => feature.generate(
                chunk,
                block_registry,
                min_y,
                height,
                feature_name,
                random,
                pos,
            ),
            Self::ProjectedRandomPatchySquare(feature) => feature.generate(
                chunk,
                block_registry,
                min_y,
                height,
                feature_name,
                random,
                pos,
            ),
            Self::RandomNeighborSpread(feature) => feature.generate(
                chunk,
                block_registry,
                min_y,
                height,
                feature_name,
                random,
                pos,
            ),
            Self::SingleBlockPillar(feature) => feature.generate(
                chunk,
                block_registry,
                min_y,
                height,
                feature_name,
                random,
                pos,
            ),
            Self::SteppedColumnCluster(feature) => {
                feature.generate(chunk, block_registry, random, pos)
            }
            Self::Template(feature) => feature.generate(chunk, block_registry, random, pos),
            Self::NoOp => false,
        }
    }
}

// generated code is now placed alongside other codegen outputs
// in `src/generated` so we don’t hide it deep under `generation/feature`.
include!("../../../../pumpkin-data/src/generated/configured_features_generated.rs");

#[cfg(test)]
mod tests {
    use super::{BONE_MEAL_FEATURES, CONFIGURED_FEATURES, ConfiguredFeature};
    use crate::generation::feature::placed_features::{
        Feature, PlacedFeature, PlacedFeatureWrapper,
    };
    use pumpkin_data::{
        Block,
        block_properties::{
            Axis, BrainCoralWallFanProperties, HayBlockProperties, HorizontalFacing,
        },
    };
    use pumpkin_util::{
        math::position::BlockPos,
        random::{RandomGenerator, legacy_rand::LegacyRand, xoroshiro128::Xoroshiro},
    };

    #[test]
    fn bonemeal_feature_tag_resolves_to_placeable_blocks() {
        assert_eq!(BONE_MEAL_FEATURES.len(), 8);
        let mut random = RandomGenerator::Xoroshiro(Xoroshiro::from_seed(0));
        for key in BONE_MEAL_FEATURES.iter() {
            let Some(ConfiguredFeature::SimpleBlock(feature)) = CONFIGURED_FEATURES.get(key) else {
                panic!("bonemeal feature {key:?} must place a block");
            };
            assert!(
                feature
                    .to_place
                    .get_for_bonemeal(&mut random, BlockPos::new(0, 64, 0))
                    .is_some()
            );
        }
    }

    #[test]
    fn bonemeal_features_use_tag_output() {
        let tag_values = pumpkin_data::tag::get_tag_values(
            pumpkin_data::tag::RegistryKey::WorldgenConfiguredFeature,
            "minecraft:can_spawn_from_bone_meal",
        );
        assert!(tag_values.is_some());
        let values = tag_values.unwrap();
        assert_eq!(values.len(), 8);
        assert!(values.contains(&"flower_default"));
        assert!(values.contains(&"wildflower"));
    }

    /// `coral/block_decoration` turns a fan from `#minecraft:wall_corals`, drawn with `nextInt(5)`
    /// in tag order, to the side it grows on: `java.util.Random(12345)` draws 1, brain coral.
    #[test]
    fn coral_decoration_turns_wall_coral_fans() {
        let Some(ConfiguredFeature::Overlay(decoration)) = CONFIGURED_FEATURES
            .get(&pumpkin_data::configured_feature::ConfiguredFeature::CoralBlockDecoration)
        else {
            panic!("coral/block_decoration must be an overlay");
        };
        let fans: Vec<_> = decoration.features[1..]
            .iter()
            .map(|wrapper| {
                let PlacedFeatureWrapper::Direct(PlacedFeature {
                    feature: Feature::Inlined(feature),
                    ..
                }) = wrapper
                else {
                    panic!("the wall coral fans must be inlined");
                };
                let ConfiguredFeature::SimpleBlock(fan) = feature.as_ref() else {
                    panic!("the wall coral fans must be simple blocks");
                };
                let mut random = RandomGenerator::Legacy(LegacyRand::from_seed(12345));
                fan.to_place
                    .get_for_bonemeal(&mut random, BlockPos::new(0, 64, 0))
                    .map(|state| state.id)
            })
            .collect();
        let expected = [
            HorizontalFacing::North,
            HorizontalFacing::East,
            HorizontalFacing::South,
            HorizontalFacing::West,
        ]
        .map(|facing| {
            let mut fan = BrainCoralWallFanProperties::default(&Block::BRAIN_CORAL_WALL_FAN);
            fan.facing = facing;
            Some(fan.to_state_id(&Block::BRAIN_CORAL_WALL_FAN))
        });
        assert_eq!(fans, expected);
    }

    /// `pile_hay` turns every bale with one `nextInt(6)` over DOWN, UP, NORTH, SOUTH, WEST, EAST:
    /// `java.util.Random(12345)` draws UP, WEST, SOUTH.
    #[test]
    fn hay_pile_turns_every_bale() {
        let Some(ConfiguredFeature::BlockPile(pile)) =
            CONFIGURED_FEATURES.get(&pumpkin_data::configured_feature::ConfiguredFeature::PileHay)
        else {
            panic!("pile_hay must be a block pile");
        };
        let mut random = RandomGenerator::Legacy(LegacyRand::from_seed(12345));
        let bales: Vec<_> = (0..3)
            .map(|_| {
                pile.state_provider
                    .get_for_bonemeal(&mut random, BlockPos::new(0, 64, 0))
                    .map(|state| state.id)
            })
            .collect();
        let expected = [Axis::Y, Axis::X, Axis::Z].map(|axis| {
            let mut bale = HayBlockProperties::default(&Block::HAY_BLOCK);
            bale.axis = axis;
            Some(bale.to_state_id(&Block::HAY_BLOCK))
        });
        assert_eq!(bales, expected);
    }
}
