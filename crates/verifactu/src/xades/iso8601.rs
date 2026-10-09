//! UTC instant rendering on `clock::civil`. `SigningTime` needs
//! `xsd:dateTime` (`Z`); bergshamra's `with_verification_time` wants
//! `YYYY-MM-DD+HH:MM:SS`.

use crate::clock::civil::{civil_from_days, days_since_epoch, time_of_day};
use crate::clock::Timestamp;

#[must_use]
pub fn format_utc(instant: Timestamp) -> String {
    let (year, month, day) = civil_from_days(days_since_epoch(instant.0));
    let (hour, minute, second) = time_of_day(instant.0);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

#[must_use]
pub fn format_verification_time(instant: Timestamp) -> String {
    let (year, month, day) = civil_from_days(days_since_epoch(instant.0));
    let (hour, minute, second) = time_of_day(instant.0);
    format!("{year:04}-{month:02}-{day:02}+{hour:02}:{minute:02}:{second:02}")
}
