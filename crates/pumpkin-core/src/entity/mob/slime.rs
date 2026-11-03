use std::sync::Arc;
use std::sync::atomic::Ordering;

use pumpkin_data::attributes::Attributes;
use pumpkin_data::damage::DamageType;
use pumpkin_data::entity::EntityType;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::tag::{self, Taggable};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::Difficulty;
use pumpkin_util::math::position::BlockPos;

use crate::entity::ai::behavior::hunting::{self, Hunt};
use crate::entity::ai::target_match::TargetMatch;
use crate::entity::custom_sound::CustomSound;
use crate::entity::mob::cube_mob::{self, CubeMob, CubeMobAttackGoal, CubeMobState};
use crate::entity::mob::equipment::RegionalDifficulty;
use crate::entity::mob::magma_cube::MAGMA_CUBE;
use crate::entity::{
    Entity, EntityBase,
    mob::{
        Mob, MobEntity, MobMetadata,
        spawn::{EntitySpawnReason, SpawnGroupData},
    },
};
use crate::world::World;
use crate::world::level_accessor::LevelAccessor;
use pumpkin_util::random::RandomImpl;
use rand::RngExt;

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
    pub(crate) cube: Arc<CubeMobState>,
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
            cube: Arc::new(CubeMobState::default()),
        });

        cube_mob::install_move_control(&slime.entity, slime.cube.clone());
        cube_mob::add_common_goals(&slime.entity.goals_selector);
        slime
            .entity
            .goals_selector
            .add_goal(2, Box::new(CubeMobAttackGoal::default()));
        let targets = &slime.entity.target_selector;
        hunting::apply(targets, &slime.entity, CUBE_MOB_HUNTS);

        slime
    }

    /// Vanilla `Slime.setSize`: the shared part, then what only slimes set.
    pub fn set_size(&self, size: i32, update_health: bool) {
        let actual_size = self.set_cube_size(size, update_health);
        let living = &self.entity.living_entity;
        living.set_attribute_base(&Attributes::ATTACK_DAMAGE, f64::from(actual_size));
        // Vanilla `MagmaCube.setSize` uses the size before clamping here.
        if let Some(armor_per_size) = self.species.armor_per_size {
            let armor = size.wrapping_mul(armor_per_size);
            living.set_attribute_base(&Attributes::ARMOR, f64::from(armor));
        }
    }

    #[must_use]
    pub fn get_size(&self) -> i32 {
        self.cube.get_size()
    }

    #[must_use]
    pub fn is_tiny(&self) -> bool {
        self.cube.is_tiny()
    }

    /// Vanilla `Slime.checkSlimeSpawnRules`: surface slimes in swamps by the moon's spawn
    /// chance, slime chunks below y 40; a spawner only needs the floor.
    pub fn check_slime_spawn_rules(
        level: &dyn LevelAccessor,
        reason: EntitySpawnReason,
        pos: &BlockPos,
    ) -> bool {
        if level.get_level().level_info.load().difficulty == Difficulty::Peaceful {
            return false;
        }
        let entity_type = &EntityType::SLIME;
        if reason.is_spawner() {
            return MobEntity::check_mob_spawn_rules(entity_type, level, reason, pos);
        }

        let mut rng = rand::rng();
        if level
            .get_biome(pos)
            .has_tag(&tag::WorldgenBiome::MINECRAFT_ALLOWS_SURFACE_SLIME_SPAWNS)
            && pos.0.y > 50
            && pos.0.y < 70
            && rng.random::<f32>() < level.get_level().surface_slime_spawn_chance(pos)
            && level.get_max_local_raw_brightness(pos) <= rng.random_range(0..8)
        {
            return MobEntity::check_mob_spawn_rules(entity_type, level, reason, pos);
        }

        let chunk_pos = pos.chunk_position();
        let slime_seed = pumpkin_util::random::seed_slime_chunk(
            chunk_pos.x,
            chunk_pos.y,
            level.get_level().level.slime_seed(),
            987_234_911,
        );
        let mut slime_rand = pumpkin_util::random::legacy_rand::LegacyRand::from_seed(slime_seed);
        if rng.random_range(0..10) == 0 && slime_rand.next_bounded_i32(10) == 0 && pos.0.y < 40 {
            return MobEntity::check_mob_spawn_rules(entity_type, level, reason, pos);
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

    /// Vanilla `AbstractCubeMob.remove`: a dying cube splits into two to four of half its size.
    fn split(&self) {
        let count = 2 + self.get_random().random_range(0..3);
        cube_mob::spawn_split_cubes(self, count, Self::new, |child, half_size| {
            child.set_size(half_size, true);
        });
    }
}

impl CustomSound for SlimeEntity {
    fn death_sound(&self) -> Option<Sound> {
        Some(self.species.death_sound.pick(self.is_tiny()))
    }

    fn hurt_sound(&self) -> Option<Sound> {
        Some(self.species.hurt_sound.pick(self.is_tiny()))
    }

    fn sound_volume(&self) -> f32 {
        self.cube_sound_volume()
    }
}

impl CubeMob for SlimeEntity {
    fn cube_state(&self) -> &CubeMobState {
        &self.cube
    }

    fn jump_sound(&self) -> Sound {
        self.species.jump_sound.pick(self.is_tiny())
    }

    fn squish_sound(&self) -> Sound {
        self.species.squish_sound.pick(self.is_tiny())
    }

    fn can_deal_damage(&self) -> bool {
        (self.species.tiny_deals_damage || !self.is_tiny()) && !self.entity.is_no_ai()
    }

    fn jump_delay(&self) -> i32 {
        self.get_random().random_range(10..30) * self.species.jump_delay_factor
    }

    fn squish_decay(&self) -> f32 {
        self.species.squish_decay
    }
}

impl Mob for SlimeEntity {
    fn as_custom_sound(&self) -> Option<&dyn CustomSound> {
        Some(self)
    }

    fn as_cube_mob(&self) -> Option<&dyn CubeMob> {
        Some(self)
    }

    fn mob_sound_source(&self) -> SoundCategory {
        SoundCategory::Hostile
    }

    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        nbt.put_int("Size", self.get_size() - 1);
        nbt.put_bool("wasOnGround", self.cube.was_on_ground());
    }

    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        self.set_size(nbt.get_int("Size").unwrap_or(0) + 1, false);
        // Vanilla reads the size before the living data, so without a saved Health the cube
        // gets the max health of its size, not of the default one.
        if nbt.get_numeric_float("Health").is_none() {
            let living = &self.entity.living_entity;
            living.set_health(living.get_max_health());
        }
        self.cube
            .set_was_on_ground(nbt.get_bool("wasOnGround").unwrap_or(false));
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
        self.cube.tick_squish();
    }

    /// Vanilla `AbstractCubeMob.tick` after `super.tick()`: the landing squish.
    fn post_tick(&self) {
        cube_mob::land_squish(self);
    }

    /// Vanilla `jumpFromGround`: straight up with the jump power, magma cubes higher by size.
    fn on_jump_from_ground(&self) {
        let boost = self.get_size() as f32 * self.species.jump_power_per_size;
        cube_mob::jump_straight_up(self, boost);
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

    /// Vanilla `AbstractCubeMob.remove`.
    fn on_removed(&self) {
        if cube_mob::should_split(self) {
            self.split();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entity::ai::test_support::world;
    use pumpkin_util::math::vector3::Vector3;

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
        slime.cube.jump_delay.store(10, Ordering::Relaxed);
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
                slime.cube.jump_delay.load(Ordering::Relaxed),
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
