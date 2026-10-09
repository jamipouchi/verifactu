//! The Spanish site-zone rule: RD 236/2002 defines exactly two legal
//! zones — `Europe/Madrid` and `Atlantic/Canary` — both on the one EU
//! DST schedule (2000/84/EC): last Sunday of March 01:00 UTC to last
//! Sunday of October 01:00 UTC — zone-independent transition instants.

use crate::clock::civil::{
    civil_from_days, days_from_civil, days_in_month, days_since_epoch, time_of_day, weekday,
};

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum SiteZone {
    EuropeMadrid,
    AtlanticCanary,
}

impl SiteZone {
    #[must_use]
    pub fn parse(code: &str) -> Option<Self> {
        match code {
            "Europe/Madrid" => Some(Self::EuropeMadrid),
            "Atlantic/Canary" => Some(Self::AtlanticCanary),
            _ => None,
        }
    }

    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::EuropeMadrid => "Europe/Madrid",
            Self::AtlanticCanary => "Atlantic/Canary",
        }
    }

    #[must_use]
    const fn base_offset_secs(self) -> i64 {
        match self {
            Self::EuropeMadrid => 3_600,
            Self::AtlanticCanary => 0,
        }
    }

    #[must_use]
    pub fn offset_secs(self, epoch_secs: u64) -> i64 {
        self.base_offset_secs() + i64::from(dst_active_utc(epoch_secs)) * 3_600
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CivilDateTime {
    pub year: i64,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
}

#[must_use]
pub fn civil_datetime(zone: SiteZone, epoch_secs: u64) -> CivilDateTime {
    let local = local_epoch_secs(zone, epoch_secs);
    let (year, month, day) = civil_from_days(days_since_epoch(local));
    let (hour, minute, second) = time_of_day(local);
    CivilDateTime {
        year,
        month,
        day,
        hour,
        minute,
        second,
    }
}

/// # Panics
///
/// Panics on a negative zone offset — unreachable for the two Spanish
/// legal zones (RD 236/2002): reaching it means the vocabulary grew a
/// negative base without revisiting this invariant.
#[must_use]
fn local_epoch_secs(zone: SiteZone, epoch_secs: u64) -> u64 {
    epoch_secs.saturating_add(
        u64::try_from(zone.offset_secs(epoch_secs))
            .expect("Spanish zone offsets are non-negative (RD 236/2002)"),
    )
}

/// The directive window of the instant's OWN UTC civil year — safe
/// because December/January are always winter in every EU zone.
fn dst_active_utc(epoch_secs: u64) -> bool {
    let year = civil_from_days(days_since_epoch(epoch_secs)).0;
    let spring = last_sunday_utc(year, 3) + 3_600; // 01:00 UTC
    let autumn = last_sunday_utc(year, 10) + 3_600; // 01:00 UTC
    epoch_secs >= spring && epoch_secs < autumn
}

fn last_sunday_utc(year: i64, month: u32) -> u64 {
    let last = days_from_civil(year, month, days_in_month(year, month));
    let sunday = last - i64::from(weekday(last));
    u64::try_from(sunday * 86_400)
        .expect("called with a year derived from a u64 epoch — at least 1970")
}

#[cfg(test)]
mod tests {
    use super::{civil_datetime, SiteZone};

    /// The DST law (RD 236/2002 + 2000/84/EC): Madrid springs forward at
    /// 01:00 UTC on March's last Sunday, falls back at 01:00 UTC on
    /// October's last Sunday — a wrong second silently corrupts every
    /// `FechaHoraHusoGenRegistro` while the hash stays consistent.
    #[test]
    fn the_march_and_october_transitions_flip_at_the_legal_utc_seconds() {
        // 2026-03-29 01:00 UTC = 1774746000 (March's last Sunday).
        let at = civil_datetime(SiteZone::EuropeMadrid, 1_774_745_999);
        assert_eq!((at.hour, at.minute, at.second), (1, 59, 59));
        let jumped = civil_datetime(SiteZone::EuropeMadrid, 1_774_746_000);
        assert_eq!((jumped.hour, jumped.minute, jumped.second), (3, 0, 0));

        // 2026-10-25 01:00 UTC = 1792890000 (October's last Sunday).
        let back = civil_datetime(SiteZone::EuropeMadrid, 1_792_890_000);
        assert_eq!((back.hour, back.day), (2, 25));
    }
}
