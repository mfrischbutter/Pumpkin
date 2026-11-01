use std::sync::{
    Arc, Weak,
    atomic::{AtomicBool, AtomicI32, AtomicUsize, Ordering},
};

use pumpkin_data::BlockState;
use pumpkin_data::attributes::Attributes;
use pumpkin_data::damage::DamageType;
use pumpkin_data::entity::{EntityStatus, EntityType};
use pumpkin_data::fluid::Fluid;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::Sound;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_protocol::bedrock::server::ActorEventID;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use rand::RngExt;

use crate::entity::ai::behavior::hunting::{self, Hunt};
use crate::entity::ai::target_match::TargetMatch;
use crate::entity::spawn_util::is_face_full_up;
use crate::entity::{
    Entity, EntityBase,
    ai::behavior::neutral::apply_targets,
    ai::goal::{
        look_around::RandomLookAroundGoal, look_at_entity::LookAtEntityGoal,
        melee_attack::MeleeAttackGoal, move_towards_target::MoveTowardsTargetGoal,
        offer_flower::OfferFlowerGoal, revenge::RevengeGoal, wander_around::WanderAroundGoal,
    },
    mob::{
        Mob, MobEntity,
        neutral::{NeutralData, NeutralMob},
    },
    player::Player,
};
use crate::world::level_accessor::LevelAccessor;
use crate::world::natural_spawner::is_valid_empty_spawn_block;

/// Represents an Iron Golem, a powerful neutral mob that protects villagers and players.
///
/// Wiki: <https://minecraft.wiki/w/Iron_Golem>
const IRON_GOLEM_HUNTS: &[Hunt] = &[Hunt::new(3, TargetMatch::Enemy, false)
    .every(5)
    .matching(|_hunter, target, _world| target.entity.entity_type != &EntityType::CREEPER)];

/// Vanilla `Crackiness.GOLEM`: the health fractions below which the cracks grow.
const CRACK_FRACTIONS: [f32; 3] = [0.75, 0.5, 0.25];
/// Vanilla `IronGolem.mobInteract`: the health an iron ingot repairs.
const REPAIR_HEALTH: f32 = 25.0;
/// Vanilla `IronGolem.doHurtTarget`: the upward push of a hit.
const HIT_LIFT: f64 = 0.4;

pub struct IronGolemEntity {
    pub mob_entity: MobEntity,
    pub neutral_data: NeutralData,
    pub player_created: AtomicBool,
    pub attack_animation_tick: AtomicI32,
    pub offer_flower_tick: AtomicI32,
    /// The crack level when the current hurt started.
    crackiness_before_hurt: AtomicUsize,
}

impl IronGolemEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let mob_entity = MobEntity::new(entity);
        let iron_golem = Self {
            mob_entity,
            neutral_data: NeutralData::default(),
            player_created: AtomicBool::new(false),
            attack_animation_tick: AtomicI32::new(0),
            offer_flower_tick: AtomicI32::new(0),
            crackiness_before_hurt: AtomicUsize::new(0),
        };
        let mob_arc = Arc::new(iron_golem);
        let mob_weak: Weak<dyn Mob> = {
            let mob_arc: Arc<dyn Mob> = mob_arc.clone();
            Arc::downgrade(&mob_arc)
        };

        {
            let goal_selector = &mob_arc.mob_entity.goals_selector;
            let target_selector = &mob_arc.mob_entity.target_selector;

            goal_selector.add_goal(1, Box::new(MeleeAttackGoal::new(1.0, true)));
            goal_selector.add_goal(2, Box::new(MoveTowardsTargetGoal::new(0.9, 32.0)));
            goal_selector.add_goal(4, Box::new(WanderAroundGoal::new(0.6)));
            goal_selector.add_goal(5, Box::new(OfferFlowerGoal::new()));
            goal_selector.add_goal(
                7,
                LookAtEntityGoal::with_default(mob_weak, &EntityType::PLAYER, 6.0),
            );
            goal_selector.add_goal(8, Box::new(RandomLookAroundGoal::default()));

            // TODO: DefendVillageTargetGoal at 1.
            target_selector.add_goal(2, Box::new(RevengeGoal::new(true)));
            apply_targets(target_selector, &mob_arc.mob_entity, 3, 4, false);
            hunting::apply(target_selector, &mob_arc.mob_entity, IRON_GOLEM_HUNTS);
        };

        mob_arc
    }

    /// Vanilla `getCrackiness`: 0 (none) to 3 (high) by the health left.
    #[must_use]
    pub fn crackiness(&self) -> usize {
        let living = &self.mob_entity.living_entity;
        crack_level(living.health.load() / living.get_max_health())
    }

    #[must_use]
    pub fn is_player_created(&self) -> bool {
        self.player_created.load(Ordering::Relaxed)
    }

    pub fn set_player_created(&self, value: bool) {
        self.player_created.store(value, Ordering::Relaxed);
        let entity = self.get_entity();
        let flag: u8 = u8::from(value);
        entity.set_synced_data(pumpkin_data::tracked_data::iron_golem::FLAGS_ID, flag);
    }

    pub fn offer_flower(&self, offer: bool) {
        let entity = self.get_entity();
        let world = entity.world.load();
        if offer {
            self.offer_flower_tick.store(400, Ordering::Relaxed);
            world.send_entity_status(
                entity,
                EntityStatus::OfferFlower,
                ActorEventID::StartOfferFlower,
            );
        } else {
            self.offer_flower_tick.store(0, Ordering::Relaxed);
            world.send_entity_status(
                entity,
                EntityStatus::StopOfferFlower,
                ActorEventID::StopOfferFlower,
            );
        }
    }

    #[must_use]
    pub fn get_offer_flower_tick(&self) -> i32 {
        self.offer_flower_tick.load(Ordering::Relaxed)
    }

    /// Trigger the attack animation for the iron golem.
    /// This sends `EntityStatus::StartAttacking` to clients and sets the `attack_animation_tick` counter.
    pub fn trigger_attack_animation(&self) {
        self.attack_animation_tick.store(10, Ordering::Relaxed);
        let entity = self.get_entity();
        let world = entity.world.load();
        world.send_entity_status(
            entity,
            EntityStatus::StartAttacking,
            ActorEventID::StartAttacking,
        );
    }
}

/// Vanilla `Crackiness.byFraction` for golems: how many thresholds the health fraction is under.
fn crack_level(fraction: f32) -> usize {
    CRACK_FRACTIONS
        .iter()
        .filter(|&&threshold| fraction < threshold)
        .count()
}

impl NeutralMob for IronGolemEntity {
    fn get_neutral_data(&self) -> &NeutralData {
        &self.neutral_data
    }
}

impl Mob for IronGolemEntity {
    /// Vanilla `IronGolem.checkSpawnObstruction`: standing room for the village golem: a floor
    /// it can stand on, two empty spawn blocks above its feet, its own block empty whatever
    /// its fluid, and no entity in the way.
    fn check_spawn_obstruction(&self, level: &dyn LevelAccessor) -> bool {
        let entity = self.get_entity();
        let pos = entity.block_pos.load();
        if !is_face_full_up(level.get_block_state(&pos.down())) {
            return false;
        }
        for height in 1..3 {
            let above = pos.add(0, height, 0);
            if !is_valid_empty_spawn_block(
                level.get_block_state(&above),
                level.get_fluid(&above),
                &EntityType::IRON_GOLEM,
            ) {
                return false;
            }
        }
        is_valid_empty_spawn_block(
            level.get_block_state(&pos),
            &Fluid::EMPTY,
            &EntityType::IRON_GOLEM,
        ) && level.is_unobstructed(entity)
    }

    /// Vanilla `AbstractGolem.removeWhenFarAway`.
    fn remove_when_far_away(&self, _distance_sq: f64) -> bool {
        false
    }

    fn mob_play_step_sound(&self, _pos: &BlockPos, _state: &BlockState) {
        self.play_entity_sound(Sound::EntityIronGolemStep, 1.0, 1.0);
    }

    /// Vanilla `IronGolem.doHurtTarget`: half the attack damage plus up to all of it again,
    /// and a hit lifts the target.
    fn do_hurt_target(&self, target: &dyn EntityBase) -> bool {
        self.trigger_attack_animation();
        let attack_damage = self
            .mob_entity
            .living_entity
            .get_attribute_value(&Attributes::ATTACK_DAMAGE) as f32;
        let whole = attack_damage as i32;
        let damage = if whole > 0 {
            attack_damage / 2.0 + self.get_random().random_range(0..whole) as f32
        } else {
            attack_damage
        };
        let hurt =
            self.mob_entity
                .try_attack_with_damage(self, target, damage, DamageType::MOB_ATTACK);
        if hurt {
            let resistance = target.get_living_entity().map_or(0.0, |living| {
                living.get_attribute_value(&Attributes::KNOCKBACK_RESISTANCE)
            });
            let lift = HIT_LIFT * (1.0 - resistance).max(0.0);
            let target_entity = target.get_entity();
            let velocity = target_entity.velocity.load();
            target_entity.set_velocity(Vector3::new(velocity.x, velocity.y + lift, velocity.z));
            target_entity.velocity_dirty.store(true, Ordering::Relaxed);
            self.on_attack(target);
        }
        self.play_entity_sound(Sound::EntityIronGolemAttack, 1.0, 1.0);
        hurt
    }

    fn pre_damage(&self, _damage_type: DamageType, _source: Option<&dyn EntityBase>) -> bool {
        self.crackiness_before_hurt
            .store(self.crackiness(), Ordering::Relaxed);
        true
    }

    /// Vanilla `IronGolem.hurtServer`: one more crack plays the damage sound.
    fn on_damage(&self, _damage_type: DamageType, _source: Option<&dyn EntityBase>) {
        if self.crackiness() != self.crackiness_before_hurt.load(Ordering::Relaxed) {
            self.play_entity_sound(Sound::EntityIronGolemDamage, 1.0, 1.0);
        }
    }

    fn can_attack(&self, target: &dyn EntityBase) -> bool {
        let target_type = target.get_entity().entity_type;
        if self.is_player_created() && target_type == &EntityType::PLAYER {
            return false;
        }
        // Vanilla's `super.canAttack` goes through `Mob` crepper and ghast
        target_type != &EntityType::CREEPER
            && target_type != &EntityType::GHAST
            && self.get_mob_entity().living_entity.can_attack(target)
    }

    fn as_neutral(&self) -> Option<&dyn NeutralMob> {
        Some(self)
    }

    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        nbt.put_bool("PlayerCreated", self.is_player_created());
    }

    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        if let Some(created) = nbt.get_bool("PlayerCreated") {
            self.set_player_created(created);
        }
    }

    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    fn as_iron_golem(&self) -> Option<&crate::entity::passive::iron_golem::IronGolemEntity> {
        Some(self)
    }

    fn mob_tick(&self, _caller: &dyn EntityBase) {
        let attack_tick = self.attack_animation_tick.load(Ordering::Relaxed);
        if attack_tick > 0 {
            self.attack_animation_tick.fetch_sub(1, Ordering::Relaxed);
        }

        let flower_tick = self.offer_flower_tick.load(Ordering::Relaxed);
        if flower_tick > 0 {
            self.offer_flower_tick.fetch_sub(1, Ordering::Relaxed);
        }
    }

    fn mob_init_data_tracker(&self) {
        let entity = self.get_entity();
        let flag: u8 = u8::from(self.is_player_created());
        entity.set_synced_data(pumpkin_data::tracked_data::iron_golem::FLAGS_ID, flag);
    }

    /// Vanilla `IronGolem.mobInteract`: an iron ingot repairs 25 health.
    fn mob_interact(&self, player: &Arc<Player>, item_stack: &mut ItemStack) -> bool {
        if item_stack.item.id != Item::IRON_INGOT.id {
            return false;
        }
        let living = &self.mob_entity.living_entity;
        let health_before = living.health.load();
        living.heal(REPAIR_HEALTH);
        if living.health.load() == health_before {
            return false;
        }
        let mut rng = self.get_random();
        let pitch = (rng.random::<f32>() - rng.random::<f32>()).mul_add(0.2, 1.0);
        self.play_entity_sound(Sound::EntityIronGolemRepair, 1.0, pitch);
        item_stack.decrement_unless_creative(player.gamemode.load(), 1);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::crack_level;

    #[test]
    fn cracks_follow_the_golem_thresholds() {
        assert_eq!(crack_level(1.0), 0);
        assert_eq!(crack_level(0.75), 0);
        assert_eq!(crack_level(0.74), 1);
        assert_eq!(crack_level(0.5), 1);
        assert_eq!(crack_level(0.49), 2);
        assert_eq!(crack_level(0.24), 3);
        assert_eq!(crack_level(0.0), 3);
    }
}
