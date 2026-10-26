use crate::wit::pumpkin::plugin::event::{BlockRandomTickEventData, Event, EventType};

use super::super::FromIntoEvent;

/// An event that occurs once per world tick with the random ticks of watched block states.
pub struct BlockRandomTickEvent;
impl FromIntoEvent for BlockRandomTickEvent {
    const EVENT_TYPE: EventType = EventType::BlockRandomTickEvent;
    type Data = BlockRandomTickEventData;

    fn data_from_event(event: Event) -> Self::Data {
        match event {
            Event::BlockRandomTickEvent(data) => data,
            _ => panic!("unexpected event"),
        }
    }

    fn data_into_event(data: Self::Data) -> Event {
        Event::BlockRandomTickEvent(data)
    }
}
