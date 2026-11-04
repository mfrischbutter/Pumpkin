//! The chunk tickets a flying ender pearl holds: vanilla `TicketType.ENDER_PEARL`.

use std::sync::Arc;

use pumpkin_util::math::vector2::Vector2;
use pumpkin_world::{chunk_system::ChunkLoading, level::Level};

/// `TicketType.ENDER_PEARL`'s timeout in ticks.
const TICKET_TIMEOUT: u32 = 40;
/// `ServerPlayer.placeEnderPearlTicket`'s radius, as a level above a full chunk.
const TICKET_LEVEL: i8 = ChunkLoading::FULL_CHUNK_LEVEL - 2;

struct HeldTicket {
    position: Vector2<i32>,
    expires_at: u32,
}

/// The tickets one pearl holds. Vanilla's stay until they time out; here they also end with
/// the pearl, when this is dropped, since the engine has no ticket lifetimes.
#[derive(Default)]
pub(super) struct PearlTickets {
    level: Option<Arc<Level>>,
    held: Vec<HeldTicket>,
    age: u32,
    /// Vanilla `ThrownEnderpearl.ticketTimer`.
    timer: u32,
}

impl PearlTickets {
    /// One tick of the pearl: renews the ticket when the timer runs out or the pearl changed
    /// chunks and lets the old ones expire. Returns whether a ticket was placed.
    pub(super) fn tick(
        &mut self,
        level: &Arc<Level>,
        chunk: Vector2<i32>,
        chunk_changed: bool,
    ) -> bool {
        if self
            .level
            .as_ref()
            .is_some_and(|held_level| !Arc::ptr_eq(held_level, level))
        {
            self.release();
        }
        self.age += 1;
        self.timer = self.timer.saturating_sub(1);
        let age = self.age;
        let place = self.timer == 0 || chunk_changed;
        if !place && !self.held.iter().any(|ticket| ticket.expires_at <= age) {
            return false;
        }

        let mut loading = level
            .chunk_loading
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if place {
            // `registerAndUpdateEnderPearlTicket` returns the timeout less one.
            self.timer = TICKET_TIMEOUT - 1;
            let expires_at = age + TICKET_TIMEOUT;
            if let Some(ticket) = self.held.iter_mut().find(|t| t.position == chunk) {
                ticket.expires_at = expires_at;
            } else {
                loading.add_ticket(chunk, TICKET_LEVEL);
                self.held.push(HeldTicket {
                    position: chunk,
                    expires_at,
                });
            }
        }
        self.held.retain(|ticket| {
            let keep = ticket.expires_at > age;
            if !keep {
                loading.remove_ticket(ticket.position, TICKET_LEVEL);
            }
            keep
        });
        loading.send_change();
        drop(loading);
        self.level = Some(level.clone());
        place
    }

    fn release(&mut self) {
        let Some(level) = self.level.take() else {
            return;
        };
        let mut loading = level
            .chunk_loading
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for ticket in self.held.drain(..) {
            loading.remove_ticket(ticket.position, TICKET_LEVEL);
        }
        loading.send_change();
    }
}

impl Drop for PearlTickets {
    fn drop(&mut self) {
        self.release();
    }
}
