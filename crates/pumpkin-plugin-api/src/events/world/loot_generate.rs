use super::super::FromIntoEvent;
use crate::wit::pumpkin::plugin::event::{Event, EventType, LootGenerateEventData};

/// Event triggered when loot is generated from a loot table, before it is placed.
///
/// Fired for container loot (chests and chest minecarts) and for brushing suspicious
/// sand and gravel. The items can be changed; cancelling the event generates no loot.
pub struct LootGenerateEvent;
impl FromIntoEvent for LootGenerateEvent {
    const EVENT_TYPE: EventType = EventType::LootGenerateEvent;
    type Data = LootGenerateEventData;

    fn data_from_event(event: Event) -> Self::Data {
        match event {
            Event::LootGenerateEvent(data) => data,
            _ => panic!("unexpected event"),
        }
    }

    fn data_into_event(data: Self::Data) -> Event {
        Event::LootGenerateEvent(data)
    }
}
