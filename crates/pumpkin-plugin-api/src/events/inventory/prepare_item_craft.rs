use super::super::FromIntoEvent;
use crate::wit::pumpkin::plugin::event::{Event, EventType, PrepareItemCraftEventData};

/// Event triggered when a crafting grid changes, before the result slot is updated.
///
/// `matrix` holds the grid row by row (`None` for empty slots). Setting `result` changes the
/// stack the result slot shows; cancelling the event empties it.
pub struct PrepareItemCraftEvent;
impl FromIntoEvent for PrepareItemCraftEvent {
    const EVENT_TYPE: EventType = EventType::PrepareItemCraftEvent;
    type Data = PrepareItemCraftEventData;

    fn data_from_event(event: Event) -> Self::Data {
        match event {
            Event::PrepareItemCraftEvent(data) => data,
            _ => panic!("unexpected event"),
        }
    }

    fn data_into_event(data: Self::Data) -> Event {
        Event::PrepareItemCraftEvent(data)
    }
}
