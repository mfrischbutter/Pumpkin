use crate::block::entities::BlockEntity;
use crate::entity::EntityBase;
use crate::entity::ai::brain::memory::{MemoryStatus, types};
use crate::world::World;
use crossbeam::atomic::AtomicCell;
use pumpkin_data::block_properties::HorizontalFacing;
use pumpkin_data::effect::StatusEffect;
use pumpkin_data::potion::Effect;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::tag;
use pumpkin_data::tag::Taggable;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::math::boundingbox::BoundingBox;
use pumpkin_util::math::position::BlockPos;
use std::any::Any;
use std::sync::Arc;

pub struct BellBlockEntity {
    pub position: BlockPos,
    pub last_side_hit: AtomicCell<Option<HorizontalFacing>>,
    pub ring_ticks: AtomicCell<i32>,
    pub ringing: AtomicCell<bool>,
    resonating: AtomicCell<bool>,
    resonate_time: AtomicCell<i32>,
}

impl BellBlockEntity {
    pub const ID: &'static str = "minecraft:bell";
    #[must_use]
    pub const fn new(position: BlockPos) -> Self {
        Self {
            position,
            last_side_hit: AtomicCell::new(None),
            ring_ticks: AtomicCell::new(0),
            resonate_time: AtomicCell::new(0),
            resonating: AtomicCell::new(false),
            ringing: AtomicCell::new(false),
        }
    }
    pub fn activate(&self, direction: HorizontalFacing) {
        self.last_side_hit.store(Some(direction));
        if self.ringing.load() {
            self.ring_ticks.store(0);
        } else {
            self.ringing.store(true);
        }
    }

    /// The living entities vanilla `BellBlockEntity.updateEntities` finds within 48 blocks.
    fn nearby_entities(&self, world: &World) -> Vec<Arc<dyn EntityBase>> {
        let center = self.position.0.to_f64();
        world
            .get_entities_at_box(&BoundingBox::new(
                center.sub_raw(48.0, 48.0, 48.0),
                center.add_raw(49.0, 49.0, 49.0),
            ))
            .into_iter()
            .filter(|entity| entity.get_entity().is_alive() && entity.get_living_entity().is_some())
            .collect()
    }

    fn is_within(&self, entity: &Arc<dyn EntityBase>, distance: f64) -> bool {
        self.position
            .to_centered_f64()
            .squared_distance_to_vec(&entity.get_entity().pos.load())
            < distance * distance
    }

    /// Vanilla `BellBlockEntity.updateEntities`: the mobs with a brain within 32 blocks hear it.
    pub fn alert_nearby_mobs(&self, world: &World) {
        let time = world.get_world_age();
        for entity in self.nearby_entities(world) {
            if self.is_within(&entity, 32.0)
                && let Some(mob) = entity.get_mob()
            {
                mob.get_mob_entity().try_with_brain(|brain| {
                    if brain.check(types::HEARD_BELL_TIME.id(), MemoryStatus::Registered) {
                        brain.set(types::HEARD_BELL_TIME, time);
                    }
                });
            }
        }
    }

    fn is_raider(entity: &Arc<dyn EntityBase>) -> bool {
        entity
            .get_entity()
            .entity_type
            .has_tag(&tag::EntityType::MINECRAFT_RAIDERS)
    }

    /// Vanilla `BellBlockEntity.areRaidersNearby`.
    fn raiders_hear_bell(&self, world: &World) -> bool {
        self.nearby_entities(world)
            .iter()
            .any(|entity| Self::is_raider(entity) && self.is_within(entity, 32.0))
    }

    /// Vanilla `BellBlockEntity.makeRaidersGlow`.
    fn make_raiders_glow(&self, world: &World) {
        for entity in self.nearby_entities(world) {
            if Self::is_raider(&entity)
                && self.is_within(&entity, 48.0)
                && let Some(living) = entity.get_living_entity()
            {
                living.add_effect(Effect {
                    effect_type: &StatusEffect::GLOWING,
                    duration: 60,
                    amplifier: 0,
                    ambient: false,
                    show_particles: true,
                    show_icon: true,
                    blend: false,
                });
            }
        }
    }
}

impl BlockEntity for BellBlockEntity {
    fn write_nbt(&self, _nbt: &mut NbtCompound) {}

    fn from_nbt(_nbt: &NbtCompound, position: BlockPos) -> Self
    where
        Self: Sized,
    {
        Self::new(position)
    }

    fn tick(&self, world: &Arc<World>) {
        if self.ringing.load() {
            self.ring_ticks.fetch_add(1);
        }
        if self.ring_ticks.load() >= 50 {
            self.ringing.store(false);
            self.ring_ticks.store(0);
        }
        if self.ring_ticks.load() >= 5
            && self.resonate_time.load() == 0
            && self.raiders_hear_bell(world)
        {
            self.resonating.store(true);
            world.play_sound_fine(
                Sound::BlockBellResonate,
                SoundCategory::Blocks,
                &self.position.to_f64(),
                1.0,
                1.0,
            );
        }

        if self.resonating.load() {
            if self.resonate_time.load() < 40 {
                self.resonate_time.fetch_add(1);
            } else {
                self.make_raiders_glow(world);
                self.resonating.store(false);
            }
        }
    }

    fn resource_location(&self) -> &'static str {
        Self::ID
    }

    fn get_position(&self) -> BlockPos {
        self.position
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}
