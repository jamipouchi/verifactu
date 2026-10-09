//! Site-zone rendering of the two instants a chain record hashes. BOTH
//! are SITE-LOCAL and derive from the SAME seal instant:
//! `fecha_expedicion` is the local civil date of `fecha_huso_gen` (the
//! decided local-day rule — a UTC-day date would disagree for
//! late-evening instants). The zone rule is `crate::clock::zone`'s; this module
//! owns the string shapes only.

use crate::clock::zone::{civil_datetime, SiteZone};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RenderedInstants {
    pub fecha_expedicion: String,
    pub fecha_huso_gen: String,
}

/// One call, one zone, one instant — the two rendered instants can never
/// disagree about the seal moment.
#[must_use]
pub fn render_instants(zone: SiteZone, epoch_secs: u64) -> RenderedInstants {
    RenderedInstants {
        fecha_expedicion: fecha_expedicion(zone, epoch_secs),
        fecha_huso_gen: fecha_huso_gen(zone, epoch_secs),
    }
}

#[must_use]
pub(crate) fn fecha_expedicion(zone: SiteZone, epoch_secs: u64) -> String {
    let civil = civil_datetime(zone, epoch_secs);
    format!("{:02}-{:02}-{:04}", civil.day, civil.month, civil.year)
}

/// With a numeric offset, never `Z` — the field is defined with the
/// site's offset.
///
/// # Panics
///
/// Unreachable for Earth's offset range: reaching it means the zone
/// vocabulary grew beyond it.
#[must_use]
pub(crate) fn fecha_huso_gen(zone: SiteZone, epoch_secs: u64) -> String {
    let offset = zone.offset_secs(epoch_secs);
    let civil = civil_datetime(zone, epoch_secs);
    let sign = if offset < 0 { '-' } else { '+' };
    let absolute = offset.unsigned_abs();
    let hours = u32::try_from(absolute / 3_600).expect("offset hours fit u32");
    let mins = u32::try_from((absolute % 3_600) / 60).expect("offset minutes fit u32");
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}{}{:02}:{:02}",
        civil.year,
        civil.month,
        civil.day,
        civil.hour,
        civil.minute,
        civil.second,
        sign,
        hours,
        mins
    )
}

#[cfg(test)]
mod tests {
    use super::fecha_huso_gen;
    use crate::clock::zone::SiteZone;

    const MADRID: SiteZone = SiteZone::EuropeMadrid;
    const CANARY: SiteZone = SiteZone::AtlanticCanary;

    /// Hand-verified epoch of the 2026 spring transition: the offset
    /// jumps in both zones at the same UTC second.
    #[test]
    fn spring_2026_transition_renders_the_offset_jump_instantly() {
        assert_eq!(
            fecha_huso_gen(MADRID, 1_774_745_999),
            "2026-03-29T01:59:59+01:00",
            "Madrid one second before: 01:59:59 CET (00:59:59 UTC)"
        );
        assert_eq!(
            fecha_huso_gen(MADRID, 1_774_746_000),
            "2026-03-29T03:00:00+02:00",
            "Madrid at the transition: 03:00:00 CEST — 02:00 does not exist"
        );
        assert_eq!(
            fecha_huso_gen(CANARY, 1_774_745_999),
            "2026-03-29T00:59:59+00:00",
            "Canary one second before: 00:59:59 WET"
        );
        assert_eq!(
            fecha_huso_gen(CANARY, 1_774_746_000),
            "2026-03-29T02:00:00+01:00",
            "Canary at the transition: 02:00:00 WEST — 01:00 does not exist again; \
             the same UTC second, one base hour behind Madrid"
        );
    }
}
