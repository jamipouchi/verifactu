//! Pure domain core — zero-I/O, no persistence, no clock, no network;
//! `f64` never appears in any money path.

/// Re-exported so dependents name the money scalar through the domain
/// boundary instead of depending on `rust_decimal` directly.
pub mod chain;
pub mod error;
pub mod money;
pub mod series;
