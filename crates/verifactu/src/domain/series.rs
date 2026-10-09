#[derive(Clone, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Series {
    /// Factura simplificada — tickets.
    T,
    /// Facturas completas.
    F,
    /// Rectificativas — refunds.
    R,
    Custom(String),
}

impl Series {
    #[must_use]
    pub fn as_str(&self) -> &str {
        match self {
            Self::T => "T",
            Self::F => "F",
            Self::R => "R",
            Self::Custom(prefix) => prefix,
        }
    }

    /// Total by design: an unknown prefix is [`Series::Custom`], never
    /// an error — the draft gates (empty prefix, prefix length) fire at
    /// `InvoiceDraft::build`, where the error can name the input.
    #[must_use]
    pub fn parse(prefix: &str) -> Self {
        match prefix {
            "T" => Self::T,
            "F" => Self::F,
            "R" => Self::R,
            other => Self::Custom(other.to_owned()),
        }
    }
}

/// The checked renderer: `None` when `n > 99_999_999` (a gapless
/// series can never legitimately reach a 9-digit correlative).
/// Consumers that render a key BEFORE emitting — dup-checks, REST path
/// params — ride this door so untrusted input can never panic; the
/// product face's gate (`InvoiceDraft::build`) answers 400-class for
/// the same bound.
#[must_use]
pub fn checked_format(series: &Series, n: u64) -> Option<String> {
    (n <= 99_999_999).then(|| format!("{}{n:0>8}", series.as_str()))
}

/// # Panics
///
/// Panics when `n > 99_999_999` — [`checked_format`] is the
/// non-panicking door. `InvoiceDraft::build` gates the bound before
/// any seal renders, so the panic is reachable only from direct
/// `ChainRecord` construction with an off-contract number.
#[must_use]
pub fn format(series: &Series, n: u64) -> String {
    checked_format(series, n).expect("gapless series cannot reach a 9-digit correlative")
}

#[cfg(test)]
mod tests {
    use super::{format, Series};

    #[test]
    fn format_renders_the_series_prefix_and_8_digit_correlative() {
        assert_eq!(format(&Series::T, 42), "T00000042");
    }

    #[test]
    fn checked_format_refuses_the_9_digit_correlative() {
        assert_eq!(
            super::checked_format(&Series::T, 99_999_999),
            Some(String::from("T99999999"))
        );
        assert_eq!(super::checked_format(&Series::T, 100_000_000), None);
    }

    #[test]
    fn parse_is_total_and_round_trips_the_builtin_prefixes() {
        for serie in [Series::T, Series::F, Series::R] {
            assert_eq!(Series::parse(serie.as_str()), serie);
        }
        assert_eq!(
            Series::parse("2026E"),
            Series::Custom(String::from("2026E"))
        );
    }
}
