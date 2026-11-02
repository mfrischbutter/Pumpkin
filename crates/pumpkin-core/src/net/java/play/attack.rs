#[allow(clippy::wildcard_imports)]
use super::*;

impl JavaClient {
    pub fn handle_attack(&self, player: &Arc<Player>, attack: &SAttack, server: &Arc<Server>) {
        if !player.has_client_loaded() {
            return;
        }
        player.update_last_action_time();
        let entity_id = attack.entity_id;
        let player_entity = &player.get_entity();
        let world = player_entity.world.load_full();

        let config = &server.advanced_config.pvp;
        if !config.enabled {
            return;
        }

        if entity_id.0 == player.entity_id() {
            self.try_kick(&TextComponent::translate_cross(
                translation::java::MULTIPLAYER_DISCONNECT_INVALID_ENTITY_ATTACKED,
                translation::java::MULTIPLAYER_DISCONNECT_INVALID_ENTITY_ATTACKED,
                [],
            ));
            return;
        }

        let player_target = world.get_player_by_id(entity_id.0);
        let target: Option<Arc<dyn EntityBase>> = player_target
            .as_ref()
            .map(|p| Arc::clone(p) as Arc<dyn EntityBase>)
            .or_else(|| world.get_entity_by_id(entity_id.0));
        // Vanilla `handleAttack`: a target that is already gone is ignored, it may just have died.
        let Some(target) = target else {
            return;
        };
        if !player.can_interact_with_entity(target.as_ref(), 1.0) {
            return;
        }
        // Vanilla `handleAttack`: items, experience orbs and arrows that cannot be hit are not
        // targets, except for a piercing weapon's sweep.
        if player
            .inventory()
            .held_item()
            .get_data_component::<pumpkin_data::data_component_impl::PiercingWeaponImpl>()
            .is_none()
            && is_invalid_attack_target(target.as_ref())
        {
            self.try_kick(&TextComponent::translate_cross(
                translation::java::MULTIPLAYER_DISCONNECT_INVALID_ENTITY_ATTACKED,
                translation::java::MULTIPLAYER_DISCONNECT_INVALID_ENTITY_ATTACKED,
                [],
            ));
            return;
        }
        let mut event = PlayerInteractEntityEvent::new(
            player,
            target,
            ActionType::Attack,
            None,
            player_entity.is_sneaking(),
        );
        server.plugin_manager.fire_blocking(server, &mut event);
        if event.cancelled {
            return;
        }
        if let Some(player_victim) = &player_target {
            if player_victim.living_entity.health.load() <= 0.0 {
                return;
            }
            if config.protect_creative && player_victim.gamemode.load() == GameMode::Creative {
                world.play_sound(
                    Sound::EntityPlayerAttackNodamage,
                    SoundCategory::Players,
                    &player_victim.position(),
                );
                return;
            }
        }
        player.attack(&event.target);
    }
}

fn is_invalid_attack_target(target: &dyn EntityBase) -> bool {
    use pumpkin_data::entity::EntityType;
    use pumpkin_data::tag::{self, Taggable};
    let kind = target.get_entity().entity_type;
    kind == &EntityType::ITEM
        || kind == &EntityType::EXPERIENCE_ORB
        || ((kind == &EntityType::ARROW
            || kind == &EntityType::SPECTRAL_ARROW
            || kind == &EntityType::TRIDENT)
            && !kind.has_tag(&tag::EntityType::MINECRAFT_REDIRECTABLE_PROJECTILE))
}
