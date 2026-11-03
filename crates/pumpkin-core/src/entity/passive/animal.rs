use std::sync::Arc;

use pumpkin_data::dye_color::DyeColor;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::particle::Particle;
use pumpkin_data::sound::{Sound, SoundCategory};

use crate::entity::{EntityBase, mob::Mob, player::Player, r#type::from_type};
use crate::world::World;
use pumpkin_data::tag;
use pumpkin_inventory::Inventory;
use pumpkin_inventory::crafting::crafting_inventory::CraftingInventory;
use pumpkin_inventory::crafting::crafting_screen_handler::match_crafting_recipe;
use pumpkin_inventory::crafting::recipe_provider::RecipeProvider;
use pumpkin_protocol::bedrock::server::actor_event::ActorEventID;
use pumpkin_util::math::vector3::Vector3;
use rand::RngExt;

pub trait Animal: Mob {
    fn is_food(&self, item_stack: &ItemStack) -> bool;

    fn get_breed_offspring(&self, _partner: &dyn EntityBase) -> Arc<dyn EntityBase> {
        let entity = self.get_entity();
        from_type(
            entity.entity_type,
            entity.pos.load(),
            &entity.world.load(),
            uuid::Uuid::new_v4(),
        )
    }

    /// Vanilla `canMate` past the shared checks (another animal of this type, both in love):
    /// species add their own conditions, such as both being tame.
    fn can_mate(&self, _partner: &dyn EntityBase) -> bool {
        true
    }

    /// Vanilla `TurtleBreedGoal.breed`: `true` when breeding gives this animal an egg to lay
    /// instead of a baby; it is told so by [`Self::on_bred_egg`].
    fn breeds_eggs(&self) -> bool {
        false
    }

    fn on_bred_egg(&self) {}

    /// Vanilla `Animal.getWalkTargetValue`: grass below is worth 10, anything else the light
    /// cost.
    fn animal_walk_target_value(
        &self,
        pos: &pumpkin_util::math::position::BlockPos,
        level: &dyn crate::world::level_accessor::LevelAccessor,
    ) -> f32 {
        if level.get_block(&pos.down()).id == pumpkin_data::Block::GRASS_BLOCK.id {
            return 10.0;
        }
        level.get_pathfinding_cost_from_light_levels(pos)
    }

    fn play_eating_sound(&self, sound: Sound) {
        let mob_entity = self.get_mob_entity();
        let entity = &mob_entity.living_entity.entity;
        let world = entity.world.load();
        world.play_sound(sound, SoundCategory::Neutral, &entity.pos.load());
    }

    fn write_animal_nbt(&self, nbt: &mut pumpkin_nbt::compound::NbtCompound) {
        let mob_entity = self.get_mob_entity();
        let in_love = mob_entity
            .love_ticks
            .load(std::sync::atomic::Ordering::Relaxed);
        nbt.put_int("InLove", in_love);
        if let Some(uuid) = mob_entity.breeder.load() {
            nbt.put_uuid("LoveCause", uuid);
        }
    }

    fn read_animal_nbt(&self, nbt: &pumpkin_nbt::compound::NbtCompound) {
        let mob_entity = self.get_mob_entity();
        let in_love = nbt.get_int("InLove").unwrap_or(0);
        let love_cause = nbt.get_uuid("LoveCause");
        mob_entity.set_love_ticks(in_love, love_cause);
    }

    /// Vanilla `Animal.usePlayerItem`: feeding uses up one of the stack.
    fn use_player_item(&self, player: &Player, item_stack: &mut ItemStack) {
        item_stack.decrement_unless_creative(player.gamemode.load(), 1);
    }

    fn animal_interact(
        &self,
        player: &Arc<Player>,
        item_stack: &mut ItemStack,
        ambient_sound: Sound,
    ) -> bool {
        let mob_entity = self.get_mob_entity();
        if self.is_food(item_stack) {
            let age = mob_entity
                .living_entity
                .entity
                .age
                .load(std::sync::atomic::Ordering::Relaxed);

            if age >= 0 && mob_entity.is_breeding_ready() && !mob_entity.is_in_love() {
                self.use_player_item(player, item_stack);

                let Some(ticks) = self.approve_love(Some(player.entity_id()), 600) else {
                    return true;
                };
                mob_entity.set_love_ticks(ticks, Some(player.gameprofile.id));
                let entity = &mob_entity.living_entity.entity;
                let world = entity.world.load();
                let pos = entity.pos.load();

                world.send_entity_status(
                    entity,
                    pumpkin_data::entity::EntityStatus::InLoveHearts,
                    ActorEventID::InLoveHearts,
                );

                world.spawn_particle(
                    pos + Vector3::new(0.0, f64::from(entity.height()), 0.0),
                    Vector3::new(0.5, 0.5, 0.5),
                    1.0,
                    7,
                    Particle::Heart,
                );
                world.play_sound(ambient_sound, SoundCategory::Neutral, &entity.pos.load());
                return true;
            }

            if age < 0 {
                self.use_player_item(player, item_stack);
                let speedup = (-age / 10).max(1);
                mob_entity
                    .living_entity
                    .entity
                    .age
                    .fetch_add(speedup, std::sync::atomic::Ordering::Relaxed);

                let entity = &mob_entity.living_entity.entity;
                let world = entity.world.load();
                let pos = entity.pos.load();

                world.spawn_particle(
                    pos + Vector3::new(0.0, f64::from(entity.height()), 0.0),
                    Vector3::new(0.5, 0.5, 0.5),
                    1.0,
                    7,
                    Particle::HappyVillager,
                );
                self.play_eating_sound(ambient_sound);
                return true;
            }
        }

        mob_entity.mob_interact(player, item_stack, self.can_be_leashed())
    }
}

#[must_use]
pub fn get_dye_color_from_item(item: &pumpkin_data::item::Item) -> Option<u8> {
    use pumpkin_data::data_component::DataComponent;
    use pumpkin_data::data_component_impl::{DyeImpl, get};

    for (id, component) in item.components {
        if *id == DataComponent::Dye {
            return Some(get::<DyeImpl>(*component).color.id());
        }
    }
    None
}

fn find_color_mix_in_recipes(
    world: &World,
    color1: DyeColor,
    color2: DyeColor,
) -> Option<DyeColor> {
    let dye_items = |color: DyeColor| {
        tag::Item::C_DYES
            .1
            .iter()
            .filter_map(|id| Item::from_id(*id))
            .filter(move |item| get_dye_color_from_item(item) == Some(color.id()))
    };
    let server = world.server.upgrade();
    let provider = server
        .as_ref()
        .map(|server| server.recipe_manager.as_ref() as &dyn RecipeProvider);
    for dye1 in dye_items(color1) {
        for dye2 in dye_items(color2) {
            let input = CraftingInventory::new(2, 1);
            input.set_stack(0, ItemStack::new(1, dye1));
            input.set_stack(1, ItemStack::new(1, dye2));
            if let Some(result) = match_crafting_recipe(&input, provider)
                && let Some(item) = Item::from_registry_key(&result.item_id)
                && let Some(color) = get_dye_color_from_item(item)
            {
                return Some(DyeColor::from(color));
            }
        }
    }
    None
}

/// Vanilla `DyeColor.getMixedColor`: the dye two dyes craft into, else one of them.
#[must_use]
pub fn get_mixed_color(world: &World, color1: DyeColor, color2: DyeColor) -> DyeColor {
    find_color_mix_in_recipes(world, color1, color2).unwrap_or_else(|| {
        if rand::rng().random::<bool>() {
            color1
        } else {
            color2
        }
    })
}
