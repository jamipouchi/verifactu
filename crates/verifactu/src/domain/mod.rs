//! Pure domain core — zero I/O, no persistence, no clock, no network;
//! `f64` never appears in any money path.

pub mod chain;
pub mod error;
pub mod money;
pub mod series;
