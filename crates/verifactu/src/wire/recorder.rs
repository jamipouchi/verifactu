//! Every exchange copied into a shared cell after it settles — the
//! tee never changes behavior.

use std::sync::{Arc, Mutex};

use crate::wire::WireError;

/// Poisoning is a bug, never an input: the lock is only ever taken to
/// push a settled exchange, never across an await.
const RECORDER_LOCK: &str = "the recording cell is never locked across an await";

/// The request as handed to the transport — the port's own truth, not
/// the socket's (hyper adds Host/connection lines below this seam).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordedRequest {
    pub method: String,
    pub uri: String,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

impl RecordedRequest {
    /// Recorded before the send, so the record and the eventual
    /// verdict cannot disagree about what was asked.
    pub(crate) fn of(req: &http::Request<String>) -> Self {
        Self {
            method: req.method().as_str().to_owned(),
            uri: req.uri().to_string(),
            headers: req
                .headers()
                .iter()
                .map(|(name, value)| {
                    (
                        name.as_str().to_owned(),
                        value
                            .to_str()
                            .unwrap_or("<non-ascii value bytes>")
                            .to_owned(),
                    )
                })
                .collect(),
            body: req.body().clone(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RecordedResponse {
    Answer {
        status: u16,
        headers: Vec<(String, String)>,
        body: String,
    },
    TransportError(String),
}

impl RecordedResponse {
    pub(crate) fn of(outcome: &Result<http::Response<String>, WireError>) -> Self {
        match outcome {
            Ok(response) => Self::Answer {
                status: response.status().as_u16(),
                headers: response
                    .headers()
                    .iter()
                    .map(|(name, value)| {
                        (
                            name.as_str().to_owned(),
                            value
                                .to_str()
                                .unwrap_or("<non-ascii value bytes>")
                                .to_owned(),
                        )
                    })
                    .collect(),
                body: response.body().clone(),
            },
            Err(error) => Self::TransportError(error.to_string()),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordedExchange {
    pub request: RecordedRequest,
    pub response: RecordedResponse,
}

#[derive(Debug, Default)]
pub(crate) struct RecorderShared(Mutex<Vec<RecordedExchange>>);

impl RecorderShared {
    pub(crate) fn push(&self, exchange: RecordedExchange) {
        self.0.lock().expect(RECORDER_LOCK).push(exchange);
    }
}

#[derive(Clone, Debug)]
pub struct Recorder {
    shared: Arc<RecorderShared>,
}

impl Recorder {
    pub(crate) fn of(shared: Arc<RecorderShared>) -> Self {
        Self { shared }
    }

    /// # Panics
    /// Only on lock poisoning (a bug, never an input).
    #[must_use]
    pub fn exchanges(&self) -> Vec<RecordedExchange> {
        self.shared.0.lock().expect(RECORDER_LOCK).clone()
    }
}
