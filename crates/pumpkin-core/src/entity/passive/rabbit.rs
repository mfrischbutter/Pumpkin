use std::sync::{
    Arc, Weak,
    atomic::{AtomicBool, AtomicI32, Ordering},
};

use pumpkin_data::Block;
use pumpkin_data::attributes::Attributes;
use pumpkin_data::block_properties::WheatLikeProperties;
use pumpkin_data::entity::{EntityStatus, EntityType};
use pumpkin_data::entity_variant::RabbitVariant;
use pumpkin_data::game_event::GameEvent;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::tag::{self, Taggable};
use pumpkin_data::world::WorldEvent;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_protocol::bedrock::server::actor_event::ActorEventID;
use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_util::text::TextComponent;
use pumpkin_world::world::BlockFlags;
use rand::RngExt;

use crate::entity::ai::behavior::hunting::{self, Hunt};
use crate::entity::ai::control::{
    Control, MoveControlTrait,
    move_control::{MoveControl, Operation},
};
use crate::entity::ai::goal::move_to_target_pos::{MoveToTargetPos, MoveToTargetPosGoal};
use crate::entity::ai::goal::revenge::RevengeGoal;
use crate::entity::attributes::{Modifier, ModifierOperation};
use crate::entity::{
    Entity, EntityBase,
    ageable::{AgeableData, AgeableMob},
    ai::behavior::fleeing::{self, Flee},
    ai::goal::{
        Controls, Goal, ParentHandle, breed::BreedGoal,
        climb_on_top_of_powder_snow::ClimbOnTopOfPowderSnowGoal, escape_danger::EscapeDangerGoal,
        look_at_entity::LookAtEntityGoal, melee_attack::MeleeAttackGoal, swim::SwimGoal,
        tempt::TemptGoal, wander_around::WanderAroundGoal,
    },
    ai::target_match::TargetMatch,
    mob::{Mob, MobEntity, spawn::SpawnGroupData},
    passive::animal::Animal,
    player::Player,
};
use crate::world::World;
use crate::world::game_event::GameEventContext;

/// Vanilla `Rabbit` speed modifiers.
const STROLL_SPEED_MOD: f64 = 0.6;
const BREED_SPEED_MOD: f64 = 0.8;
const FOLLOW_SPEED_MOD: f64 = 1.0;
const FLEE_SPEED_MOD: f64 = 2.2;
const ATTACK_SPEED_MOD: f64 = 1.4;
/// Vanilla `RabbitMoveControl.setWantedPosition`: the speed in water.
const WATER_SPEED_MOD: f64 = 1.5;
const BABY_JUMP_HEIGHT: f64 = 0.5;
const ADULT_JUMP_HEIGHT: f64 = 1.5;
/// Vanilla `jumpFromGround`: the push forward when a hop starts nearly still.
const JUMP_PUSH_SPEED: f32 = 0.1;
const JUMP_PUSH_MAX_SPEED_SQR: f64 = 0.01;
const JUMP_DELAY_TICKS: i32 = 10;
const PANIC_JUMP_DELAY_TICKS: i32 = 3;
const JUMP_DURATION_IN_TICKS: i32 = 15;
/// Vanilla `getJumpPower`: jump powers before scaling to the jump strength attribute.
const JUMP_POWER: f32 = 0.3;
const SLOW_JUMP_POWER: f32 = 0.2;
const HIGH_JUMP_POWER: f32 = 0.5;
const DEFAULT_JUMP_POWER: f32 = 0.42;
/// Vanilla `getJumpPower`: a target this much higher takes a high jump.
const HIGH_JUMP_RISE: f64 = 0.5;
/// Vanilla `customServerAiStep`: a killer bunny leaps at targets within 4 blocks.
const EVIL_LEAP_DISTANCE_SQR: f64 = 16.0;
const KILLER_BUNNY: &str = "entity.minecraft.killer_bunny";
const EVIL_ATTACK_POWER_INCREMENT: f64 = 5.0;
const EVIL_ATTACK_POWER_MODIFIER: &str = "minecraft:evil";
const EVIL_ARMOR_VALUE: f64 = 8.0;
const MORE_CARROTS_DELAY: i32 = 40;
/// Vanilla `RabbitGroupData`: `super(1.0F)`, every rabbit after the first is a baby.
const GROUP_BABY_SPAWN_CHANCE: f32 = 1.0;
/// Vanilla `getBreedOffspring`: one in this many babies takes the biome's variant.
const OFFSPRING_BIOME_VARIANT_CHANCE: i32 = 20;
/// Vanilla `getRandomRabbitVariant` rolls out of 100.
const WHITE_CHANCE: i32 = 80;
const BROWN_CHANCE: i32 = 50;
const SALT_CHANCE: i32 = 90;
/// Vanilla `RaidGardenGoal`: `MoveToBlockGoal(rabbit, 0.7F, 16)`, retried after 10 ticks.
const RAID_GARDEN_SPEED: f32 = 0.7;
const RAID_GARDEN_RANGE: i32 = 16;
const RAID_GARDEN_RETRY_TICKS: i32 = 10;
const RAID_GARDEN_LOOK_YAW: f32 = 10.0;
/// Vanilla `CropBlock.MAX_AGE`.
const CARROT_MAX_AGE: u8 = 7;

const RABBIT_FLEES: &[Flee] = &[
    Flee::new(4, TargetMatch::Type(&EntityType::PLAYER), 8.0, 2.2, 2.2).when(is_not_evil),
    Flee::new(4, TargetMatch::Type(&EntityType::WOLF), 10.0, 2.2, 2.2).when(is_not_evil),
    Flee::new(4, TargetMatch::Monster, 4.0, 2.2, 2.2).when(is_not_evil),
];

fn as_rabbit(mob: &dyn Mob) -> Option<&RabbitEntity> {
    mob.cast_any().downcast_ref::<RabbitEntity>()
}

fn is_not_evil(mob: &dyn Mob) -> bool {
    as_rabbit(mob).is_some_and(|rabbit| rabbit.get_variant() != RabbitVariant::Evil)
}

const KILLER_BUNNY_HUNTS: &[Hunt] = &[
    Hunt::new(2, TargetMatch::Type(&EntityType::PLAYER), true),
    Hunt::new(2, TargetMatch::Type(&EntityType::WOLF), true),
];

/// Vanilla `getRandomRabbitVariant`.
fn random_variant_at(world: &World, pos: &BlockPos) -> RabbitVariant {
    let biome = world.get_biome(pos);
    let roll = rand::rng().random_range(0..100);
    if biome.has_tag(&tag::WorldgenBiome::MINECRAFT_SPAWNS_WHITE_RABBITS) {
        if roll < WHITE_CHANCE {
            RabbitVariant::White
        } else {
            RabbitVariant::WhiteSplotched
        }
    } else if biome.has_tag(&tag::WorldgenBiome::MINECRAFT_SPAWNS_GOLD_RABBITS) {
        RabbitVariant::Gold
    } else if roll < BROWN_CHANCE {
        RabbitVariant::Brown
    } else if roll < SALT_CHANCE {
        RabbitVariant::Salt
    } else {
        RabbitVariant::Black
    }
}

fn random_pitch(mob: &dyn Mob) -> f32 {
    let mut rng = mob.get_random();
    (rng.random::<f32>() - rng.random::<f32>()) * 0.2 + 1.0
}

pub struct RabbitEntity {
    pub mob_entity: MobEntity,
    pub ageable_data: AgeableData,
    pub variant: AtomicI32,
    pub more_carrot_ticks: AtomicI32,
    jump_ticks: AtomicI32,
    jump_duration: AtomicI32,
    was_on_ground: AtomicBool,
    jump_delay_ticks: AtomicI32,
}

impl RabbitEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let mob_entity = MobEntity::new(entity);
        let rabbit = Self {
            mob_entity,
            ageable_data: AgeableData::default(),
            variant: AtomicI32::new(RabbitVariant::DEFAULT.id()),
            more_carrot_ticks: AtomicI32::new(0),
            jump_ticks: AtomicI32::new(0),
            jump_duration: AtomicI32::new(0),
            was_on_ground: AtomicBool::new(false),
            jump_delay_ticks: AtomicI32::new(0),
        };
        let mob_arc = Arc::new(rabbit);
        let mob_weak: Weak<dyn Mob> = {
            let mob_arc: Arc<dyn Mob> = mob_arc.clone();
            Arc::downgrade(&mob_arc)
        };

        {
            let mut move_control = mob_arc
                .mob_entity
                .move_control
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            *move_control = Box::new(RabbitMoveControl::new(Arc::downgrade(&mob_arc)));
        };
        mob_arc.set_speed_modifier(0.0);

        {
            let goal_selector = &mob_arc.mob_entity.goals_selector;

            goal_selector.add_goal(1, Box::new(SwimGoal::default()));
            goal_selector.add_goal(1, Box::new(ClimbOnTopOfPowderSnowGoal));
            goal_selector.add_goal(1, Box::new(RabbitPanicGoal::new()));
            goal_selector.add_goal(2, BreedGoal::new(BREED_SPEED_MOD));
            goal_selector.add_goal(
                3,
                Box::new(TemptGoal::with_tag(
                    FOLLOW_SPEED_MOD,
                    &tag::Item::MINECRAFT_RABBIT_FOOD,
                    false,
                )),
            );
            fleeing::apply(goal_selector, RABBIT_FLEES);
            goal_selector.add_goal(5, Box::new(RaidGardenGoal::new()));
            goal_selector.add_goal(
                6,
                Box::new(WanderAroundGoal::water_avoiding(STROLL_SPEED_MOD)),
            );
            goal_selector.add_goal(
                11,
                LookAtEntityGoal::with_default(mob_weak, &EntityType::PLAYER, 10.0),
            );
        };

        mob_arc
    }

    #[must_use]
    pub fn get_variant(&self) -> RabbitVariant {
        RabbitVariant::by_id(self.variant.load(Ordering::Relaxed))
    }

    /// Vanilla `setVariant`: the killer bunny gets armor, attack power, its goals and its name.
    pub fn set_variant(&self, variant: RabbitVariant) {
        let entity = self.get_entity();
        let living = &self.mob_entity.living_entity;
        if variant == RabbitVariant::Evil {
            living.set_attribute_base(&Attributes::ARMOR, EVIL_ARMOR_VALUE);
            self.add_killer_bunny_goals();
            living.update_attribute(&Attributes::ATTACK_DAMAGE, |damage| {
                damage.add_or_replace_modifier(Modifier {
                    id: EVIL_ATTACK_POWER_MODIFIER.to_string(),
                    amount: EVIL_ATTACK_POWER_INCREMENT,
                    operation: ModifierOperation::Add,
                });
            });
            if entity.custom_name.load().is_none() {
                entity.set_custom_name(TextComponent::translate(KILLER_BUNNY, []));
            }
        } else {
            living.update_attribute(&Attributes::ATTACK_DAMAGE, |damage| {
                damage.remove_modifier(EVIL_ATTACK_POWER_MODIFIER);
            });
        }

        self.variant.store(variant.id(), Ordering::Relaxed);
        entity.set_synced_data(
            pumpkin_data::tracked_data::rabbit::DATA_TYPE_ID,
            VarInt(variant.id()),
        );
    }

    fn add_killer_bunny_goals(&self) {
        let goal_selector = &self.mob_entity.goals_selector;
        goal_selector.add_goal(4, Box::new(MeleeAttackGoal::new(ATTACK_SPEED_MOD, true)));

        let target_selector = &self.mob_entity.target_selector;
        target_selector.add_goal(1, Box::new(RevengeGoal::new(true).alerting_others()));
        hunting::apply(target_selector, &self.mob_entity, KILLER_BUNNY_HUNTS);
    }

    /// Vanilla `wantsMoreFood`.
    fn wants_more_food(&self) -> bool {
        self.more_carrot_ticks.load(Ordering::Relaxed) <= 0
    }

    /// Vanilla `setSpeedModifier`: the speed of the next hops.
    pub fn set_speed_modifier(&self, speed: f64) {
        self.mob_entity.steered().navigator().set_speed(speed);
        let mut move_control = self
            .mob_entity
            .move_control
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let wanted = move_control.get_wanted_position();
        move_control.set_wanted_position(wanted.x, wanted.y, wanted.z, speed);
    }

    fn speed_modifier(&self) -> f64 {
        self.mob_entity
            .move_control
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get_speed_modifier()
    }

    /// Vanilla `setJumping`: every hop starts with the jump sound.
    fn set_jumping(&self, jump: bool) {
        self.mob_entity
            .living_entity
            .jumping
            .store(jump, Ordering::SeqCst);
        if jump {
            let pitch = random_pitch(self) * 0.8;
            self.play_entity_sound(Sound::EntityRabbitJump, 1.0, pitch);
        }
    }

    /// Vanilla `startJumping`.
    fn start_jumping(&self) {
        self.set_jumping(true);
        self.jump_duration
            .store(JUMP_DURATION_IN_TICKS, Ordering::Relaxed);
        self.jump_ticks.store(0, Ordering::Relaxed);
    }

    /// Vanilla `facePoint`.
    fn face_point(&self, x: f64, z: f64) {
        let entity = self.get_entity();
        let pos = entity.pos.load();
        let yaw = (z - pos.z).atan2(x - pos.x).to_degrees() as f32 - 90.0;
        entity.yaw.store(yaw);
    }

    /// Vanilla `checkLandingDelay`: a panicking rabbit hops again sooner.
    fn check_landing_delay(&self) {
        let delay = if self.speed_modifier() < FLEE_SPEED_MOD {
            JUMP_DELAY_TICKS
        } else {
            PANIC_JUMP_DELAY_TICKS
        };
        self.jump_delay_ticks.store(delay, Ordering::Relaxed);
    }

    /// Vanilla `customServerAiStep`: a killer bunny leaps at a close target.
    fn leap_at_target(&self) {
        let Some(target) = self.mob_entity.get_target() else {
            return;
        };
        let target_pos = target.get_entity().pos.load();
        let pos = self.get_entity().pos.load();
        if pos.squared_distance_to_vec(&target_pos) >= EVIL_LEAP_DISTANCE_SQR {
            return;
        }
        self.face_point(target_pos.x, target_pos.z);
        {
            let mut move_control = self
                .mob_entity
                .move_control
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let speed = move_control.get_speed_modifier();
            move_control.set_wanted_position(target_pos.x, target_pos.y, target_pos.z, speed);
        };
        self.start_jumping();
        self.was_on_ground.store(true, Ordering::Relaxed);
    }

    /// Vanilla `customServerAiStep`: hops toward the next waypoint once the landing delay is over.
    fn hop_to_wanted_position(&self) {
        let (has_wanted, wanted) = {
            let move_control = self
                .mob_entity
                .move_control
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            (
                move_control.has_wanted(),
                move_control.get_wanted_position(),
            )
        };
        if !has_wanted || self.jump_delay_ticks.load(Ordering::Relaxed) != 0 {
            return;
        }
        let pos = self.next_path_position().unwrap_or(wanted);
        self.face_point(pos.x, pos.z);
        self.start_jumping();
    }

    /// Vanilla `path.getNextEntityPos(this)` while a path is being followed.
    fn next_path_position(&self) -> Option<Vector3<f64>> {
        let width = self.get_entity().width();
        self.mob_entity
            .navigator
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get_path()
            .filter(|path| !path.is_done())
            .and_then(|path| path.get_next_entity_pos(width))
    }
}

impl AgeableMob for RabbitEntity {
    fn get_ageable_data(&self) -> &AgeableData {
        &self.ageable_data
    }
}

impl Animal for RabbitEntity {
    fn is_food(&self, item_stack: &ItemStack) -> bool {
        item_stack.item.has_tag(&tag::Item::MINECRAFT_RABBIT_FOOD)
    }

    /// Vanilla `getBreedOffspring`: a parent's variant, now and then the biome's.
    fn get_breed_offspring(&self, partner: &dyn EntityBase) -> Arc<dyn EntityBase> {
        let entity = self.get_entity();
        let world = entity.world.load_full();
        let baby = Self::new(Entity::new(
            world.clone(),
            entity.pos.load(),
            &EntityType::RABBIT,
        ));
        let biome_variant = random_variant_at(&world, &entity.block_pos.load());
        let mut rng = self.get_random();
        let variant = if rng.random_range(0..OFFSPRING_BIOME_VARIANT_CHANCE) == 0 {
            biome_variant
        } else {
            partner
                .cast_any()
                .downcast_ref::<Self>()
                .filter(|_| rng.random::<bool>())
                .map_or_else(|| self.get_variant(), Self::get_variant)
        };
        baby.set_variant(variant);
        baby
    }
}

impl Mob for RabbitEntity {
    /// Vanilla `finalizeSpawn`: the whole group shares the first rabbit's variant.
    fn finalize_spawn(
        &self,
        world: &Arc<World>,
        group_data: Option<SpawnGroupData>,
    ) -> Option<SpawnGroupData> {
        let random_variant = random_variant_at(world, &self.get_entity().block_pos.load());
        let (variant, size) = match group_data {
            Some(SpawnGroupData::Rabbit { variant, size }) => (variant, size),
            _ => (random_variant, 0),
        };
        self.set_variant(variant);
        self.finalize_ageable_spawn(Some(SpawnGroupData::Ageable {
            group_size: size,
            should_spawn_baby: true,
            baby_spawn_chance: GROUP_BABY_SPAWN_CHANCE,
        }));
        Some(SpawnGroupData::Rabbit {
            variant,
            size: size + 1,
        })
    }

    fn mob_sound_source(&self) -> SoundCategory {
        if self.get_variant() == RabbitVariant::Evil {
            SoundCategory::Hostile
        } else {
            SoundCategory::Neutral
        }
    }

    fn get_ambient_sound(&self) -> Option<Sound> {
        Some(Sound::EntityRabbitAmbient)
    }

    /// Vanilla `playAttackSound`: only the killer bunny makes one.
    fn on_attack(&self, _target: &dyn EntityBase) {
        if self.get_variant() == RabbitVariant::Evil {
            self.play_entity_sound(Sound::EntityRabbitAttack, 1.0, random_pitch(self));
        }
    }

    /// Vanilla `RabbitJumpControl.tick`: a wanted jump starts a hop, nothing ends one.
    fn tick_jump_control(&self, jump: bool) {
        if jump {
            self.start_jumping();
        }
    }

    /// Vanilla `getJumpPower`: high hops up steps, low ones while strolling.
    fn get_jump_power(&self) -> f64 {
        let living = &self.mob_entity.living_entity;
        let y = living.entity.pos.load().y;
        let (speed, wanted_y) = {
            let move_control = self
                .mob_entity
                .move_control
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            (
                move_control.get_speed_modifier(),
                move_control.get_wanted_position().y,
            )
        };
        let waypoint_above = self
            .next_path_position()
            .is_some_and(|next| next.y > y + HIGH_JUMP_RISE);
        let blocked = living.entity.horizontal_collision.load(Ordering::Relaxed)
            || (living.jumping.load(Ordering::Relaxed) && wanted_y > y + HIGH_JUMP_RISE);
        let jump_power = if waypoint_above || blocked {
            HIGH_JUMP_POWER
        } else if speed <= STROLL_SPEED_MOD {
            SLOW_JUMP_POWER
        } else {
            JUMP_POWER
        };
        living.get_jump_velocity(f64::from(jump_power / DEFAULT_JUMP_POWER))
    }

    /// Vanilla `jumpFromGround`: a hop from standing pushes forward, clients animate it.
    fn on_jump_from_ground(&self) {
        let entity = self.get_entity();
        if self.speed_modifier() > 0.0
            && entity.velocity.load().horizontal_length_squared() < JUMP_PUSH_MAX_SPEED_SQR
        {
            let height = if self.is_baby() {
                BABY_JUMP_HEIGHT
            } else {
                ADULT_JUMP_HEIGHT
            };
            let push = Vector3::new(0.0, height, 1.0);
            entity.update_velocity_from_input(push, f64::from(JUMP_PUSH_SPEED));
        }
        let world = entity.world.load();
        world.send_entity_status(entity, EntityStatus::Jump, ActorEventID::None);
    }

    /// Vanilla `customServerAiStep`: hop timing, carrot hunger and the killer bunny's leap.
    fn custom_server_ai_step(&self, _caller: &dyn EntityBase) {
        if self.jump_delay_ticks.load(Ordering::Relaxed) > 0 {
            self.jump_delay_ticks.fetch_sub(1, Ordering::Relaxed);
        }
        let more_carrot_ticks = self.more_carrot_ticks.load(Ordering::Relaxed);
        if more_carrot_ticks > 0 {
            let ticks = (more_carrot_ticks - self.get_random().random_range(0..3)).max(0);
            self.more_carrot_ticks.store(ticks, Ordering::Relaxed);
        }

        let on_ground = self.get_entity().on_ground.load(Ordering::Relaxed);
        if on_ground {
            if !self.was_on_ground.load(Ordering::Relaxed) {
                self.set_jumping(false);
                self.check_landing_delay();
            }
            if self.get_variant() == RabbitVariant::Evil
                && self.jump_delay_ticks.load(Ordering::Relaxed) == 0
            {
                self.leap_at_target();
            }
            let wants_jump = self
                .mob_entity
                .jump_control
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .wants_jump();
            if !wants_jump {
                self.hop_to_wanted_position();
            }
        }
        self.was_on_ground.store(on_ground, Ordering::Relaxed);
    }

    /// Vanilla `aiStep`: a hop ends after its duration.
    fn mob_ai_step(&self) {
        let jump_ticks = self.jump_ticks.load(Ordering::Relaxed);
        let jump_duration = self.jump_duration.load(Ordering::Relaxed);
        if jump_ticks != jump_duration {
            self.jump_ticks.store(jump_ticks + 1, Ordering::Relaxed);
        } else if jump_duration != 0 {
            self.jump_ticks.store(0, Ordering::Relaxed);
            self.jump_duration.store(0, Ordering::Relaxed);
            self.set_jumping(false);
        }
    }

    fn as_ageable(&self) -> Option<&dyn AgeableMob> {
        Some(self)
    }

    fn as_animal(&self) -> Option<&dyn Animal> {
        Some(self)
    }

    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        nbt.put_int("RabbitType", self.get_variant().id());
        nbt.put_int(
            "MoreCarrotTicks",
            self.more_carrot_ticks.load(Ordering::Relaxed),
        );
    }

    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        let variant = nbt
            .get_int("RabbitType")
            .map_or(RabbitVariant::DEFAULT, RabbitVariant::by_id);
        self.set_variant(variant);
        let ticks = nbt.get_int("MoreCarrotTicks").unwrap_or(0);
        self.more_carrot_ticks.store(ticks, Ordering::Relaxed);
    }

    fn mob_set_variant_name(&self, name: &str) {
        if let Some(variant) = RabbitVariant::from_name(name) {
            self.set_variant(variant);
        }
    }

    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    fn mob_init_data_tracker(&self) {
        let entity = self.get_entity();
        let is_baby = entity.age.load(Ordering::Relaxed) < 0;
        if is_baby {
            entity.set_synced_data(pumpkin_data::tracked_data::rabbit::DATA_BABY_ID, true);
        }
        entity.set_synced_data(
            pumpkin_data::tracked_data::rabbit::DATA_TYPE_ID,
            VarInt(self.get_variant().id()),
        );
    }

    fn mob_interact(&self, player: &Arc<Player>, item_stack: &mut ItemStack) -> bool {
        self.animal_interact(player, item_stack, Sound::EntityRabbitAmbient)
    }
}

/// Vanilla `RabbitMoveControl`: moves only while a hop is under way.
pub struct RabbitMoveControl {
    rabbit: Weak<RabbitEntity>,
    move_control: MoveControl,
    next_jump_speed: f64,
}

impl RabbitMoveControl {
    #[must_use]
    pub fn new(rabbit: Weak<RabbitEntity>) -> Self {
        Self {
            rabbit,
            move_control: MoveControl::default(),
            next_jump_speed: 0.0,
        }
    }

    /// Vanilla `Rabbit.setSpeedModifier`, from inside the control while it is locked.
    fn set_rabbit_speed_modifier(&mut self, mob: &dyn Mob, speed: f64) {
        let mob_entity = mob.get_mob_entity();
        mob_entity.steered().navigator().set_speed(speed);
        let wanted = self.get_wanted_position();
        self.set_wanted_position(wanted.x, wanted.y, wanted.z, speed);
    }
}

impl Control for RabbitMoveControl {}

impl MoveControlTrait for RabbitMoveControl {
    fn tick(&mut self, mob: &dyn Mob) {
        let mob_entity = mob.get_mob_entity();
        let living = &mob_entity.living_entity;
        let wants_jump = mob_entity
            .jump_control
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .wants_jump();
        if living.entity.on_ground.load(Ordering::Relaxed)
            && !living.jumping.load(Ordering::Relaxed)
            && !wants_jump
        {
            self.set_rabbit_speed_modifier(mob, 0.0);
        } else if self.move_control.has_wanted()
            || self.move_control.operation == Operation::Jumping
        {
            self.set_rabbit_speed_modifier(mob, self.next_jump_speed);
        }
        self.move_control.tick(mob);
    }

    fn set_wanted_position(&mut self, x: f64, y: f64, z: f64, speed: f64) {
        let in_water = self
            .rabbit
            .upgrade()
            .is_some_and(|rabbit| rabbit.get_entity().is_in_water());
        let speed = if in_water { WATER_SPEED_MOD } else { speed };
        self.move_control.set_wanted_position(x, y, z, speed);
        if speed > 0.0 {
            self.next_jump_speed = speed;
        }
    }

    fn strafe(&mut self, forward: f32, right: f32) {
        self.move_control.strafe(forward, right);
    }

    fn has_wanted(&self) -> bool {
        self.move_control.has_wanted()
    }

    fn get_speed_modifier(&self) -> f64 {
        self.move_control.get_speed_modifier()
    }

    fn get_wanted_position(&self) -> Vector3<f64> {
        self.move_control.get_wanted_position()
    }
}

/// Vanilla `RabbitPanicGoal`: keeps the flee speed for every hop while panicking.
struct RabbitPanicGoal {
    escape_danger_goal: EscapeDangerGoal,
}

impl RabbitPanicGoal {
    fn new() -> Self {
        Self {
            escape_danger_goal: *EscapeDangerGoal::new(FLEE_SPEED_MOD),
        }
    }
}

impl Goal for RabbitPanicGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        self.escape_danger_goal.can_start(mob)
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        self.escape_danger_goal.should_continue(mob)
    }

    fn start(&mut self, mob: &dyn Mob) {
        self.escape_danger_goal.start(mob);
    }

    fn stop(&mut self, mob: &dyn Mob) {
        self.escape_danger_goal.stop(mob);
    }

    fn tick(&mut self, mob: &dyn Mob) {
        self.escape_danger_goal.tick(mob);
        if let Some(rabbit) = as_rabbit(mob) {
            rabbit.set_speed_modifier(FLEE_SPEED_MOD);
        }
    }

    fn controls(&self) -> Controls {
        self.escape_danger_goal.controls()
    }
}

/// Vanilla `RaidGardenGoal` state that its block search reads and sets.
struct GardenTarget {
    wants_to_raid: AtomicBool,
    can_raid: AtomicBool,
}

impl MoveToTargetPos for GardenTarget {
    /// Vanilla `RaidGardenGoal.isValidTarget`: farmland under a grown carrot, while hungry.
    fn is_target_pos(&self, world: Arc<World>, block_pos: BlockPos) -> bool {
        let supports_crops = world
            .get_block(&block_pos)
            .has_tag(&tag::Block::MINECRAFT_SUPPORTS_CROPS);
        if !supports_crops
            || !self.wants_to_raid.load(Ordering::Relaxed)
            || self.can_raid.load(Ordering::Relaxed)
        {
            return false;
        }
        let (block, state) = world.get_block_and_state_id(&block_pos.up());
        if block.id == Block::CARROTS.id
            && WheatLikeProperties::from_state_id(state).age >= CARROT_MAX_AGE
        {
            self.can_raid.store(true, Ordering::Relaxed);
            return true;
        }
        false
    }
}

/// Vanilla `RaidGardenGoal`: walks to a grown carrot and nibbles it down, when mob griefing.
struct RaidGardenGoal {
    move_to_block_goal: MoveToTargetPosGoal<GardenTarget>,
    target: Box<GardenTarget>,
}

impl RaidGardenGoal {
    fn new() -> Self {
        let target = Box::new(GardenTarget {
            wants_to_raid: AtomicBool::new(false),
            can_raid: AtomicBool::new(false),
        });
        // SAFETY: the target has its own heap allocation owned by this goal, so it never moves
        // and outlives the handle, and it is only ever read through shared references.
        let handle: ParentHandle<GardenTarget> = unsafe { ParentHandle::new(&target) };
        let speed = f64::from(RAID_GARDEN_SPEED);
        let goal = MoveToTargetPosGoal::with_default(handle, speed, RAID_GARDEN_RANGE);
        Self {
            move_to_block_goal: goal,
            target,
        }
    }

    /// Vanilla `RaidGardenGoal.tick`: eats one age off the carrot, the last bite removes it.
    fn raid_carrot(mob: &dyn Mob, crops_pos: BlockPos) -> bool {
        let world = mob.get_entity().world.load_full();
        let (block, state) = world.get_block_and_state_id(&crops_pos);
        if block.id != Block::CARROTS.id {
            return false;
        }
        let mut properties = WheatLikeProperties::from_state_id(state);
        if properties.age == 0 {
            world.set_block_state(
                &crops_pos,
                Block::AIR.default_state.id,
                BlockFlags::NOTIFY_LISTENERS,
            );
        } else {
            properties.age -= 1;
            world.set_block_state(
                &crops_pos,
                properties.to_state_id(block),
                BlockFlags::NOTIFY_LISTENERS,
            );
            let source: &dyn EntityBase = mob;
            world.emit_game_event_with_context(
                GameEvent::BlockChange.name(),
                crops_pos.to_centered_f64(),
                &GameEventContext {
                    source_entity: Some(source),
                },
            );
            world.sync_world_event(
                WorldEvent::ParticlesAndSoundDestroyBlock,
                crops_pos,
                i32::from(state.as_u16()),
            );
        }
        true
    }
}

impl Goal for RaidGardenGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        if self.move_to_block_goal.cooldown <= 0 {
            let world = mob.get_entity().world.load();
            if !world.level_info.load().game_rules.mob_griefing {
                return false;
            }
            let hungry = as_rabbit(mob).is_some_and(RabbitEntity::wants_more_food);
            self.target.can_raid.store(false, Ordering::Relaxed);
            self.target.wants_to_raid.store(hungry, Ordering::Relaxed);
        }
        self.move_to_block_goal.can_start(mob)
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        let can_raid = self.target.can_raid.load(Ordering::Relaxed);
        can_raid && self.move_to_block_goal.should_continue(mob)
    }

    fn start(&mut self, mob: &dyn Mob) {
        self.move_to_block_goal.start(mob);
    }

    fn tick(&mut self, mob: &dyn Mob) {
        self.move_to_block_goal.tick(mob);
        let block_pos = self.move_to_block_goal.target_pos;
        mob.get_mob_entity()
            .look_control
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .look_at_with_range(
                f64::from(block_pos.0.x) + 0.5,
                f64::from(block_pos.0.y + 1),
                f64::from(block_pos.0.z) + 0.5,
                RAID_GARDEN_LOOK_YAW,
                mob.get_max_look_pitch_change(),
            );
        if !self.move_to_block_goal.is_reached_target() {
            return;
        }
        if self.target.can_raid.load(Ordering::Relaxed)
            && Self::raid_carrot(mob, block_pos.up())
            && let Some(rabbit) = as_rabbit(mob)
        {
            rabbit
                .more_carrot_ticks
                .store(MORE_CARROTS_DELAY, Ordering::Relaxed);
        }
        self.target.can_raid.store(false, Ordering::Relaxed);
        self.move_to_block_goal.cooldown = RAID_GARDEN_RETRY_TICKS;
    }

    fn should_run_every_tick(&self) -> bool {
        self.move_to_block_goal.should_run_every_tick()
    }

    fn controls(&self) -> Controls {
        self.move_to_block_goal.controls()
    }
}
