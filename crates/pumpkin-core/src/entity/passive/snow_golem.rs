use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};

use pumpkin_data::damage::DamageType;
use pumpkin_data::entity::EntityType;
use pumpkin_data::environment_attribute::EnvironmentAttribute;
use pumpkin_data::game_event::GameEvent;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::{Block, BlockStateId};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::math::position::BlockPos;
use pumpkin_world::world::BlockFlags;
use rand::RngExt;

use crate::entity::ai::behavior::hunting::{self, Hunt};
use crate::entity::ai::target_match::TargetMatch;
use crate::entity::{
    Entity, EntityBase,
    ai::goal::{
        look_around::RandomLookAroundGoal, look_at_entity::LookAtEntityGoal,
        ranged_attack::RangedAttackGoal, wander_around::WanderAroundGoal,
    },
    item::ItemEntity,
    mob::{Mob, MobEntity, RangedAttackMob},
    player::Player,
    projectile::snowball::SnowballEntity,
};
use crate::world::World;
use crate::world::game_event::GameEventContext;

/// Vanilla `SnowGolem.registerGoals`: the stroll keeps to land only this rarely.
const STROLL_LAND_PROBABILITY: f32 = 1.000_000_1E-5;
/// Vanilla `SnowGolem.rangedAttackUncertainty`.
const SNOWBALL_UNCERTAINTY: f64 = 12.0;
/// Vanilla `SnowGolem.performRangedAttack`: aims this far below the target's eyes.
const AIM_BELOW_EYES: f64 = 1.1;

const SNOW_GOLEM_HUNTS: &[Hunt] = &[Hunt::new(1, TargetMatch::Enemy, true)];

pub struct SnowGolemEntity {
    pub mob_entity: MobEntity,
    pub has_pumpkin: AtomicBool,
}

impl SnowGolemEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let mob_entity = MobEntity::new(entity);
        let snow_golem = Self {
            mob_entity,
            has_pumpkin: AtomicBool::new(true),
        };
        let mob_arc = Arc::new(snow_golem);
        let mob_weak: Weak<dyn Mob> = {
            let mob_arc: Arc<dyn Mob> = mob_arc.clone();
            Arc::downgrade(&mob_arc)
        };
        let ranged_weak: Weak<dyn RangedAttackMob> = {
            let ranged_arc: Arc<dyn RangedAttackMob> = mob_arc.clone();
            Arc::downgrade(&ranged_arc)
        };

        {
            let goal_selector = &mob_arc.mob_entity.goals_selector;
            let target_selector = &mob_arc.mob_entity.target_selector;

            goal_selector.add_goal(
                1,
                Box::new(RangedAttackGoal::new(ranged_weak, 1.25, 20, 10.0)),
            );
            goal_selector.add_goal(
                2,
                Box::new(WanderAroundGoal::water_avoiding_with_probability(
                    1.0,
                    STROLL_LAND_PROBABILITY,
                )),
            );
            goal_selector.add_goal(
                3,
                LookAtEntityGoal::with_default(mob_weak, &EntityType::PLAYER, 6.0),
            );
            goal_selector.add_goal(4, Box::new(RandomLookAroundGoal::default()));

            hunting::apply(target_selector, &mob_arc.mob_entity, SNOW_GOLEM_HUNTS);
        };

        mob_arc
    }

    #[must_use]
    pub fn has_pumpkin(&self) -> bool {
        self.has_pumpkin.load(Ordering::Relaxed)
    }

    pub fn set_has_pumpkin(&self, has_pumpkin: bool) {
        self.has_pumpkin.store(has_pumpkin, Ordering::Relaxed);
        let entity = self.get_entity();
        let flags = if has_pumpkin { 16u8 } else { 0u8 };
        entity.set_synced_data(
            pumpkin_data::tracked_data::snow_golem::DATA_PUMPKIN_ID,
            flags as i8,
        );
    }

    pub fn throw_snowball(&self, target: &Arc<dyn EntityBase>) {
        let entity = self.get_entity();
        let world = entity.world.load_full();

        let snowball_entity = Entity::new(world.clone(), entity.pos.load(), &EntityType::SNOWBALL);
        let snowball = SnowballEntity::new_shot(snowball_entity, entity);

        let mob_pos = entity.pos.load();
        let target_entity = target.get_entity();
        let target_pos = target_entity.pos.load();

        // Vanilla `SnowGolem.performRangedAttack`.
        let dx = target_pos.x - mob_pos.x;
        let dy = target_entity.get_eye_y() - AIM_BELOW_EYES - snowball.get_entity().pos.load().y;
        let dz = target_pos.z - mob_pos.z;
        let yo = dx.hypot(dz) * 0.2;

        snowball
            .thrown
            .set_velocity(dx, dy + yo, dz, 1.6, SNOWBALL_UNCERTAINTY);

        let snowball_arc: Arc<dyn EntityBase> = Arc::new(snowball);
        world.spawn_entity(snowball_arc);

        let pitch = 0.4 / self.get_random().random::<f32>().mul_add(0.4, 0.8);
        self.play_entity_sound(Sound::EntitySnowGolemShoot, 1.0, pitch);
    }

    /// Vanilla `SnowGolem.aiStep` past `super`: melting where it is too hot, and a snow trail
    /// where mobs may grief.
    fn melt_and_leave_snow(&self) {
        let entity = self.get_entity();
        let world = entity.world.load_full();
        let pos = entity.pos.load();
        if world.environment_attributes().get_value_bool(
            EnvironmentAttribute::GameplaySnowGolemMelts,
            &pos.to_block_pos(),
        ) {
            self.damage(self, 1.0, DamageType::ON_FIRE);
        }
        if !world.level_info.load().game_rules.mob_griefing {
            return;
        }
        let snow = Block::SNOW.default_state.id;
        for i in 0..4 {
            let x = f64::from(i % 2 * 2 - 1).mul_add(0.25, pos.x).floor() as i32;
            let z = f64::from(i / 2 % 2 * 2 - 1).mul_add(0.25, pos.z).floor() as i32;
            let snow_pos = BlockPos::new(x, pos.y.floor() as i32, z);
            if world.get_block_state(&snow_pos).is_air()
                && can_snow_survive(&world, &snow_pos, snow)
            {
                world.set_block_state(&snow_pos, snow, BlockFlags::NOTIFY_ALL);
                let source: &dyn EntityBase = self;
                world.emit_game_event_with_context(
                    GameEvent::BlockPlace.name(),
                    snow_pos.to_centered_f64(),
                    &GameEventContext {
                        source_entity: Some(source),
                    },
                );
            }
        }
    }
}

fn can_snow_survive(world: &Arc<World>, pos: &BlockPos, snow: BlockStateId) -> bool {
    world.block_registry.can_place_at(
        None,
        Some(&**world),
        &**world,
        None,
        &Block::SNOW,
        snow.to_state(),
        pos,
        None,
        None,
    )
}

impl Mob for SnowGolemEntity {
    fn get_ambient_sound(&self) -> Option<Sound> {
        Some(Sound::EntitySnowGolemAmbient)
    }

    fn is_sensitive_to_water(&self) -> bool {
        true
    }

    fn mob_ai_step(&self) {
        self.melt_and_leave_snow();
    }

    /// Vanilla `AbstractGolem.removeWhenFarAway`.
    fn remove_when_far_away(&self, _distance_sq: f64) -> bool {
        false
    }

    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        nbt.put_bool("Pumpkin", self.has_pumpkin());
    }

    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        if let Some(pumpkin) = nbt.get_bool("Pumpkin") {
            self.set_has_pumpkin(pumpkin);
        }
    }

    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    fn mob_init_data_tracker(&self) {
        let entity = self.get_entity();
        let flags = if self.has_pumpkin() { 16u8 } else { 0u8 };
        entity.set_synced_data(
            pumpkin_data::tracked_data::snow_golem::DATA_PUMPKIN_ID,
            flags as i8,
        );
    }

    fn mob_interact(&self, player: &Arc<Player>, item_stack: &mut ItemStack) -> bool {
        if item_stack.get_item() == &Item::SHEARS && self.has_pumpkin() {
            let entity = self.get_entity();
            let world = entity.world.load();
            if let Some(server) = world.server.upgrade() {
                let mut event = crate::plugin::api::events::player::player_shear_entity::PlayerShearEntityEvent {
                    player: player.clone(),
                    entity_id: entity.entity_id,
                    hand: 0,
                    cancelled: false,
                };
                server.plugin_manager.fire_blocking(&server, &mut event);
                if event.cancelled {
                    return false;
                }
            }
            self.set_has_pumpkin(false);
            let pos = entity.pos.load();
            world.play_sound(Sound::EntitySnowGolemShear, SoundCategory::Players, &pos);
            let item_entity = Arc::new(ItemEntity::new(
                Entity::new(world.clone(), pos, &EntityType::ITEM),
                ItemStack::new(1, &Item::CARVED_PUMPKIN),
            ));
            world.spawn_entity(item_entity);
            if player.gamemode.load() != pumpkin_util::GameMode::Creative {
                let _ = item_stack.damage_item(1);
            }
            return true;
        }
        false
    }
}

impl RangedAttackMob for SnowGolemEntity {
    fn perform_ranged_attack(&self, target: &Arc<dyn EntityBase>, _power: f32) {
        self.throw_snowball(target);
    }
}
