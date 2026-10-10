//! The shared HTTP(S) client leg: pooled HTTP/1.1 over plain `http`
//! and rustls `https`. The ring provider is pinned over rustls's
//! default aws-lc-rs, which needs cmake+CC at build time.

mod client;
mod recorder;

pub use client::HttpWireClient;
pub use recorder::{RecordedExchange, RecordedRequest, RecordedResponse, Recorder};

#[cfg(feature = "test-util")]
pub mod test_util;

/// Classing law: connect failures — refused dials and every TLS
/// failure (connect, handshake, certificate) — are
/// [`WireError::Unreachable`], transient; identified timeouts are
/// [`WireError::Timeout`]; everything about the exchange's shape (bad
/// scheme, non-UTF-8 body, a client certificate rejected at
/// construction) is [`WireError::Protocol`].
#[derive(Clone, Debug, thiserror::Error, PartialEq, Eq)]
pub enum WireError {
    #[error("unreachable: {detail}")]
    Unreachable { detail: String },
    #[error("request deadline elapsed")]
    Timeout,
    #[error("protocol: {detail}")]
    Protocol { detail: String },
}
