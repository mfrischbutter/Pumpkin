use std::sync::{
    Arc, Weak,
    atomic::{AtomicI32, Ordering},
};

use pumpkin_data::BlockState;
use pumpkin_data::block_state::SoundType;
use pumpkin_data::damage::DamageType;
use pumpkin_data::data_component::DataComponent;
use pumpkin_data::data_component_impl::{DataComponentImpl, EquipmentSlot, HorseVariantImpl};
use pumpkin_data::entity::EntityType;
use pumpkin_data::entity_variant::{HorseMarkings, HorseVariant};
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::Sound;
use pumpkin_data::tag::{self, Taggable};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_util::math::position::BlockPos;
use rand::RngExt;
use uuid::Uuid;

use crate::entity::custom_sound::CustomSound;
use crate::entity::passive::abstract_horse::{self as horse_base, AbstractHorse, HorseData};
use crate::entity::passive::chested_horse::{self, ChestedHorseEntity};
use crate::entity::{
    Entity, EntityBase, GearDamage,
    ageable::{AgeableData, AgeableMob, DEFAULT_BABY_SPAWN_CHANCE},
    mob::{Mob, MobEntity, spawn::SpawnGroupData},
    passive::animal::Animal,
    player::Player,
};
use crate::world::World;

/// Vanilla `Horse.playGallopSound`: one gallop in ten comes with a breath, this much quieter
/// than the block's sound.
const BREATHE_CHANCE: i32 = 10;
const BREATHE_VOLUME_SCALE: f32 = 0.6;

pub struct HorseEntity {
    pub mob_entity: MobEntity,
    pub ageable_data: AgeableData,
    pub variant: AtomicI32,
    horse: HorseData,
}

impl HorseEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let horse = Self {
            mob_entity: MobEntity::new(entity),
            ageable_data: AgeableData::default(),
            variant: AtomicI32::new(0),
            horse: HorseData::default(),
        };
        let mob_arc = Arc::new(horse);
        let mob_weak: Weak<dyn Mob> = {
            let mob_arc: Arc<dyn Mob> = mob_arc.clone();
            Arc::downgrade(&mob_arc)
        };
        let goal_selector = &mob_arc.mob_entity.goals_selector;
        horse_base::register_horse_goals(goal_selector, mob_weak, true);
        horse_base::add_behaviour_goals(goal_selector);
        mob_arc
    }

    /// The packed variant: colour in the low byte, markings in the next.
    #[must_use]
    pub fn get_variant(&self) -> i32 {
        self.variant.load(Ordering::Relaxed)
    }

    pub fn set_variant(&self, variant: i32) {
        self.variant.store(variant, Ordering::Relaxed);
        self.get_entity().set_synced_data(
            pumpkin_data::tracked_data::horse::DATA_ID_TYPE_VARIANT,
            VarInt(variant),
        );
    }

    fn set_variant_and_markings(&self, color: i32, markings: i32) {
        self.set_variant((color & 0xFF) | ((markings << 8) & 0xFF00));
    }

    #[must_use]
    pub fn is_tame(&self) -> bool {
        horse_base::is_tamed(self)
    }

    /// Vanilla `Horse.getBreedOffspring` for two horses: each of colour and markings comes
    /// from a parent, now and then at random.
    fn foal_variant(&self, partner: &Self) -> (i32, i32) {
        let mut rng = self.get_random();
        let mine = self.get_variant();
        let theirs = partner.get_variant();
        let color = match rng.random_range(0..9) {
            0..4 => mine & 0xFF,
            4..8 => theirs & 0xFF,
            _ => HorseVariant::ALL[rng.random_range(0..HorseVariant::ALL.len())].id(),
        };
        let markings = match rng.random_range(0..5) {
            0..2 => (mine >> 8) & 0xFF,
            2..4 => (theirs >> 8) & 0xFF,
            _ => HorseMarkings::ALL[rng.random_range(0..HorseMarkings::ALL.len())].id(),
        };
        (color, markings)
    }
}

impl AgeableMob for HorseEntity {
    fn get_ageable_data(&self) -> &AgeableData {
        &self.ageable_data
    }

    fn age_boundary_reached(&self) {
        horse_base::refresh_age_dimensions(self);
    }
}

impl Animal for HorseEntity {
    /// Vanilla `AbstractHorse.isFood`.
    fn is_food(&self, item_stack: &ItemStack) -> bool {
        item_stack.item.has_tag(&tag::Item::MINECRAFT_HORSE_FOOD)
    }

    /// Vanilla `Horse.canMate`: with a horse or a donkey, both ready to parent.
    fn can_mate(&self, partner: &dyn EntityBase) -> bool {
        let Some(other) = partner.get_mob().and_then(|mob| mob.as_abstract_horse()) else {
            return false;
        };
        let kind = partner.get_entity().entity_type;
        (kind == &EntityType::HORSE || kind == &EntityType::DONKEY)
            && horse_base::can_parent(self)
            && horse_base::can_parent(other)
    }

    /// Vanilla `Horse.getBreedOffspring`: a mule with a donkey, else a foal with the parents'
    /// looks; stats from both.
    fn get_breed_offspring(&self, partner: &dyn EntityBase) -> Arc<dyn EntityBase> {
        let entity = self.get_entity();
        let new_entity = |kind| Entity::new(entity.world.load_full(), entity.pos.load(), kind);
        if partner.get_entity().entity_type == &EntityType::DONKEY {
            let mule = ChestedHorseEntity::new(new_entity(&EntityType::MULE), &chested_horse::MULE);
            horse_base::set_offspring_attributes(self, partner, mule.as_ref());
            return mule;
        }
        let foal = Self::new(new_entity(&EntityType::HORSE));
        let (color, markings) = partner.cast_any().downcast_ref::<Self>().map_or_else(
            || (self.get_variant() & 0xFF, 0),
            |other| self.foal_variant(other),
        );
        foal.set_variant_and_markings(color, markings);
        horse_base::set_offspring_attributes(self, partner, foal.as_ref());
        foal
    }
}

impl AbstractHorse for HorseEntity {
    fn horse_data(&self) -> &HorseData {
        &self.horse
    }

    fn horse_eating_sound(&self) -> Option<Sound> {
        Some(if self.is_baby() {
            Sound::EntityBabyHorseEat
        } else {
            Sound::EntityHorseEat
        })
    }

    fn horse_angry_sound(&self) -> Option<Sound> {
        Some(if self.is_baby() {
            Sound::EntityBabyHorseAngry
        } else {
            Sound::EntityHorseAngry
        })
    }

    /// Vanilla `Horse.playGallopSound`: now and then a breath with the gallop.
    fn play_gallop_sound(&self, sound_type: &SoundType) {
        horse_base::play_gallop(self, sound_type);
        if self.get_random().random_range(0..BREATHE_CHANCE) == 0 {
            let breathe = if self.is_baby() {
                Sound::EntityBabyHorseBreathe
            } else {
                Sound::EntityHorseBreathe
            };
            self.play_entity_sound(
                breathe,
                sound_type.volume * BREATHE_VOLUME_SCALE,
                sound_type.pitch,
            );
        }
    }
}

impl CustomSound for HorseEntity {
    fn hurt_sound(&self) -> Option<Sound> {
        Some(if self.is_baby() {
            Sound::EntityBabyHorseHurt
        } else {
            Sound::EntityHorseHurt
        })
    }

    fn death_sound(&self) -> Option<Sound> {
        Some(if self.is_baby() {
            Sound::EntityBabyHorseDeath
        } else {
            Sound::EntityHorseDeath
        })
    }

    fn sound_volume(&self) -> f32 {
        horse_base::SOUND_VOLUME
    }
}

impl Mob for HorseEntity {
    /// Vanilla `AbstractHorse.isPushable`.
    fn mob_is_pushable(&self) -> bool {
        !self.get_entity().has_passengers()
    }

    /// Vanilla `supportQuadLeash`.
    fn mob_support_quad_leash(&self) -> bool {
        true
    }

    /// Vanilla `Horse.getAmbientSound`.
    fn get_ambient_sound(&self) -> Option<Sound> {
        Some(if self.is_baby() {
            Sound::EntityBabyHorseAmbient
        } else {
            Sound::EntityHorseAmbient
        })
    }

    fn get_ambient_sound_interval(&self) -> i32 {
        horse_base::AMBIENT_SOUND_INTERVAL
    }

    fn as_custom_sound(&self) -> Option<&dyn CustomSound> {
        Some(self)
    }

    fn is_tamed(&self) -> bool {
        self.is_tame()
    }

    /// Vanilla `AbstractHorse.getOwnerReference`.
    fn get_owner_uuid(&self) -> Option<Uuid> {
        horse_base::owner(self)
    }

    fn mob_play_step_sound(&self, pos: &BlockPos, state: &BlockState) {
        horse_base::play_step_sound(self, pos, state, true);
    }

    fn as_ageable(&self) -> Option<&dyn AgeableMob> {
        Some(self)
    }

    fn as_animal(&self) -> Option<&dyn Animal> {
        Some(self)
    }

    fn as_abstract_horse(&self) -> Option<&dyn AbstractHorse> {
        Some(self)
    }

    fn is_mob_immobile(&self) -> bool {
        horse_base::is_immobile(self)
    }

    fn mob_tick(&self, _caller: &dyn EntityBase) {
        horse_base::horse_tick(self);
        horse_base::horse_ai_step(self);
    }

    fn on_damage(&self, _damage_type: DamageType, _source: Option<&dyn EntityBase>) {
        horse_base::on_hurt(self);
    }

    fn on_elastic_leash_pull(&self) {
        horse_base::on_leash_pull(self);
    }

    fn mob_cause_fall_damage(&self, fall_distance: f32) {
        horse_base::play_land_sound(self, fall_distance);
    }

    /// Vanilla `Horse.hurtArmor`: only the body armour wears.
    fn mob_hurt_gear_slots(&self, gear: GearDamage) -> &'static [EquipmentSlot] {
        match gear {
            GearDamage::Armor => &[EquipmentSlot::BODY],
            GearDamage::Helmet => &[],
        }
    }

    fn mob_start_rider_jump(&self, _strength: i32) {
        if self.is_saddled() {
            horse_base::on_rider_jump(self);
        }
    }

    /// Vanilla `Horse.finalizeSpawn` with `AbstractHorse.randomizeAttributes`: a herd shares
    /// the first horse's colour (`HorseGroupData`), each rolls its markings and stats.
    fn finalize_spawn(
        &self,
        _world: &Arc<World>,
        group_data: Option<SpawnGroupData>,
    ) -> Option<SpawnGroupData> {
        let (color, markings, group_size) = {
            let mut rng = self.get_random();
            let (color, group_size) = match group_data {
                Some(SpawnGroupData::Horse { color, group_size }) => (color, group_size),
                _ => (
                    HorseVariant::ALL[rng.random_range(0..HorseVariant::ALL.len())],
                    0,
                ),
            };
            let markings = HorseMarkings::ALL[rng.random_range(0..HorseMarkings::ALL.len())];
            (color, markings, group_size)
        };
        self.set_variant_and_markings(color.id(), markings.id());
        horse_base::randomize_attributes(self);
        // `HorseGroupData` is an `AgeableMobGroupData(true)`: foals at the plain chance, not
        // the 0.2 that `AbstractHorse.finalizeSpawn` gives a group without data.
        self.finalize_ageable_spawn(Some(SpawnGroupData::Ageable {
            group_size,
            should_spawn_baby: true,
            baby_spawn_chance: DEFAULT_BABY_SPAWN_CHANCE,
        }));
        Some(SpawnGroupData::Horse {
            color,
            group_size: group_size + 1,
        })
    }

    /// Vanilla `Horse.get(HORSE_VARIANT)`: the coat colour.
    fn mob_get_component(&self, component: DataComponent) -> Option<Box<dyn DataComponentImpl>> {
        (component == DataComponent::HorseVariant).then(|| {
            Box::new(HorseVariantImpl {
                value: HorseVariant::by_id(self.get_variant() & 0xFF).name().into(),
            }) as Box<dyn DataComponentImpl>
        })
    }

    /// Vanilla `Horse.applyImplicitComponent(HORSE_VARIANT)`: the colour, keeping the markings.
    fn mob_set_variant_name(&self, name: &str) {
        if let Some(color) = HorseVariant::from_name(name) {
            self.set_variant((color.id() & 0xFF) | (self.get_variant() & !0xFF));
        }
    }

    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        self.write_ageable_nbt(nbt);
        horse_base::write_horse_nbt(self, nbt);
        nbt.put_int("Variant", self.get_variant());
    }

    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        self.read_ageable_nbt(nbt);
        horse_base::read_horse_nbt(self, nbt);
        self.set_variant(nbt.get_int("Variant").unwrap_or(0));
        crate::entity::mob::saddle::read_legacy_saddle(self, nbt);
    }

    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    /// Vanilla `AbstractHorse.canUseSlot(SADDLE)`: a tamed adult.
    fn can_be_saddled(&self) -> bool {
        self.get_entity().is_alive() && !self.is_baby() && self.is_tame()
    }

    /// Vanilla `AbstractHorse.getControllingPassenger`: a player in a saddle.
    fn mob_controlling_player(&self) -> Option<Arc<Player>> {
        horse_base::saddled_rider(self)
    }

    /// Vanilla `AbstractHorse.tickRidden`, the turn to the rider's look.
    fn mob_tick_ridden(&self, controller: &Player) {
        crate::entity::mob::saddle::face_rider(self, controller, 0.5);
    }

    fn mob_init_data_tracker(&self) {
        let entity = self.get_entity();
        if entity.age.load(Ordering::Relaxed) < 0 {
            entity.set_synced_data(pumpkin_data::tracked_data::horse::DATA_BABY_ID, true);
        }
        entity.set_synced_data(
            pumpkin_data::tracked_data::horse::DATA_ID_TYPE_VARIANT,
            VarInt(self.get_variant()),
        );
        entity.set_synced_data(
            pumpkin_data::tracked_data::horse::DATA_ID_FLAGS,
            horse_base::flags(self) as i8,
        );
    }

    /// Vanilla `Horse.mobInteract` with `AbstractHorse.mobInteract`.
    fn mob_interact(&self, player: &Arc<Player>, item_stack: &mut ItemStack) -> bool {
        horse_base::horse_interact(self, player, item_stack, |_| false)
    }
}
