use crate::wit::pumpkin::plugin::event::{BlockScheduledTickEventData, Event, EventType};

use super::super::FromIntoEvent;

/// An event that occurs before a scheduled tick of a hooked block runs.
pub struct BlockScheduledTickEvent;
impl FromIntoEvent for BlockScheduledTickEvent {
    const EVENT_TYPE: EventType = EventType::BlockScheduledTickEvent;
    type Data = BlockScheduledTickEventData;

    fn data_from_event(event: Event) -> Self::Data {
        match event {
            Event::BlockScheduledTickEvent(data) => data,
            _ => panic!("unexpected event"),
        }
    }

    fn data_into_event(data: Self::Data) -> Event {
        Event::BlockScheduledTickEvent(data)
    }
}
