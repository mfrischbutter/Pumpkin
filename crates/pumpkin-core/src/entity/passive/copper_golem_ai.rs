use pumpkin_data::Block;
use pumpkin_data::entity::EntityType;
use pumpkin_data::environment_attribute::Activity;
use pumpkin_data::sound::Sound;
use pumpkin_data::tag::{self, Taggable};

use super::copper_golem::{CopperGolemEntity, CopperGolemState};
use crate::block::blocks::chest_container;
use crate::entity::EntityBase;
use crate::entity::ai::brain::activity::ActivityData;
use crate::entity::ai::brain::behavior::transport_item_target::TransportItemTarget;
use crate::entity::ai::brain::behavior::transport_items::{
    ContainerInteractionState, TARGET_INTERACTION_TIME, TransportItemsBetweenContainers,
    TransportItemsSettings,
};
use crate::entity::ai::brain::behavior::{
    AnimalPanic, BehaviorControl, CountDownCooldownTicks, DoNothing, LookAtTargetSink,
    MoveToTargetSink, Timed, interact_with_door, random_stroll, run_one_with_conditions,
    set_entity_look_target_sometimes,
};
use crate::entity::ai::brain::memory::{MemoryStatus, types};
use crate::entity::ai::brain::{BrainState, BrainTick};
use crate::entity::mob::Mob;

/// Vanilla `CopperGolemAi.SPEED_MULTIPLIER_WHEN_PANICKING`.
const SPEED_MULTIPLIER_WHEN_PANICKING: f32 = 1.5;
/// Vanilla `CopperGolemAi.SPEED_MULTIPLIER_WHEN_IDLING`.
const SPEED_MULTIPLIER_WHEN_IDLING: f32 = 1.0;
/// Vanilla `CopperGolemAi.TRANSPORT_ITEM_HORIZONTAL_SEARCH_RADIUS`.
const TRANSPORT_ITEM_HORIZONTAL_SEARCH_RADIUS: i32 = 32;
/// Vanilla `CopperGolemAi.TRANSPORT_ITEM_VERTICAL_SEARCH_RADIUS`.
const TRANSPORT_ITEM_VERTICAL_SEARCH_RADIUS: i32 = 8;
/// Vanilla `CopperGolemAi.TICK_TO_START_ON_REACHED_INTERACTION`.
const TICK_TO_START_ON_REACHED_INTERACTION: i32 = 1;
/// Vanilla `CopperGolemAi.TICK_TO_PLAY_ON_REACHED_SOUND`.
const TICK_TO_PLAY_ON_REACHED_SOUND: i32 = 9;

fn as_copper_golem(mob: &dyn Mob) -> Option<&CopperGolemEntity> {
    mob.cast_any().downcast_ref::<CopperGolemEntity>()
}

/// Vanilla `CopperGolemAi.TRANSPORT_ITEM_SOURCE_BLOCK`.
fn is_transport_item_source_block(block: &Block) -> bool {
    block.has_tag(&tag::Block::MINECRAFT_COPPER_CHESTS)
}

/// Vanilla `CopperGolemAi.TRANSPORT_ITEM_DESTINATION_BLOCK`.
fn is_transport_item_destination_block(block: &Block) -> bool {
    block == &Block::CHEST || block == &Block::TRAPPED_CHEST
}

/// Vanilla `CopperGolemAi.getTargetReachedInteractions` with `onReachedTargetInteraction`.
fn on_reached_target_interaction(
    tick: &mut BrainTick<'_>,
    target: &TransportItemTarget,
    interaction_state: ContainerInteractionState,
    ticks_since_reaching_target: i32,
) {
    let Some(golem) = as_copper_golem(tick.mob) else {
        return;
    };
    // Vanilla's `COPPER_GOLEM_ITEM_GET` and `_ITEM_NO_GET` are the `no_item_*` sound events.
    let (state, sound) = match interaction_state {
        ContainerInteractionState::PickupItem => (
            CopperGolemState::GettingItem,
            Sound::EntityCopperGolemNoItemGet,
        ),
        ContainerInteractionState::PickupNoItem => (
            CopperGolemState::GettingNoItem,
            Sound::EntityCopperGolemNoItemNoGet,
        ),
        ContainerInteractionState::PlaceItem => (
            CopperGolemState::DroppingItem,
            Sound::EntityCopperGolemItemDrop,
        ),
        ContainerInteractionState::PlaceNoItem => (
            CopperGolemState::DroppingNoItem,
            Sound::EntityCopperGolemItemNoDrop,
        ),
    };
    if ticks_since_reaching_target == TICK_TO_START_ON_REACHED_INTERACTION {
        golem.set_opened_chest(target.container.clone());
        golem.set_state(state);
    }
    if ticks_since_reaching_target == TICK_TO_PLAY_ON_REACHED_SOUND {
        golem.play_entity_sound(sound, 1.0, 1.0);
    }
    if ticks_since_reaching_target == TARGET_INTERACTION_TIME {
        golem.clear_opened_chest();
    }
}

/// Vanilla `CopperGolemAi.onTravelling`.
fn on_travelling(mob: &dyn Mob) {
    if let Some(golem) = as_copper_golem(mob) {
        golem.clear_opened_chest();
        golem.set_state(CopperGolemState::Idle);
    }
}

/// Vanilla `CopperGolemAi.shouldQueueForTarget`, with the opener count standing in for
/// `getEntitiesWithContainerOpen`: both count the players and golems at the chest.
fn should_queue_for_target(target: &TransportItemTarget) -> bool {
    chest_container::chest_opener_count(target.block_entity.as_ref())
        .is_some_and(|count| count > 0)
}

fn transport_items() -> TransportItemsBetweenContainers {
    TransportItemsBetweenContainers::new(TransportItemsSettings {
        speed_modifier: SPEED_MULTIPLIER_WHEN_IDLING,
        source_block_type: is_transport_item_source_block,
        destination_block_type: is_transport_item_destination_block,
        horizontal_search_distance: TRANSPORT_ITEM_HORIZONTAL_SEARCH_RADIUS,
        vertical_search_distance: TRANSPORT_ITEM_VERTICAL_SEARCH_RADIUS,
        on_target_interaction: on_reached_target_interaction,
        on_start_travelling: on_travelling,
        should_queue_for_target,
    })
}

/// Vanilla `CopperGolemAi.initCoreActivity`.
fn core_activity() -> ActivityData {
    let behaviors: Vec<Box<dyn BehaviorControl>> = vec![
        Box::new(Timed::new(AnimalPanic::new(SPEED_MULTIPLIER_WHEN_PANICKING))),
        Box::new(Timed::new(LookAtTargetSink::new(45, 90))),
        Box::new(Timed::new(MoveToTargetSink::default())),
        Box::new(interact_with_door::create()),
        Box::new(CountDownCooldownTicks::new(types::GAZE_COOLDOWN_TICKS)),
        Box::new(CountDownCooldownTicks::new(
            types::TRANSPORT_ITEMS_COOLDOWN_TICKS,
        )),
    ];
    ActivityData::with_priority_start(Activity::Core, 0, behaviors)
}

/// Vanilla `CopperGolemAi.initIdleActivity`.
fn idle_activity() -> ActivityData {
    ActivityData::with_pairs(
        Activity::Idle,
        vec![
            (0, Box::new(Timed::new(transport_items()))),
            (
                1,
                Box::new(set_entity_look_target_sometimes::create_for(
                    &EntityType::PLAYER,
                    6.0,
                    40,
                    80,
                )),
            ),
            (
                2,
                Box::new(run_one_with_conditions(
                    vec![
                        (types::WALK_TARGET.id(), MemoryStatus::ValueAbsent),
                        (
                            types::TRANSPORT_ITEMS_COOLDOWN_TICKS.id(),
                            MemoryStatus::ValuePresent,
                        ),
                    ],
                    vec![
                        (
                            Box::new(random_stroll::stroll_within(
                                SPEED_MULTIPLIER_WHEN_IDLING,
                                2,
                                2,
                            )),
                            1,
                        ),
                        (Box::new(DoNothing::new(30, 60)), 1),
                    ],
                )),
            ),
        ],
    )
}

/// Vanilla `CopperGolemAi.getActivities`.
pub fn activities(_mob: &dyn Mob) -> Vec<ActivityData> {
    vec![core_activity(), idle_activity()]
}

/// Vanilla `CopperGolemAi.updateActivity`.
pub fn update_activity(brain: &mut BrainState) {
    brain.set_active_activity_to_first_valid(&[Activity::Idle]);
}
