use crate::wit::pumpkin::plugin::event::{Event, EventType, PlayerArmorChangeEventData};

use super::super::FromIntoEvent;

/// An event that occurs after the stack in one of a player's armor slots changed.
///
/// The associated [`PlayerArmorChangeEventData`] contains the player, the armor slot and the
/// stacks before and after the change (`None` for an empty slot). The server compares the armor
/// slots once per tick, so the event also fires when only the count or a component (such as
/// durability) changed, and on the first tick after joining. This event is not cancellable.
pub struct PlayerArmorChangeEvent;
impl FromIntoEvent for PlayerArmorChangeEvent {
    const EVENT_TYPE: EventType = EventType::PlayerArmorChangeEvent;
    type Data = PlayerArmorChangeEventData;

    fn data_from_event(event: Event) -> Self::Data {
        match event {
            Event::PlayerArmorChangeEvent(data) => data,
            _ => panic!("unexpected event"),
        }
    }

    fn data_into_event(data: Self::Data) -> Event {
        Event::PlayerArmorChangeEvent(data)
    }
}
