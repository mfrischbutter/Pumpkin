use crate::wit::pumpkin::plugin::event::{BlockShapeUpdateEventData, Event, EventType};

use super::super::FromIntoEvent;

/// An event that occurs before a shape update changes a watched block state.
pub struct BlockShapeUpdateEvent;
impl FromIntoEvent for BlockShapeUpdateEvent {
    const EVENT_TYPE: EventType = EventType::BlockShapeUpdateEvent;
    type Data = BlockShapeUpdateEventData;

    fn data_from_event(event: Event) -> Self::Data {
        match event {
            Event::BlockShapeUpdateEvent(data) => data,
            _ => panic!("unexpected event"),
        }
    }

    fn data_into_event(data: Self::Data) -> Event {
        Event::BlockShapeUpdateEvent(data)
    }
}
