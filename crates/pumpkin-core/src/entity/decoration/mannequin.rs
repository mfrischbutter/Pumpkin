use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU8, Ordering},
};

use pumpkin_data::data_component_impl::{DataComponentImpl, ProfileImpl};
use pumpkin_data::entity::EntityPose;
use pumpkin_data::tracked_data::mannequin as tracked;
use pumpkin_data::translation;
use pumpkin_nbt::{compound::NbtCompound, tag::NbtTag};
use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_util::text::TextComponent;

use crate::entity::{
    Entity, EntityBase, living::LivingEntity, text_component_from_nbt, text_component_to_nbt,
};

/// Vanilla `PlayerModelPart`, by bit.
const MODEL_PARTS: [&str; 7] = [
    "cape",
    "jacket",
    "left_sleeve",
    "right_sleeve",
    "left_pants_leg",
    "right_pants_leg",
    "hat",
];
/// Vanilla `Mannequin.ALL_LAYERS`.
const ALL_LAYERS: u8 = 0x7F;
/// Vanilla `Avatar.DEFAULT_MODEL_CUSTOMIZATION`.
const DEFAULT_MODEL_CUSTOMIZATION: u8 = 0;
/// Vanilla `HumanoidArm` ids.
const ARM_LEFT: u8 = 0;
const ARM_RIGHT: u8 = 1;
/// Vanilla `Mannequin.VALID_POSES`.
const VALID_POSES: [(&str, EntityPose); 5] = [
    ("standing", EntityPose::Standing),
    ("crouching", EntityPose::Crouching),
    ("swimming", EntityPose::Swimming),
    ("fall_flying", EntityPose::FallFlying),
    ("sleeping", EntityPose::Sleeping),
];
/// Vanilla `StringUtil.isValidPlayerName`.
const MAX_PLAYER_NAME_LENGTH: usize = 16;

/// Vanilla `Mannequin.DEFAULT_DESCRIPTION`.
fn default_description() -> TextComponent {
    TextComponent::translate_cross(
        translation::java::ENTITY_MINECRAFT_MANNEQUIN_LABEL,
        translation::java::ENTITY_MINECRAFT_MANNEQUIN_LABEL,
        [],
    )
}

/// Vanilla `ExtraCodecs.PLAYER_NAME`, which `ResolvableProfile.CODEC` applies to the name.
fn has_valid_name(profile: &ProfileImpl) -> bool {
    profile.name.as_ref().is_none_or(|name| {
        name.chars().count() <= MAX_PLAYER_NAME_LENGTH
            && name.chars().all(|c| c > ' ' && c < '\u{7f}')
    })
}

/// Vanilla `Mannequin.LAYERS_CODEC`: the save lists the parts that are hidden.
fn hidden_layers_from_nbt(tag: &NbtTag) -> Option<u8> {
    let mut layers = ALL_LAYERS;
    for part in tag.extract_list()? {
        let name = part.extract_string()?;
        let bit = MODEL_PARTS.iter().position(|&known| known == name)?;
        layers &= !(1u8 << bit);
    }
    Some(layers)
}

fn hidden_layers_to_nbt(layers: u8) -> Vec<NbtTag> {
    MODEL_PARTS
        .iter()
        .enumerate()
        .filter(|(bit, _)| layers & (1u8 << bit) == 0)
        .map(|(_, part)| NbtTag::String((*part).to_string().into()))
        .collect()
}

/// Vanilla `Mannequin`: a player-shaped living entity without a player or an AI.
pub struct MannequinEntity {
    pub living_entity: LivingEntity,
    profile: Mutex<ProfileImpl>,
    model_customisation: AtomicU8,
    main_hand: AtomicU8,
    immovable: AtomicBool,
    description: Mutex<TextComponent>,
    hide_description: AtomicBool,
}

impl MannequinEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let data = &entity.synched_data;
        data.define(tracked::PLAYER_MAIN_HAND, VarInt(i32::from(ARM_RIGHT)));
        data.define(tracked::PLAYER_MODE_CUSTOMISATION, DEFAULT_MODEL_CUSTOMIZATION);
        data.define(tracked::PROFILE, ProfileImpl::default());
        data.define(tracked::IMMOVABLE, false);
        data.define(tracked::DESCRIPTION, Some(default_description()));
        // Vanilla's constructor: unlike the define, all layers are shown and sent.
        entity.set_synced_data(tracked::PLAYER_MODE_CUSTOMISATION, ALL_LAYERS);
        Arc::new(Self {
            living_entity: LivingEntity::new(entity),
            profile: Mutex::new(ProfileImpl::default()),
            model_customisation: AtomicU8::new(ALL_LAYERS),
            main_hand: AtomicU8::new(ARM_RIGHT),
            immovable: AtomicBool::new(false),
            description: Mutex::new(default_description()),
            hide_description: AtomicBool::new(false),
        })
    }

    const fn entity(&self) -> &Entity {
        &self.living_entity.entity
    }

    #[must_use]
    pub fn get_profile(&self) -> ProfileImpl {
        self.profile
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Vanilla `Mannequin.setProfile`. The client resolves a name or id on its own.
    pub fn set_profile(&self, profile: ProfileImpl) {
        *self
            .profile
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = profile.clone();
        self.entity().set_synced_data(tracked::PROFILE, profile);
    }

    fn set_model_customisation(&self, layers: u8) {
        self.model_customisation.store(layers, Ordering::Relaxed);
        self.entity()
            .set_synced_data(tracked::PLAYER_MODE_CUSTOMISATION, layers);
    }

    fn set_main_hand(&self, arm: u8) {
        self.main_hand.store(arm, Ordering::Relaxed);
        self.entity()
            .set_synced_data(tracked::PLAYER_MAIN_HAND, VarInt(i32::from(arm)));
    }

    #[must_use]
    pub fn is_immovable(&self) -> bool {
        self.immovable.load(Ordering::Relaxed)
    }

    pub fn set_immovable(&self, immovable: bool) {
        self.immovable.store(immovable, Ordering::Relaxed);
        self.entity().set_synced_data(tracked::IMMOVABLE, immovable);
    }

    /// Vanilla `Mannequin.getDescription`.
    #[must_use]
    pub fn get_description(&self) -> Option<TextComponent> {
        (!self.hide_description.load(Ordering::Relaxed)).then(|| {
            self.description
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
        })
    }

    /// Vanilla `Mannequin.setDescription` and `setHideDescription`.
    pub fn set_description(&self, description: TextComponent, hide: bool) {
        *self
            .description
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = description;
        self.hide_description.store(hide, Ordering::Relaxed);
        self.entity()
            .set_synced_data(tracked::DESCRIPTION, self.get_description());
    }
}

impl EntityBase for MannequinEntity {
    /// Vanilla `Mannequin.addAdditionalSaveData`.
    fn write_custom_nbt(&self, nbt: &mut NbtCompound) {
        nbt.put("profile", self.get_profile().write_data());
        nbt.put_list(
            "hidden_layers",
            hidden_layers_to_nbt(self.model_customisation.load(Ordering::Relaxed)),
        );
        let arm = self.main_hand.load(Ordering::Relaxed);
        let main_hand = if arm == ARM_LEFT { "left" } else { "right" };
        nbt.put_string("main_hand", main_hand.to_string());
        // `POSE_CODEC` refuses any other pose, so it is not saved.
        let pose = self.entity().pose.load();
        if let Some((name, _)) = VALID_POSES.iter().find(|(_, valid)| *valid == pose) {
            nbt.put_string("pose", (*name).to_string());
        }
        nbt.put_bool("immovable", self.is_immovable());
        match self.get_description() {
            Some(description) if description != default_description() => {
                nbt.put("description", text_component_to_nbt(&description));
            }
            Some(_) => {}
            None => nbt.put_bool("hide_description", true),
        }
    }

    /// Vanilla `Mannequin.readAdditionalSaveData`.
    fn read_custom_nbt(&self, nbt: &NbtCompound) {
        if let Some(profile) = nbt
            .get("profile")
            .and_then(ProfileImpl::read_data)
            .filter(has_valid_name)
        {
            self.set_profile(profile);
        }
        self.set_model_customisation(
            nbt.get("hidden_layers")
                .and_then(hidden_layers_from_nbt)
                .unwrap_or(ALL_LAYERS),
        );
        self.set_main_hand(if nbt.get_string("main_hand") == Some("left") {
            ARM_LEFT
        } else {
            ARM_RIGHT
        });
        let pose = nbt.get_string("pose").and_then(|name| {
            VALID_POSES
                .iter()
                .find(|(valid, _)| *valid == name)
                .map(|(_, pose)| *pose)
        });
        self.entity().set_pose(pose.unwrap_or(EntityPose::Standing));
        self.set_immovable(nbt.get_bool("immovable").unwrap_or(false));
        self.set_description(
            nbt.get("description")
                .map_or_else(default_description, text_component_from_nbt),
            nbt.get_bool("hide_description").unwrap_or(false),
        );
    }

    fn get_entity(&self) -> &Entity {
        self.entity()
    }

    fn get_living_entity(&self) -> Option<&LivingEntity> {
        Some(&self.living_entity)
    }

    fn cast_any(&self) -> &dyn std::any::Any {
        self
    }

    /// Vanilla `Mannequin.isImmobile`.
    fn is_immobile(&self) -> bool {
        self.is_immovable() || self.living_entity.is_dead_or_dying()
    }

    /// Vanilla `Mannequin.isEffectiveAi`: an immovable one does not travel, so it does not fall.
    fn is_effective_ai(&self) -> bool {
        !self.is_immovable()
    }

    /// Vanilla `LivingEntity.isPickable`.
    fn can_hit(&self) -> bool {
        self.entity().is_alive()
    }

    fn is_pushable(&self) -> bool {
        self.living_entity.is_pushable()
    }

    fn get_gravity(&self) -> f64 {
        self.living_entity.get_gravity()
    }

    fn tick_in_void(&self, dyn_self: &dyn EntityBase) {
        self.living_entity.tick_in_void(dyn_self);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Saved are the hidden parts, the synced data has the bits of the shown ones.
    #[test]
    fn hidden_layers_are_the_inverse_of_the_shown_bits() {
        assert!(hidden_layers_to_nbt(ALL_LAYERS).is_empty());

        let hat_hidden = NbtTag::List(vec![NbtTag::String("hat".to_string().into())]);
        let layers = hidden_layers_from_nbt(&hat_hidden);
        assert_eq!(layers, Some(0x3F));
        assert_eq!(hidden_layers_to_nbt(0x3F).len(), 1);
    }

    #[test]
    fn an_unknown_model_part_rejects_the_whole_list() {
        let unknown = NbtTag::List(vec![NbtTag::String("tail".to_string().into())]);
        assert_eq!(hidden_layers_from_nbt(&unknown), None);
    }

    #[test]
    fn player_names_follow_the_codec() {
        let named = |name: &str| ProfileImpl {
            name: Some(name.to_string()),
            ..Default::default()
        };
        assert!(has_valid_name(&named("Steve_1")));
        assert!(!has_valid_name(&named("two words")));
        assert!(!has_valid_name(&named("seventeen_letters")));
        assert!(has_valid_name(&ProfileImpl::default()));
    }
}
