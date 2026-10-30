use std::sync::Arc;
use std::sync::atomic::Ordering;

use crate::{
    entity::{EntityBase, player::Player, r#type::from_type},
    item::{ItemBehaviour, ItemMetadata},
};
use pumpkin_data::{
    Block, BlockDirection,
    data_component_impl::{
        BucketEntityDataImpl, CustomNameImpl, EquipmentSlot, SulfurCubeContentImpl,
    },
    entity::EntityType,
    fluid::Fluid,
    game_event::GameEvent,
    item::Item,
    item_stack::ItemStack,
    sound::{Sound, SoundCategory},
    statistic::StatisticCategory,
};
use pumpkin_inventory::player::player_inventory::PlayerInventory;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::{
    GameMode, Hand,
    math::{position::BlockPos, vector3::Vector3},
};
use pumpkin_world::{tick::TickPriority, world::BlockFlags};
use uuid::Uuid;

use crate::world::World;

pub struct EmptyBucketItem;
pub struct FilledBucketItem;
pub struct MilkBucketItem;

impl ItemMetadata for EmptyBucketItem {
    fn ids() -> Box<[u16]> {
        [Item::BUCKET.id].into()
    }
}

impl ItemMetadata for FilledBucketItem {
    fn ids() -> Box<[u16]> {
        [
            Item::WATER_BUCKET.id,
            Item::LAVA_BUCKET.id,
            Item::POWDER_SNOW_BUCKET.id,
            Item::AXOLOTL_BUCKET.id,
            Item::COD_BUCKET.id,
            Item::SALMON_BUCKET.id,
            Item::TROPICAL_FISH_BUCKET.id,
            Item::PUFFERFISH_BUCKET.id,
            Item::TADPOLE_BUCKET.id,
            Item::SULFUR_CUBE_BUCKET.id,
        ]
        .into()
    }
}

impl ItemMetadata for MilkBucketItem {
    fn ids() -> Box<[u16]> {
        [Item::MILK_BUCKET.id].into()
    }
}

const fn get_mob_for_bucket(item: &Item) -> Option<(&'static EntityType, Sound)> {
    if item.id == Item::AXOLOTL_BUCKET.id {
        Some((&EntityType::AXOLOTL, Sound::ItemBucketEmptyAxolotl))
    } else if item.id == Item::COD_BUCKET.id {
        Some((&EntityType::COD, Sound::ItemBucketEmptyFish))
    } else if item.id == Item::SALMON_BUCKET.id {
        Some((&EntityType::SALMON, Sound::ItemBucketEmptyFish))
    } else if item.id == Item::TROPICAL_FISH_BUCKET.id {
        Some((&EntityType::TROPICAL_FISH, Sound::ItemBucketEmptyFish))
    } else if item.id == Item::PUFFERFISH_BUCKET.id {
        Some((&EntityType::PUFFERFISH, Sound::ItemBucketEmptyFish))
    } else if item.id == Item::TADPOLE_BUCKET.id {
        Some((&EntityType::TADPOLE, Sound::ItemBucketEmptyTadpole))
    } else if item.id == Item::SULFUR_CUBE_BUCKET.id {
        Some((&EntityType::SULFUR_CUBE, Sound::ItemBucketEmptySulfurCube))
    } else {
        None
    }
}

const fn get_empty_sound(item: &Item) -> Sound {
    if let Some((_, sound)) = get_mob_for_bucket(item) {
        sound
    } else if item.id == Item::LAVA_BUCKET.id {
        Sound::ItemBucketEmptyLava
    } else if item.id == Item::POWDER_SNOW_BUCKET.id {
        Sound::ItemBucketEmptyPowderSnow
    } else {
        Sound::ItemBucketEmpty
    }
}

const fn get_fill_sound(item: &Item) -> Sound {
    if item.id == Item::LAVA_BUCKET.id {
        Sound::ItemBucketFillLava
    } else if item.id == Item::POWDER_SNOW_BUCKET.id {
        Sound::ItemBucketFillPowderSnow
    } else {
        Sound::ItemBucketFill
    }
}

fn give_filled_bucket_stack(
    stack: &mut ItemStack,
    player: &Player,
    mut filled: ItemStack,
    limit_creative_stack_size: bool,
) {
    player.trigger_advancement(
        crate::entity::player::advancement::trigger::AdvancementTrigger::FilledBucket {
            item_id: filled.item.registry_key.to_string(),
        },
    );
    if limit_creative_stack_size && player.gamemode.load() == GameMode::Creative {
        let has_item = {
            let inv = player
                .inventory
                .main_inventory
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            inv.iter().any(|s| s.are_equal(&filled))
        };
        if has_item {
            return;
        }
        player.inventory.insert_stack_anywhere(&mut filled);
    } else if stack.item_count == 1 && player.gamemode.load() != GameMode::Creative {
        *stack = filled;
    } else {
        stack.decrement_unless_creative(player.gamemode.load(), 1);
        let was_added = player.inventory.insert_stack_anywhere(&mut filled);
        if !was_added && !filled.is_empty() {
            player
                .world()
                .drop_stack(&player.position().to_block_pos(), filled);
        }
    }
}

pub(crate) fn give_filled_mob_bucket(stack: &mut ItemStack, player: &Player, bucket: ItemStack) {
    give_filled_bucket_stack(stack, player, bucket, false);
}

pub(crate) fn fill_bucket_stack(stack: &mut ItemStack, player: &Player, item: &'static Item) {
    give_filled_bucket_stack(stack, player, ItemStack::new(1, item), true);
}

fn fill_mob_bucket_stack(
    stack: &mut ItemStack,
    player: &Player,
    item: &'static Item,
    entity: &dyn EntityBase,
) {
    let mut bucket_stack = ItemStack::new(1, item);
    if let Some(custom_name) = entity.get_entity().custom_name.load().as_ref() {
        bucket_stack.set_data_component(CustomNameImpl {
            name: custom_name.clone(),
        });
    }
    if entity.get_entity().entity_type == &EntityType::SULFUR_CUBE
        && let Some(living) = entity.get_living_entity()
    {
        let body_stack = living
            .entity_equipment
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&EquipmentSlot::BODY);
        if !body_stack.is_empty() {
            bucket_stack.set_data_component(SulfurCubeContentImpl { item: body_stack });
        }
    }
    let mut nbt = NbtCompound::new();
    entity.write_nbt(&mut nbt);
    for key in [
        "id",
        "UUID",
        "Pos",
        "Motion",
        "Rotation",
        "FallDistance",
        "Dimension",
        "PortalCooldown",
    ] {
        nbt.child_tags.remove(key);
    }
    bucket_stack.set_data_component(BucketEntityDataImpl { nbt: Some(nbt) });
    give_filled_bucket_stack(stack, player, bucket_stack, true);
}

fn apply_mob_bucket_components(bucket_stack: &ItemStack, mob: &dyn EntityBase, player: &Player) {
    crate::item::items::spawn_egg::apply_entity_variant(bucket_stack, mob, Some(player));
    if let Some(bucketable) = mob.get_mob().and_then(|mob| mob.as_bucketable()) {
        if let Some(data) = bucket_stack.get_data_component::<BucketEntityDataImpl>()
            && let Some(nbt) = &data.nbt
        {
            bucketable.load_from_bucket_tag(nbt);
        }
        bucketable.set_from_bucket(true);
        return;
    }
    if let Some(bucket_data) = bucket_stack.get_data_component::<BucketEntityDataImpl>()
        && let Some(ref raw_nbt) = bucket_data.nbt
        && !raw_nbt.is_empty()
    {
        let mut nbt = raw_nbt.clone();
        for key in ["id", "UUID", "Pos"] {
            nbt.child_tags.remove(key);
        }
        if !nbt.is_empty() {
            let mut merged = NbtCompound::new();
            mob.write_nbt(&mut merged);
            merged.merge(&nbt);
            mob.read_nbt_non_mut(&merged);
        }
    }
    if let Some(custom_name) = bucket_stack.get_data_component::<CustomNameImpl>() {
        mob.get_entity().set_custom_name(custom_name.name.clone());
    }
    if let Some(sulfur_content) = bucket_stack.get_data_component::<SulfurCubeContentImpl>()
        && !sulfur_content.item.is_empty()
        && let Some(living) = mob.get_living_entity()
    {
        living
            .entity_equipment
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .put(&EquipmentSlot::BODY, sulfur_content.item.clone());
    }
    if let Some(mob_trait) = mob.get_mob() {
        mob_trait
            .get_mob_entity()
            .persistence_required
            .store(true, Ordering::Relaxed);
    }
}

fn give_player_bucket_item(player: &Player, hand: Hand, filled: ItemStack) {
    let mut hand_stack = player.inventory.get_stack_in_hand(hand);
    give_filled_bucket_stack(&mut hand_stack, player, filled, true);
    player.inventory.set_stack_in_hand(hand, hand_stack);
}

/// Resends the stack in `hand`, which the client changed in its own prediction of the bucket use.
fn sync_bucket_hand(player: &Player, hand: Hand) {
    let slot = match hand {
        Hand::Right => player.inventory.get_selected_slot() as usize,
        Hand::Left => PlayerInventory::OFF_HAND_SLOT,
    };
    player.sync_hand_slot(slot, player.inventory.get_stack_in_hand(hand));
}

/// `can_pickup` is asked with the position and the filled bucket right before
/// the world changes; returning false leaves the world as it is.
pub(crate) fn try_pickup_fluid_at(
    world: &Arc<World>,
    block_pos: BlockPos,
    can_pickup: impl FnOnce(BlockPos, &'static Item) -> bool,
) -> Option<&'static Item> {
    let (block, state) = world.get_block_and_state_id(&block_pos);

    if block == &Block::POWDER_SNOW {
        if !can_pickup(block_pos, &Item::POWDER_SNOW_BUCKET) {
            return None;
        }
        world.break_block(
            &block_pos,
            None,
            BlockFlags::NOTIFY_ALL | BlockFlags::SKIP_DROPS,
        );
        return Some(&Item::POWDER_SNOW_BUCKET);
    }

    if block.is_waterlogged(state) {
        if !can_pickup(block_pos, &Item::WATER_BUCKET) {
            return None;
        }
        let state_id = block.set_waterlogged(state, false).unwrap_or(state);
        world.set_block_state(&block_pos, state_id, BlockFlags::NOTIFY_ALL);
        world.schedule_fluid_tick(&Fluid::WATER, block_pos, 5, TickPriority::Normal);
        return Some(&Item::WATER_BUCKET);
    }

    if state == Block::LAVA.default_state.id || state == Block::WATER.default_state.id {
        let item = if state == Block::LAVA.default_state.id {
            &Item::LAVA_BUCKET
        } else {
            &Item::WATER_BUCKET
        };
        if !can_pickup(block_pos, item) {
            return None;
        }
        world.set_block_state(
            &block_pos,
            Block::AIR.default_state.id,
            BlockFlags::NOTIFY_ALL,
        );
        return Some(item);
    }

    None
}

pub(crate) const fn should_evaporate_in_nether(item: &Item, world: &World) -> bool {
    item.id != Item::LAVA_BUCKET.id
        && item.id != Item::POWDER_SNOW_BUCKET.id
        && item.id != Item::SULFUR_CUBE_BUCKET.id
        && world.dimension.water_evaporates
}

pub(crate) fn play_bucket_evaporation(world: &Arc<World>, position: &Vector3<f64>) {
    world.play_sound_raw(
        Sound::BlockFireExtinguish as u16,
        SoundCategory::Blocks,
        position,
        0.5,
        (rand::random::<f32>() - rand::random::<f32>()).mul_add(0.8, 2.6),
    );
}

fn try_place_powder_snow(
    world: &Arc<World>,
    pos: BlockPos,
    direction: BlockDirection,
    can_place: impl FnOnce(BlockPos) -> bool,
) -> Option<BlockPos> {
    let state = world.get_block_state(&pos);
    let target_pos = if state.replaceable() {
        pos
    } else {
        pos.offset(direction.to_offset())
    };
    let target_state = world.get_block_state(&target_pos);
    if !target_state.is_air() && !target_state.is_liquid() && !target_state.replaceable() {
        return None;
    }
    if !can_place(target_pos) {
        return None;
    }
    if !target_state.is_air() && !target_state.is_liquid() {
        world.break_block(&target_pos, None, BlockFlags::NOTIFY_ALL);
    }
    world.set_block_state(
        &target_pos,
        Block::POWDER_SNOW.default_state.id,
        BlockFlags::NOTIFY_ALL,
    );
    world.play_sound(
        Sound::ItemBucketEmptyPowderSnow,
        SoundCategory::Blocks,
        &target_pos.to_f64(),
    );
    world.emit_game_event(GameEvent::FluidPlace.name(), target_pos.to_f64());
    Some(target_pos)
}

/// `can_place` is asked with the position that changes right before the world
/// changes; returning false leaves the world as it is.
fn try_place_liquid(
    world: &Arc<World>,
    item: &Item,
    pos: BlockPos,
    direction: BlockDirection,
    is_sneaking: bool,
    can_place: impl FnOnce(BlockPos) -> bool,
) -> Option<BlockPos> {
    if item.id == Item::SULFUR_CUBE_BUCKET.id {
        let place_pos = pos.offset(direction.to_offset());
        return can_place(place_pos).then_some(place_pos);
    }
    if item.id == Item::POWDER_SNOW_BUCKET.id {
        return try_place_powder_snow(world, pos, direction, can_place);
    }

    let (block, state) = world.get_block_and_state(&pos);
    let is_water = item.id != Item::LAVA_BUCKET.id;
    let can_waterlog_clicked =
        is_water && !is_sneaking && block.is_waterloggable() && !block.is_waterlogged(state.id);

    let place_pos = if can_waterlog_clicked {
        pos
    } else {
        pos.offset(direction.to_offset())
    };

    let (target_block, target_state) = world.get_block_and_state(&place_pos);
    let can_waterlog_target = is_water
        && target_block.is_waterloggable()
        && !target_block.is_waterlogged(target_state.id);
    let can_replace_target =
        target_state.is_air() || target_state.is_liquid() || target_state.replaceable();

    if !can_waterlog_target && !can_replace_target {
        return None;
    }
    if !can_place(place_pos) {
        return None;
    }

    if should_evaporate_in_nether(item, world) {
        play_bucket_evaporation(world, &place_pos.to_f64());
        return Some(place_pos);
    }

    if can_waterlog_target {
        let state_id = target_block
            .set_waterlogged(target_state.id, true)
            .unwrap_or(target_state.id);
        world.set_block_state(&place_pos, state_id, BlockFlags::NOTIFY_ALL);
        world.schedule_fluid_tick(&Fluid::WATER, place_pos, 5, TickPriority::Normal);
        return Some(place_pos);
    }

    if !target_state.is_air() && !target_state.is_liquid() {
        world.break_block(&place_pos, None, BlockFlags::NOTIFY_ALL);
    }
    world.set_block_state(
        &place_pos,
        if item.id == Item::LAVA_BUCKET.id {
            Block::LAVA.default_state.id
        } else {
            Block::WATER.default_state.id
        },
        BlockFlags::NOTIFY_ALL,
    );
    Some(place_pos)
}

/// `can_place` is asked with the position that changes right before the world
/// changes; returning false leaves the world as it is.
pub(crate) fn try_place_filled_bucket(
    world: &Arc<World>,
    item: &Item,
    pos: BlockPos,
    direction: BlockDirection,
    can_place: impl FnOnce(BlockPos) -> bool,
) -> bool {
    try_place_liquid(world, item, pos, direction, false, can_place).is_some()
}

impl ItemBehaviour for EmptyBucketItem {
    fn normal_use(&self, item: &Item, player: &Player) {
        let (yaw, pitch) = player.rotation();
        self.normal_use_with_hand(item, player, yaw, pitch, Hand::Right);
    }

    fn normal_use_with_rotation(&self, item: &Item, player: &Player, yaw: f32, pitch: f32) {
        self.normal_use_with_hand(item, player, yaw, pitch, Hand::Right);
    }

    fn normal_use_with_hand(&self, item: &Item, player: &Player, yaw: f32, pitch: f32, hand: Hand) {
        let world = player.world();
        let (start_pos, end_pos) = self.get_start_and_end_pos_with_rotation(player, yaw, pitch);

        let hit = world.clip(&pumpkin_world::clip::ClipContext::new(
            start_pos,
            end_pos,
            pumpkin_world::clip::ClipBlock::Outline,
            pumpkin_world::clip::ClipFluid::SourceOnly,
        ));
        if hit.is_miss {
            return;
        }

        let block_pos = hit.block_pos;

        let used_stack = player.inventory.get_stack_in_hand(hand);
        let mut result = None;
        let Some(filled_item) = try_pickup_fluid_at(&world, block_pos, |pos, filled_item| {
            result = Some(ItemStack::new(1, filled_item));
            if let Some(server) = world.server.upgrade()
                && let Some(player_arc) = world.get_player_by_uuid(player.gameprofile.id)
            {
                let mut event =
                    crate::plugin::api::events::player::player_bucket::PlayerBucketFillEvent::new(
                        player_arc,
                        pos,
                        filled_item.registry_key.to_string(),
                        hand,
                        used_stack.clone(),
                        result.take(),
                    );
                server.plugin_manager.fire_blocking(&server, &mut event);
                if event.cancelled {
                    // The client already shows the filled bucket.
                    sync_bucket_hand(player, hand);
                    return false;
                }
                result = event.result;
            }
            true
        }) else {
            return;
        };

        world.play_sound(
            get_fill_sound(filled_item),
            SoundCategory::Blocks,
            &block_pos.to_f64(),
        );
        world.emit_game_event(GameEvent::FluidPickup.name(), block_pos.to_f64());
        player.increment_stat(StatisticCategory::Used, i32::from(item.id), 1);

        let Some(filled) = result else {
            // A plugin left the hand as it is; the client already shows the filled bucket.
            sync_bucket_hand(player, hand);
            return;
        };
        give_player_bucket_item(player, hand, filled);
    }

    fn use_on_entity(&self, item: &mut ItemStack, player: &Player, entity: Arc<dyn EntityBase>) {
        let ent = entity.get_entity();
        let entity_type = ent.entity_type;
        if entity_type == &EntityType::SULFUR_CUBE && ent.age.load(Ordering::Relaxed) >= 0 {
            let world = ent.world.load();
            world.play_sound(
                Sound::ItemBucketFillSulfurCube,
                SoundCategory::Neutral,
                &ent.pos.load(),
            );
            fill_mob_bucket_stack(item, player, &Item::SULFUR_CUBE_BUCKET, entity.as_ref());
            ent.remove();
        } else if (entity_type == &EntityType::MOOSHROOM || entity_type == &EntityType::GOAT)
            && ent.age.load(Ordering::Relaxed) >= 0
        {
            let world = ent.world.load();
            let sound = if entity_type == &EntityType::GOAT {
                if let Some(goat) = entity
                    .cast_any()
                    .downcast_ref::<crate::entity::passive::goat::GoatEntity>()
                    && goat.is_screaming()
                {
                    Sound::EntityGoatScreamingMilk
                } else {
                    Sound::EntityGoatMilk
                }
            } else {
                Sound::EntityCowMilk
            };
            world.play_sound(sound, SoundCategory::Neutral, &ent.pos.load());
            fill_bucket_stack(item, player, &Item::MILK_BUCKET);
        }
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

impl ItemBehaviour for FilledBucketItem {
    fn normal_use(&self, item: &Item, player: &Player) {
        let (yaw, pitch) = player.rotation();
        self.normal_use_with_hand(item, player, yaw, pitch, Hand::Right);
    }

    fn normal_use_with_rotation(&self, item: &Item, player: &Player, yaw: f32, pitch: f32) {
        self.normal_use_with_hand(item, player, yaw, pitch, Hand::Right);
    }

    fn normal_use_with_hand(&self, item: &Item, player: &Player, yaw: f32, pitch: f32, hand: Hand) {
        let world = player.world();
        let (start_pos, end_pos) = self.get_start_and_end_pos_with_rotation(player, yaw, pitch);
        let hit = world.clip(&pumpkin_world::clip::ClipContext::new(
            start_pos,
            end_pos,
            pumpkin_world::clip::ClipBlock::Outline,
            pumpkin_world::clip::ClipFluid::None,
        ));
        if hit.is_miss {
            return;
        }

        let pos = hit.block_pos;
        let direction = hit.direction;
        let is_sneaking = player.get_entity().is_sneaking();

        let used_stack = player.inventory.get_stack_in_hand(hand);
        // Players with infinite materials keep the filled bucket.
        let infinite_materials = player.gamemode.load() == GameMode::Creative;
        let mut result = (!infinite_materials).then(|| ItemStack::new(1, &Item::BUCKET));
        let Some(place_pos) = try_place_liquid(
            &world,
            item,
            pos,
            direction,
            is_sneaking,
            |place_pos| {
                if let Some(server) = world.server.upgrade()
                    && let Some(player_arc) = world.get_player_by_uuid(player.gameprofile.id)
                {
                    let mut event =
                        crate::plugin::api::events::player::player_bucket::PlayerBucketEmptyEvent::new(
                            player_arc,
                            place_pos,
                            item.registry_key.to_string(),
                            hand,
                            used_stack.clone(),
                            result.take(),
                        );
                    server.plugin_manager.fire_blocking(&server, &mut event);
                    if event.cancelled {
                        // The client already shows the empty bucket.
                        sync_bucket_hand(player, hand);
                        return false;
                    }
                    result = event.result;
                }
                true
            },
        ) else {
            return;
        };

        if item.id != Item::POWDER_SNOW_BUCKET.id && !should_evaporate_in_nether(item, &world) {
            let sound_category = if get_mob_for_bucket(item).is_some() {
                SoundCategory::Neutral
            } else {
                SoundCategory::Blocks
            };

            world.play_sound(get_empty_sound(item), sound_category, &place_pos.to_f64());
            world.emit_game_event(GameEvent::FluidPlace.name(), place_pos.to_f64());
        } else if get_mob_for_bucket(item).is_some() && should_evaporate_in_nether(item, &world) {
            world.play_sound(
                get_empty_sound(item),
                SoundCategory::Neutral,
                &place_pos.to_f64(),
            );
            world.emit_game_event(GameEvent::EntityPlace.name(), place_pos.to_f64());
        }

        if let Some((entity_type, _)) = get_mob_for_bucket(item) {
            let spawn_coord = Vector3::new(
                f64::from(place_pos.0.x) + 0.5,
                f64::from(place_pos.0.y),
                f64::from(place_pos.0.z) + 0.5,
            );
            let mob = from_type(entity_type, spawn_coord, &world, Uuid::new_v4());
            if mob.get_mob().and_then(|mob| mob.as_bucketable()).is_some() {
                crate::entity::mob::spawn::finalize_spawn(&mob, &world, None);
                apply_mob_bucket_components(&used_stack, mob.as_ref(), player);
                if world.spawn_creature(
                    mob.clone(),
                    crate::plugin::api::events::entity::creature_spawn::CreatureSpawnReason::Bucket,
                    world.get_player_by_uuid(player.gameprofile.id),
                ) && let Some(fish) = mob.get_mob().and_then(|mob| mob.as_fish())
                {
                    fish.play_ambient_sound();
                }
            } else {
                apply_mob_bucket_components(&used_stack, mob.as_ref(), player);
                world.spawn_entity(mob);
            }
        }

        player.increment_stat(StatisticCategory::Used, i32::from(item.id), 1);

        let Some(result) = result else {
            if !infinite_materials {
                // A plugin kept the filled bucket; the client already shows it emptied.
                sync_bucket_hand(player, hand);
            }
            return;
        };
        let mut updated_stack = player.inventory.get_stack_in_hand(hand);
        give_filled_bucket_stack(&mut updated_stack, player, result, false);
        player.inventory.set_stack_in_hand(hand, updated_stack);
    }

    fn use_on_entity(&self, item: &mut ItemStack, player: &Player, entity: Arc<dyn EntityBase>) {
        if entity
            .get_mob()
            .and_then(|mob| mob.as_bucketable())
            .is_some()
        {
            return;
        }
        if item.item.id == Item::WATER_BUCKET.id {
            let entity_type = entity.get_entity().entity_type;
            let result_item = if entity_type == &EntityType::AXOLOTL {
                Some((&Item::AXOLOTL_BUCKET, Sound::ItemBucketFillAxolotl))
            } else if entity_type == &EntityType::TADPOLE {
                Some((&Item::TADPOLE_BUCKET, Sound::ItemBucketFillTadpole))
            } else {
                None
            };

            if let Some((mob_bucket, sound)) = result_item {
                let ent = entity.get_entity();
                let world = ent.world.load();
                world.play_sound(sound, SoundCategory::Neutral, &ent.pos.load());
                fill_mob_bucket_stack(item, player, mob_bucket, entity.as_ref());
                ent.remove();
            }
        }
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

impl ItemBehaviour for MilkBucketItem {
    fn normal_use(&self, _item: &Item, _player: &Player) {
        // Drinking is handled by the consumable flow in the server (active hand + consumption tick).
    }

    fn get_use_duration(&self) -> i32 {
        32
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}
