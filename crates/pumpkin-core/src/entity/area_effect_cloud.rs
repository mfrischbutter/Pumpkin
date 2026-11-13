use std::collections::HashMap;
use std::sync::atomic::{AtomicI32, Ordering::Relaxed};
use std::sync::{Arc, Mutex, PoisonError};

use crossbeam::atomic::AtomicCell;
use pumpkin_data::data_component_impl::{
    DataComponentImpl, PotionContentsImpl, StatusEffectInstance,
};
use pumpkin_data::effect::StatusEffect;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::particle::Particle;
use pumpkin_data::potion::Potion;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_protocol::codec::var_int::VarInt;

use crate::entity::living::EffectSource;
use crate::entity::projectile::owner::EntityReference;
use crate::entity::projectile::thrown_potion::living_in_box;
use crate::item::potion::{INFINITE_DURATION, PotionContents, PotionEffect};
use crate::{
    entity::{Entity, EntityBase},
    server::Server,
};

const TIME_BETWEEN_APPLICATIONS: i32 = 5;
const MAX_RADIUS: f32 = 32.0;
const MINIMAL_RADIUS: f32 = 0.5;
const DEFAULT_RADIUS: f32 = 3.0;
const HEIGHT: f32 = 0.5;
const DEFAULT_WAIT_TIME: i32 = 20;
const DEFAULT_REAPPLICATION_DELAY: i32 = 20;
const DEFAULT_LINGERING_DURATION: i32 = 600;
/// `applyInstantaneousEffect` of a cloud, at any distance.
const INSTANT_EFFECT_SCALE: f64 = 0.5;
/// `ThrownLingeringPotion.onHitAsPotion`.
const LINGERING_RADIUS_ON_USE: f32 = -0.5;
const LINGERING_WAIT_TIME: i32 = 10;
/// `DragonFireball.onHit` and `DragonSittingFlamingPhase.doServerTick`.
const DRAGON_BREATH_DURATION_SCALE: f32 = 0.25;

/// Particle types whose NBT carries options this server cannot read back yet.
const PARTICLES_WITH_OPTIONS: &[&str] = &[
    "block",
    "block_crumble",
    "block_marker",
    "dust",
    "dust_color_transition",
    "dust_pillar",
    "effect",
    "entity_effect",
    "falling_dust",
    "flash",
    "instant_effect",
    "item",
    "sculk_charge",
    "shriek",
    "tinted_leaves",
    "trail",
    "vibration",
];

#[derive(Clone)]
struct ParticleMeta {
    particle_id: VarInt,
    data: Box<[u8]>,
}

impl ParticleMeta {
    /// Vanilla `ColorParticleOption` of `ENTITY_EFFECT`.
    fn entity_effect(argb: i32) -> Self {
        Self {
            particle_id: VarInt(Particle::EntityEffect as i32),
            data: Box::new(argb.to_be_bytes()),
        }
    }

    /// Vanilla `PowerParticleOption` of `DRAGON_BREATH`.
    fn dragon_breath(power: f32) -> Self {
        Self {
            particle_id: VarInt(Particle::DragonBreath as i32),
            data: Box::new(power.to_be_bytes()),
        }
    }

    /// Reads vanilla's `custom_particle` tag, if its options are understood here.
    fn from_nbt(nbt: &NbtCompound) -> Option<Self> {
        let name = nbt.get_string("type")?;
        let name = name.strip_prefix("minecraft:").unwrap_or(name);
        if name == Particle::DragonBreath.to_name() {
            return Some(Self::dragon_breath(
                nbt.get_numeric_float("power").unwrap_or(1.0),
            ));
        }
        if PARTICLES_WITH_OPTIONS.contains(&name) {
            return None;
        }
        Particle::from_name(name).map(|particle| Self {
            particle_id: VarInt(particle as i32),
            data: Box::new([]),
        })
    }
}

impl pumpkin_protocol::java::client::play::MetadataSerializer for ParticleMeta {
    fn write_metadata(
        &self,
        writer: &mut impl std::io::Write,
        _version: &pumpkin_util::version::JavaMinecraftVersion,
    ) -> Result<(), pumpkin_protocol::ser::WritingError> {
        use pumpkin_protocol::ser::NetworkWriteExt;
        writer.write_var_int(&self.particle_id)?;
        writer.write_slice(&self.data)
    }
}

/// Vanilla `AreaEffectCloud`, for example what a lingering potion leaves where it lands.
pub struct AreaEffectCloudEntity {
    pub entity: Entity,
    /// Vanilla `owner`: credited with the instant damage, and what a bottle looks for.
    pub owner: EntityReference,
    contents: Mutex<PotionContentsImpl>,
    potion_duration_scale: AtomicCell<f32>,
    /// Vanilla `customParticle`: the saved tag, and what it means when it could be read.
    custom_particle: Mutex<Option<(NbtCompound, Option<ParticleMeta>)>>,
    /// Vanilla `victims`: entity id to the tick it can be affected again.
    victims: Mutex<HashMap<i32, i32>>,
    radius: AtomicCell<f32>,
    duration: AtomicI32,
    wait_time: AtomicI32,
    reapplication_delay: AtomicI32,
    duration_on_use: AtomicI32,
    radius_on_use: AtomicCell<f32>,
    radius_per_tick: AtomicCell<f32>,
}

impl AreaEffectCloudEntity {
    fn build(entity: Entity) -> Self {
        entity.no_physics.store(true, Relaxed);
        let cloud = Self {
            entity,
            owner: EntityReference::none(),
            contents: Mutex::new(PotionContentsImpl {
                potion_id: None,
                custom_color: None,
                custom_effects: Vec::new(),
                custom_name: None,
            }),
            potion_duration_scale: AtomicCell::new(1.0),
            custom_particle: Mutex::new(None),
            victims: Mutex::new(HashMap::new()),
            radius: AtomicCell::new(DEFAULT_RADIUS),
            duration: AtomicI32::new(INFINITE_DURATION),
            wait_time: AtomicI32::new(DEFAULT_WAIT_TIME),
            reapplication_delay: AtomicI32::new(DEFAULT_REAPPLICATION_DELAY),
            duration_on_use: AtomicI32::new(0),
            radius_on_use: AtomicCell::new(0.0),
            radius_per_tick: AtomicCell::new(0.0),
        };
        cloud.set_radius(DEFAULT_RADIUS);
        cloud
    }

    #[allow(clippy::new_ret_no_self)]
    pub fn new(entity: Entity) -> Arc<dyn EntityBase> {
        Arc::new(Self::build(entity))
    }

    /// The cloud of vanilla `ThrownLingeringPotion.onHitAsPotion`, with the stack's
    /// `applyComponentsFromItemStack`.
    pub fn from_lingering_potion(
        entity: Entity,
        stack: &ItemStack,
        owner: Option<&Entity>,
    ) -> Arc<dyn EntityBase> {
        let cloud = Self::build(entity);
        cloud.owner.set(owner);
        cloud.radius_on_use.store(LINGERING_RADIUS_ON_USE);
        cloud.duration.store(DEFAULT_LINGERING_DURATION, Relaxed);
        cloud.wait_time.store(LINGERING_WAIT_TIME, Relaxed);
        cloud
            .radius_per_tick
            .store(-DEFAULT_RADIUS / DEFAULT_LINGERING_DURATION as f32);
        if let Some(contents) = stack.get_data_component::<PotionContentsImpl>() {
            *cloud
                .contents
                .lock()
                .unwrap_or_else(PoisonError::into_inner) = contents.clone();
        }
        cloud
            .potion_duration_scale
            .store(PotionContents::duration_scale(stack));
        Arc::new(cloud)
    }

    /// A cloud of dragon's breath: instant damage and its own particle. `radius_per_tick` is
    /// what vanilla's `setRadiusPerTick` sets.
    pub fn dragon_breath(
        entity: Entity,
        radius: f32,
        duration: i32,
        radius_per_tick: f32,
        amplifier: u8,
        owner: Option<&Entity>,
    ) -> Arc<dyn EntityBase> {
        let cloud = Self::build(entity);
        cloud.owner.set(owner);
        cloud.set_radius(radius);
        cloud.duration.store(duration, Relaxed);
        cloud.radius_per_tick.store(radius_per_tick);
        cloud
            .potion_duration_scale
            .store(DRAGON_BREATH_DURATION_SCALE);
        let mut particle = NbtCompound::new();
        particle.put_string("type", "minecraft:dragon_breath".to_string());
        particle.put_float("power", 1.0);
        cloud.set_custom_particle(Some(particle));
        cloud
            .contents
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .custom_effects
            .push(StatusEffectInstance {
                effect_id: StatusEffect::INSTANT_DAMAGE.minecraft_name.into(),
                amplifier: i32::from(amplifier),
                duration: 1,
                ambient: false,
                show_particles: true,
                show_icon: true,
            });
        Arc::new(cloud)
    }

    /// Vanilla `setCustomParticle`.
    fn set_custom_particle(&self, nbt: Option<NbtCompound>) {
        *self
            .custom_particle
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = nbt.map(|nbt| {
            let meta = ParticleMeta::from_nbt(&nbt);
            (nbt, meta)
        });
    }

    #[must_use]
    pub fn radius(&self) -> f32 {
        self.radius.load()
    }

    /// Vanilla `setRadius` and `refreshDimensions`: the box follows the radius, the position
    /// stays.
    pub fn set_radius(&self, radius: f32) {
        let radius = radius.clamp(0.0, MAX_RADIUS);
        self.radius.store(radius);
        self.entity.set_synced_data(
            pumpkin_data::tracked_data::area_effect_cloud::RADIUS,
            radius,
        );
        self.entity.set_dimensions(radius * 2.0, HEIGHT);
    }

    /// Vanilla `updateParticle`: the custom particle, else an opaque potion color.
    fn sync_particle(&self) {
        let custom = self
            .custom_particle
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .and_then(|(_, meta)| meta.clone());
        let meta = custom.unwrap_or_else(|| {
            let color = PotionContents::color(
                &self.contents.lock().unwrap_or_else(PoisonError::into_inner),
            );
            ParticleMeta::entity_effect(color | (0xFFi32 << 24))
        });
        self.entity.set_synced_data(
            pumpkin_data::tracked_data::area_effect_cloud::PARTICLE,
            meta,
        );
    }

    /// Vanilla `serverTick` up to the applications.
    fn server_tick(&self) {
        let tick = self.entity.tick_count.load(Relaxed);
        let duration = self.duration.load(Relaxed);
        let wait_time = self.wait_time.load(Relaxed);
        if duration != INFINITE_DURATION && tick - wait_time >= duration {
            self.entity.remove();
            return;
        }

        let should_wait = tick < wait_time;
        self.entity.set_synced_data(
            pumpkin_data::tracked_data::area_effect_cloud::WAITING,
            should_wait,
        );
        if should_wait {
            return;
        }

        let mut radius = self.radius();
        let radius_per_tick = self.radius_per_tick.load();
        if radius_per_tick != 0.0 {
            radius += radius_per_tick;
            if radius < MINIMAL_RADIUS {
                self.entity.remove();
                return;
            }
            self.set_radius(radius);
        }

        if tick % TIME_BETWEEN_APPLICATIONS == 0 {
            self.apply_to_victims(tick, radius);
        }
    }

    /// Vanilla `forEachEffect`: the effects with their durations scaled.
    fn scaled_effects(&self) -> Vec<PotionEffect> {
        let scale = self.potion_duration_scale.load();
        let mut effects = PotionContents::effects_of(
            &self.contents.lock().unwrap_or_else(PoisonError::into_inner),
        );
        for entry in &mut effects {
            entry.1 = PotionContents::with_scaled_duration(entry.1, scale);
        }
        effects
    }

    /// Vanilla `getOwner`: only a living owner counts.
    fn living_owner(&self) -> Option<Arc<dyn EntityBase>> {
        self.owner
            .get(&self.entity.world.load())
            .filter(|owner| owner.get_living_entity().is_some())
    }

    /// The applications of vanilla `serverTick`, every `TIME_BETWEEN_APPLICATIONS` ticks.
    #[allow(clippy::semicolon_outside_block)]
    fn apply_to_victims(&self, tick: i32, mut radius: f32) {
        let effects = self.scaled_effects();
        {
            let mut victims = self.victims.lock().unwrap_or_else(PoisonError::into_inner);
            if effects.is_empty() {
                victims.clear();
                return;
            }
            victims.retain(|_, until| tick < *until);
        }

        let world = self.entity.world.load_full();
        let owner = self.living_owner();
        let position = self.entity.pos.load();
        for target in living_in_box(&world, &self.entity.bounding_box.load()) {
            let Some(living) = target.get_living_entity() else {
                continue;
            };
            let target_entity = target.get_entity();
            let id = target_entity.entity_id;
            if self
                .victims
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .contains_key(&id)
                || !living.is_affected_by_potions()
                || !effects
                    .iter()
                    .any(|entry| living.can_be_affected(&PotionContents::to_effect(entry)))
            {
                continue;
            }
            let target_position = target_entity.pos.load();
            let (xd, zd) = (
                target_position.x - position.x,
                target_position.z - position.z,
            );
            if xd * xd + zd * zd > f64::from(radius * radius) {
                continue;
            }

            self.victims
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .insert(id, tick + self.reapplication_delay.load(Relaxed));
            for entry in &effects {
                if !living.apply_instant_effect(
                    entry.0,
                    entry.2,
                    Some(INSTANT_EFFECT_SCALE),
                    Some(EffectSource {
                        direct: self,
                        owner: owner.as_deref(),
                    }),
                ) {
                    living.add_effect(PotionContents::to_effect(entry));
                }
            }

            let radius_on_use = self.radius_on_use.load();
            if radius_on_use != 0.0 {
                radius += radius_on_use;
                if radius < MINIMAL_RADIUS {
                    self.entity.remove();
                    return;
                }
                self.set_radius(radius);
            }
            if !self.use_duration() {
                self.entity.remove();
                return;
            }
        }
    }

    /// Takes `durationOnUse` off the duration; `false` when nothing is left.
    fn use_duration(&self) -> bool {
        let duration_on_use = self.duration_on_use.load(Relaxed);
        let duration = self.duration.load(Relaxed);
        if duration_on_use == 0 || duration == INFINITE_DURATION {
            return true;
        }
        let duration = duration + duration_on_use;
        self.duration.store(duration, Relaxed);
        duration > 0
    }

    /// Vanilla `PotionContents.CODEC`: a compound, or the potion's name alone.
    fn read_contents(nbt: &NbtCompound) -> Option<PotionContentsImpl> {
        if let Some(name) = nbt.get_string("potion_contents") {
            let name = name.strip_prefix("minecraft:").unwrap_or(name);
            return Some(PotionContentsImpl {
                potion_id: Potion::from_name(name).map(|potion| i32::from(potion.id)),
                custom_color: None,
                custom_effects: Vec::new(),
                custom_name: None,
            });
        }
        PotionContentsImpl::read_data(nbt.get("potion_contents")?)
    }
}

impl EntityBase for AreaEffectCloudEntity {
    fn init_data_tracker(&self) {
        self.entity.set_synced_data(
            pumpkin_data::tracked_data::area_effect_cloud::RADIUS,
            self.radius(),
        );
        self.sync_particle();
    }

    fn tick(&self, caller: &dyn EntityBase, server: &Server) {
        self.entity.tick(caller, server);
        if !self.entity.is_removed() {
            self.server_tick();
        }
    }

    fn get_entity(&self) -> &Entity {
        &self.entity
    }

    fn get_living_entity(&self) -> Option<&crate::entity::living::LivingEntity> {
        None
    }

    fn cast_any(&self) -> &dyn std::any::Any {
        self
    }

    fn get_owner_entity(&self) -> Option<Arc<dyn EntityBase>> {
        self.living_owner()
    }

    /// Vanilla `addAdditionalSaveData`.
    fn write_custom_nbt(&self, nbt: &mut NbtCompound) {
        nbt.put_int("Age", self.entity.tick_count.load(Relaxed));
        nbt.put_int("Duration", self.duration.load(Relaxed));
        nbt.put_int("WaitTime", self.wait_time.load(Relaxed));
        nbt.put_int("ReapplicationDelay", self.reapplication_delay.load(Relaxed));
        nbt.put_int("DurationOnUse", self.duration_on_use.load(Relaxed));
        nbt.put_float("RadiusOnUse", self.radius_on_use.load());
        nbt.put_float("RadiusPerTick", self.radius_per_tick.load());
        nbt.put_float("Radius", self.radius());
        if let Some((particle, _)) = &*self
            .custom_particle
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
        {
            nbt.put_compound("custom_particle", particle.clone());
        }
        self.owner.write_nbt(nbt, "Owner");
        let contents = self.contents.lock().unwrap_or_else(PoisonError::into_inner);
        if contents.potion_id.is_some()
            || contents.custom_color.is_some()
            || contents.custom_name.is_some()
            || !contents.custom_effects.is_empty()
        {
            nbt.put("potion_contents", contents.write_data());
        }
        let scale = self.potion_duration_scale.load();
        if scale != 1.0 {
            nbt.put_float("potion_duration_scale", scale);
        }
    }

    /// Vanilla `readAdditionalSaveData`, updating only the keys present.
    fn read_custom_nbt(&self, nbt: &NbtCompound) {
        for (key, field) in [
            ("Age", &self.entity.tick_count),
            ("Duration", &self.duration),
            ("WaitTime", &self.wait_time),
            ("ReapplicationDelay", &self.reapplication_delay),
            ("DurationOnUse", &self.duration_on_use),
        ] {
            if let Some(value) = nbt.get_int(key) {
                field.store(value, Relaxed);
            }
        }
        for (key, field) in [
            ("RadiusOnUse", &self.radius_on_use),
            ("RadiusPerTick", &self.radius_per_tick),
            ("potion_duration_scale", &self.potion_duration_scale),
        ] {
            if let Some(value) = nbt.get_numeric_float(key) {
                field.store(value);
            }
        }
        if let Some(radius) = nbt.get_numeric_float("Radius") {
            self.set_radius(radius);
        }
        self.owner.read_nbt(nbt, "Owner");
        if let Some(particle) = nbt.get_compound("custom_particle") {
            self.set_custom_particle(Some(particle.clone()));
        }
        if let Some(contents) = Self::read_contents(nbt) {
            *self.contents.lock().unwrap_or_else(PoisonError::into_inner) = contents;
        }
        self.sync_particle();
    }

    fn remove_custom_nbt(&self, removed: &[Box<str>]) {
        for key in removed {
            match &**key {
                "Owner" => self.owner.clear(),
                "custom_particle" => self.set_custom_particle(None),
                "potion_contents" => {
                    *self.contents.lock().unwrap_or_else(PoisonError::into_inner) =
                        PotionContentsImpl {
                            potion_id: None,
                            custom_color: None,
                            custom_effects: Vec::new(),
                            custom_name: None,
                        };
                }
                "potion_duration_scale" => self.potion_duration_scale.store(1.0),
                _ => {}
            }
        }
        self.sync_particle();
    }
}
