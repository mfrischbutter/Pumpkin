use std::sync::{
    Arc, Weak,
    atomic::{AtomicBool, AtomicI32, AtomicU8, Ordering},
};

use pumpkin_data::BlockState;
use pumpkin_data::attributes::Attributes;
use pumpkin_data::damage::DamageType;
use pumpkin_data::data_component_impl::EquipmentSlot;
use pumpkin_data::entity::EntityType;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::Sound;
use pumpkin_data::tag::{self, Taggable};
use pumpkin_data::tracked_data::panda as data;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use rand::RngExt;

use crate::entity::ai::goal::avoid_entity::{AvoidEntityGoal, not_spectator};
use crate::entity::ai::goal::melee_attack::MeleeAttackGoal;
use crate::entity::ai::goal::revenge::RevengeGoal;
use crate::entity::ai::target_match::TargetMatch;
use crate::entity::custom_sound::CustomSound;
use crate::entity::item::ItemEntity;
use crate::entity::{
    Entity, EntityBase,
    ageable::{AgeableData, AgeableMob},
    ai::goal::{
        breed::BreedGoal, escape_danger::EscapeDangerGoal, follow_parent::FollowParentGoal,
        look_around::RandomLookAroundGoal, look_at_entity::LookAtEntityGoal, swim::SwimGoal,
        tempt::TemptGoal, wander_around::WanderAroundGoal,
    },
    mob::{Mob, MobEntity, spawn::SpawnGroupData},
    passive::animal::Animal,
    player::Player,
};
use crate::world::World;
use crate::world::loot::{LootContextParameters, generate_loot_from_handle};

mod goals;

use goals::{
    PandaBreedGoal, PandaLieOnBackGoal, PandaLookAtPlayerGoal, PandaMoveControl, PandaPanicGoal,
    PandaRollGoal, PandaSitGoal, PandaSneezeGoal,
};

pub const FLAG_SNEEZE: u8 = 2;
pub const FLAG_ROLL: u8 = 4;
pub const FLAG_SIT: u8 = 8;
pub const FLAG_ON_BACK: u8 = 16;

/// Vanilla `Panda.TOTAL_ROLL_STEPS`.
const TOTAL_ROLL_STEPS: i32 = 32;
/// Vanilla `Panda.TOTAL_UNHAPPY_TIME`.
const TOTAL_UNHAPPY_TIME: i32 = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u8)]
pub enum PandaGene {
    #[default]
    Normal = 0,
    Lazy = 1,
    Worried = 2,
    Playful = 3,
    Brown = 4,
    Weak = 5,
    Aggressive = 6,
}

impl PandaGene {
    #[must_use]
    pub const fn from_id(id: u8) -> Self {
        match id {
            1 => Self::Lazy,
            2 => Self::Worried,
            3 => Self::Playful,
            4 => Self::Brown,
            5 => Self::Weak,
            6 => Self::Aggressive,
            _ => Self::Normal,
        }
    }

    #[must_use]
    pub const fn id(self) -> u8 {
        self as u8
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::Lazy => "lazy",
            Self::Worried => "worried",
            Self::Playful => "playful",
            Self::Brown => "brown",
            Self::Weak => "weak",
            Self::Aggressive => "aggressive",
        }
    }

    #[must_use]
    pub fn from_name(name: &str) -> Self {
        match name {
            "lazy" => Self::Lazy,
            "worried" => Self::Worried,
            "playful" => Self::Playful,
            "brown" => Self::Brown,
            "weak" => Self::Weak,
            "aggressive" => Self::Aggressive,
            _ => Self::Normal,
        }
    }

    /// Vanilla `Gene.isRecessive`: brown and weak only show when both genes have them.
    #[must_use]
    pub const fn is_recessive(self) -> bool {
        matches!(self, Self::Brown | Self::Weak)
    }

    /// Vanilla `Gene.getRandom`: 1 in 16 each for lazy, worried, playful and aggressive, 5
    /// weak, 2 brown, the rest normal.
    #[must_use]
    pub fn random_gene(rng: &mut impl rand::Rng) -> Self {
        match rng.random_range(0..16) {
            0 => Self::Lazy,
            1 => Self::Worried,
            2 => Self::Playful,
            4 => Self::Aggressive,
            n if n < 9 => Self::Weak,
            n if n < 11 => Self::Brown,
            _ => Self::Normal,
        }
    }

    /// Vanilla `Gene.getVariantFromGenes`.
    #[must_use]
    pub fn variant(main: Self, hidden: Self) -> Self {
        if main.is_recessive() && main != hidden {
            Self::Normal
        } else {
            main
        }
    }
}

pub struct PandaEntity {
    pub mob_entity: MobEntity,
    pub ageable_data: AgeableData,
    pub main_gene: AtomicU8,
    pub hidden_gene: AtomicU8,
    pub flags: AtomicU8,
    pub eat_counter: AtomicI32,
    pub sneeze_counter: AtomicI32,
    pub unhappy_counter: AtomicI32,
    roll_counter: AtomicI32,
    roll_delta: crossbeam::atomic::AtomicCell<Vector3<f64>>,
    got_bamboo: AtomicBool,
    did_bite: AtomicBool,
}

fn as_panda(mob: &dyn Mob) -> Option<&PandaEntity> {
    mob.cast_any().downcast_ref::<PandaEntity>()
}

/// Vanilla `Panda.canPickUpAndEat`.
fn can_pick_up_and_eat(item: &ItemEntity) -> bool {
    item.get_entity().is_alive()
        && !item.has_pickup_delay()
        && item
            .get_item_stack()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .item
            .has_tag(&tag::Item::MINECRAFT_PANDA_EATS_FROM_GROUND)
}

/// Items within `range` the panda would eat, vanilla `getEntitiesOfClass(ItemEntity, ...)`.
fn edible_items(mob: &dyn Mob, range: f64) -> Vec<Arc<dyn EntityBase>> {
    let entity = mob.get_entity();
    let area = entity.bounding_box.load().expand(range, range, range);
    entity
        .world
        .load()
        .get_entities_at_box(&area)
        .into_iter()
        .filter(|candidate| candidate.get_item_entity().is_some_and(can_pick_up_and_eat))
        .collect()
}

impl PandaEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let mut mob_entity = MobEntity::new(entity);
        // Vanilla `Panda` constructor.
        *mob_entity
            .move_control
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            Box::new(PandaMoveControl::default());
        mob_entity.set_can_pick_up_loot(true);
        let panda = Self {
            mob_entity,
            ageable_data: AgeableData::default(),
            main_gene: AtomicU8::new(0),
            hidden_gene: AtomicU8::new(0),
            flags: AtomicU8::new(0),
            eat_counter: AtomicI32::new(0),
            sneeze_counter: AtomicI32::new(0),
            unhappy_counter: AtomicI32::new(0),
            roll_counter: AtomicI32::new(0),
            roll_delta: crossbeam::atomic::AtomicCell::new(Vector3::default()),
            got_bamboo: AtomicBool::new(false),
            did_bite: AtomicBool::new(false),
        };
        let mob_arc = Arc::new(panda);
        let mob_weak: Weak<dyn Mob> = {
            let mob_arc: Arc<dyn Mob> = mob_arc.clone();
            Arc::downgrade(&mob_arc)
        };
        Self::register_goals(&mob_arc, mob_weak);
        mob_arc
    }

    /// Vanilla `Panda.registerGoals`.
    fn register_goals(panda: &Arc<Self>, mob_weak: Weak<dyn Mob>) {
        let goal_selector = &panda.mob_entity.goals_selector;
        let can_act = |mob: &dyn Mob| as_panda(mob).is_some_and(Self::can_perform_action);
        let worried_can_act = |mob: &dyn Mob| {
            as_panda(mob).is_some_and(|panda| panda.is_worried() && panda.can_perform_action())
        };
        goal_selector.add_goal(0, Box::new(SwimGoal::default()));
        goal_selector.add_goal(
            2,
            Box::new(PandaPanicGoal(EscapeDangerGoal::with_panic_causes(
                2.0,
                |_| &tag::DamageType::MINECRAFT_PANIC_ENVIRONMENTAL_CAUSES,
            ))),
        );
        goal_selector.add_goal(
            2,
            Box::new(PandaBreedGoal {
                inner: BreedGoal::new(1.0),
                unhappy_cooldown: 0,
            }),
        );
        goal_selector.add_goal(
            3,
            Box::new(MeleeAttackGoal::new(1.2, true).gated_by(can_act)),
        );
        goal_selector.add_goal(
            4,
            Box::new(TemptGoal::with_tag(
                1.0,
                &tag::Item::MINECRAFT_PANDA_FOOD,
                false,
            )),
        );
        for (threat, distance) in [
            (TargetMatch::Type(&EntityType::PLAYER), 8.0),
            (TargetMatch::Monster, 4.0),
        ] {
            goal_selector.add_goal(
                6,
                Box::new(
                    AvoidEntityGoal::new(threat, distance, 2.0, 2.0)
                        .matching(not_spectator)
                        .when(worried_can_act),
                ),
            );
        }
        goal_selector.add_goal(7, Box::new(PandaSitGoal { cooldown: 0 }));
        goal_selector.add_goal(8, Box::new(PandaLieOnBackGoal { cooldown: 0 }));
        goal_selector.add_goal(8, Box::new(PandaSneezeGoal));
        goal_selector.add_goal(
            9,
            Box::new(PandaLookAtPlayerGoal(LookAtEntityGoal::with_default(
                mob_weak,
                &EntityType::PLAYER,
                6.0,
            ))),
        );
        goal_selector.add_goal(10, Box::new(RandomLookAroundGoal::default()));
        goal_selector.add_goal(12, Box::new(PandaRollGoal));
        goal_selector.add_goal(13, Box::new(FollowParentGoal::new(1.25)));
        goal_selector.add_goal(14, Box::new(WanderAroundGoal::water_avoiding(1.0)));
        panda.mob_entity.target_selector.add_goal(
            1,
            Box::new(
                RevengeGoal::new(true)
                    .alerting_others()
                    .alerting_only(|other| as_panda(other).is_some_and(Self::is_aggressive))
                    .continuing_while(|mob| {
                        as_panda(mob).is_none_or(|panda| {
                            !panda.got_bamboo.load(Ordering::Relaxed)
                                && !panda.did_bite.load(Ordering::Relaxed)
                        })
                    }),
            ),
        );
    }

    #[must_use]
    pub fn get_main_gene(&self) -> PandaGene {
        PandaGene::from_id(self.main_gene.load(Ordering::Relaxed))
    }

    pub fn set_main_gene(&self, gene: PandaGene) {
        self.main_gene.store(gene.id(), Ordering::Relaxed);
        self.get_entity()
            .set_synced_data(data::MAIN_GENE_ID, gene.id() as i8);
    }

    #[must_use]
    pub fn get_hidden_gene(&self) -> PandaGene {
        PandaGene::from_id(self.hidden_gene.load(Ordering::Relaxed))
    }

    pub fn set_hidden_gene(&self, gene: PandaGene) {
        self.hidden_gene.store(gene.id(), Ordering::Relaxed);
        self.get_entity()
            .set_synced_data(data::HIDDEN_GENE_ID, gene.id() as i8);
    }

    /// Vanilla `getVariant`: the gene that shows.
    #[must_use]
    pub fn get_variant(&self) -> PandaGene {
        PandaGene::variant(self.get_main_gene(), self.get_hidden_gene())
    }

    fn is_lazy(&self) -> bool {
        self.get_variant() == PandaGene::Lazy
    }

    fn is_worried(&self) -> bool {
        self.get_variant() == PandaGene::Worried
    }

    fn is_playful(&self) -> bool {
        self.get_variant() == PandaGene::Playful
    }

    fn is_weak(&self) -> bool {
        self.get_variant() == PandaGene::Weak
    }

    fn is_aggressive(&self) -> bool {
        self.get_variant() == PandaGene::Aggressive
    }

    #[must_use]
    pub fn has_flag(&self, flag: u8) -> bool {
        (self.flags.load(Ordering::Relaxed) & flag) != 0
    }

    pub fn set_flag(&self, flag: u8, val: bool) {
        let current = self.flags.load(Ordering::Relaxed);
        let new_flags = if val { current | flag } else { current & !flag };
        self.flags.store(new_flags, Ordering::Relaxed);
        self.get_entity()
            .set_synced_data(data::DATA_ID_FLAGS, new_flags as i8);
    }

    #[must_use]
    pub fn is_sneezing(&self) -> bool {
        self.has_flag(FLAG_SNEEZE)
    }

    /// Vanilla `sneeze`: stopping resets the counter.
    pub fn set_sneezing(&self, val: bool) {
        self.set_flag(FLAG_SNEEZE, val);
        if !val {
            self.set_counter(&self.sneeze_counter, data::SNEEZE_COUNTER, 0);
        }
    }

    #[must_use]
    pub fn is_sitting(&self) -> bool {
        self.has_flag(FLAG_SIT)
    }

    pub fn set_sitting(&self, val: bool) {
        self.set_flag(FLAG_SIT, val);
    }

    #[must_use]
    pub fn is_on_back(&self) -> bool {
        self.has_flag(FLAG_ON_BACK)
    }

    pub fn set_on_back(&self, val: bool) {
        self.set_flag(FLAG_ON_BACK, val);
    }

    #[must_use]
    pub fn is_rolling(&self) -> bool {
        self.has_flag(FLAG_ROLL)
    }

    pub fn set_rolling(&self, val: bool) {
        self.set_flag(FLAG_ROLL, val);
    }

    fn is_eating(&self) -> bool {
        self.eat_counter.load(Ordering::Relaxed) > 0
    }

    /// Vanilla `eat`: the eat counter starts at 1 or stops.
    fn set_eating(&self, eating: bool) {
        self.set_counter(&self.eat_counter, data::EAT_COUNTER, i32::from(eating));
    }

    fn set_counter(
        &self,
        counter: &AtomicI32,
        tracked: pumpkin_data::tracked_data::TrackedData,
        value: i32,
    ) {
        counter.store(value, Ordering::Relaxed);
        self.get_entity().set_synced_data(tracked, VarInt(value));
    }

    /// Vanilla `isScared`: a worried panda during a thunderstorm.
    fn is_scared(&self) -> bool {
        self.is_worried() && self.get_entity().world.load().is_thundering()
    }

    /// Vanilla `canPerformAction`.
    fn can_perform_action(&self) -> bool {
        !self.is_on_back()
            && !self.is_scared()
            && !self.is_eating()
            && !self.is_rolling()
            && !self.is_sitting()
    }

    fn held_item(&self) -> ItemStack {
        self.mob_entity
            .living_entity
            .entity_equipment
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&EquipmentSlot::MAIN_HAND)
    }

    /// Vanilla `tryToSit`: stops and sits, unless in water.
    fn try_to_sit(&self) {
        let living = &self.mob_entity.living_entity;
        if living.entity.is_in_water() {
            return;
        }
        let mut input = living.movement_input.load();
        input.z = 0.0;
        living.movement_input.store(input);
        self.mob_entity.steered().navigator().stop();
        self.set_sitting(true);
    }

    /// Vanilla `setAttributes`: weak pandas have 10 health, lazy ones crawl.
    fn set_gene_attributes(&self) {
        let living = &self.mob_entity.living_entity;
        if self.is_weak() {
            living.set_attribute_base(&Attributes::MAX_HEALTH, 10.0);
            living.set_health(living.health.load().min(10.0));
        }
        if self.is_lazy() {
            living.set_attribute_base(&Attributes::MOVEMENT_SPEED, 0.07);
        }
    }

    fn random_gene_of(&self) -> PandaGene {
        if self.get_random().random::<bool>() {
            self.get_main_gene()
        } else {
            self.get_hidden_gene()
        }
    }

    /// Vanilla `setGeneFromParents`: a gene from each parent, each mutating 1 in 32.
    fn set_genes_from_parents(&self, first: &Self, second: Option<&Self>) {
        let mut rng = self.get_random();
        let swap = rng.random::<bool>();
        let (main, hidden) = match second {
            None if swap => (first.random_gene_of(), PandaGene::random_gene(&mut rng)),
            None => (PandaGene::random_gene(&mut rng), first.random_gene_of()),
            Some(second) if swap => (first.random_gene_of(), second.random_gene_of()),
            Some(second) => (second.random_gene_of(), first.random_gene_of()),
        };
        let main = if rng.random_range(0..32) == 0 {
            PandaGene::random_gene(&mut rng)
        } else {
            main
        };
        let hidden = if rng.random_range(0..32) == 0 {
            PandaGene::random_gene(&mut rng)
        } else {
            hidden
        };
        drop(rng);
        self.set_main_gene(main);
        self.set_hidden_gene(hidden);
    }

    /// Vanilla `Panda.tick`, the server part.
    fn tick_panda(&self) {
        let entity = self.get_entity();
        if self.is_worried() {
            if entity.world.load().is_thundering() && !entity.is_in_water() {
                self.set_sitting(true);
                self.set_eating(false);
            } else if !self.is_eating() {
                self.set_sitting(false);
            }
        }
        let target = self.mob_entity.get_target();
        if target.is_none() {
            self.got_bamboo.store(false, Ordering::Relaxed);
            self.did_bite.store(false, Ordering::Relaxed);
        }
        let unhappy = self.unhappy_counter.load(Ordering::Relaxed);
        if unhappy > 0 {
            if let Some(target) = &target {
                entity.look_at(target.get_entity().get_eye_pos());
            }
            if unhappy == 29 || unhappy == 14 {
                self.play_entity_sound(Sound::EntityPandaCantBreed, 1.0, 1.0);
            }
            self.set_counter(&self.unhappy_counter, data::UNHAPPY_COUNTER, unhappy - 1);
        }
        if self.is_sneezing() {
            let sneeze = self.sneeze_counter.load(Ordering::Relaxed) + 1;
            self.set_counter(&self.sneeze_counter, data::SNEEZE_COUNTER, sneeze);
            if sneeze > 20 {
                self.set_sneezing(false);
                self.after_sneeze();
            } else if sneeze == 1 {
                self.play_entity_sound(Sound::EntityPandaPreSneeze, 1.0, 1.0);
            }
        }
        if self.is_rolling() {
            self.handle_roll();
        } else {
            self.roll_counter.store(0, Ordering::Relaxed);
        }
        if self.is_sitting() {
            entity.pitch.store(0.0);
        }
        self.handle_eating();
    }

    /// Vanilla `handleEating`: a sitting panda with food eats it for 4 to 5 seconds.
    fn handle_eating(&self) {
        let held = self.held_item();
        let random_start = !self.is_eating()
            && self.is_sitting()
            && !self.is_scared()
            && !held.is_empty()
            && self.get_random().random_range(0..80) == 1;
        if random_start {
            self.set_eating(true);
        } else if held.is_empty() || !self.is_sitting() {
            self.set_eating(false);
        }
        if !self.is_eating() {
            return;
        }
        let counter = self.eat_counter.load(Ordering::Relaxed);
        if counter % 5 == 0 {
            let (volume, pitch) = {
                let mut rng = self.get_random();
                let volume = 0.5f32.mul_add(rng.random_range(0..2) as f32, 0.5);
                let pitch = (rng.random::<f32>() - rng.random::<f32>()).mul_add(0.2, 1.0);
                (volume, pitch)
            };
            self.play_entity_sound(Sound::EntityPandaEat, volume, pitch);
        }
        if counter > 80 && self.get_random().random_range(0..20) == 1 {
            if counter > 100
                && held
                    .item
                    .has_tag(&tag::Item::MINECRAFT_PANDA_EATS_FROM_GROUND)
            {
                self.mob_entity
                    .set_item_slot(&EquipmentSlot::MAIN_HAND, ItemStack::EMPTY.clone());
                let pos = self.get_entity().pos.load();
                self.get_entity()
                    .world
                    .load()
                    .emit_game_event(pumpkin_data::game_event::GameEvent::Eat.name(), pos);
                self.set_sitting(false);
            }
            self.set_eating(false);
            return;
        }
        self.set_counter(&self.eat_counter, data::EAT_COUNTER, counter + 1);
    }

    /// Vanilla `handleRoll`: a 32 tick somersault forward with three hops.
    fn handle_roll(&self) {
        let counter = self.roll_counter.fetch_add(1, Ordering::Relaxed) + 1;
        if counter > TOTAL_ROLL_STEPS {
            self.set_rolling(false);
            return;
        }
        let entity = self.get_entity();
        let movement = entity.velocity.load();
        if counter == 1 {
            let angle = entity.yaw.load().to_radians();
            let multiplier = if self.is_baby() { 0.1 } else { 0.2 };
            let delta = Vector3::new(
                movement.x + f64::from(-angle.sin() * multiplier),
                0.0,
                movement.z + f64::from(angle.cos() * multiplier),
            );
            self.roll_delta.store(delta);
            entity.velocity.store(delta.add_raw(0.0, 0.27, 0.0));
        } else if counter != 7 && counter != 15 && counter != 23 {
            let delta = self.roll_delta.load();
            entity
                .velocity
                .store(Vector3::new(delta.x, movement.y, delta.z));
        } else {
            let y = if entity.on_ground.load(Ordering::Relaxed) {
                0.27
            } else {
                movement.y
            };
            entity.velocity.store(Vector3::new(0.0, y, 0.0));
        }
    }

    /// Vanilla `afterSneeze`: pandas around jump, and the sneeze may drop slime.
    fn after_sneeze(&self) {
        self.play_entity_sound(Sound::EntityPandaSneeze, 1.0, 1.0);
        let entity = self.get_entity();
        let world = entity.world.load_full();
        let area = entity.bounding_box.load().expand(10.0, 10.0, 10.0);
        for other in world.get_entities_at_box(&area) {
            let Some(panda) = other.cast_any().downcast_ref::<Self>() else {
                continue;
            };
            let other_entity = panda.get_entity();
            if !panda.is_baby()
                && other_entity.on_ground.load(Ordering::Relaxed)
                && !other_entity.is_in_water()
                && panda.can_perform_action()
            {
                panda
                    .mob_entity
                    .living_entity
                    .jump_from_ground(other.as_ref());
            }
        }
        if world.level_info.load().game_rules.mob_drops
            && let Some(table) = world.get_loot_table("minecraft:gameplay/panda_sneeze")
        {
            let params = LootContextParameters {
                world: Some(&world),
                position: Some(entity.pos.load()),
                this_entity: Some(self),
                ..Default::default()
            };
            for stack in generate_loot_from_handle(&table, self.get_random().random(), &params) {
                entity.spawn_at_location(stack);
            }
        }
    }

    /// Vanilla `Panda.pickUpItem`: bamboo and cake go into its empty paw.
    fn pick_up_items(&self) {
        let mob_entity = &self.mob_entity;
        mob_entity.for_each_item_in_pickup_reach(|item| {
            if !self.held_item().is_empty() || !can_pick_up_and_eat(item) {
                return;
            }
            let stack = item
                .get_item_stack()
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone();
            mob_entity
                .living_entity
                .send_pickup_animation(item.get_entity(), u32::from(stack.item_count));
            mob_entity.set_item_slot_and_drop_when_killed(&EquipmentSlot::MAIN_HAND, stack);
            item.get_entity().remove();
        });
    }

    /// The last branch of vanilla `mobInteract` with food: a fed adult that cannot breed now
    /// sits down to eat it, giving back what it held.
    fn take_food(&self, player: &Arc<Player>, item_stack: &mut ItemStack) -> bool {
        if self.is_sitting() || self.get_entity().is_in_water() {
            return false;
        }
        self.try_to_sit();
        self.set_eating(true);
        let held = self.held_item();
        if !held.is_empty() && !player.is_creative() {
            self.get_entity().spawn_at_location(held);
        }
        self.mob_entity
            .set_item_slot(&EquipmentSlot::MAIN_HAND, item_stack.copy_with_count(1));
        item_stack.decrement_unless_creative(player.gamemode.load(), 1);
        true
    }
}

impl AgeableMob for PandaEntity {
    fn get_ageable_data(&self) -> &AgeableData {
        &self.ageable_data
    }

    fn default_baby_spawn_chance(&self) -> f32 {
        crate::entity::ageable::PANDA_BABY_SPAWN_CHANCE
    }
}

impl Animal for PandaEntity {
    /// Vanilla `Panda.isFood`.
    fn is_food(&self, item_stack: &ItemStack) -> bool {
        item_stack.item.has_tag(&tag::Item::MINECRAFT_PANDA_FOOD)
    }

    /// Vanilla `Panda.getBreedOffspring`: genes from both parents.
    fn get_breed_offspring(&self, partner: &dyn EntityBase) -> Arc<dyn EntityBase> {
        let entity = self.get_entity();
        let baby = Self::new(Entity::new(
            entity.world.load_full(),
            entity.pos.load(),
            &EntityType::PANDA,
        ));
        baby.set_genes_from_parents(self, partner.cast_any().downcast_ref::<Self>());
        baby.set_gene_attributes();
        baby
    }
}

impl CustomSound for PandaEntity {
    fn hurt_sound(&self) -> Option<Sound> {
        Some(Sound::EntityPandaHurt)
    }

    fn death_sound(&self) -> Option<Sound> {
        Some(Sound::EntityPandaDeath)
    }
}

impl Mob for PandaEntity {
    /// Vanilla `Panda.getAmbientSound`.
    fn get_ambient_sound(&self) -> Option<Sound> {
        Some(if self.is_aggressive() {
            Sound::EntityPandaAggressiveAmbient
        } else if self.is_worried() {
            Sound::EntityPandaWorriedAmbient
        } else {
            Sound::EntityPandaAmbient
        })
    }

    fn as_custom_sound(&self) -> Option<&dyn CustomSound> {
        Some(self)
    }

    fn mob_play_step_sound(&self, _pos: &BlockPos, _state: &BlockState) {
        self.play_entity_sound(Sound::EntityPandaStep, 0.15, 1.0);
    }

    fn can_be_leashed(&self) -> bool {
        false
    }

    /// Vanilla `Panda.doHurtTarget` and `playAttackSound`: a panda that is not aggressive
    /// lets go after one bite.
    fn do_hurt_target(&self, target: &dyn EntityBase) -> bool {
        if !self.is_aggressive() {
            self.did_bite.store(true, Ordering::Relaxed);
        }
        let damaged = self.mob_entity.try_attack(self.get_entity(), target);
        if damaged {
            self.play_entity_sound(Sound::EntityPandaBite, 1.0, 1.0);
            self.on_attack(target);
        }
        damaged
    }

    /// Vanilla `Panda.hurtServer`: being hurt makes it stand up.
    fn on_damage(&self, _damage_type: DamageType, _source: Option<&dyn EntityBase>) {
        self.set_sitting(false);
    }

    fn mob_tick(&self, _caller: &dyn EntityBase) {
        self.tick_panda();
        self.pick_up_items();
    }

    /// Vanilla `Panda.finalizeSpawn`: random genes, a 20% baby chance.
    fn finalize_spawn(
        &self,
        _world: &Arc<World>,
        group_data: Option<SpawnGroupData>,
    ) -> Option<SpawnGroupData> {
        let (main, hidden) = {
            let mut rng = self.get_random();
            (
                PandaGene::random_gene(&mut rng),
                PandaGene::random_gene(&mut rng),
            )
        };
        self.set_main_gene(main);
        self.set_hidden_gene(hidden);
        self.set_gene_attributes();
        self.finalize_spawn_super(group_data)
    }

    fn as_ageable(&self) -> Option<&dyn AgeableMob> {
        Some(self)
    }

    fn as_animal(&self) -> Option<&dyn Animal> {
        Some(self)
    }

    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        self.write_ageable_nbt(nbt);
        nbt.put_string("MainGene", self.get_main_gene().as_str().to_string());
        nbt.put_string("HiddenGene", self.get_hidden_gene().as_str().to_string());
    }

    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        self.read_ageable_nbt(nbt);
        self.set_main_gene(
            nbt.get_string("MainGene")
                .map_or_else(PandaGene::default, PandaGene::from_name),
        );
        self.set_hidden_gene(
            nbt.get_string("HiddenGene")
                .map_or_else(PandaGene::default, PandaGene::from_name),
        );
    }

    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    fn mob_init_data_tracker(&self) {
        let entity = self.get_entity();
        let is_baby = entity.age.load(Ordering::Relaxed) < 0;
        if is_baby {
            entity.set_synced_data(data::DATA_BABY_ID, true);
        }
        entity.set_synced_data(
            data::MAIN_GENE_ID,
            self.main_gene.load(Ordering::Relaxed) as i8,
        );
        entity.set_synced_data(
            data::HIDDEN_GENE_ID,
            self.hidden_gene.load(Ordering::Relaxed) as i8,
        );
        entity.set_synced_data(
            data::DATA_ID_FLAGS,
            self.flags.load(Ordering::Relaxed) as i8,
        );
        entity.set_synced_data(
            data::UNHAPPY_COUNTER,
            VarInt(self.unhappy_counter.load(Ordering::Relaxed)),
        );
        entity.set_synced_data(
            data::SNEEZE_COUNTER,
            VarInt(self.sneeze_counter.load(Ordering::Relaxed)),
        );
        entity.set_synced_data(
            data::EAT_COUNTER,
            VarInt(self.eat_counter.load(Ordering::Relaxed)),
        );
    }

    /// Vanilla `Panda.mobInteract`.
    fn mob_interact(&self, player: &Arc<Player>, item_stack: &mut ItemStack) -> bool {
        if self.is_scared() {
            return false;
        }
        if self.is_on_back() {
            self.set_on_back(false);
            return true;
        }
        if !self.is_food(item_stack) {
            return self.is_baby()
                && item_stack.item == &Item::GOLDEN_DANDELION
                && self.animal_interact(player, item_stack, Sound::EntityPandaAmbient);
        }
        if self.mob_entity.get_target().is_some() {
            self.got_bamboo.store(true, Ordering::Relaxed);
        }
        let entity = self.get_entity();
        let age = entity.age.load(Ordering::Relaxed);
        if age < 0 {
            return self.animal_interact(player, item_stack, Sound::EntityPandaAmbient);
        }
        if age == 0 && self.mob_entity.is_breeding_ready() && !self.mob_entity.is_in_love() {
            return self.animal_interact(player, item_stack, Sound::EntityPandaAmbient);
        }
        self.take_food(player, item_stack)
    }
}
