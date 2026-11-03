use std::sync::{
    Arc, Weak,
    atomic::{AtomicI32, Ordering},
};

use crossbeam::atomic::AtomicCell;
use pumpkin_data::BlockState;
use pumpkin_data::damage::DamageType;
use pumpkin_data::effect::StatusEffect;
use pumpkin_data::entity::EntityType;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::Sound;
use pumpkin_data::tag::{self, Taggable};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_util::math::position::BlockPos;
use rand::RngExt;

use crate::entity::{
    Entity, EntityBase,
    ai::control::flying_move_control::FlyingMoveControl,
    ai::goal::{
        escape_danger::EscapeDangerGoal, follow_mob::FollowMobGoal, follow_owner::FollowOwnerGoal,
        look_at_entity::LookAtEntityGoal, sit_when_ordered_to::SitWhenOrderedToGoal,
        swim::SwimGoal, water_avoiding_random_flying::WaterAvoidingRandomFlyingGoal,
    },
    ai::pathfinder::{Navigator, node::PathType},
    mob::{Mob, MobEntity, spawn::SpawnGroupData},
    passive::{
        animal::Animal,
        tamable::{TamableAnimal, TamableData},
    },
    player::Player,
};
use crate::world::World;

/// Duration in ticks of the poison a parrot gets from eating a cookie, matching
/// vanilla `Parrot.mobInteract`.
const COOKIE_POISON_DURATION: i32 = 900;
/// Vanilla `Parrot.mobInteract`: one seed in this many tames it.
const TAME_CHANCE: i32 = 10;
/// Vanilla `Parrot.Variant`: red-blue, blue, green, yellow-blue and gray.
const VARIANT_COUNT: i32 = 5;

/// Represents a Parrot, a passive flying mob that can mimic nearby mob sounds.
///
/// Wiki: <https://minecraft.wiki/w/Parrot>
pub struct ParrotEntity {
    pub mob_entity: MobEntity,
    pub tamable_data: TamableData,
    variant: AtomicI32,
    flap_speed: AtomicCell<f32>,
    next_flap: AtomicCell<f32>,
}

impl ParrotEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let mut mob_entity = MobEntity::new(entity);
        // Vanilla `Parrot` constructor and `createNavigation`.
        *mob_entity
            .move_control
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            Box::new(FlyingMoveControl::new(10, false));
        let navigator = mob_entity
            .navigator
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *navigator = Navigator::flying();
        navigator.set_can_open_doors(false);
        navigator.set_can_float(true);
        navigator.set_pathfinding_malus(PathType::DangerFire, -1.0);
        navigator.set_pathfinding_malus(PathType::DamageFire, -1.0);
        navigator.set_pathfinding_malus(PathType::Cocoa, -1.0);
        let parrot = Self {
            mob_entity,
            tamable_data: TamableData::default(),
            variant: AtomicI32::new(0),
            flap_speed: AtomicCell::new(0.0),
            next_flap: AtomicCell::new(1.0),
        };
        let mob_arc = Arc::new(parrot);
        let mob_weak: Weak<dyn Mob> = {
            let mob_arc: Arc<dyn Mob> = mob_arc.clone();
            Arc::downgrade(&mob_arc)
        };

        {
            let goal_selector = &mob_arc.mob_entity.goals_selector;

            // Vanilla `Parrot.registerGoals`; `ParrotWanderGoal` is the plain flying stroll here.
            goal_selector.add_goal(0, EscapeDangerGoal::new(1.25));
            goal_selector.add_goal(0, Box::new(SwimGoal::default()));
            goal_selector.add_goal(
                1,
                LookAtEntityGoal::with_default(mob_weak, &EntityType::PLAYER, 8.0),
            );
            goal_selector.add_goal(2, Box::new(SitWhenOrderedToGoal::new()));
            goal_selector.add_goal(2, FollowOwnerGoal::new(1.0, 5.0, 1.0));
            goal_selector.add_goal(2, Box::new(WaterAvoidingRandomFlyingGoal::new(1.0)));
            goal_selector.add_goal(3, Box::new(FollowMobGoal::new(1.0, 3.0, 7.0)));
        };

        mob_arc
    }

    #[must_use]
    pub fn get_variant(&self) -> i32 {
        self.variant.load(Ordering::Relaxed)
    }

    pub fn set_variant(&self, variant: i32) {
        self.variant.store(variant, Ordering::Relaxed);
        self.get_entity().set_synced_data(
            pumpkin_data::tracked_data::parrot::DATA_VARIANT_ID,
            VarInt(variant),
        );
    }

    /// Vanilla `Parrot.isFlying`.
    fn is_flying(&self) -> bool {
        !self.get_entity().on_ground.load(Ordering::Relaxed)
    }

    /// Vanilla `Parrot.mobInteract` with parrot food: one seed in ten tames a wild parrot.
    fn eat_seeds(&self, player: &Arc<Player>, item_stack: &mut ItemStack) {
        item_stack.decrement_unless_creative(player.gamemode.load(), 1);
        let mut rng = self.get_random();
        let pitch = (rng.random::<f32>() - rng.random::<f32>()).mul_add(0.2, 1.0);
        self.play_entity_sound(Sound::EntityParrotEat, 1.0, pitch);
        if rng.random_range(0..TAME_CHANCE) == 0 && self.approve_taming(player) {
            TamableAnimal::tame(self, player.gameprofile.id);
            self.spawn_taming_particles(true);
        } else {
            self.spawn_taming_particles(false);
        }
    }

    /// Feeds the parrot a cookie: it is poisoned and then killed, as in vanilla
    /// `Parrot.mobInteract`.
    fn eat_cookie(&self, player: &Arc<Player>, item_stack: &mut ItemStack) {
        item_stack.decrement_unless_creative(player.gamemode.load(), 1);

        self.mob_entity
            .living_entity
            .add_effect(pumpkin_data::potion::Effect {
                effect_type: &StatusEffect::POISON,
                duration: COOKIE_POISON_DURATION,
                amplifier: 0,
                ambient: false,
                show_particles: true,
                show_icon: true,
                blend: true,
            });

        // Vanilla guards this call with `player.isCreative() || !this.isInvulnerable()`,
        // but `hurt` re-checks invulnerability itself and `player_attack` doesn't bypass
        // it, so the guard only skips a call that would do nothing anyway.
        self.damage_with_context(
            self,
            f32::MAX,
            DamageType::PLAYER_ATTACK,
            None,
            Some(player.as_ref()),
            Some(player.as_ref()),
        );
    }
}

impl Animal for ParrotEntity {
    /// Vanilla `Parrot.isFood`: parrots never breed.
    fn is_food(&self, _item_stack: &ItemStack) -> bool {
        false
    }

    fn can_mate(&self, _partner: &dyn EntityBase) -> bool {
        false
    }
}

impl TamableAnimal for ParrotEntity {
    fn get_tamable_data(&self) -> &TamableData {
        &self.tamable_data
    }
}

impl Mob for ParrotEntity {
    /// Vanilla `Parrot.isPushable`.
    fn mob_is_pushable(&self) -> bool {
        true
    }

    fn as_animal(&self) -> Option<&dyn Animal> {
        Some(self)
    }

    fn as_tamable(&self) -> Option<&dyn TamableAnimal> {
        Some(self)
    }

    /// Vanilla `Parrot.hurtServer`: a hurt parrot stands up.
    fn on_damage(&self, _damage_type: DamageType, _source: Option<&dyn EntityBase>) {
        self.order_to_sit(false);
    }

    /// Vanilla `Parrot.finalizeSpawn`: a random variant.
    fn finalize_spawn(
        &self,
        _world: &Arc<World>,
        group_data: Option<SpawnGroupData>,
    ) -> Option<SpawnGroupData> {
        self.set_variant(self.get_random().random_range(0..VARIANT_COUNT));
        self.finalize_spawn_super(group_data)
    }

    fn get_ambient_sound(&self) -> Option<Sound> {
        Some(Sound::EntityParrotAmbient)
    }

    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        nbt.put_int("Variant", self.get_variant());
    }

    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        let variant = nbt.get_int("Variant").unwrap_or(0);
        self.set_variant(if (0..VARIANT_COUNT).contains(&variant) {
            variant
        } else {
            0
        });
    }

    fn mob_init_data_tracker(&self) {
        self.get_entity().set_synced_data(
            pumpkin_data::tracked_data::parrot::DATA_VARIANT_ID,
            VarInt(self.get_variant()),
        );
    }

    fn mob_ai_step(&self) {
        // Vanilla `calculateFlapping`.
        let entity = self.get_entity();
        let on_ground = entity.on_ground.load(Ordering::Relaxed);
        let flap_change: f32 = if on_ground || entity.has_vehicle() {
            -1.0
        } else {
            4.0
        };
        self.flap_speed.store(
            flap_change
                .mul_add(0.3, self.flap_speed.load())
                .clamp(0.0, 1.0),
        );
        let velocity = entity.velocity.load();
        if !on_ground && velocity.y < 0.0 {
            entity.set_velocity(velocity.multiply(1.0, 0.6, 1.0));
        }
    }

    fn mob_is_flapping(&self) -> bool {
        self.get_entity().fly_dist.load() > self.next_flap.load()
    }

    fn mob_on_flap(&self) {
        self.play_entity_sound(Sound::EntityParrotFly, 0.15, 1.0);
        self.next_flap
            .store(self.get_entity().fly_dist.load() + self.flap_speed.load() / 2.0);
    }

    fn mob_play_step_sound(&self, _pos: &BlockPos, _state: &BlockState) {
        self.play_entity_sound(Sound::EntityParrotStep, 0.15, 1.0);
    }

    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    /// Vanilla `Parrot.mobInteract`: seeds tame, a cookie kills, the owner makes a landed
    /// parrot sit or stand.
    fn mob_interact(&self, player: &Arc<Player>, item_stack: &mut ItemStack) -> bool {
        let item = item_stack.get_item();
        if !self.is_tame() && item.has_tag(&tag::Item::MINECRAFT_PARROT_FOOD) {
            self.eat_seeds(player, item_stack);
            return true;
        }
        if item.has_tag(&tag::Item::MINECRAFT_PARROT_POISONOUS_FOOD) {
            self.eat_cookie(player, item_stack);
            return true;
        }
        if !self.is_flying() && self.is_tame() && self.is_owned_by(&player.gameprofile.id) {
            self.order_to_sit(!self.is_ordered_to_sit());
            return true;
        }
        self.mob_entity
            .mob_interact(player, item_stack, self.can_be_leashed())
    }
}

#[cfg(test)]
mod tests {
    use super::COOKIE_POISON_DURATION;
    use pumpkin_data::item::Item;
    use pumpkin_data::tag::{self, Taggable};

    /// The interaction is gated on the vanilla `parrot_poisonous_food` tag rather than
    /// on a hardcoded cookie id, so check the tag actually resolves the way the
    /// interaction assumes.
    #[test]
    fn cookie_is_poisonous_parrot_food() {
        assert!(Item::COOKIE.has_tag(&tag::Item::MINECRAFT_PARROT_POISONOUS_FOOD));
    }

    /// Seeds tame a parrot in vanilla and must not reach the poison branch.
    #[test]
    fn parrot_food_is_not_poisonous() {
        assert!(!Item::WHEAT_SEEDS.has_tag(&tag::Item::MINECRAFT_PARROT_POISONOUS_FOOD));
        assert!(!Item::COOKED_CHICKEN.has_tag(&tag::Item::MINECRAFT_PARROT_POISONOUS_FOOD));
    }

    #[test]
    fn poison_lasts_45_seconds() {
        assert_eq!(COOKIE_POISON_DURATION, 900);
    }
}
