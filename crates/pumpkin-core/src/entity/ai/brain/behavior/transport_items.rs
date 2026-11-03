use std::sync::Arc;

use pumpkin_data::Block;
use pumpkin_data::data_component_impl::EquipmentSlot;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_inventory::Inventory;
use pumpkin_util::math::boundingbox::BoundingBox;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;

use super::super::BrainTick;
use super::super::memory::position_tracker::{BlockPosTracker, PositionTracker};
use super::super::memory::{MemoryModuleId, MemoryStatus, types};
use super::timed::Behavior;
use super::transport_item_target::{
    TransportItemTarget, center_pos, chests_around, get_interaction_range,
    has_valid_travelling_path, is_in_search_area, is_position_already_visited,
    is_within_target_distance, target_has_not_changed,
};
use super::{poi, utils};
use crate::block::blocks::chest_container;
use crate::block::entities::BlockEntity;
use crate::entity::mob::Mob;
use crate::world::World;

/// Vanilla `TransportItemsBetweenContainers.TARGET_INTERACTION_TIME`.
pub const TARGET_INTERACTION_TIME: i32 = 60;
/// Vanilla `VISITED_POSITIONS_MEMORY_TIME`.
const VISITED_POSITIONS_MEMORY_TIME: i64 = 6000;
/// Vanilla `TRANSPORTED_ITEM_MAX_STACK_SIZE`.
const TRANSPORTED_ITEM_MAX_STACK_SIZE: u8 = 16;
/// Vanilla `MAX_VISITED_POSITIONS`.
const MAX_VISITED_POSITIONS: usize = 10;
/// Vanilla `MAX_UNREACHABLE_POSITIONS`.
const MAX_UNREACHABLE_POSITIONS: usize = 50;
/// Vanilla `PASSENGER_MOB_TARGET_SEARCH_DISTANCE`.
const PASSENGER_MOB_TARGET_SEARCH_DISTANCE: i32 = 1;
/// Vanilla `IDLE_COOLDOWN`.
const IDLE_COOLDOWN: i32 = 140;
/// Vanilla `CLOSE_ENOUGH_TO_START_QUEUING_DISTANCE`.
const CLOSE_ENOUGH_TO_START_QUEUING_DISTANCE: f64 = 3.0;
/// Vanilla `CLOSE_ENOUGH_TO_CONTINUE_INTERACTING_WITH_TARGET`.
const CLOSE_ENOUGH_TO_CONTINUE_INTERACTING_WITH_TARGET: f64 = 2.0;
/// How far vanilla's `ChunkPos.rangeClosed` steps per chunk of search distance.
const BLOCKS_PER_CHUNK: i32 = 16;
/// Vanilla overrides `timedOut` to never time out; `Timed` has no such hook.
const NEVER_TIMES_OUT: i32 = i32::MAX - 1;

const ENTRY_CONDITIONS: [(MemoryModuleId, MemoryStatus); 4] = [
    (
        types::VISITED_BLOCK_POSITIONS.id(),
        MemoryStatus::Registered,
    ),
    (
        types::UNREACHABLE_TRANSPORT_BLOCK_POSITIONS.id(),
        MemoryStatus::Registered,
    ),
    (
        types::TRANSPORT_ITEMS_COOLDOWN_TICKS.id(),
        MemoryStatus::ValueAbsent,
    ),
    (types::IS_PANICKING.id(), MemoryStatus::ValueAbsent),
];

/// Vanilla `TransportItemsBetweenContainers.ContainerInteractionState`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ContainerInteractionState {
    PickupItem,
    PickupNoItem,
    PlaceItem,
    PlaceNoItem,
}

/// Vanilla `TransportItemsBetweenContainers.TransportItemState`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum TransportItemState {
    Travelling,
    Queuing,
    Interacting,
}

/// Vanilla `TransportItemsBetweenContainers.OnTargetReachedInteraction`.
pub type OnTargetReachedInteraction =
    fn(&mut BrainTick<'_>, &TransportItemTarget, ContainerInteractionState, i32);

/// The constructor arguments of vanilla `TransportItemsBetweenContainers`.
pub struct TransportItemsSettings {
    pub speed_modifier: f32,
    pub source_block_type: fn(&Block) -> bool,
    pub destination_block_type: fn(&Block) -> bool,
    pub horizontal_search_distance: i32,
    pub vertical_search_distance: i32,
    pub on_target_interaction: OnTargetReachedInteraction,
    pub on_start_travelling: fn(&dyn Mob),
    pub should_queue_for_target: fn(&TransportItemTarget) -> bool,
}

/// Vanilla `TransportItemsBetweenContainers`: carries items from source to destination chests.
pub struct TransportItemsBetweenContainers {
    settings: TransportItemsSettings,
    target: Option<Arc<TransportItemTarget>>,
    state: TransportItemState,
    interaction_state: Option<ContainerInteractionState>,
    ticks_since_reaching_target: i32,
}

impl TransportItemsBetweenContainers {
    #[must_use]
    pub const fn new(settings: TransportItemsSettings) -> Self {
        Self {
            settings,
            target: None,
            state: TransportItemState::Travelling,
            interaction_state: None,
            ticks_since_reaching_target: 0,
        }
    }

    /// Vanilla `updateInvalidTarget`.
    fn update_invalid_target(&mut self, tick: &mut BrainTick<'_>) -> bool {
        if self.has_valid_target(tick) {
            return false;
        }
        self.stop_targeting_current_target(tick);
        if let Some(target) = self.get_transport_target(tick) {
            let pos = target.pos;
            self.target = Some(Arc::new(target));
            self.on_start_travelling(tick);
            self.set_visited_block_pos(tick, pos);
        } else {
            self.enter_cooldown_after_no_matching_target_found(tick);
        }
        true
    }

    /// Vanilla `onQueuingForTarget`.
    fn on_queuing_for_target(&mut self, target: &TransportItemTarget, tick: &mut BrainTick<'_>) {
        if !self.is_another_mob_interacting_with_target(target, tick.world) {
            self.state = TransportItemState::Travelling;
            self.walk_towards_target(tick);
        }
    }

    /// Vanilla `onTravelToTarget`.
    fn on_travel_to_target(&mut self, target: &TransportItemTarget, tick: &mut BrainTick<'_>) {
        let center = center_pos(tick.mob);
        if is_within_target_distance(
            CLOSE_ENOUGH_TO_START_QUEUING_DISTANCE,
            target,
            tick.mob,
            center,
        ) && self.is_another_mob_interacting_with_target(target, tick.world)
        {
            stop_in_place(tick.mob);
            self.state = TransportItemState::Queuing;
        } else if is_within_target_distance(
            get_interaction_range(tick.mob),
            target,
            tick.mob,
            center,
        ) {
            self.interaction_state = Some(container_interaction_state(target, tick));
            self.state = TransportItemState::Interacting;
        } else {
            self.walk_towards_target(tick);
        }
    }

    /// Vanilla `onReachedTarget`.
    fn on_reached_target(&mut self, target: &TransportItemTarget, tick: &mut BrainTick<'_>) {
        if !is_within_target_distance(
            CLOSE_ENOUGH_TO_CONTINUE_INTERACTING_WITH_TARGET,
            target,
            tick.mob,
            center_pos(tick.mob),
        ) {
            self.on_start_travelling(tick);
            return;
        }
        self.ticks_since_reaching_target += 1;
        self.on_target_interaction(target, tick);
        if self.ticks_since_reaching_target >= TARGET_INTERACTION_TIME {
            match container_interaction_state(target, tick) {
                ContainerInteractionState::PickupItem => {
                    self.pick_up_items(tick, target.container.as_ref());
                }
                ContainerInteractionState::PlaceItem => {
                    self.put_down_item(tick, target.container.as_ref());
                }
                ContainerInteractionState::PickupNoItem
                | ContainerInteractionState::PlaceNoItem => {
                    self.stop_targeting_current_target(tick);
                }
            }
            self.on_start_travelling(tick);
        }
    }

    /// Vanilla `walkTowardsTarget`.
    fn walk_towards_target(&self, tick: &mut BrainTick<'_>) {
        if let Some(target) = &self.target {
            utils::set_walk_and_look_target_memories_to_block(
                tick.state,
                target.pos,
                self.settings.speed_modifier,
                0,
            );
        }
    }

    /// Vanilla `onStartTravelling`.
    fn on_start_travelling(&mut self, tick: &BrainTick<'_>) {
        (self.settings.on_start_travelling)(tick.mob);
        self.state = TransportItemState::Travelling;
        self.interaction_state = None;
        self.ticks_since_reaching_target = 0;
    }

    /// Vanilla `onTargetInteraction`.
    fn on_target_interaction(&self, target: &TransportItemTarget, tick: &mut BrainTick<'_>) {
        tick.state.set(
            types::LOOK_TARGET,
            Arc::new(BlockPosTracker::new(target.pos)) as Arc<dyn PositionTracker>,
        );
        stop_in_place(tick.mob);
        if let Some(interaction_state) = self.interaction_state {
            (self.settings.on_target_interaction)(
                tick,
                target,
                interaction_state,
                self.ticks_since_reaching_target,
            );
        }
    }

    /// Vanilla `getTransportTarget`: the closest wanted chest.
    fn get_transport_target(&self, tick: &BrainTick<'_>) -> Option<TransportItemTarget> {
        let entity = tick.mob.get_entity();
        let block_pos = entity.block_pos.load();
        let position = entity.pos.load();
        let horizontal = self.horizontal_search_distance(tick.mob);
        let vertical = self.vertical_search_distance(tick.mob);
        let search_area = BoundingBox::from_block(&block_pos).expand(
            f64::from(horizontal),
            f64::from(vertical),
            f64::from(horizontal),
        );
        let chunk_radius = horizontal.div_euclid(BLOCKS_PER_CHUNK) + 1;
        let mut target = None;
        let mut closest_distance = f64::from(f32::MAX);
        for block_entity in chests_around(tick.world, block_pos.chunk_position(), chunk_radius) {
            let distance = block_entity
                .get_position()
                .to_centered_f64()
                .squared_distance_to_vec(&position);
            if distance < closest_distance
                && let Some(candidate) =
                    self.is_target_valid_to_pick(tick, block_entity, &search_area)
            {
                target = Some(candidate);
                closest_distance = distance;
            }
        }
        target
    }

    /// Vanilla `isTargetValidToPick`; Pumpkin's chests have no lock for `isContainerLocked`.
    fn is_target_valid_to_pick(
        &self,
        tick: &BrainTick<'_>,
        block_entity: Arc<dyn BlockEntity>,
        search_area: &BoundingBox,
    ) -> Option<TransportItemTarget> {
        if !is_in_search_area(search_area, block_entity.get_position()) {
            return None;
        }
        let target = TransportItemTarget::try_create_possible_target(block_entity, tick.world)?;
        (self.is_wanted_block(tick.mob, target.block)
            && !is_position_already_visited(tick, &target))
        .then_some(target)
    }

    /// Vanilla `hasValidTarget`.
    fn has_valid_target(&mut self, tick: &mut BrainTick<'_>) -> bool {
        let Some(target) = self.target.clone() else {
            return false;
        };
        let target_is_of_valid_type = self.is_wanted_block(tick.mob, target.block)
            && target_has_not_changed(tick.world, &target);
        if target_is_of_valid_type && !chest_container::is_chest_blocked(tick.world, &target.pos) {
            if self.state != TransportItemState::Travelling
                || has_valid_travelling_path(&target, tick)
            {
                return true;
            }
            self.mark_visited_block_pos_as_unreachable(tick, target.pos);
        }
        false
    }

    /// Vanilla `getHorizontalSearchDistance`.
    fn horizontal_search_distance(&self, mob: &dyn Mob) -> i32 {
        if mob.get_entity().has_vehicle() {
            PASSENGER_MOB_TARGET_SEARCH_DISTANCE
        } else {
            self.settings.horizontal_search_distance
        }
    }

    /// Vanilla `getVerticalSearchDistance`.
    fn vertical_search_distance(&self, mob: &dyn Mob) -> i32 {
        if mob.get_entity().has_vehicle() {
            PASSENGER_MOB_TARGET_SEARCH_DISTANCE
        } else {
            self.settings.vertical_search_distance
        }
    }

    /// Vanilla `setVisitedBlockPos`.
    fn set_visited_block_pos(&mut self, tick: &mut BrainTick<'_>, target: BlockPos) {
        let mut visited = tick
            .state
            .get(types::VISITED_BLOCK_POSITIONS)
            .cloned()
            .unwrap_or_default();
        visited.insert(poi::global_pos(tick.world, target));
        if visited.len() > MAX_VISITED_POSITIONS {
            self.enter_cooldown_after_no_matching_target_found(tick);
        } else {
            tick.state.set_with_expiry(
                types::VISITED_BLOCK_POSITIONS,
                visited,
                VISITED_POSITIONS_MEMORY_TIME,
            );
        }
    }

    /// Vanilla `markVisitedBlockPosAsUnreachable`.
    fn mark_visited_block_pos_as_unreachable(
        &mut self,
        tick: &mut BrainTick<'_>,
        target: BlockPos,
    ) {
        let global_target = poi::global_pos(tick.world, target);
        let mut visited = tick
            .state
            .get(types::VISITED_BLOCK_POSITIONS)
            .cloned()
            .unwrap_or_default();
        visited.remove(&global_target);
        let mut unreachable = tick
            .state
            .get(types::UNREACHABLE_TRANSPORT_BLOCK_POSITIONS)
            .cloned()
            .unwrap_or_default();
        unreachable.insert(global_target);
        if unreachable.len() > MAX_UNREACHABLE_POSITIONS {
            self.enter_cooldown_after_no_matching_target_found(tick);
        } else {
            tick.state.set_with_expiry(
                types::VISITED_BLOCK_POSITIONS,
                visited,
                VISITED_POSITIONS_MEMORY_TIME,
            );
            tick.state.set_with_expiry(
                types::UNREACHABLE_TRANSPORT_BLOCK_POSITIONS,
                unreachable,
                VISITED_POSITIONS_MEMORY_TIME,
            );
        }
    }

    /// Vanilla `isWantedBlock`.
    fn is_wanted_block(&self, mob: &dyn Mob, block: &Block) -> bool {
        if is_picking_up_items(mob) {
            (self.settings.source_block_type)(block)
        } else {
            (self.settings.destination_block_type)(block)
        }
    }

    /// Vanilla `isAnotherMobInteractingWithTarget`, over the target and its other half.
    fn is_another_mob_interacting_with_target(
        &self,
        target: &TransportItemTarget,
        world: &World,
    ) -> bool {
        let should_queue = self.settings.should_queue_for_target;
        should_queue(target)
            || target
                .connected_target(world)
                .is_some_and(|connected| should_queue(&connected))
    }

    /// Vanilla `pickUpItems`.
    fn pick_up_items(&mut self, tick: &mut BrainTick<'_>, container: &dyn Inventory) {
        let mob_entity = tick.mob.get_mob_entity();
        mob_entity.set_item_slot(
            &EquipmentSlot::MAIN_HAND,
            pickup_item_from_container(container),
        );
        mob_entity.set_guaranteed_drop(&EquipmentSlot::MAIN_HAND);
        container.mark_dirty();
        self.clear_memories_after_matching_target_found(tick);
    }

    /// Vanilla `putDownItem`.
    fn put_down_item(&mut self, tick: &mut BrainTick<'_>, container: &dyn Inventory) {
        let items_left_after_visiting_chest = add_items_to_container(tick.mob, container);
        container.mark_dirty();
        let finished = items_left_after_visiting_chest.is_empty();
        tick.mob
            .get_mob_entity()
            .set_item_slot(&EquipmentSlot::MAIN_HAND, items_left_after_visiting_chest);
        if finished {
            self.clear_memories_after_matching_target_found(tick);
        } else {
            self.stop_targeting_current_target(tick);
        }
    }

    /// Vanilla `stopTargetingCurrentTarget`.
    fn stop_targeting_current_target(&mut self, tick: &mut BrainTick<'_>) {
        self.ticks_since_reaching_target = 0;
        self.target = None;
        tick.mob.get_mob_entity().steered().navigator().stop();
        tick.state.erase(types::WALK_TARGET.id());
    }

    /// Vanilla `clearMemoriesAfterMatchingTargetFound`.
    fn clear_memories_after_matching_target_found(&mut self, tick: &mut BrainTick<'_>) {
        self.stop_targeting_current_target(tick);
        tick.state.erase(types::VISITED_BLOCK_POSITIONS.id());
        tick.state
            .erase(types::UNREACHABLE_TRANSPORT_BLOCK_POSITIONS.id());
    }

    /// Vanilla `enterCooldownAfterNoMatchingTargetFound`.
    fn enter_cooldown_after_no_matching_target_found(&mut self, tick: &mut BrainTick<'_>) {
        self.stop_targeting_current_target(tick);
        tick.state
            .set(types::TRANSPORT_ITEMS_COOLDOWN_TICKS, IDLE_COOLDOWN);
        tick.state.erase(types::VISITED_BLOCK_POSITIONS.id());
        tick.state
            .erase(types::UNREACHABLE_TRANSPORT_BLOCK_POSITIONS.id());
    }
}

fn main_hand_item(mob: &dyn Mob) -> ItemStack {
    mob.get_mob_entity()
        .get_item_by_slot(&EquipmentSlot::MAIN_HAND)
}

/// Vanilla `isPickingUpItems`.
fn is_picking_up_items(mob: &dyn Mob) -> bool {
    main_hand_item(mob).is_empty()
}

/// Vanilla `doReachedTargetInteraction`'s four outcomes, with the loot a vanilla chest unpacks
/// on `isEmpty` opened first so a golem's items do not go into a chest still holding a table.
fn container_interaction_state(
    target: &TransportItemTarget,
    tick: &BrainTick<'_>,
) -> ContainerInteractionState {
    chest_container::unpack_chest_loot(tick.world, &target.pos);
    let container = target.container.as_ref();
    if is_picking_up_items(tick.mob) {
        if container.is_empty() {
            ContainerInteractionState::PickupNoItem
        } else {
            ContainerInteractionState::PickupItem
        }
    } else if container.is_empty() || has_item_matching_hand_item(tick.mob, container) {
        ContainerInteractionState::PlaceItem
    } else {
        ContainerInteractionState::PlaceNoItem
    }
}

/// Vanilla `hasItemMatchingHandItem`.
fn has_item_matching_hand_item(mob: &dyn Mob, container: &dyn Inventory) -> bool {
    let hand_item = main_hand_item(mob).item.id;
    (0..container.size()).any(|slot| {
        let stack = container.get_stack(slot);
        !stack.is_empty() && stack.item.id == hand_item
    })
}

/// Vanilla `pickupItemFromContainer`: up to a limit from the first filled slot.
fn pickup_item_from_container(container: &dyn Inventory) -> ItemStack {
    for slot in 0..container.size() {
        let stack = container.get_stack(slot);
        if !stack.is_empty() {
            let item_count = stack.item_count.min(TRANSPORTED_ITEM_MAX_STACK_SIZE);
            return container.remove_stack_specific(slot, item_count);
        }
    }
    ItemStack::EMPTY.clone()
}

/// Vanilla `addItemsToContainer`: fills matching stacks in slot order, up to the first free slot.
fn add_items_to_container(mob: &dyn Mob, container: &dyn Inventory) -> ItemStack {
    let mut stack = main_hand_item(mob);
    for slot in 0..container.size() {
        let mut existing = container.get_stack(slot);
        if existing.is_empty() {
            container.set_stack(slot, stack);
            return ItemStack::EMPTY.clone();
        }
        let max_stack_size = existing.get_max_stack_size();
        if existing.are_items_and_components_equal(&stack) && existing.item_count < max_stack_size {
            let count_that_can_be_added = max_stack_size - existing.item_count;
            existing.increment(count_that_can_be_added.min(stack.item_count));
            // Vanilla subtracts the whole room here too; below zero is simply empty for it.
            stack.decrement(count_that_can_be_added);
            container.set_stack(slot, existing);
            if stack.is_empty() {
                return ItemStack::EMPTY.clone();
            }
        }
    }
    stack
}

/// Vanilla `stopInPlace`.
fn stop_in_place(mob: &dyn Mob) {
    let mob_entity = mob.get_mob_entity();
    mob_entity.steered().navigator().stop();
    mob_entity
        .living_entity
        .movement_input
        .store(Vector3::default());
    mob_entity.set_speed(0.0);
    let velocity = &mob.get_entity().velocity;
    velocity.store(Vector3::new(0.0, velocity.load().y, 0.0));
}

fn set_can_path_to_targets_below_surface(mob: &dyn Mob, can_path: bool) {
    mob.get_mob_entity()
        .steered()
        .navigator()
        .set_can_path_to_targets_below_surface(can_path);
}

impl Behavior for TransportItemsBetweenContainers {
    fn entry_conditions(&self) -> &[(MemoryModuleId, MemoryStatus)] {
        &ENTRY_CONDITIONS
    }

    fn min_duration(&self) -> i32 {
        NEVER_TIMES_OUT
    }

    fn max_duration(&self) -> i32 {
        NEVER_TIMES_OUT
    }

    fn check_extra_start_conditions(&mut self, tick: &mut BrainTick<'_>) -> bool {
        !tick.mob.get_entity().is_leashed()
    }

    fn can_still_use(&mut self, tick: &mut BrainTick<'_>) -> bool {
        !tick
            .state
            .has_memory_value(types::TRANSPORT_ITEMS_COOLDOWN_TICKS.id())
            && !tick.state.has_memory_value(types::IS_PANICKING.id())
            && !tick.mob.get_entity().is_leashed()
    }

    fn start(&mut self, tick: &mut BrainTick<'_>) {
        set_can_path_to_targets_below_surface(tick.mob, true);
    }

    fn tick(&mut self, tick: &mut BrainTick<'_>) {
        let updated_invalid_target = self.update_invalid_target(tick);
        let Some(target) = self.target.clone() else {
            // As in vanilla the behavior stays running: `stop` here is only its cleanup.
            self.stop(tick);
            return;
        };
        if updated_invalid_target {
            return;
        }
        if self.state == TransportItemState::Queuing {
            self.on_queuing_for_target(&target, tick);
        }
        if self.state == TransportItemState::Travelling {
            self.on_travel_to_target(&target, tick);
        }
        if self.state == TransportItemState::Interacting {
            self.on_reached_target(&target, tick);
        }
    }

    fn stop(&mut self, tick: &mut BrainTick<'_>) {
        self.on_start_travelling(tick);
        set_can_path_to_targets_below_surface(tick.mob, false);
    }

    fn debug_name(&self) -> &'static str {
        "TransportItemsBetweenContainers"
    }
}
