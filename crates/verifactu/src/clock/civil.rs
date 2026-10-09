//! Chrono-free civil-calendar math — Howard Hinnant's
//! `civil_from_days`/`days_from_civil` pair; no calendar math outside
//! this module. UTC-only building blocks; timezone policy never lives
//! here.

/// Hinnant's era arithmetic — exact for the whole `i64` range.
#[must_use]
pub fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    (
        if m <= 2 { y + 1 } else { y },
        m.try_into().unwrap_or(1),
        d.try_into().unwrap_or(1),
    )
}

#[must_use]
pub(crate) fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400; // [0, 399]
    let mp: i64 = if month > 2 {
        i64::from(month) - 3
    } else {
        i64::from(month) + 9
    }; // [0, 11]
    let doy = (153 * mp + 2) / 5 + i64::from(day) - 1; // [0, 365]
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0, 146096]
    era * 146_097 + doe - 719_468
}

#[must_use]
pub(crate) fn is_leap_year(year: i64) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

#[must_use]
pub(crate) fn days_in_month(year: i64, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap_year(year) => 29,
        _ => 28,
    }
}

/// `0` = Sunday … `6` = Saturday; day 0 (1970-01-01) was a Thursday,
/// hence the `+ 4`.
#[must_use]
pub(crate) fn weekday(days: i64) -> u32 {
    (days + 4).rem_euclid(7).try_into().unwrap_or(0)
}

/// The `i64::MAX` fallback is unreachable: `u64::MAX / 86_400` fits
/// `i64`.
#[must_use]
pub fn days_since_epoch(epoch_secs: u64) -> i64 {
    i64::try_from(epoch_secs / 86_400).unwrap_or(i64::MAX)
}

#[must_use]
pub fn time_of_day(epoch_secs: u64) -> (u32, u32, u32) {
    let secs = epoch_secs % 86_400;
    let (hour, rest) = (secs / 3600, secs % 3600);
    (
        hour.try_into().unwrap_or(0),
        (rest / 60).try_into().unwrap_or(0),
        (rest % 60).try_into().unwrap_or(0),
    )
}
