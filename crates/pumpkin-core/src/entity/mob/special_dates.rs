//! Vanilla `SpecialDates`: the calendar days with behaviour of their own.

use time::{Month, OffsetDateTime};

/// Vanilla `SpecialDates.isHalloween`: today is October 31 in the server's time zone.
#[must_use]
pub fn is_halloween() -> bool {
    let today = OffsetDateTime::now_local().unwrap_or_else(|_| OffsetDateTime::now_utc());
    today.month() == Month::October && today.day() == 31
}
