use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use uuid::Uuid;

use pumpkin_data::{item_stack::ItemStack, tracked_data::interaction as tracked};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::math::boundingbox::EntityDimensions;

use crate::{
    entity::{
        Entity, EntityBase, living::LivingEntity, player::Player,
        player::advancement::trigger::AdvancementTrigger,
    },
    server::Server,
};

const DEFAULT_WIDTH: f32 = 1.0;
const DEFAULT_HEIGHT: f32 = 1.0;
const DEFAULT_RESPONSE: bool = false;
/// Vanilla `EntityDimensions.scalable`: the eye sits at this fraction of the height.
const EYE_HEIGHT_RATIO: f32 = 0.85;

/// Vanilla `Interaction.getDimensions`.
const fn dimensions(width: f32, height: f32) -> EntityDimensions {
    EntityDimensions::new(width, height, height * EYE_HEIGHT_RATIO)
}

/// Vanilla `Interaction.PlayerAction`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlayerAction {
    pub player: Uuid,
    pub timestamp: i64,
}

impl PlayerAction {
    #[must_use]
    pub fn to_nbt(&self) -> NbtCompound {
        let mut compound = NbtCompound::new();
        compound.put_uuid("player", self.player);
        compound.put_long("timestamp", self.timestamp);
        compound
    }

    #[must_use]
    pub fn from_nbt(compound: &NbtCompound) -> Option<Self> {
        let player = compound.get_uuid("player")?;
        let timestamp = compound.get_long("timestamp")?;
        Some(Self { player, timestamp })
    }
}

/// Vanilla `Interaction`: an invisible hitbox that remembers who last hit and used it.
pub struct InteractionEntity {
    pub entity: Entity,
    pub width: Mutex<f32>,
    pub height: Mutex<f32>,
    pub response: AtomicBool,
    pub attack: Mutex<Option<PlayerAction>>,
    pub interaction: Mutex<Option<PlayerAction>>,
}

impl InteractionEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        entity.no_physics.store(true, Ordering::Relaxed);
        let data = &entity.synched_data;
        data.define(tracked::WIDTH, DEFAULT_WIDTH);
        data.define(tracked::HEIGHT, DEFAULT_HEIGHT);
        data.define(tracked::RESPONSE, DEFAULT_RESPONSE);
        entity.set_default_dimensions(dimensions(DEFAULT_WIDTH, DEFAULT_HEIGHT));

        Arc::new(Self {
            entity,
            width: Mutex::new(DEFAULT_WIDTH),
            height: Mutex::new(DEFAULT_HEIGHT),
            response: AtomicBool::new(DEFAULT_RESPONSE),
            attack: Mutex::new(None),
            interaction: Mutex::new(None),
        })
    }

    #[must_use]
    pub fn get_width(&self) -> f32 {
        *self
            .width
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub fn set_width(&self, width: f32) {
        *self
            .width
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = width;
        self.update_dimensions();
        self.entity.set_synced_data(tracked::WIDTH, width);
    }

    #[must_use]
    pub fn get_height(&self) -> f32 {
        *self
            .height
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub fn set_height(&self, height: f32) {
        *self
            .height
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = height;
        self.update_dimensions();
        self.entity.set_synced_data(tracked::HEIGHT, height);
    }

    #[must_use]
    pub fn get_response(&self) -> bool {
        self.response.load(Ordering::Relaxed)
    }

    pub fn set_response(&self, response: bool) {
        self.response.store(response, Ordering::Relaxed);
        self.entity.set_synced_data(tracked::RESPONSE, response);
    }

    /// Vanilla `Interaction.onSyncedDataUpdated`. Stored as the entity's own size, so a teleport
    /// keeps the box.
    pub fn update_dimensions(&self) {
        self.entity
            .set_default_dimensions(dimensions(self.get_width(), self.get_height()));
    }

    /// The player's action behind vanilla's `Attackable.getLastAttacker`.
    #[must_use]
    pub fn get_last_attacker(&self) -> Option<PlayerAction> {
        *self
            .attack
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The player's action behind vanilla's `Targeting.getTarget`.
    #[must_use]
    pub fn get_target(&self) -> Option<PlayerAction> {
        *self
            .interaction
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// `getGameTime`, which `/time set` leaves alone.
    fn action_now(&self, player: &Player) -> PlayerAction {
        PlayerAction {
            player: player.gameprofile.id,
            timestamp: self.entity.world.load().get_world_age(),
        }
    }
}

impl EntityBase for InteractionEntity {
    /// Vanilla `Interaction.addAdditionalSaveData`.
    fn write_custom_nbt(&self, nbt: &mut NbtCompound) {
        nbt.put_float("width", self.get_width());
        nbt.put_float("height", self.get_height());
        if let Some(attack) = self.get_last_attacker() {
            nbt.put_compound("attack", attack.to_nbt());
        }
        if let Some(interaction) = self.get_target() {
            nbt.put_compound("interaction", interaction.to_nbt());
        }
        nbt.put_bool("response", self.get_response());
    }

    /// Vanilla `Interaction.readAdditionalSaveData`: through the setters, so `/data` reaches the
    /// clients.
    fn read_custom_nbt(&self, nbt: &NbtCompound) {
        self.set_width(nbt.get_numeric_float("width").unwrap_or(DEFAULT_WIDTH));
        self.set_height(nbt.get_numeric_float("height").unwrap_or(DEFAULT_HEIGHT));
        *self
            .attack
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            nbt.get_compound("attack").and_then(PlayerAction::from_nbt);
        *self
            .interaction
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = nbt
            .get_compound("interaction")
            .and_then(PlayerAction::from_nbt);
        self.set_response(nbt.get_bool("response").unwrap_or(DEFAULT_RESPONSE));
    }

    fn tick(&self, _caller: &dyn EntityBase, _server: &Server) {}

    /// Every value already reached the synced data through its setter.
    fn init_data_tracker(&self) {}

    fn get_entity(&self) -> &Entity {
        &self.entity
    }

    fn get_living_entity(&self) -> Option<&LivingEntity> {
        None
    }

    fn cast_any(&self) -> &dyn std::any::Any {
        self
    }

    fn is_pushed_by_fluids(&self) -> bool {
        false
    }

    /// Vanilla `Interaction.isPickable`.
    fn can_hit(&self) -> bool {
        true
    }

    fn can_be_hit_by_projectile(&self) -> bool {
        false
    }

    fn is_immune_to_explosion(&self) -> bool {
        true
    }

    /// Vanilla `Interaction.skipAttackInteraction`: without `response` the attack ends here,
    /// without a sound, a cooldown or a used weapon.
    fn skip_attack_interaction(&self, source: &Player) -> bool {
        *self
            .attack
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(self.action_now(source));
        source.trigger_advancement(AdvancementTrigger::AttackedInteractionEntity);
        !self.get_response()
    }

    /// Vanilla `Interaction.interact`: always `CONSUME`.
    fn interact(&self, player: &Arc<Player>, _item_stack: &mut ItemStack) -> bool {
        *self
            .interaction
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(self.action_now(player));
        true
    }
}
