use crate::wit::pumpkin::plugin::event::{Event, EventType, PlayerPickEntityEventData};

use super::super::FromIntoEvent;

/// An event that occurs when a player picks an entity.
///
/// The `item` of the associated [`PlayerPickEntityEventData`] is what the player gets selected
/// or, in creative, given. Replace it to change that, or set it to `None` to give nothing.
pub struct PlayerPickEntityEvent;
impl FromIntoEvent for PlayerPickEntityEvent {
    const EVENT_TYPE: EventType = EventType::PlayerPickEntityEvent;
    type Data = PlayerPickEntityEventData;

    fn data_from_event(event: Event) -> Self::Data {
        match event {
            Event::PlayerPickEntityEvent(data) => data,
            _ => panic!("unexpected event"),
        }
    }

    fn data_into_event(data: Self::Data) -> Event {
        Event::PlayerPickEntityEvent(data)
    }
}
