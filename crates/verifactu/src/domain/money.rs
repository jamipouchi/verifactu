use std::fmt;
use std::iter::Sum;
use std::ops::{Add, Neg, Sub};

use rust_decimal::Decimal;

/// An exact-decimal amount; construction enforces ≤2 decimal places.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct Money(Decimal);

/// Deserializing validates the ≤2-decimal-place invariant a derived
/// impl would not — `1.234` is money this type refuses to be.
#[cfg(feature = "serde")]
impl<'de> serde::Deserialize<'de> for Money {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        // The trait call spelled fully: `Decimal` carries an INHERENT
        // `deserialize([u8; 16])` (bincode-style) that shadows it.
        let value = <Decimal as serde::Deserialize>::deserialize(deserializer)?;
        Money::try_from_decimal(value).ok_or_else(|| {
            serde::de::Error::custom(format!("money carries more than 2 decimal places: {value}"))
        })
    }
}

impl Money {
    pub const ZERO: Money = Money(Decimal::ZERO);

    #[must_use]
    pub fn from_cents(cents: i64) -> Money {
        Money(Decimal::new(cents, 2))
    }

    /// The exact cents — lossless by the ≤2-decimal-place invariant,
    /// so consumers never touch `rust_decimal` to echo an amount.
    ///
    /// # Panics
    ///
    /// Never on legal input: `ImporteSgn12.2` × 100 cannot overflow
    /// `i64` cents.
    #[must_use]
    pub fn as_cents(self) -> i64 {
        let value = self.0;
        let cents = match value.scale() {
            2 => value.mantissa(),
            1 => value.mantissa() * 10,
            _ => value.mantissa() * 100,
        };
        i64::try_from(cents).expect("ImporteSgn12.2 × 100 fits i64 cents")
    }

    /// The checked door for decimals from outside: `None` when `value`
    /// carries more than 2 decimal places (money never rounds silently).
    #[must_use]
    pub fn try_from_decimal(value: Decimal) -> Option<Money> {
        (value.scale() <= 2).then_some(Money(value))
    }

    /// # Panics
    ///
    /// Panics when `value` carries more than 2 decimal places —
    /// [`Money::try_from_decimal`] is the non-panicking door.
    #[must_use]
    pub fn from_decimal(value: Decimal) -> Money {
        Money::try_from_decimal(value)
            .unwrap_or_else(|| panic!("money carries more than 2 decimal places: {value}"))
    }

    #[must_use]
    pub fn as_decimal(self) -> Decimal {
        self.0
    }

    #[must_use]
    pub fn negated(self) -> Money {
        Money(-self.0)
    }
}

impl Add for Money {
    type Output = Money;

    fn add(self, rhs: Money) -> Money {
        Money(self.0 + rhs.0)
    }
}

impl Sub for Money {
    type Output = Money;

    fn sub(self, rhs: Money) -> Money {
        Money(self.0 - rhs.0)
    }
}

impl Neg for Money {
    type Output = Money;

    fn neg(self) -> Money {
        self.negated()
    }
}

impl fmt::Display for Money {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

impl Sum for Money {
    fn sum<I: Iterator<Item = Money>>(iter: I) -> Money {
        iter.fold(Money::ZERO, |acc, amount| acc + amount)
    }
}

#[cfg(test)]
mod tests {
    use super::Money;
    use rust_decimal::Decimal;

    #[test]
    fn cents_round_trip_is_lossless_at_every_legal_scale() {
        for cents in [0_i64, 210, -210, 12_100, 9_999_999_999] {
            assert_eq!(Money::from_cents(cents).as_cents(), cents);
        }
        assert_eq!(
            Money::from_decimal(Decimal::new(15, 1)).as_cents(),
            150,
            "scale-1 money (1.5) resolves to 150 cents"
        );
        assert_eq!(Money::from_decimal(Decimal::from(7)).as_cents(), 700);
    }
}
