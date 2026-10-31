use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::{Arc, PoisonError, Weak};

use crossbeam::atomic::AtomicCell;
use pumpkin_data::attributes::Attributes;
use pumpkin_data::damage::DamageType;
use pumpkin_data::effect::StatusEffect;
use pumpkin_data::entity::EntityType;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::tag::{self, Taggable};
use pumpkin_data::tracked_data;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::Difficulty;
use pumpkin_util::math::boundingbox::EntityDimensions;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_util::math::wrap_degrees;

use crate::entity::ai::behavior::hunting::{self, Hunt};
use crate::entity::ai::target_match::TargetMatch;
use crate::entity::custom_sound::CustomSound;
use crate::entity::mob::equipment::RegionalDifficulty;
use crate::entity::mob::magma_cube::MAGMA_CUBE;
use crate::entity::{
    Entity, EntityBase, RemovalReason,
    ai::control::{Control, MoveControlTrait, move_control::MoveControl},
    ai::goal::{Controls, Goal, to_goal_ticks},
    mob::{
        Mob, MobEntity, MobMetadata,
        spawn::{EntitySpawnReason, SpawnGroupData},
    },
};
use crate::world::World;
use pumpkin_util::random::RandomImpl;
use rand::RngExt;

const MIN_SIZE: i32 = 1;
const MAX_SIZE: i32 = 127;

/// Speed modifier `CubeMobFloatGoal` swims with.
const FLOAT_SPEED_MODIFIER: f64 = 1.2;
/// Speed modifier `CubeMobKeepOnJumpingGoal` hops with.
const JUMPING_SPEED_MODIFIER: f64 = 1.0;
/// Ticks `CubeMobAttackGoal` chases before it gives up for a moment.
const GROW_TIRED_TICKS: i32 = 300;
/// Degrees per tick `CubeMobAttackGoal` turns towards its target.
const ATTACK_TURN_SPEED: f32 = 10.0;

const CUBE_MOB_HUNTS: &[Hunt] = &[
    Hunt::new(1, TargetMatch::Type(&EntityType::PLAYER), true)
        .matching(hunting::within_four_blocks_height),
    Hunt::new(3, TargetMatch::Type(&EntityType::IRON_GOLEM), true),
];

/// A sound in its tiny and its bigger version.
#[derive(Clone, Copy)]
pub struct SizedSound {
    pub tiny: Sound,
    pub big: Sound,
}

impl SizedSound {
    const fn pick(self, tiny: bool) -> Sound {
        if tiny { self.tiny } else { self.big }
    }
}

/// Vanilla `MagmaCube.jumpInLiquid` in lava: the upward speed, base and per size.
#[derive(Clone, Copy)]
pub struct LavaJump {
    pub base: f32,
    pub per_size: f32,
}

/// What a cube mob species overrides of vanilla `AbstractCubeMob`.
pub struct CubeSpecies {
    pub hurt_sound: SizedSound,
    pub death_sound: SizedSound,
    pub squish_sound: SizedSound,
    pub jump_sound: SizedSound,
    /// `getJumpDelay`: factor on the base delay.
    pub jump_delay_factor: i32,
    /// `decreaseSquish`: the part of the squish kept each tick.
    pub squish_decay: f32,
    /// `canDealDamage`: whether tiny ones hurt what they touch.
    pub tiny_deals_damage: bool,
    /// `getAttackDamage`: added to the attack damage attribute.
    pub attack_damage_bonus: f32,
    /// `setSize`: armor per size, `None` leaves the armor alone.
    pub armor_per_size: Option<i32>,
    /// `jumpFromGround`: jump power added per size.
    pub jump_power_per_size: f32,
    /// `jumpInLiquid`: how it jumps in lava, `None` swims up like in water.
    pub lava_jump: Option<LavaJump>,
}

/// Vanilla `Slime`.
pub static SLIME: CubeSpecies = CubeSpecies {
    hurt_sound: SizedSound {
        tiny: Sound::EntitySlimeHurtSmall,
        big: Sound::EntitySlimeHurt,
    },
    death_sound: SizedSound {
        tiny: Sound::EntitySlimeDeathSmall,
        big: Sound::EntitySlimeDeath,
    },
    squish_sound: SizedSound {
        tiny: Sound::EntitySlimeSquishSmall,
        big: Sound::EntitySlimeSquish,
    },
    jump_sound: SizedSound {
        tiny: Sound::EntitySlimeJumpSmall,
        big: Sound::EntitySlimeJump,
    },
    jump_delay_factor: 1,
    squish_decay: 0.6,
    tiny_deals_damage: false,
    attack_damage_bonus: 0.0,
    armor_per_size: None,
    jump_power_per_size: 0.0,
    lava_jump: None,
};

/// A hopping cube that splits when it dies (vanilla `AbstractCubeMob`).
///
/// Slimes and magma cubes, told apart by their [`CubeSpecies`].
///
/// Wiki: <https://minecraft.wiki/w/Slime>
pub struct SlimeEntity {
    entity: Arc<MobEntity>,
    species: &'static CubeSpecies,
    size: AtomicI32,
    jump_delay: AtomicI32,
    target_yaw: AtomicCell<f32>,
    is_aggressive: AtomicBool,
    speed_modifier: AtomicCell<Option<f64>>,
    was_on_ground: AtomicBool,
    pub squish: AtomicCell<f32>,
    pub target_squish: AtomicCell<f32>,
    pub o_squish: AtomicCell<f32>,
}

impl SlimeEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let species = if entity.entity_type == &EntityType::MAGMA_CUBE {
            &MAGMA_CUBE
        } else {
            &SLIME
        };
        let slime = Arc::new(Self {
            entity: Arc::new(MobEntity::new(entity)),
            species,
            size: AtomicI32::new(MIN_SIZE),
            jump_delay: AtomicI32::new(0),
            target_yaw: AtomicCell::new(0.0),
            is_aggressive: AtomicBool::new(false),
            speed_modifier: AtomicCell::new(None),
            was_on_ground: AtomicBool::new(false),
            squish: AtomicCell::new(0.0),
            target_squish: AtomicCell::new(0.0),
            o_squish: AtomicCell::new(0.0),
        });

        let move_control = Box::new(SlimeMoveControl::new(Arc::downgrade(&slime)));
        *slime
            .entity
            .move_control
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = move_control;
        slime
            .entity
            .navigator
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .set_can_float(true);

        let goals = &slime.entity.goals_selector;
        goals.add_goal(1, Box::new(CubeMobFloatGoal));
        goals.add_goal(4, Box::new(CubeMobRandomDirectionGoal::default()));
        goals.add_goal(5, Box::new(CubeMobKeepOnJumpingGoal));
        goals.add_goal(2, Box::new(CubeMobAttackGoal::default()));
        let targets = &slime.entity.target_selector;
        hunting::apply(targets, &slime.entity, CUBE_MOB_HUNTS);

        slime
    }

    /// Vanilla `setSize`: resizes the cube and sets the attributes that follow its size.
    pub fn set_size(&self, size: i32, update_health: bool) {
        let actual_size = size.clamp(MIN_SIZE, MAX_SIZE);
        let living = &self.entity.living_entity;
        let entity = &living.entity;
        self.size.store(actual_size, Ordering::Relaxed);
        entity.set_synced_data(tracked_data::slime::ID_SIZE, actual_size);

        // Vanilla `AbstractCubeMob.refreshDimensions` keeps the position a resize would fudge.
        let pos = entity.pos.load();
        let entity_type = entity.entity_type;
        let dimensions = EntityDimensions::new(
            entity_type.dimension[0],
            entity_type.dimension[1],
            entity_type.eye_height,
        );
        entity.set_default_dimensions(dimensions.scale(actual_size as f32));
        entity.set_pos(pos);

        living.set_attribute_base(
            &Attributes::MAX_HEALTH,
            f64::from(actual_size * actual_size),
        );
        living.set_attribute_base(
            &Attributes::MOVEMENT_SPEED,
            f64::from(0.2f32 + 0.1 * actual_size as f32),
        );
        if update_health {
            living.set_health(living.get_max_health());
        }
        living.set_attribute_base(&Attributes::ATTACK_DAMAGE, f64::from(actual_size));
        // Vanilla `MagmaCube.setSize` uses the size before clamping here.
        if let Some(armor_per_size) = self.species.armor_per_size {
            let armor = size.wrapping_mul(armor_per_size);
            living.set_attribute_base(&Attributes::ARMOR, f64::from(armor));
        }

        if let Some(meta) = self.mob_metadata() {
            entity.send_bedrock_actor_data(&meta.to_bedrock_actor_data());
        }
    }

    #[must_use]
    pub fn get_size(&self) -> i32 {
        self.size.load(Ordering::Relaxed)
    }

    #[must_use]
    pub fn is_tiny(&self) -> bool {
        self.get_size() <= 1
    }

    /// Vanilla `Slime.checkSlimeSpawnRules`: surface slimes in swamps by the moon's spawn
    /// chance, slime chunks below y 40; a spawner only needs the floor.
    pub fn check_slime_spawn_rules(
        world: &World,
        reason: EntitySpawnReason,
        pos: &BlockPos,
    ) -> bool {
        if world.level_info.load().difficulty == Difficulty::Peaceful {
            return false;
        }
        let entity_type = &EntityType::SLIME;
        if reason.is_spawner() {
            return MobEntity::check_mob_spawn_rules(entity_type, world, reason, pos);
        }

        let mut rng = rand::rng();
        if world
            .get_biome(pos)
            .has_tag(&tag::WorldgenBiome::MINECRAFT_ALLOWS_SURFACE_SLIME_SPAWNS)
            && pos.0.y > 50
            && pos.0.y < 70
            && rng.random::<f32>() < world.surface_slime_spawn_chance(pos)
            && world.get_max_local_raw_brightness(pos) <= rng.random_range(0..8)
        {
            return MobEntity::check_mob_spawn_rules(entity_type, world, reason, pos);
        }

        let chunk_pos = pos.chunk_position();
        let slime_seed = pumpkin_util::random::seed_slime_chunk(
            chunk_pos.x,
            chunk_pos.y,
            world.level.slime_seed(),
            987_234_911,
        );
        let mut slime_rand = pumpkin_util::random::legacy_rand::LegacyRand::from_seed(slime_seed);
        if rng.random_range(0..10) == 0 && slime_rand.next_bounded_i32(10) == 0 && pos.0.y < 40 {
            return MobEntity::check_mob_spawn_rules(entity_type, world, reason, pos);
        }

        false
    }

    // const fn get_spawn_chance(moon_phase: i64) -> f32 {
    //     match moon_phase {
    //         0 => 1.0,
    //         1 | 7 => 0.75,
    //         2 | 6 => 0.5,
    //         3 | 5 => 0.25,
    //         _ => 0.0,
    //     }
    // }

    /// Vanilla `CubeMobMoveControl.setDirection`: the yaw to turn to and the jump rhythm.
    fn set_direction(&self, yaw: f32, is_aggressive: bool) {
        self.target_yaw.store(yaw);
        self.is_aggressive.store(is_aggressive, Ordering::Relaxed);
    }

    /// Vanilla `CubeMobMoveControl.setWantedMovement`: hop on next tick.
    fn set_wanted_movement(&self, speed_modifier: f64) {
        self.speed_modifier.store(Some(speed_modifier));
    }

    /// Vanilla `canDealDamage`: whether touching hurts.
    fn can_deal_damage(&self) -> bool {
        (self.species.tiny_deals_damage || !self.is_tiny()) && !self.entity.is_no_ai()
    }

    /// Vanilla `AbstractCubeMob.dealDamage`: hurts `target` when it is in reach and in sight.
    fn deal_damage(&self, target: &dyn EntityBase) {
        let this: &dyn EntityBase = self;
        let living = &self.entity.living_entity;
        if !living.is_alive()
            || !this.do_teams_allow_damage(target)
            || !self.entity.is_in_attack_range(target)
            || !living.entity.has_line_of_sight(target.get_entity())
        {
            return;
        }
        let damage = living.get_attribute_value(&Attributes::ATTACK_DAMAGE) as f32
            + self.species.attack_damage_bonus;
        if target.damage_with_context(
            target,
            damage,
            DamageType::MOB_ATTACK,
            None,
            Some(this),
            Some(this),
        ) {
            let mut rng = self.get_random();
            let pitch = (rng.random::<f32>() - rng.random::<f32>()) * 0.2 + 1.0;
            self.play_entity_sound(Sound::EntitySlimeAttack, 1.0, pitch);
        }
    }

    /// Vanilla `Mob.lookAt`: turns towards `target` by at most `max_turn` degrees.
    fn look_at(&self, target: &dyn EntityBase, max_turn: f32) {
        let entity = self.get_entity();
        let target_entity = target.get_entity();
        let pos = entity.pos.load();
        let target_pos = target_entity.pos.load();
        let dx = target_pos.x - pos.x;
        let dz = target_pos.z - pos.z;
        let dy = if target.get_living_entity().is_some() {
            target_entity.get_eye_y() - entity.get_eye_y()
        } else {
            let bounding_box = target_entity.bounding_box.load();
            f64::midpoint(bounding_box.min.y, bounding_box.max.y) - entity.get_eye_y()
        };
        let horizontal = dx.hypot(dz);
        let yaw = dz.atan2(dx).to_degrees() as f32 - 90.0;
        let pitch = -(dy.atan2(horizontal).to_degrees() as f32);
        entity.set_pitch(rotlerp(entity.pitch.load(), pitch, max_turn));
        entity.yaw.store(rotlerp(entity.yaw.load(), yaw, max_turn));
    }

    /// Vanilla `AbstractCubeMob.remove`: a dying cube splits into two to four of half its size.
    fn split(&self, size: i32) {
        let entity = self.get_entity();
        let world = entity.world.load_full();
        let pos = entity.pos.load();
        let offset = entity.entity_dimension.load().width / 2.0;
        let half_size = size / 2;
        let mut rng = self.get_random();
        let count = 2 + rng.random_range(0..3);
        let team = self.get_team_name();
        for i in 0..count {
            let xd = ((i % 2) as f32 - 0.5) * offset;
            let zd = ((i / 2) as f32 - 0.5) * offset;
            let child = Self::new(Entity::new(world.clone(), pos, entity.entity_type));
            self.copy_split_state(&child, team.as_deref());
            child.set_size(half_size, true);
            let child_entity = child.get_entity();
            child_entity.set_pos(Vector3::new(
                pos.x + f64::from(xd),
                pos.y + 0.5,
                pos.z + f64::from(zd),
            ));
            child_entity.yaw.store(rng.random::<f32>() * 360.0);
            child_entity.set_pitch(0.0);
            world.spawn_entity(child);
        }
    }

    /// Vanilla `ConversionType.convertCommon`, for the state Pumpkin keeps.
    fn copy_split_state(&self, child: &Self, team: Option<&str>) {
        let from = self.get_entity();
        let to = child.get_entity();
        let custom_name = from.custom_name.load_full();
        if let Some(name) = &*custom_name {
            to.set_custom_name(name.clone());
        }
        to.set_custom_name_visible(from.custom_name_visible.load(Ordering::Relaxed));
        child.entity.set_left_handed(self.entity.is_left_handed());
        child.entity.set_no_ai(self.entity.is_no_ai());
        if self.entity.persistence_required.load(Ordering::Relaxed) {
            child.entity.set_persistence_required();
        }
        to.set_invulnerable(from.invulnerable.load(Ordering::Relaxed));
        to.set_has_no_gravity(from.has_no_gravity());
        to.portal_cooldown.store(
            from.portal_cooldown.load(Ordering::Relaxed),
            Ordering::Relaxed,
        );
        to.set_silent(from.is_silent());
        let tags = from
            .scoreboard_tags
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        *to.scoreboard_tags
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = tags;
        if let Some(team) = team {
            to.join_team(team);
        }
    }

    fn get_jump_delay(&self) -> i32 {
        self.get_random().random_range(10..30) * self.species.jump_delay_factor
    }

    fn get_sound_volume(&self) -> f32 {
        0.4 * self.get_size() as f32
    }

    /// Vanilla `getSoundPitch`, for the jump sound.
    fn get_sound_pitch(&self) -> f32 {
        let pitch_adjuster = if self.is_tiny() { 1.4 } else { 0.8 };
        let mut rng = self.get_random();
        ((rng.random::<f32>() - rng.random::<f32>()) * 0.2 + 1.0) * pitch_adjuster
    }
}

/// Vanilla `Mob.rotlerp`: `current` turned towards `target` by at most `max_turn`, unwrapped.
fn rotlerp(current: f32, target: f32, max_turn: f32) -> f32 {
    current + wrap_degrees(target - current).clamp(-max_turn, max_turn)
}

impl CustomSound for SlimeEntity {
    fn death_sound(&self) -> Option<Sound> {
        Some(self.species.death_sound.pick(self.is_tiny()))
    }

    fn hurt_sound(&self) -> Option<Sound> {
        Some(self.species.hurt_sound.pick(self.is_tiny()))
    }

    fn sound_volume(&self) -> f32 {
        self.get_sound_volume()
    }
}

impl Mob for SlimeEntity {
    fn as_custom_sound(&self) -> Option<&dyn CustomSound> {
        Some(self)
    }

    fn mob_sound_source(&self) -> SoundCategory {
        SoundCategory::Hostile
    }

    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        nbt.put_int("Size", self.get_size() - 1);
        nbt.put_bool("wasOnGround", self.was_on_ground.load(Ordering::Relaxed));
    }

    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        self.set_size(nbt.get_int("Size").unwrap_or(0) + 1, false);
        // Vanilla reads the size before the living data, so without a saved Health the cube
        // gets the max health of its size, not of the default one.
        if nbt.get_numeric_float("Health").is_none() {
            let living = &self.entity.living_entity;
            living.set_health(living.get_max_health());
        }
        self.was_on_ground.store(
            nbt.get_bool("wasOnGround").unwrap_or(false),
            Ordering::Relaxed,
        );
    }

    fn get_mob_entity(&self) -> &MobEntity {
        &self.entity
    }

    fn mob_metadata(&self) -> Option<MobMetadata> {
        Some(MobMetadata::new().slime_size(self.get_size()))
    }

    fn get_base_experience_reward(&self) -> u32 {
        self.get_size() as u32
    }

    /// Vanilla `AbstractCubeMob.finalizeSpawn`: a random size, bigger on harder regions.
    fn finalize_spawn(
        &self,
        world: &Arc<World>,
        group_data: Option<SpawnGroupData>,
    ) -> Option<SpawnGroupData> {
        let group_data = self.finalize_spawn_super(group_data);
        let difficulty = RegionalDifficulty::at(world, self.get_entity().pos.load());
        let mut rng = self.get_random();
        let mut size_scale = rng.random_range(0..3);
        if size_scale < 2 && rng.random::<f32>() < 0.5 * difficulty.special_multiplier {
            size_scale += 1;
        }
        self.set_size(1 << size_scale, true);
        group_data
    }

    /// Vanilla `AbstractCubeMob.tick` before `super.tick()`.
    fn mob_tick(&self, _caller: &dyn EntityBase) {
        let squish = self.squish.load();
        self.o_squish.store(squish);
        self.squish
            .store(squish + (self.target_squish.load() - squish) * 0.5);
    }

    /// Vanilla `AbstractCubeMob.tick` after `super.tick()`: the landing squish.
    fn post_tick(&self) {
        let on_ground = self.get_entity().on_ground.load(Ordering::Relaxed);
        let was_on_ground = self.was_on_ground.swap(on_ground, Ordering::Relaxed);
        if on_ground && !was_on_ground {
            // The client draws the landing particles; vanilla's server adds none.
            let mut rng = self.get_random();
            let pitch = ((rng.random::<f32>() - rng.random::<f32>()) * 0.2 + 1.0) / 0.8;
            self.play_entity_sound(
                self.species.squish_sound.pick(self.is_tiny()),
                self.get_sound_volume(),
                pitch,
            );
            self.target_squish.store(-0.5);
        } else if !on_ground && was_on_ground {
            self.target_squish.store(1.0);
        }
        self.target_squish
            .store(self.target_squish.load() * self.species.squish_decay);
    }

    /// Vanilla `jumpFromGround`: straight up with the jump power, magma cubes higher by size.
    fn on_jump_from_ground(&self) {
        let entity = self.get_entity();
        let boost = self.get_size() as f32 * self.species.jump_power_per_size;
        let mut velocity = entity.velocity.load();
        velocity.y = self.get_jump_power() + f64::from(boost);
        entity.velocity.store(velocity);
        entity.velocity_dirty.store(true, Ordering::Relaxed);
    }

    /// Vanilla `MagmaCube.jumpInLiquid`: magma cubes shoot up out of lava.
    fn jump_in_liquid(&self, lava: bool) {
        let living = &self.entity.living_entity;
        let Some(lava_jump) = self.species.lava_jump.filter(|_| lava) else {
            living.jump_in_liquid();
            return;
        };
        let speed = lava_jump.base + self.get_size() as f32 * lava_jump.per_size;
        let entity = &living.entity;
        let mut velocity = entity.velocity.load();
        velocity.y = f64::from(speed);
        entity.velocity.store(velocity);
        entity.velocity_dirty.store(true, Ordering::Relaxed);
    }

    /// Vanilla `AbstractCubeMob.push`: hurts an iron golem it bumps into.
    fn mob_push(&self, entity: &dyn EntityBase) {
        crate::entity::push_apart(self, entity);
        if entity.get_entity().entity_type == &EntityType::IRON_GOLEM && self.can_deal_damage() {
            self.deal_damage(entity);
        }
    }

    /// Vanilla `AbstractCubeMob.playerTouch`.
    fn mob_player_collision(&self, player: &Arc<crate::entity::player::Player>) {
        if self.can_deal_damage() {
            self.deal_damage(player.as_ref());
        }
    }

    /// Vanilla `AbstractCubeMob.remove`. Chunk unloads keep the dying cube whole, so its
    /// children never land in an unloading chunk.
    fn on_removed(&self) {
        let size = self.get_size();
        let destroyed = self
            .get_entity()
            .removal_reason
            .load()
            .as_ref()
            .is_some_and(RemovalReason::should_destroy);
        if size > MIN_SIZE && destroyed && self.entity.living_entity.is_dead_or_dying() {
            self.split(size);
        }
    }
}

pub struct SlimeMoveControl {
    slime: Weak<SlimeEntity>,
}

impl SlimeMoveControl {
    #[must_use]
    pub const fn new(slime: Weak<SlimeEntity>) -> Self {
        Self { slime }
    }
}

impl Control for SlimeMoveControl {}

/// Vanilla `CubeMobMoveControl`: turns to the wanted yaw and hops with a delay between jumps.
impl MoveControlTrait for SlimeMoveControl {
    fn set_wanted_position(&mut self, _x: f64, _y: f64, _z: f64, speed_modifier: f64) {
        if let Some(slime) = self.slime.upgrade() {
            slime.set_wanted_movement(speed_modifier);
        }
    }

    fn strafe(&mut self, _forward: f32, _right: f32) {
        if let Some(slime) = self.slime.upgrade() {
            slime.speed_modifier.store(None);
        }
    }

    fn has_wanted(&self) -> bool {
        self.slime
            .upgrade()
            .is_some_and(|slime| slime.speed_modifier.load().is_some())
    }

    fn tick(&mut self, mob: &dyn Mob) {
        let Some(slime) = self.slime.upgrade() else {
            return;
        };
        let mob_entity = mob.get_mob_entity();
        let living_entity = &mob_entity.living_entity;
        let entity = &living_entity.entity;

        let current_yaw = entity.yaw.load();
        let new_yaw =
            MoveControl::rotlerp(current_yaw, slime.target_yaw.load(), MoveControl::MAX_TURN);
        entity.yaw.store(new_yaw);
        entity.head_yaw.store(new_yaw);
        entity.body_yaw.store(new_yaw);

        let Some(speed_modifier) = slime.speed_modifier.take() else {
            let input = living_entity.movement_input.load();
            living_entity
                .movement_input
                .store(Vector3::new(input.x, input.y, 0.0));
            return;
        };
        mob_entity.set_speed(
            (speed_modifier * living_entity.get_attribute_value(&Attributes::MOVEMENT_SPEED))
                as f32,
        );
        if !entity.on_ground.load(Ordering::Relaxed) {
            return;
        }
        if slime.jump_delay.fetch_sub(1, Ordering::Relaxed) <= 0 {
            let mut next_delay = slime.get_jump_delay();
            if slime.is_aggressive.load(Ordering::Relaxed) {
                next_delay /= 3;
            }
            slime.jump_delay.store(next_delay, Ordering::Relaxed);
            mob_entity
                .jump_control
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .jump();
            // Vanilla `doPlayJumpSound`.
            if slime.get_size() > 0 {
                slime.play_entity_sound(
                    slime.species.jump_sound.pick(slime.is_tiny()),
                    slime.get_sound_volume(),
                    slime.get_sound_pitch(),
                );
            }
        } else {
            mob_entity.set_speed(0.0);
            let input = living_entity.movement_input.load();
            living_entity
                .movement_input
                .store(Vector3::new(0.0, input.y, 0.0));
        }
    }
}

fn cube_mob(mob: &dyn Mob) -> Option<&SlimeEntity> {
    mob.cast_any().downcast_ref::<SlimeEntity>()
}

fn is_in_liquid(entity: &Entity) -> bool {
    entity.touching_water.load(Ordering::Relaxed) || entity.touching_lava.load(Ordering::Relaxed)
}

/// Vanilla `CubeMobFloatGoal`: jumps and swims while in water or lava.
pub struct CubeMobFloatGoal;

impl Goal for CubeMobFloatGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        is_in_liquid(mob.get_entity())
    }

    fn tick(&mut self, mob: &dyn Mob) {
        if mob.get_random().random::<f32>() < 0.8 {
            mob.get_mob_entity()
                .jump_control
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .jump();
        }
        if let Some(slime) = cube_mob(mob) {
            slime.set_wanted_movement(FLOAT_SPEED_MODIFIER);
        }
    }

    fn should_run_every_tick(&self) -> bool {
        true
    }

    fn controls(&self) -> Controls {
        Controls::JUMP | Controls::MOVE
    }
}

/// Vanilla `CubeMobAttackGoal`: turns towards the target and hops at it faster.
#[derive(Default)]
pub struct CubeMobAttackGoal {
    grow_tired_timer: i32,
}

impl CubeMobAttackGoal {
    fn can_attack_target(mob: &dyn Mob) -> bool {
        mob.get_mob_entity()
            .get_target()
            .is_some_and(|target| mob.can_attack(target.as_ref()))
    }
}

impl Goal for CubeMobAttackGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        Self::can_attack_target(mob)
    }

    fn start(&mut self, _mob: &dyn Mob) {
        self.grow_tired_timer = to_goal_ticks(GROW_TIRED_TICKS);
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        if !Self::can_attack_target(mob) {
            return false;
        }
        self.grow_tired_timer -= 1;
        self.grow_tired_timer > 0
    }

    fn tick(&mut self, mob: &dyn Mob) {
        let Some(slime) = cube_mob(mob) else {
            return;
        };
        if let Some(target) = slime.entity.get_target() {
            slime.look_at(target.as_ref(), ATTACK_TURN_SPEED);
        }
        slime.set_direction(mob.get_entity().yaw.load(), slime.can_deal_damage());
    }

    fn should_run_every_tick(&self) -> bool {
        true
    }

    fn controls(&self) -> Controls {
        Controls::LOOK
    }
}

/// Vanilla `CubeMobRandomDirectionGoal`: faces a new random way every few seconds.
#[derive(Default)]
pub struct CubeMobRandomDirectionGoal {
    chosen_degrees: f32,
    next_randomize_time: i32,
}

impl Goal for CubeMobRandomDirectionGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        let mob_entity = mob.get_mob_entity();
        let living = &mob_entity.living_entity;
        mob_entity.get_target().is_none()
            && (living.entity.on_ground.load(Ordering::Relaxed)
                || is_in_liquid(&living.entity)
                || living.has_effect(&StatusEffect::LEVITATION))
    }

    fn tick(&mut self, mob: &dyn Mob) {
        self.next_randomize_time -= 1;
        if self.next_randomize_time <= 0 {
            let mut rng = mob.get_random();
            self.next_randomize_time = self.get_tick_count(40 + rng.random_range(0..60));
            self.chosen_degrees = rng.random_range(0..360) as f32;
        }
        if let Some(slime) = cube_mob(mob) {
            slime.set_direction(self.chosen_degrees, false);
        }
    }

    fn controls(&self) -> Controls {
        Controls::LOOK
    }
}

/// Vanilla `CubeMobKeepOnJumpingGoal`: hops on whenever it is not riding.
pub struct CubeMobKeepOnJumpingGoal;

impl Goal for CubeMobKeepOnJumpingGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        !mob.get_entity().has_vehicle()
    }

    fn tick(&mut self, mob: &dyn Mob) {
        if let Some(slime) = cube_mob(mob) {
            slime.set_wanted_movement(JUMPING_SPEED_MODIFIER);
        }
    }

    fn controls(&self) -> Controls {
        Controls::JUMP | Controls::MOVE
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entity::ai::test_support::world;

    #[tokio::test]
    async fn zero_speed_requests_are_consumed_as_movement_operations()
    -> Result<(), Box<dyn std::error::Error>> {
        let folder = tempfile::tempdir()?;
        let world = world(&folder);
        let slime = SlimeEntity::new(Entity::new(
            world.clone(),
            Vector3::new(0.0, 64.0, 0.0),
            &EntityType::SLIME,
        ));
        let living = &slime.entity.living_entity;
        living.entity.on_ground.store(true, Ordering::Relaxed);
        slime.jump_delay.store(10, Ordering::Relaxed);
        slime.entity.set_speed(0.5);
        let (speed, delay, pending) = {
            let mut control = slime
                .entity
                .move_control
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            control.set_wanted_position(0.0, 64.0, 8.0, 0.0);
            control.tick(&*slime);
            (
                living.get_speed(),
                slime.jump_delay.load(Ordering::Relaxed),
                control.has_wanted(),
            )
        };
        slime.entity.set_speed(0.5);
        let waiting_speed = {
            let mut control = slime
                .entity
                .move_control
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            control.tick(&*slime);
            living.get_speed()
        };
        world.level.shutdown().await;
        assert_eq!((speed, delay, pending), (0.0, 9, false));
        assert_eq!(waiting_speed, 0.5);
        assert_eq!(living.movement_input.load().z, 0.0);
        Ok(())
    }
}
