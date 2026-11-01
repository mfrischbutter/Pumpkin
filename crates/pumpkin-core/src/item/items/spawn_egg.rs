use crate::block::registry::BlockActionResult;
use std::sync::Arc;

use crate::block::entities::mob_spawner::MobSpawnerBlockEntity;
use crate::entity::EntityBase;
use crate::entity::mob::spawn::{EntitySpawnReason, create_at, finalize_spawn};
use crate::entity::player::Player;
use crate::entity::r#type::from_type;
use crate::item::{ItemBehaviour, ItemMetadata};
use crate::plugin::api::events::entity::creature_spawn::CreatureSpawnReason;
use crate::server::Server;
use crate::world::World;
use pumpkin_data::data_component::DataComponent;
use pumpkin_data::data_component_impl::{
    AxolotlVariantImpl, CatCollarImpl, CatSoundVariantImpl, CatVariantImpl,
    ChickenSoundVariantImpl, ChickenVariantImpl, CowSoundVariantImpl, CowVariantImpl,
    CustomNameImpl, EntityDataImpl, FoxVariantImpl, FrogVariantImpl, HorseVariantImpl,
    LlamaVariantImpl, MooshroomVariantImpl, ParrotVariantImpl, PigSoundVariantImpl, PigVariantImpl,
    RabbitVariantImpl, SalmonSizeImpl, SheepColorImpl, ShulkerColorImpl, TropicalFishBaseColorImpl,
    TropicalFishPatternColorImpl, TropicalFishPatternImpl, VillagerVariantImpl, WolfCollarImpl,
    WolfSoundVariantImpl, WolfVariantImpl, ZombieNautilusVariantImpl,
};
use pumpkin_data::dye_color::DyeColor;
use pumpkin_data::entity::EntityType;
use pumpkin_data::entity::entity_from_egg;
use pumpkin_data::game_event::GameEvent;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::translation;
use pumpkin_data::{Block, BlockDirection};
use pumpkin_inventory::player::player_inventory::PlayerInventory;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_util::permission::PermissionLvl;
use pumpkin_util::text::TextComponent;
use pumpkin_util::{Difficulty, Hand};
use uuid::Uuid;

pub struct SpawnEggItem;

impl ItemMetadata for SpawnEggItem {
    fn ids() -> Box<[u16]> {
        pumpkin_data::entity::spawn_egg_ids()
    }
}

/// Vanilla `EntityType.canSpawn`: a type not allowed in peaceful needs another difficulty.
pub(crate) fn can_spawn(entity_type: &EntityType, world: &World) -> bool {
    entity_type.is_allowed_in_peaceful || world.level_info.load().difficulty != Difficulty::Peaceful
}

/// Vanilla's `EntityTypes.OP_ONLY_CUSTOM_DATA`: types whose `entity_data` only a server
/// operator may set. None of them has a spawn egg today, but a datapack-authored loot table
/// can still hand out an item with this component, so the gate is checked here too.
fn only_op_can_set_nbt(entity_type: &EntityType) -> bool {
    std::ptr::eq(entity_type, &EntityType::FALLING_BLOCK)
        || std::ptr::eq(entity_type, &EntityType::COMMAND_BLOCK_MINECART)
        || std::ptr::eq(entity_type, &EntityType::SPAWNER_MINECART)
}

/// Permission node for op-only `entity_data`, so permission plugins can grant or deny it.
pub const NBT_PLACE_PERMISSION: &str = "minecraft:nbt.place";

/// Vanilla `PlayerList.isOp`: any ops list entry, whatever its level, passes the node's
/// `Op(One)` default, so without plugins or attachments the result matches vanilla.
fn can_place_op_nbt(player: &Player) -> bool {
    let world = player.world();
    let Some(server) = world.server.upgrade() else {
        return false;
    };
    let Some(player) = world.get_player_by_uuid(player.gameprofile.id) else {
        return false;
    };
    let is_op = server
        .data
        .operator_config
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get_entry(&player.gameprofile.id)
        .is_some();
    let level = if is_op {
        PermissionLvl::Four
    } else {
        PermissionLvl::Zero
    };
    player.has_permission_at_level(&server, NBT_PLACE_PERMISSION, level)
}

/// Loads the stack's `entity_data` NBT into the mob. Identity and placement stay as spawned.
///
/// `user` is the player who caused the spawn, matching vanilla's `updateCustomEntityTag` user
/// argument; it is `None` for non-player sources such as a dispenser.
fn apply_entity_data(item: &ItemStack, mob: &dyn EntityBase, user: Option<&Player>) {
    let Some(nbt) = item
        .get_data_component::<EntityDataImpl>()
        .and_then(|comp| comp.nbt.as_ref())
    else {
        return;
    };
    let entity_type = mob.get_entity().entity_type;
    // Vanilla EntityType.updateCustomEntityTag loads the data only into the type it names.
    if let Some(id) = nbt.get_string("id")
        && id.strip_prefix("minecraft:").unwrap_or(id) != entity_type.resource_name
    {
        return;
    }
    if only_op_can_set_nbt(entity_type) && !user.is_some_and(can_place_op_nbt) {
        return;
    }
    // Vanilla `TypedEntityData.loadInto` keeps the entity's own UUID; a `Pos` in the data
    // moves the mob like any loaded position.
    let mut nbt = nbt.clone();
    for key in ["id", "UUID"] {
        nbt.child_tags.remove(key);
    }
    if !nbt.is_empty() {
        // Vanilla TypedEntityData.loadInto merges into the mob's own save so unset keys keep their state.
        let mut merged = NbtCompound::new();
        mob.write_nbt(&mut merged);
        merged.merge(&nbt);
        mob.read_nbt_non_mut(&merged);
    }
}

/// Vanilla `EntityType.appendDefaultStackConfig`: item components first, `entity_data` last.
pub(crate) fn apply_entity_variant(item: &ItemStack, mob: &dyn EntityBase, user: Option<&Player>) {
    apply_variant(item, mob);
    apply_entity_data(item, mob, user);
}

fn apply_variant(item: &ItemStack, mob: &dyn EntityBase) {
    if let Some(custom_name) = item.get_data_component::<CustomNameImpl>() {
        mob.get_entity().set_custom_name(custom_name.name.clone());
    }
    macro_rules! apply_variant {
        ($($ty:ty),+ $(,)?) => {
            $(
                if let Some(comp) = item.get_data_component::<$ty>() {
                    mob.set_variant_name(&comp.value);
                }
            )+
        };
    }
    apply_variant!(
        ChickenVariantImpl,
        FrogVariantImpl,
        WolfVariantImpl,
        CatVariantImpl,
        VillagerVariantImpl,
        FoxVariantImpl,
        MooshroomVariantImpl,
        RabbitVariantImpl,
        PigVariantImpl,
        CowVariantImpl,
        HorseVariantImpl,
        LlamaVariantImpl,
        AxolotlVariantImpl,
        SheepColorImpl,
        ShulkerColorImpl,
        ZombieNautilusVariantImpl,
        SalmonSizeImpl,
        ParrotVariantImpl,
    );

    macro_rules! apply_component {
        ($($ty:ty => $component:ident),+ $(,)?) => {
            $(
                if let Some(comp) = item.get_data_component::<$ty>() {
                    mob.apply_implicit_component(DataComponent::$component, &comp.value);
                }
            )+
        };
    }
    apply_component!(
        TropicalFishPatternImpl => TropicalFishPattern,
        TropicalFishBaseColorImpl => TropicalFishBaseColor,
        TropicalFishPatternColorImpl => TropicalFishPatternColor,
    );

    macro_rules! apply_sound_variant {
        ($($ty:ty),+ $(,)?) => {
            $(
                if let Some(comp) = item.get_data_component::<$ty>() {
                    mob.set_sound_variant_name(&comp.value);
                }
            )+
        };
    }
    apply_sound_variant!(
        WolfSoundVariantImpl,
        CatSoundVariantImpl,
        PigSoundVariantImpl,
        CowSoundVariantImpl,
        ChickenSoundVariantImpl,
    );

    if let Some(comp) = item.get_data_component::<WolfCollarImpl>()
        && let Some(color) =
            DyeColor::by_name(comp.value.strip_prefix("minecraft:").unwrap_or(&comp.value))
        && let Some(wolf) = mob
            .cast_any()
            .downcast_ref::<crate::entity::passive::wolf::WolfEntity>()
    {
        wolf.set_collar_color(color.id());
    }
    if let Some(comp) = item.get_data_component::<CatCollarImpl>()
        && let Some(color) =
            DyeColor::by_name(comp.value.strip_prefix("minecraft:").unwrap_or(&comp.value))
        && let Some(cat) = mob
            .cast_any()
            .downcast_ref::<crate::entity::passive::cat::CatEntity>()
    {
        cat.set_collar_color(color.id());
    }
}

/// Finalizes a mob spawned by a spawn egg, then applies the egg's components (vanilla order).
pub(crate) fn prepare_egg_mob(
    item: &ItemStack,
    mob: &Arc<dyn EntityBase>,
    world: &Arc<World>,
    user: Option<&Player>,
    spawn_reason: EntitySpawnReason,
) {
    finalize_spawn(mob, world, spawn_reason, None);
    apply_entity_variant(item, mob.as_ref(), user);
}

/// Vanilla `EntityType.spawn(level, itemStack, user, pos, reason, tryMoveDown, movedUp)`: the
/// mob created at the spot, finalized and configured from the item, added to the world and
/// making its ambient sound; `None` when a plugin cancelled the spawn.
#[expect(clippy::too_many_arguments)]
pub(crate) fn spawn_from_item(
    entity_type: &'static EntityType,
    item: &ItemStack,
    world: &Arc<World>,
    user: Option<&Player>,
    spawn_pos: &BlockPos,
    try_move_down: bool,
    moved_up: bool,
    spawn_reason: EntitySpawnReason,
    reason: CreatureSpawnReason,
) -> Option<Arc<dyn EntityBase>> {
    let mob = create_at(entity_type, world, spawn_pos, try_move_down, moved_up);
    prepare_egg_mob(item, &mob, world, user, spawn_reason);
    let player = user.and_then(|user| world.get_player_by_uuid(user.gameprofile.id));
    if !world.spawn_creature(mob.clone(), reason, player) {
        return None;
    }
    if let Some(mob) = mob.get_mob() {
        mob.play_ambient_sound();
    }
    Some(mob)
}

impl ItemBehaviour for SpawnEggItem {
    fn normal_use(&self, item: &Item, player: &Player) {
        let (yaw, pitch) = player.rotation();
        self.normal_use_with_hand(item, player, yaw, pitch, Hand::Right);
    }

    fn normal_use_with_hand(&self, item: &Item, player: &Player, yaw: f32, pitch: f32, hand: Hand) {
        if let Some(entity_type) = entity_from_egg(item.id) {
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

            let hit_pos = hit.block_pos;
            let hit_block = world.get_block(&hit_pos);
            if hit_block != &Block::WATER && hit_block != &Block::LAVA {
                return;
            }
            if !can_spawn(entity_type, &world) {
                return;
            }

            let mut stack = player.inventory.get_stack_in_hand(hand);
            if stack.is_empty() || stack.item.id != item.id {
                return;
            }

            // Vanilla `spawnMob(type, player, stack, level, pos, false, false)`.
            if spawn_from_item(
                entity_type,
                &stack,
                &world,
                Some(player),
                &hit_pos,
                false,
                false,
                EntitySpawnReason::SpawnItemUse,
                CreatureSpawnReason::SpawnerEgg,
            )
            .is_none()
            {
                return;
            }

            stack.decrement_unless_creative(player.gamemode.load(), 1);
            player.inventory.set_stack_in_hand(hand, stack.clone());
            let slot_index = match hand {
                Hand::Right => player.inventory.get_selected_slot() as usize,
                Hand::Left => PlayerInventory::OFF_HAND_SLOT,
            };
            player.sync_hand_slot(slot_index, stack);
            world.emit_game_event(GameEvent::EntityPlace.name(), hit_pos.to_centered_f64());
        }
    }

    fn use_on_block(
        &self,
        item: &mut ItemStack,
        player: &Player,
        location: BlockPos,
        face: BlockDirection,
        _cursor_pos: Vector3<f32>,
        _block: &Block,
        _server: &Server,
    ) -> BlockActionResult {
        if let Some(entity_type) = entity_from_egg(item.item.id) {
            let world = player.world();
            if !can_spawn(entity_type, &world) {
                return BlockActionResult::Fail;
            }

            if let Some(block_entity) = world.get_block_entity(&location) {
                let spawner = block_entity
                    .as_any()
                    .downcast_ref::<MobSpawnerBlockEntity>();
                let trial_spawner = block_entity
                    .as_any()
                    .downcast_ref::<crate::block::entities::trial_spawner::TrialSpawnerBlockEntity>(
                );
                if spawner.is_some() || trial_spawner.is_some() {
                    // Vanilla: the egg sets the spawner's entity while spawner blocks work; an
                    // operator's egg with entity data sets the data too.
                    if !world.level_info.load().game_rules.spawner_blocks_work {
                        player.send_system_message(&TextComponent::translate(
                            translation::java::ADVMODE_NOTENABLED_SPAWNER,
                            Vec::new(),
                        ));
                        return BlockActionResult::Fail;
                    }
                    let entity_data = item
                        .get_data_component::<EntityDataImpl>()
                        .and_then(|data| data.nbt.clone())
                        .filter(|_| can_place_op_nbt(player));
                    if let Some(spawner) = spawner {
                        match &entity_data {
                            Some(data) => spawner.set_entity_data(entity_type, data),
                            None => spawner.set_entity_id(entity_type),
                        }
                    } else if let Some(trial_spawner) = trial_spawner {
                        trial_spawner.set_entity_type(entity_type, &world);
                    }
                    world.update_block_entity(&block_entity);
                    world.emit_game_event(GameEvent::BlockChange.name(), location.to_f64());
                    // Vanilla `itemStack.shrink(1)`: the egg goes, in creative too.
                    item.decrement(1);
                    return BlockActionResult::Success;
                }
            }

            let target_state = world.get_block_state(&location);
            let spawn_block_pos = if target_state.collision_shapes.is_empty() {
                location
            } else {
                BlockPos(location.0 + face.to_offset())
            };
            // Vanilla `spawnMob(..., tryMoveDown, movedUp)`: the mob sinks onto the block, two
            // blocks far when the egg went up a top face.
            let moved_up = spawn_block_pos != location && face == BlockDirection::Up;
            if spawn_from_item(
                entity_type,
                item,
                &world,
                Some(player),
                &spawn_block_pos,
                true,
                moved_up,
                EntitySpawnReason::SpawnItemUse,
                CreatureSpawnReason::SpawnerEgg,
            )
            .is_none()
            {
                return BlockActionResult::Fail;
            }
            item.decrement_unless_creative(player.gamemode.load(), 1);
            world.emit_game_event(
                GameEvent::EntityPlace.name(),
                spawn_block_pos.to_centered_f64(),
            );
            BlockActionResult::Success
        } else {
            BlockActionResult::Pass
        }
    }

    /// Vanilla `SpawnEggItem.spawnOffspringFromSpawnEgg`: an egg on a mob of its type
    /// breeds a baby at the parent's spot.
    fn use_on_entity(&self, item: &mut ItemStack, player: &Player, entity: Arc<dyn EntityBase>) {
        let Some(entity_type) = entity_from_egg(item.item.id) else {
            return;
        };
        if entity.get_entity().entity_type.id != entity_type.id {
            return;
        }
        let Some(parent) = entity.get_mob() else {
            return;
        };
        let world = player.world();
        let pos = entity.get_entity().pos.load();
        // An ageable parent breeds its offspring; any other type is created anew, which
        // peaceful refuses for a type not allowed in it.
        let offspring = if let Some(animal) = parent.as_animal() {
            animal.get_breed_offspring(entity.as_ref())
        } else {
            if !can_spawn(entity_type, &world) {
                return;
            }
            from_type(entity_type, pos, &world, Uuid::new_v4())
        };
        // `setBaby(true)` then `isBaby`: no baby form, no offspring.
        if !offspring
            .get_mob()
            .is_some_and(crate::entity::mob::Mob::spawn_as_baby)
        {
            return;
        }
        offspring.get_entity().set_pos(pos);
        offspring.get_entity().set_rotation(0.0, 0.0);
        // Vanilla `applyComponentsFromItemStack`: the components, not the entity data.
        apply_variant(item, offspring.as_ref());
        if world.spawn_creature(
            offspring,
            CreatureSpawnReason::SpawnerEgg,
            world.get_player_by_uuid(player.gameprofile.id),
        ) {
            item.decrement_unless_creative(player.gamemode.load(), 1);
        }
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}
