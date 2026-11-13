use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicI8, Ordering::Relaxed};

use pumpkin_data::BlockState;
use pumpkin_data::damage::DamageType;
use pumpkin_data::sound::Sound;
use pumpkin_data::tag::{self, Taggable};
use pumpkin_data::tracked_data;
use pumpkin_data::world::WorldEvent;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_world::chunk::ChunkHeightmapType;
use rand::RngExt;

use crate::entity::ai::target_predicate::TargetPredicate;
use crate::entity::custom_sound::CustomSound;
use crate::entity::mob::{Mob, MobEntity};
use crate::entity::{Entity, EntityBase};
use crate::world::World;
use crate::world::level_accessor::LevelAccessor;

const ROOSTING_FLAG: i8 = 1;
const CLOSE_PLAYER_DISTANCE: f64 = 4.0;

pub struct BatEntity {
    pub mob_entity: MobEntity,
    hanging_position: Mutex<Option<BlockPos>>,
    /// Vanilla `DATA_ID_FLAGS`, kept whole like vanilla even though only resting is used.
    flags: AtomicI8,
}

impl BatEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let mob_entity = MobEntity::new(entity);
        let bat = Self {
            mob_entity,
            hanging_position: Mutex::new(None),
            flags: AtomicI8::new(ROOSTING_FLAG),
        };
        Arc::new(bat)
    }

    #[must_use]
    pub fn check_bat_spawn_rules(level: &dyn LevelAccessor, pos: &BlockPos) -> bool {
        // Vanilla `getHeightmapPos` is the block above the top one.
        if pos.0.y
            > level.get_level().get_heightmap_height(
                ChunkHeightmapType::WorldSurface,
                pos.0.x,
                pos.0.z,
            )
        {
            return false;
        }
        if rand::random_bool(0.5) {
            return false;
        }
        if level.get_max_local_raw_brightness(pos) > rand::random_range(0..4) {
            return false;
        }
        let below_pos = BlockPos::new(pos.0.x, pos.0.y - 1, pos.0.z);
        level
            .get_block(&below_pos)
            .has_tag(&tag::Block::MINECRAFT_BATS_SPAWNABLE_ON)
    }

    #[must_use]
    pub fn is_roosting(&self) -> bool {
        self.flags.load(Relaxed) & ROOSTING_FLAG != 0
    }

    pub fn set_roosting(&self, roosting: bool) {
        let flags = if roosting {
            self.flags.fetch_or(ROOSTING_FLAG, Relaxed) | ROOSTING_FLAG
        } else {
            self.flags.fetch_and(!ROOSTING_FLAG, Relaxed) & !ROOSTING_FLAG
        };
        self.mob_entity
            .living_entity
            .entity
            .set_synced_data(tracked_data::bat::DATA_ID_FLAGS, flags);
    }

    /// Vanilla `levelEvent(null, 1025, pos, 0)`, unless the bat is silent.
    fn play_takeoff(&self, world: &World) {
        let entity = self.get_entity();
        if !entity.is_silent() {
            world.sync_world_event(WorldEvent::SoundBatLiftoff, entity.block_pos.load(), 0);
        }
    }

    /// Vanilla `level.getNearestPlayer(BAT_RESTING_TARGETING, this)`, within four blocks.
    fn player_is_near(&self, world: &World, pos: Vector3<f64>) -> bool {
        let targeting = TargetPredicate::create_non_attackable().set_base_max_distance(4.0);
        world
            .get_nearby_players(pos, CLOSE_PLAYER_DISTANCE)
            .iter()
            .any(|player| targeting.test(world, Some(self), player.as_ref()))
    }

    /// The resting half of vanilla `customServerAiStep`.
    fn tick_resting(&self, world: &World, above: &BlockPos, pos: Vector3<f64>) {
        let entity = self.get_entity();
        if !world.get_block_state(above).is_solid_block() {
            self.set_roosting(false);
            self.play_takeoff(world);
            return;
        }
        let head_yaw = {
            let mut rng = rand::rng();
            (rng.random_range(0u32..200) == 0).then(|| rng.random_range(0i32..360) as f32)
        };
        if let Some(head_yaw) = head_yaw {
            entity.head_yaw.store(head_yaw);
        }
        if self.player_is_near(world, pos) {
            self.set_roosting(false);
            self.play_takeoff(world);
        }
    }

    /// Vanilla `targetPosition.closerToCenterThan(position(), 2.0)`.
    fn is_close_to(target: BlockPos, pos: Vector3<f64>) -> bool {
        target.to_centered_f64().squared_distance_to_vec(&pos) < 4.0
    }

    /// The target of the flying half of vanilla `customServerAiStep`.
    fn next_target(&self, world: &World, pos: Vector3<f64>) -> BlockPos {
        let mut target = self
            .hanging_position
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if target.is_some_and(|hp| {
            !world.get_block_state(&hp).is_air() || hp.0.y <= world.dimension.min_y
        }) {
            *target = None;
        }
        let mut rng = rand::rng();
        match *target {
            Some(hp) if rng.random_range(0u32..30) != 0 && !Self::is_close_to(hp, pos) => hp,
            _ => {
                // Added up left to right in doubles, like vanilla's `getX() + a - b`.
                let chosen = BlockPos::floored(
                    pos.x + f64::from(rng.random_range(0i32..7))
                        - f64::from(rng.random_range(0i32..7)),
                    pos.y + f64::from(rng.random_range(0i32..6)) - 2.0,
                    pos.z + f64::from(rng.random_range(0i32..7))
                        - f64::from(rng.random_range(0i32..7)),
                );
                *target = Some(chosen);
                chosen
            }
        }
    }

    /// The flying half of vanilla `customServerAiStep`.
    fn tick_flying(&self, world: &World, above: &BlockPos, pos: Vector3<f64>) {
        let entity = self.get_entity();
        let target = self.next_target(world, pos);
        let dx = f64::from(target.0.x) + 0.5 - pos.x;
        let dy = f64::from(target.0.y) + 0.1 - pos.y;
        let dz = f64::from(target.0.z) + 0.5 - pos.z;

        let velocity = entity.velocity.load();
        // Vanilla's float constants, widened to double like Java does.
        let acceleration = f64::from(0.1f32);
        let vertical_speed = f64::from(0.7f32);

        let new_velocity = Vector3::new(
            velocity.x + (pumpkin_util::math::signum(dx) * 0.5 - velocity.x) * acceleration,
            velocity.y
                + (pumpkin_util::math::signum(dy) * vertical_speed - velocity.y) * acceleration,
            velocity.z + (pumpkin_util::math::signum(dz) * 0.5 - velocity.z) * acceleration,
        );
        entity.velocity.store(new_velocity);

        let yaw = (pumpkin_util::math::atan2(new_velocity.z, new_velocity.x) * 180.0
            / f64::from(std::f32::consts::PI)) as f32
            - 90.0;
        let old_yaw = entity.yaw.load();
        let yaw_diff = pumpkin_util::math::wrap_degrees(yaw - old_yaw);

        // Vanilla only sets `zza`.
        let input = self.mob_entity.living_entity.movement_input.load();
        self.mob_entity
            .living_entity
            .movement_input
            .store(Vector3::new(input.x, input.y, 0.5));

        entity.yaw.store(old_yaw + yaw_diff);

        if rand::rng().random_range(0u32..100) == 0 && world.get_block_state(above).is_solid_block()
        {
            self.set_roosting(true);
        }
    }
}

impl CustomSound for BatEntity {
    fn hurt_sound(&self) -> Option<Sound> {
        Some(Sound::EntityBatHurt)
    }

    fn death_sound(&self) -> Option<Sound> {
        Some(Sound::EntityBatDeath)
    }

    /// Vanilla `Bat.getVoicePitch`: for the ambient, hurt and death sounds alike.
    fn voice_pitch(&self) -> f32 {
        self.mob_entity.living_entity.get_pitch() * 0.95
    }

    /// Vanilla `Bat.getSoundVolume`.
    fn sound_volume(&self) -> f32 {
        0.1
    }
}

impl Mob for BatEntity {
    /// Vanilla `Bat.isPushable`.
    fn mob_is_pushable(&self) -> bool {
        false
    }

    /// Vanilla `Bat.doPush` and `pushEntities`: it pushes nothing.
    fn pushes_entities(&self) -> bool {
        false
    }

    /// Vanilla `AmbientCreature.canBeLeashed`.
    fn can_be_leashed(&self) -> bool {
        false
    }

    fn mob_is_flapping(&self) -> bool {
        !self.is_roosting() && self.get_entity().tick_count.load(Relaxed) % 10 == 0
    }

    fn mob_init_data_tracker(&self) {
        let entity = self.get_entity();
        let flags = self.flags.load(Relaxed);
        entity.set_synced_data(tracked_data::bat::DATA_ID_FLAGS, flags);
    }

    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        nbt.put_byte("BatFlags", self.flags.load(Relaxed));
    }

    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        let flags = nbt.get_byte("BatFlags").unwrap_or(0);
        self.flags.store(flags, Relaxed);
        self.get_entity()
            .set_synced_data(tracked_data::bat::DATA_ID_FLAGS, flags);
    }

    fn mob_metadata(&self) -> Option<crate::entity::mob::MobMetadata> {
        Some(crate::entity::mob::MobMetadata::new().bat_flags(self.flags.load(Relaxed)))
    }

    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    fn custom_server_ai_step(&self, _caller: &dyn EntityBase) {
        let entity = &self.mob_entity.living_entity.entity;
        let block_pos = entity.block_pos.load();
        let above_pos = BlockPos::new(block_pos.0.x, block_pos.0.y + 1, block_pos.0.z);
        let world = entity.world.load();
        let pos = entity.pos.load();

        if self.is_roosting() {
            self.tick_resting(&world, &above_pos, pos);
        } else {
            self.tick_flying(&world, &above_pos, pos);
        }
    }

    /// Vanilla `Bat.getAmbientSound`.
    fn get_ambient_sound(&self) -> Option<Sound> {
        (!self.is_roosting() || rand::rng().random_range(0..4) == 0)
            .then_some(Sound::EntityBatAmbient)
    }

    fn as_custom_sound(&self) -> Option<&dyn CustomSound> {
        Some(self)
    }

    /// Vanilla `Bat.getMovementEmission`: only game events, no step sounds.
    fn mob_play_step_sound(&self, _pos: &BlockPos, _state: &BlockState) {}

    /// Vanilla `Bat.checkFallDamage`.
    fn should_check_fall_damage(&self) -> bool {
        false
    }

    /// Vanilla `Bat.tick` after `super.tick()`: a resting bat hangs still under its block, a
    /// flying one keeps 60% of its vertical speed on top of the air drag.
    fn post_tick(&self) {
        let entity = &self.mob_entity.living_entity.entity;
        if self.is_roosting() {
            entity.velocity.store(Vector3::new(0.0, 0.0, 0.0));
            let pos = entity.pos.load();
            let snapped_y = (pos.y.floor()) + 1.0 - f64::from(entity.height());
            entity.set_pos(Vector3::new(pos.x, snapped_y, pos.z));
        } else {
            let mut velocity = entity.velocity.load();
            velocity.y *= 0.6;
            entity.velocity.store(velocity);
        }
    }

    /// Vanilla `Bat.hurtServer`: being hurt wakes it before the hurt cooldown is checked, unless
    /// it is invulnerable; no takeoff sound.
    fn pre_damage_with_context(
        &self,
        damage_type: DamageType,
        _source: Option<&dyn EntityBase>,
        cause: Option<&dyn EntityBase>,
    ) -> bool {
        if self.get_entity().is_invulnerable_to(&damage_type, cause) {
            return false;
        }
        if self.is_roosting() {
            self.set_roosting(false);
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entity::ai::test_support::world;
    use pumpkin_data::entity::EntityType;

    #[tokio::test]
    async fn bat_flags_preserve_other_bits_when_resting_changes()
    -> Result<(), Box<dyn std::error::Error>> {
        let folder = tempfile::tempdir()?;
        let world = world(&folder);
        let bat = BatEntity::new(Entity::new(
            world.clone(),
            Vector3::new(0.0, 64.0, 0.0),
            &EntityType::BAT,
        ));
        let mut saved = NbtCompound::new();
        saved.put_byte("BatFlags", -127);
        bat.mob_read_nbt(&saved);
        assert!(bat.is_roosting());
        bat.mob_write_nbt(&mut saved);
        assert_eq!(saved.get_byte("BatFlags"), Some(-127));
        bat.set_roosting(false);
        bat.mob_write_nbt(&mut saved);
        assert_eq!(saved.get_byte("BatFlags"), Some(-128));
        bat.set_roosting(true);
        bat.mob_write_nbt(&mut saved);
        assert_eq!(saved.get_byte("BatFlags"), Some(-127));
        world.level.shutdown().await;
        Ok(())
    }

    #[tokio::test]
    async fn bat_does_not_power_pressure_plates_or_accumulate_fall_distance()
    -> Result<(), Box<dyn std::error::Error>> {
        use crate::block::blocks::redstone::pressure_plate::{
            PressurePlate, plate::PressurePlateBlock, weighted::WeightedPressurePlateBlock,
        };
        use pumpkin_data::Block;

        let folder = tempfile::tempdir()?;
        let world = world(&folder);
        let pos = BlockPos::new(0, 64, 0);
        let bat = BatEntity::new(Entity::new(
            world.clone(),
            Vector3::new(0.5, 64.0, 0.5),
            &EntityType::BAT,
        ));
        world.add_entity_silent(bat.clone());
        assert_eq!(
            world
                .get_entities_at_box(&pumpkin_util::math::boundingbox::BoundingBox::from_block(
                    &pos
                ))
                .len(),
            1
        );
        let normal =
            PressurePlateBlock.calculate_redstone_output(&world, &Block::OAK_PRESSURE_PLATE, &pos);
        let weighted = WeightedPressurePlateBlock.calculate_redstone_output(
            &world,
            &Block::LIGHT_WEIGHTED_PRESSURE_PLATE,
            &pos,
        );
        bat.mob_entity
            .living_entity
            .fall(bat.as_ref(), -10.0, false, false);
        let fall_distance = bat.mob_entity.living_entity.fall_distance.load();
        world.level.shutdown().await;
        assert_eq!(normal, 0);
        assert_eq!(weighted, 0);
        assert_eq!(fall_distance, 0.0);
        Ok(())
    }

    #[tokio::test]
    async fn damage_wakes_bat_before_hurt_cooldown_but_not_when_invulnerable()
    -> Result<(), Box<dyn std::error::Error>> {
        let folder = tempfile::tempdir()?;
        let world = world(&folder);
        let bat = BatEntity::new(Entity::new(
            world.clone(),
            Vector3::new(0.0, 64.0, 0.0),
            &EntityType::BAT,
        ));
        let entity = bat.get_entity();
        entity.set_invulnerable(true);
        let invulnerable_damage = bat.damage(bat.as_ref(), 1.0, DamageType::GENERIC);
        let stayed_roosting = bat.is_roosting();

        entity.set_invulnerable(false);
        bat.mob_entity
            .living_entity
            .hurt_cooldown
            .store(20, Relaxed);
        bat.mob_entity.living_entity.last_damage_taken.store(2.0);
        let cooldown_damage = bat.damage(bat.as_ref(), 1.0, DamageType::GENERIC);
        let woke_up = !bat.is_roosting();
        world.level.shutdown().await;

        assert!(!invulnerable_damage);
        assert!(stayed_roosting);
        assert!(!cooldown_damage);
        assert!(woke_up);
        Ok(())
    }
}
