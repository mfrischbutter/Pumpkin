use rand::{RngExt, rng};
use std::sync::Arc;
use std::sync::Mutex;

use crate::block::blocks::carved_pumpkin::find_golem_pattern;
use crate::block::blocks::redstone::block_receives_redstone_power;
use crate::block::blocks::tnt::TNTBlock;
use crate::block::blocks::wither_skull::find_wither_pattern;
use crate::block::registry::BlockActionResult;
use crate::block::{
    BlockBehaviour, GetComparatorOutputArgs, GetScreenHandlerFactoryArgs, NormalUseArgs,
    OnNeighborUpdateArgs, OnPlaceArgs, OnScheduledTickArgs, PlacedArgs,
};
use crate::entity::decoration::armor_stand::ArmorStandEntity;
use crate::entity::item::ItemEntity;
use crate::entity::passive::copper_golem::CopperGolemEntity;
use crate::entity::mob::sulfur_cube::{SulfurCubeEntity, is_swallowable_item};
use crate::entity::passive::sheep::SheepEntity;
use crate::entity::projectile::projectile_item::{
    DispenseConfig, DispensePosition, dispense_config, shoot_projectile_item, triangle,
};
use crate::entity::tnt::TNTEntity;
use crate::entity::vehicle::boat::BoatEntity;
use crate::entity::vehicle::minecart::MinecartEntity;
use crate::entity::{Entity, EntityBase};
use crate::item::ItemMetadata;
use crate::item::items::boat::BoatItem;
use crate::item::items::bucket::{
    FilledBucketItem, play_bucket_evaporation, should_evaporate_in_nether, try_pickup_fluid_at,
    try_place_filled_bucket,
};
use crate::item::items::honeycomb::try_wax_block;
use crate::item::items::ignite::ignition::Ignition;
use crate::item::items::minecart::MinecartItem;
use crate::item::items::spawn_egg::{can_spawn, spawn_from_item};
use crate::plugin::api::events::entity::creature_spawn::CreatureSpawnReason;
use crate::world::World;

use crate::block::entities::dispenser::DispenserBlockEntity;
use pumpkin_data::block_properties::{
    BeeNestLikeProperties, BlockProperties, EndRodLikeProperties, Facing,
    PoweredRailLikeProperties, RailLikeProperties, RespawnAnchorLikeProperties,
    SkeletonSkullLikeProperties,
};
use pumpkin_data::data_component::DataComponent;
use pumpkin_data::data_component_impl::{EquippableImpl, IDSet, PotionContentsImpl};
use pumpkin_data::entity::{EntityType, entity_from_egg};
use pumpkin_data::fluid::Fluid;
use pumpkin_data::game_event::GameEvent;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::particle::Particle;
use pumpkin_data::potion::Potion;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::tag::{self, Taggable};
use pumpkin_data::translation;
use pumpkin_data::world::WorldEvent;
use pumpkin_data::{Block, BlockStateId, FacingExt};
use pumpkin_inventory::Inventory;
use pumpkin_inventory::generic_container_screen_handler::create_generic_3x3;
use pumpkin_inventory::player::player_inventory::PlayerInventory;
use pumpkin_inventory::screen_handler::{
    InventoryPlayer, ScreenHandlerFactory, SharedScreenHandler,
};
use pumpkin_macros::pumpkin_block;
use pumpkin_util::math::boundingbox::{BoundingBox, EntityDimensions};
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_util::text::TextComponent;
use pumpkin_world::tick::TickPriority;
use pumpkin_world::world::BlockFlags;

struct DispenserScreenFactory(Arc<dyn Inventory>);

impl ScreenHandlerFactory for DispenserScreenFactory {
    fn create_screen_handler(
        &self,
        sync_id: u8,
        player_inventory: &Arc<PlayerInventory>,
        player: &dyn InventoryPlayer,
    ) -> Option<SharedScreenHandler> {
        let handler = create_generic_3x3(sync_id, player_inventory, self.0.clone(), player);
        let screen_handler_arc = Arc::new(Mutex::new(handler));

        Some(screen_handler_arc as SharedScreenHandler)
    }

    fn get_display_name(&self) -> TextComponent {
        pumpkin_macros::translate_cross!(
            translation::java::CONTAINER_DISPENSER,
            translation::bedrock::CONTAINER_DISPENSER
        )
    }
}

#[pumpkin_block("minecraft:dispenser")]
pub struct DispenserBlock;

type DispenserLikeProperties = pumpkin_data::block_properties::DispenserLikeProperties;

struct DispenseContext<'a> {
    world: &'a Arc<World>,
    position: &'a BlockPos,
    facing: Facing,
}

const fn to_normal(facing: Facing) -> Vector3<f64> {
    match facing {
        Facing::North => Vector3::new(0., 0., -1.),
        Facing::East => Vector3::new(1., 0., 0.),
        Facing::South => Vector3::new(0., 0., 1.),
        Facing::West => Vector3::new(-1., 0., 0.),
        Facing::Up => Vector3::new(0., 1., 0.),
        Facing::Down => Vector3::new(0., -1., 0.),
    }
}

const fn to_data3d(facing: Facing) -> i32 {
    match facing {
        Facing::North => 2,
        Facing::East => 5,
        Facing::South => 3,
        Facing::West => 4,
        Facing::Up => 1,
        Facing::Down => 0,
    }
}

const fn to_rotation16(facing: Facing) -> u8 {
    match facing {
        Facing::South | Facing::Up | Facing::Down => 0,
        Facing::West => 4,
        Facing::North => 8,
        Facing::East => 12,
    }
}

fn is_allowed_entity(
    allowed: Option<&IDSet<EntityType>>,
    entity_type: &'static EntityType,
) -> bool {
    match allowed {
        None => true,
        Some(IDSet::Tag(tag)) => entity_type.is_tagged_with(tag).unwrap_or(false),
        Some(IDSet::IDs(types)) => types.contains(&entity_type),
    }
}

fn water_bottle() -> ItemStack {
    ItemStack::new_with_component(
        1,
        &Item::POTION,
        vec![(
            DataComponent::PotionContents,
            Some(Box::new(PotionContentsImpl {
                potion_id: Some(i32::from(Potion::WATER.id)),
                custom_color: None,
                custom_effects: Vec::new(),
                custom_name: None,
            }) as Box<_>),
        )],
    )
}

fn is_water_bottle(stack: &ItemStack) -> bool {
    stack.item.id == Item::POTION.id
        && stack
            .get_data_component::<PotionContentsImpl>()
            .and_then(|contents| contents.potion_id)
            == Some(i32::from(Potion::WATER.id))
}

impl BlockBehaviour for DispenserBlock {
    fn normal_use(&self, args: NormalUseArgs<'_>) -> BlockActionResult {
        if let Some(factory) = self.get_screen_handler_factory(GetScreenHandlerFactoryArgs {
            server: args.server,
            world: args.world,
            block: args.block,
            position: args.position,
            player: args.player,
        }) {
            args.player.increment_stat(
                pumpkin_data::statistic::StatisticCategory::Custom,
                pumpkin_data::statistic::CustomStatistic::InspectDispenser as i32,
                1,
            );
            args.player
                .open_handled_screen(factory.as_ref(), Some(*args.position));
        }
        BlockActionResult::Success
    }

    fn get_screen_handler_factory(
        &self,
        args: GetScreenHandlerFactoryArgs<'_>,
    ) -> Option<Box<dyn ScreenHandlerFactory>> {
        let block_entity = args.world.get_block_entity(args.position)?;
        let inventory = block_entity.get_inventory()?;
        Some(Box::new(DispenserScreenFactory(inventory)))
    }

    fn on_place(&self, args: OnPlaceArgs<'_>) -> BlockStateId {
        let mut props = DispenserLikeProperties::default(args.block);
        props.facing = args.player.get_entity().get_facing().opposite();
        props.to_state_id(args.block)
    }

    fn placed(&self, args: PlacedArgs<'_>) {
        let dispenser_block_entity = DispenserBlockEntity::new(*args.position);
        args.world
            .add_block_entity(Arc::new(dispenser_block_entity));
    }

    fn on_neighbor_update(&self, args: OnNeighborUpdateArgs<'_>) {
        let powered = block_receives_redstone_power(args.world, args.position)
            || block_receives_redstone_power(args.world, &args.position.up());

        let mut props =
            DispenserLikeProperties::from_state_id(args.world.get_block_state(args.position).id);

        if powered && !props.triggered {
            args.world
                .schedule_block_tick(args.block, *args.position, 4, TickPriority::Normal);
            props.triggered = true;
            args.world.set_block_state(
                args.position,
                props.to_state_id(args.block),
                BlockFlags::NOTIFY_LISTENERS,
            );
        } else if !powered && props.triggered {
            props.triggered = false;
            args.world.set_block_state(
                args.position,
                props.to_state_id(args.block),
                BlockFlags::NOTIFY_LISTENERS,
            );
        }
    }

    fn on_scheduled_tick(&self, args: OnScheduledTickArgs<'_>) {
        let (_block, state) = args.world.get_block_and_state(args.position);
        if let Some(block_entity) = args.world.get_block_entity(args.position) {
            let Some(dispenser) = block_entity.as_any().downcast_ref::<DispenserBlockEntity>()
            else {
                return;
            };

            if let Some((slot_index, mut item)) = dispenser.get_random_slot() {
                let props = DispenserLikeProperties::from_state_id(state.id);
                let ctx = DispenseContext {
                    world: args.world,
                    position: args.position,
                    facing: props.facing,
                };
                Self::dispense(&ctx, dispenser, &mut item);
                dispenser.set_stack(slot_index, item);
            } else {
                args.world
                    .sync_world_event(WorldEvent::SoundDispenserFail, *args.position, 0);
                args.world.emit_game_event(
                    GameEvent::BlockActivate.name(),
                    args.position.to_centered_f64(),
                );
            }
        }
    }

    fn get_comparator_output(&self, args: GetComparatorOutputArgs<'_>) -> Option<u8> {
        crate::block::container_comparator_output(&args)
    }
}

impl DispenserBlock {
    fn dispense(ctx: &DispenseContext<'_>, dispenser: &DispenserBlockEntity, item: &mut ItemStack) {
        let mut event = crate::plugin::api::events::block::block_dispense::BlockDispenseEvent::new(
            *ctx.position,
            item.item.registry_key.to_string(),
        );
        if let Some(server) = ctx.world.server.upgrade() {
            server.plugin_manager.fire_blocking(&server, &mut event);
        }
        if event.cancelled {
            ctx.world
                .sync_world_event(WorldEvent::SoundDispenserFail, *ctx.position, 0);
            return;
        }

        // Still missing some specific dispenser behavior that you can find here:
        // https://minecraft.wiki/w/Dispenser#Usage
        let boats = BoatItem::ids();

        if let Some(config) = Self::projectile_config(item.item) {
            Self::dispense_projectile(ctx, item, &config);
        } else if boats.contains(&item.item.id) {
            // Boats
            if !Self::dispense_boat(ctx, item) {
                Self::drop_item(ctx, item);
            }
        } else if MinecartItem::ids().contains(&item.item.id) {
            // Minecarts
            if !Self::dispense_minecart(ctx, item) {
                Self::drop_item(ctx, item);
            }
        } else if item.item.id == Item::ARMOR_STAND.id {
            // Armor stands
            if !Self::dispense_armor_stand(ctx, item) {
                Self::drop_item(ctx, item);
            }
        } else if item.item.id == Item::TNT.id {
            // TNT
            Self::dispense_tnt(ctx, item);
        } else if item.item.id == Item::BUCKET.id {
            // Empty buckets pick up the fluid in front of the dispenser
            Self::dispense_empty_bucket(ctx, dispenser, item);
        } else if FilledBucketItem::ids().contains(&item.item.id) {
            // Filled buckets place their fluid in front of the dispenser
            Self::dispense_filled_bucket(ctx, item);
        } else if item.item.id == Item::FLINT_AND_STEEL.id {
            // Flint and steel light fires and prime TNT
            Self::dispense_flint_and_steel(ctx, item);
        } else if item.item.id == Item::HONEYCOMB.id {
            // Honeycombs wax copper blocks
            Self::dispense_honeycomb(ctx, item);
        } else if entity_from_egg(item.item.id).is_some() {
            // Spawn eggs
            Self::dispense_spawn_egg(ctx, item);
        } else if item.item.id == Item::SHEARS.id {
            // Shears harvest full beehives and shear sheep
            Self::dispense_shears(ctx, item);
        } else if item.item.id == Item::GLASS_BOTTLE.id {
            // Glass bottles fill from water and full beehives
            if !Self::dispense_glass_bottle(ctx, dispenser, item) {
                Self::drop_item_with_sound(ctx, item, WorldEvent::SoundDispenserFail);
            }
        } else if is_water_bottle(item) {
            // Water bottles convert dirt-likes into mud
            if !Self::dispense_water_bottle(ctx, item) {
                Self::drop_item(ctx, item);
            }
        } else if item.item.id == Item::GLOWSTONE.id {
            match Self::dispense_glowstone(ctx, item) {
                Some(true) => Self::play_dispense_effects(ctx, WorldEvent::SoundDispenserDispense),
                Some(false) => Self::play_dispense_effects(ctx, WorldEvent::SoundDispenserFail),
                None => Self::drop_item(ctx, item),
            }
        } else if item.item.id == Item::WITHER_SKELETON_SKULL.id {
            // Placed only when it completes a wither, otherwise worn as a helmet
            Self::dispense_mob_head(ctx, item, &Block::WITHER_SKELETON_SKULL);
        } else if item.item.id == Item::CARVED_PUMPKIN.id {
            // Placed only when it completes a golem, otherwise worn as a helmet
            Self::dispense_mob_head(ctx, item, &Block::CARVED_PUMPKIN);
        } else if Block::from_item_id(item.item.id)
            .is_some_and(|block| block.has_tag(&tag::Block::MINECRAFT_SHULKER_BOXES))
        {
            // Shulker boxes place themselves
            if !Self::dispense_shulker_box(ctx, item) {
                Self::play_dispense_effects(ctx, WorldEvent::SoundDispenserFail);
            }
        } else if Self::dispense_equipment(ctx, item) {
            // Armor, elytra, heads, saddles, horse/wolf armor and llama carpets
            Self::play_dispense_effects(ctx, WorldEvent::SoundDispenserDispense);
        } else if is_swallowable_item(item)
            && SulfurCubeEntity::dispense_block(ctx.world, &Self::target_position(ctx), item)
        {
            // Blocks into the sulfur cube in front
            Self::play_dispense_effects(ctx, WorldEvent::SoundDispenserDispense);
        } else {
            // TODO: Bone meal, chests onto llamas, brushes onto armadillos
            // Default / Drop
            Self::drop_item(ctx, item);
        }
    }

    fn finish_projectile_launch(
        ctx: &DispenseContext<'_>,
        projectile: Arc<dyn EntityBase>,
        launch_event: WorldEvent,
    ) {
        ctx.world.spawn_entity(projectile);
        Self::play_dispense_effects(ctx, launch_event);
    }

    fn play_dispense_effects(ctx: &DispenseContext<'_>, sound_event: WorldEvent) {
        ctx.world.sync_world_event(sound_event, *ctx.position, 0);
        ctx.world.sync_world_event(
            WorldEvent::ParticlesShootSmoke,
            *ctx.position,
            to_data3d(ctx.facing),
        );
    }

    /// Vanilla `DispenseItemBehavior.bootStrap`'s `registerProjectileBehavior` calls: the
    /// projectile items a dispenser shoots (not the trident), with their launch settings.
    fn projectile_config(item: &Item) -> Option<DispenseConfig> {
        const SHOT: [&Item; 13] = [
            &Item::ARROW,
            &Item::TIPPED_ARROW,
            &Item::SPECTRAL_ARROW,
            &Item::EGG,
            &Item::BLUE_EGG,
            &Item::BROWN_EGG,
            &Item::SNOWBALL,
            &Item::EXPERIENCE_BOTTLE,
            &Item::SPLASH_POTION,
            &Item::LINGERING_POTION,
            &Item::FIREWORK_ROCKET,
            &Item::FIRE_CHARGE,
            &Item::WIND_CHARGE,
        ];
        SHOT.iter()
            .any(|shot| shot.id == item.id)
            .then(|| dispense_config(item))
            .flatten()
    }

    /// Vanilla `ProjectileDispenseBehavior`: the projectile leaves from the config's
    /// position, shot along the facing and a tenth upwards.
    fn dispense_projectile(
        ctx: &DispenseContext<'_>,
        item: &mut ItemStack,
        config: &DispenseConfig,
    ) {
        let projectile = item.split(1);
        let facing = to_normal(ctx.facing);
        let centre = ctx.position.to_centered_f64();
        let position = match config.position {
            DispensePosition::Default => centre.add(&(facing * 0.7)).add_raw(0.0, 0.1, 0.0),
            DispensePosition::FullBlock => centre.add(&facing),
            DispensePosition::JustOutside => centre.add(&(facing * 0.500_009_999_999_747_4)),
        };
        let Some(entity) = shoot_projectile_item(
            ctx.world,
            position,
            &projectile,
            facing,
            facing.add_raw(0.0, 0.1, 0.0),
            config,
            None,
        ) else {
            return;
        };
        Self::finish_projectile_launch(
            ctx,
            entity,
            config
                .override_dispense_event
                .unwrap_or(WorldEvent::SoundDispenserProjectileLaunch),
        );
    }

    fn target_position(ctx: &DispenseContext<'_>) -> BlockPos {
        let facing = to_normal(ctx.facing);
        ctx.position.offset(Vector3::new(
            facing.x as i32,
            facing.y as i32,
            facing.z as i32,
        ))
    }

    fn has_room_for(
        ctx: &DispenseContext<'_>,
        spawn_pos: Vector3<f64>,
        size: &EntityDimensions,
    ) -> bool {
        let bounding_box = BoundingBox::new_from_pos(spawn_pos.x, spawn_pos.y, spawn_pos.z, size);
        ctx.world.is_space_empty(bounding_box)
            && ctx.world.get_entities_at_box(&bounding_box).is_empty()
    }

    fn dispense_boat(ctx: &DispenseContext<'_>, item: &mut ItemStack) -> bool {
        let target = Self::target_position(ctx);
        let is_water = |id: u16| id == Fluid::WATER.id || id == Fluid::FLOWING_WATER.id;

        let spawn_pos = if is_water(ctx.world.get_fluid(&target).id) {
            target.to_f64()
        } else if ctx.world.get_block_state(&target).is_air()
            && is_water(ctx.world.get_fluid(&target.down()).id)
        {
            target.down().to_f64()
        } else {
            return false;
        };

        let entity_type = BoatItem::item_to_entity(item.item);
        let dimensions = EntityDimensions::new(
            entity_type.dimension[0],
            entity_type.dimension[1],
            entity_type.eye_height,
        );
        if !Self::has_room_for(ctx, spawn_pos, &dimensions) {
            return false;
        }

        let facing = to_normal(ctx.facing);
        let entity = Entity::new(ctx.world.clone(), spawn_pos, entity_type);
        entity.set_rotation(facing.x.atan2(facing.z) as f32 * 57.295_776, 0.0);
        let boat = Arc::new(BoatEntity::new(entity));
        // Vanilla `BoatDispenseItemBehavior`: `EntityType.createDefaultStackConfig`.
        crate::item::items::spawn_egg::apply_entity_variant(item, boat.as_ref(), None);
        let _ = item.split(1);
        ctx.world.spawn_entity(boat);

        ctx.world
            .sync_world_event(WorldEvent::SoundDispenserDispense, *ctx.position, 0);
        true
    }

    fn dispense_armor_stand(ctx: &DispenseContext<'_>, item: &mut ItemStack) -> bool {
        let target = Self::target_position(ctx);
        let spawn_pos = target.to_f64();
        let dimensions = EntityDimensions::new(
            EntityType::ARMOR_STAND.dimension[0],
            EntityType::ARMOR_STAND.dimension[1],
            EntityType::ARMOR_STAND.eye_height,
        );
        if !Self::has_room_for(ctx, spawn_pos, &dimensions) {
            return false;
        }

        let _ = item.split(1);
        let facing = to_normal(ctx.facing);
        let entity = Entity::new(ctx.world.clone(), spawn_pos, &EntityType::ARMOR_STAND);
        entity.set_rotation(facing.x.atan2(facing.z) as f32 * 57.295_776, 0.0);

        ctx.world.play_sound(
            Sound::EntityArmorStandPlace,
            SoundCategory::Blocks,
            &spawn_pos,
        );
        ctx.world
            .spawn_entity(Arc::new(ArmorStandEntity::new(entity)));

        ctx.world
            .sync_world_event(WorldEvent::SoundDispenserDispense, *ctx.position, 0);
        true
    }

    fn dispense_tnt(ctx: &DispenseContext<'_>, item: &mut ItemStack) {
        // Vanilla keeps the item and plays the fail click when TNT is disabled.
        if !ctx.world.level_info.load().game_rules.tnt_explodes {
            Self::play_dispense_effects(ctx, WorldEvent::SoundDispenserFail);
            return;
        }

        let target = Self::target_position(ctx);
        if SulfurCubeEntity::dispense_block(ctx.world, &target, item) {
            Self::play_dispense_effects(ctx, WorldEvent::SoundDispenserDispense);
            return;
        }
        let _ = item.split(1);

        let tnt = TNTEntity::primed(ctx.world, &target, TNTEntity::DEFAULT_FUSE);
        let spawn_pos = tnt.get_entity().pos.load();
        ctx.world.spawn_entity(tnt);
        ctx.world
            .play_sound(Sound::EntityTntPrimed, SoundCategory::Blocks, &spawn_pos);
        ctx.world
            .emit_game_event(GameEvent::EntityPlace.name(), target.to_centered_f64());

        Self::play_dispense_effects(ctx, WorldEvent::SoundDispenserDispense);
    }

    fn dispense_spawn_egg(ctx: &DispenseContext<'_>, item: &mut ItemStack) {
        let Some(entity_type) = entity_from_egg(item.item.id) else {
            return;
        };

        // Vanilla `SpawnEggItemBehavior`: the mob at the facing block, sinking onto it unless
        // the dispenser faces up; the egg stays when the type may not spawn or a plugin
        // cancelled the spawn. A dispenser has no acting player.
        if can_spawn(entity_type, ctx.world)
            && spawn_from_item(
                entity_type,
                item,
                ctx.world,
                None,
                &Self::target_position(ctx),
                ctx.facing != Facing::Up,
                false,
                crate::entity::mob::spawn::EntitySpawnReason::Dispenser,
                CreatureSpawnReason::DispenseEgg,
            )
            .is_some()
        {
            item.decrement(1);
            ctx.world.emit_game_event(
                GameEvent::EntityPlace.name(),
                ctx.position.to_centered_f64(),
            );
        }

        ctx.world
            .sync_world_event(WorldEvent::SoundDispenserDispense, *ctx.position, 0);
    }

    fn dispense_empty_bucket(
        ctx: &DispenseContext<'_>,
        dispenser: &DispenserBlockEntity,
        item: &mut ItemStack,
    ) {
        let front = Self::target_position(ctx);
        let Some(filled) = try_pickup_fluid_at(ctx.world, front, |_, _| true) else {
            Self::drop_item(ctx, item);
            return;
        };

        Self::consume_with_remainder(ctx, dispenser, item, ItemStack::new(1, filled));
        Self::play_dispense_effects(ctx, WorldEvent::SoundDispenserDispense);
    }

    /// Places `stack` into the first empty slot, returning it back if every slot is occupied.
    /// The slot currently being dispensed from still holds its pre-dispense stack, so it is
    /// never considered free.
    fn add_to_first_free_slot(
        dispenser: &DispenserBlockEntity,
        stack: ItemStack,
    ) -> Option<ItemStack> {
        let mut items = dispenser
            .items
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for slot in items.iter_mut() {
            if slot.is_empty() {
                *slot = stack;
                dispenser.mark_dirty();
                return None;
            }
        }
        Some(stack)
    }

    fn dispense_filled_bucket(ctx: &DispenseContext<'_>, item: &mut ItemStack) {
        let front = Self::target_position(ctx);

        // TODO: Spawn the stored entity for axolotl/fish/tadpole buckets, like the player path.
        let emptied = if should_evaporate_in_nether(item.item, ctx.world) {
            play_bucket_evaporation(ctx.world, &front.to_f64());
            true
        } else {
            try_place_filled_bucket(
                ctx.world,
                item.item,
                *ctx.position,
                ctx.facing.to_block_direction(),
                |_| true,
            )
        };

        if emptied {
            *item = ItemStack::new(1, &Item::BUCKET);
            Self::play_dispense_effects(ctx, WorldEvent::SoundDispenserDispense);
        } else {
            Self::drop_item(ctx, item);
        }
    }

    fn dispense_flint_and_steel(ctx: &DispenseContext<'_>, item: &mut ItemStack) {
        let front = Self::target_position(ctx);
        let front_block = ctx.world.get_block(&front);

        let ignited = if SulfurCubeEntity::ignite_explosive_at(ctx.world, &front) {
            true
        } else if front_block == &Block::TNT {
            TNTBlock::prime(ctx.world, &front)
        } else {
            Ignition::ignite_block(
                |world: Arc<World>, pos: BlockPos, new_state_id: BlockStateId| {
                    world.set_block_state(&pos, new_state_id, BlockFlags::NOTIFY_ALL);
                },
                ctx.world,
                front,
                front,
                front_block,
            )
        };

        if ignited {
            // `damage_item` already consumes the tool from the stack when it breaks.
            let _ = item.damage_item(1);
            Self::play_dispense_effects(ctx, WorldEvent::SoundDispenserDispense);
        } else {
            Self::play_dispense_effects(ctx, WorldEvent::SoundDispenserFail);
        }
    }

    fn dispense_honeycomb(ctx: &DispenseContext<'_>, item: &mut ItemStack) {
        let front = Self::target_position(ctx);
        let front_block = ctx.world.get_block(&front);

        if try_wax_block(ctx.world, front, front_block) {
            item.decrement(1);
            Self::play_dispense_effects(ctx, WorldEvent::SoundDispenserDispense);
        } else {
            Self::play_dispense_effects(ctx, WorldEvent::SoundDispenserFail);
        }
    }

    fn dispense_minecart(ctx: &DispenseContext<'_>, item: &mut ItemStack) -> bool {
        fn rail_is_ascending(world: &Arc<World>, pos: &BlockPos) -> Option<bool> {
            let (block, state_id) = world.get_block_and_state_id(pos);
            if !block.has_tag(&tag::Block::MINECRAFT_RAILS) {
                return None;
            }
            Some(if PoweredRailLikeProperties::handles_block_id(block.id) {
                PoweredRailLikeProperties::from_state_id(state_id)
                    .shape
                    .is_ascending()
            } else {
                RailLikeProperties::from_state_id(state_id)
                    .shape
                    .is_ascending()
            })
        }

        let target = Self::target_position(ctx);
        let height = if let Some(ascending) = rail_is_ascending(ctx.world, &target) {
            if ascending { 0.6 } else { 0.1 }
        } else if ctx.world.get_block_state(&target).is_air()
            && let Some(ascending) = rail_is_ascending(ctx.world, &target.down())
        {
            if ascending && ctx.facing != Facing::Down {
                -0.4
            } else {
                -0.9
            }
        } else {
            return false;
        };

        let entity_type = MinecartItem::item_to_entity(item.item);
        let dispensed = item.split(1);

        let normal = to_normal(ctx.facing);
        let center = ctx.position.to_centered_f64();
        let spawn_pos = Vector3::new(
            normal.x.mul_add(1.125, center.x),
            f64::from(ctx.position.0.y) + normal.y + height,
            normal.z.mul_add(1.125, center.z),
        );

        let minecart = MinecartEntity::new(Entity::new(ctx.world.clone(), spawn_pos, entity_type));
        minecart.apply_item_stack(&dispensed);
        ctx.world.spawn_entity(Arc::new(minecart));

        ctx.world
            .sync_world_event(WorldEvent::SoundDispenserDispense, *ctx.position, 0);
        true
    }

    fn dispense_shears(ctx: &DispenseContext<'_>, item: &mut ItemStack) {
        if Self::shear_beehive(ctx) || Self::shear_entity_in_front(ctx, item) {
            // `damage_item` already consumes the tool from the stack when it breaks.
            let _ = item.damage_item(1);
            Self::play_dispense_effects(ctx, WorldEvent::SoundDispenserDispense);
        } else {
            Self::play_dispense_effects(ctx, WorldEvent::SoundDispenserFail);
        }
    }

    fn shear_beehive(ctx: &DispenseContext<'_>) -> bool {
        const FULL_HONEY_LEVEL: u8 = 5;
        const HARVESTED_HONEYCOMBS: u8 = 3;

        let target = Self::target_position(ctx);
        let (block, state_id) = ctx.world.get_block_and_state_id(&target);
        if !block.has_tag(&tag::Block::MINECRAFT_BEEHIVES) {
            return false;
        }

        let mut props = BeeNestLikeProperties::from_state_id(state_id);
        if props.honey_level < FULL_HONEY_LEVEL {
            return false;
        }

        ctx.world
            .play_block_sound(Sound::BlockBeehiveShear, SoundCategory::Blocks, target);
        Self::drop_at(
            ctx.world,
            target.to_centered_f64(),
            ItemStack::new(HARVESTED_HONEYCOMBS, &Item::HONEYCOMB),
        );

        props.honey_level = 0;
        ctx.world
            .set_block_state(&target, props.to_state_id(block), BlockFlags::NOTIFY_ALL);

        true
    }

    fn shear_entity_in_front(ctx: &DispenseContext<'_>, tool: &ItemStack) -> bool {
        let target = Self::target_position(ctx);
        let target_box = BoundingBox::from_block(&target);

        for entity in ctx.world.get_entities_at_box(&target_box) {
            if let Some(golem) = entity.cast_any().downcast_ref::<CopperGolemEntity>()
                && golem.ready_for_shearing()
                && entity.get_entity().is_alive()
            {
                golem.shear(SoundCategory::Blocks);
                ctx.world
                    .emit_game_event(GameEvent::Shear.name(), target.to_centered_f64());
                return true;
            }
            if let Some(cube) = entity.cast_any().downcast_ref::<SulfurCubeEntity>() {
                if cube.ready_for_shearing() && entity.get_entity().is_alive() {
                    cube.shear();
                    ctx.world
                        .emit_game_event(GameEvent::Shear.name(), target.to_centered_f64());
                    return true;
                }
                continue;
            }
            let Some(sheep) = entity.cast_any().downcast_ref::<SheepEntity>() else {
                continue;
            };
            if !sheep.ready_for_shearing() || !entity.get_entity().is_alive() {
                continue;
            }

            sheep.shear(SoundCategory::Blocks, tool);
            ctx.world
                .emit_game_event(GameEvent::Shear.name(), target.to_centered_f64());

            return true;
        }

        false
    }

    fn dispense_glass_bottle(
        ctx: &DispenseContext<'_>,
        dispenser: &DispenserBlockEntity,
        item: &mut ItemStack,
    ) -> bool {
        const FULL_HONEY_LEVEL: u8 = 5;

        let target = Self::target_position(ctx);
        let (block, state_id) = ctx.world.get_block_and_state_id(&target);

        if block.has_tag(&tag::Block::MINECRAFT_BEEHIVES) {
            let mut props = BeeNestLikeProperties::from_state_id(state_id);
            if props.honey_level < FULL_HONEY_LEVEL {
                return false;
            }

            props.honey_level = 0;
            ctx.world
                .set_block_state(&target, props.to_state_id(block), BlockFlags::NOTIFY_ALL);
            Self::consume_with_remainder(
                ctx,
                dispenser,
                item,
                ItemStack::new(1, &Item::HONEY_BOTTLE),
            );
            Self::play_dispense_effects(ctx, WorldEvent::SoundDispenserDispense);
            return true;
        }

        let fluid = ctx.world.get_fluid(&target);
        if fluid.id == Fluid::WATER.id || fluid.id == Fluid::FLOWING_WATER.id {
            Self::consume_with_remainder(ctx, dispenser, item, water_bottle());
            Self::play_dispense_effects(ctx, WorldEvent::SoundDispenserDispense);
            return true;
        }

        false
    }

    fn dispense_water_bottle(ctx: &DispenseContext<'_>, item: &mut ItemStack) -> bool {
        let target = Self::target_position(ctx);
        if !ctx
            .world
            .get_block(&target)
            .has_tag(&tag::Block::MINECRAFT_CONVERTIBLE_TO_MUD)
        {
            return false;
        }

        ctx.world.spawn_particle(
            target.to_centered_f64(),
            Vector3::new(0.5, 0.5, 0.5),
            1.0,
            5,
            Particle::Splash,
        );
        ctx.world
            .play_block_sound(Sound::ItemBottleEmpty, SoundCategory::Blocks, target);
        ctx.world
            .set_block_state(&target, Block::MUD.default_state.id, BlockFlags::NOTIFY_ALL);

        *item = ItemStack::new(1, &Item::GLASS_BOTTLE);
        Self::play_dispense_effects(ctx, WorldEvent::SoundDispenserDispense);
        true
    }

    fn dispense_glowstone(ctx: &DispenseContext<'_>, item: &mut ItemStack) -> Option<bool> {
        const MAX_CHARGES: u8 = 4;

        let target = Self::target_position(ctx);
        let (block, state_id) = ctx.world.get_block_and_state_id(&target);
        if block != &Block::RESPAWN_ANCHOR {
            return None;
        }

        let mut props = RespawnAnchorLikeProperties::from_state_id(state_id);
        if props.charges >= MAX_CHARGES {
            return Some(false);
        }

        props.charges += 1;
        let _ = item.split(1);
        ctx.world
            .set_block_state(&target, props.to_state_id(block), BlockFlags::NOTIFY_ALL);
        ctx.world.play_block_sound(
            Sound::BlockRespawnAnchorCharge,
            SoundCategory::Blocks,
            target,
        );

        Some(true)
    }

    fn dispense_mob_head(ctx: &DispenseContext<'_>, item: &mut ItemStack, block: &'static Block) {
        let target = Self::target_position(ctx);

        let summons_mob = ctx.world.get_block_state(&target).is_air()
            && if block == &Block::WITHER_SKELETON_SKULL {
                find_wither_pattern(ctx.world, &target).is_some()
            } else {
                find_golem_pattern(ctx.world, &target).is_some()
            };

        if summons_mob {
            let state_id = if block == &Block::WITHER_SKELETON_SKULL {
                let mut props = SkeletonSkullLikeProperties::default(block);
                props.rotation = to_rotation16(ctx.facing);
                props.to_state_id(block)
            } else {
                block.default_state.id
            };

            let _ = item.split(1);
            ctx.world
                .set_block_state(&target, state_id, BlockFlags::NOTIFY_ALL);
            Self::play_dispense_effects(ctx, WorldEvent::SoundDispenserDispense);
        } else if Self::dispense_equipment(ctx, item) {
            Self::play_dispense_effects(ctx, WorldEvent::SoundDispenserDispense);
        } else {
            Self::play_dispense_effects(ctx, WorldEvent::SoundDispenserFail);
        }
    }

    fn dispense_shulker_box(ctx: &DispenseContext<'_>, item: &mut ItemStack) -> bool {
        let Some(block) = Block::from_item_id(item.item.id) else {
            return false;
        };

        let target = Self::target_position(ctx);
        if !ctx.world.get_block_state(&target).replaceable() {
            return false;
        }

        let mut props = EndRodLikeProperties::default(block);
        props.facing = if ctx.world.get_block_state(&target.down()).is_air() {
            ctx.facing
        } else {
            Facing::Up
        };

        let placed = item.split(1);
        ctx.world
            .set_block_state(&target, props.to_state_id(block), BlockFlags::NOTIFY_ALL);
        if let Some(block_entity) = ctx.world.get_block_entity(&target) {
            block_entity.apply_item_components(&placed);
        }
        Self::play_dispense_effects(ctx, WorldEvent::SoundDispenserDispense);

        true
    }

    fn dispense_equipment(ctx: &DispenseContext<'_>, item: &mut ItemStack) -> bool {
        let (slot, allowed_entities, equip_sound) = {
            let Some(equippable) = item.get_data_component::<EquippableImpl>() else {
                return false;
            };
            if !equippable.dispensable {
                return false;
            }
            (
                equippable.slot,
                equippable.allowed_entities.clone(),
                equippable.equip_sound.clone(),
            )
        };

        let target_box = BoundingBox::from_block(&Self::target_position(ctx));
        let players = ctx
            .world
            .get_players_at_box(&target_box)
            .into_iter()
            .map(|player| player as Arc<dyn EntityBase>);

        for entity in ctx
            .world
            .get_entities_at_box(&target_box)
            .into_iter()
            .chain(players)
        {
            let Some(living) = entity.get_living_entity() else {
                continue;
            };
            if !living.is_alive()
                || entity.is_spectator()
                || !is_allowed_entity(allowed_entities.as_ref(), entity.get_entity().entity_type)
                || entity
                    .get_mob()
                    .is_some_and(|mob| !mob.can_dispenser_equip_into_slot(slot))
            {
                continue;
            }

            let mut equipment = living
                .entity_equipment
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if !equipment.get(slot).is_empty() {
                continue;
            }

            let stack = item.split(1);
            equipment.put(slot, stack.clone());
            drop(equipment);
            // Vanilla `EquipmentDispenseItemBehavior`: a mob keeps the piece as a guaranteed
            // drop and stays loaded.
            if let Some(mob) = entity.get_mob() {
                let mob = mob.get_mob_entity();
                mob.set_guaranteed_drop(slot);
                mob.persistence_required
                    .store(true, std::sync::atomic::Ordering::Relaxed);
            }

            living.send_equipment_changes(&[(slot.clone(), stack)]);
            ctx.world.play_sound_event(
                &equip_sound,
                SoundCategory::Blocks,
                &entity.get_entity().pos.load(),
            );

            return true;
        }

        false
    }

    fn consume_with_remainder(
        ctx: &DispenseContext<'_>,
        dispenser: &DispenserBlockEntity,
        item: &mut ItemStack,
        remainder: ItemStack,
    ) {
        item.decrement(1);
        if item.is_empty() {
            *item = remainder;
        } else if let Some(rest) = Self::add_to_first_free_slot(dispenser, remainder) {
            Self::eject_item(ctx, rest);
        }
    }

    fn drop_at(world: &Arc<World>, position: Vector3<f64>, stack: ItemStack) {
        let entity = Entity::new(world.clone(), position, &EntityType::ITEM);
        world.spawn_entity(Arc::new(ItemEntity::new(entity, stack)));
    }

    fn drop_item(ctx: &DispenseContext<'_>, item: &mut ItemStack) {
        Self::drop_item_with_sound(ctx, item, WorldEvent::SoundDispenserDispense);
    }

    fn drop_item_with_sound(ctx: &DispenseContext<'_>, item: &mut ItemStack, sound: WorldEvent) {
        let drop_item = item.split(1);
        Self::eject_item(ctx, drop_item);
        Self::play_dispense_effects(ctx, sound);
    }

    fn eject_item(ctx: &DispenseContext<'_>, stack: ItemStack) {
        let facing = to_normal(ctx.facing);
        // Vanilla `DispenserBlock.getDispensePosition`.
        let position = ctx.position.to_centered_f64().add(&(facing * 0.7));
        spawn_item(ctx.world, stack, DISPENSE_SPEED, facing, position);
    }
}

/// Vanilla `DefaultDispenseItemBehavior.spawnItem`'s accuracy for a dispenser.
const DISPENSE_SPEED: i32 = 6;

/// Vanilla `DefaultDispenseItemBehavior.spawnItem`: an item thrown out at `position` along
/// `facing` (a unit normal) with the given accuracy, the way a dispenser ejects it.
pub(crate) fn spawn_item(
    world: &Arc<World>,
    stack: ItemStack,
    accuracy: i32,
    facing: Vector3<f64>,
    mut position: Vector3<f64>,
) {
    position.y -= if facing.y == 0.0 { 0.15625 } else { 0.125 };
    let entity = Entity::new(world.clone(), position, &EntityType::ITEM);
    let rd = rng().random::<f64>().mul_add(0.1, 0.2);
    let deviation = 0.017_227_5 * f64::from(accuracy);
    let velocity = Vector3::new(
        triangle(&mut rng(), facing.x * rd, deviation),
        triangle(&mut rng(), 0.2, deviation),
        triangle(&mut rng(), facing.z * rd, deviation),
    );
    let item_entity = Arc::new(ItemEntity::new_with_velocity(entity, stack, velocity, 40));
    world.spawn_entity(item_entity);
}
