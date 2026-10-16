//! The AEAT SOAP 1.1 wire adapter. The client certificate is the TLS
//! identity only — AEAT authenticates the channel, never the message;
//! it never rides the payload.
//!
//! `SOAPAction` stays ABSENT: the WSDL's binding declares
//! `soapAction=""` (`contracts/aeat-verifactu/SistemaFacturacion.wsdl`),
//! and AEAT pruebas accepts it absent.

pub mod p12;

use std::time::Duration;

use crate::fiscal::transport::{
    parse_soap_answer, SendFuture, TransportFailure, VerifactuTransport,
};
use crate::wire::{HttpWireClient, Recorder};

use http::StatusCode;

use p12::ClientIdentity;

const CONTENT_TYPE: &str = "text/xml; charset=utf-8";

#[derive(Debug, thiserror::Error)]
pub enum WireBuildError {
    #[error("verifactu endpoint does not parse: {0}")]
    Endpoint(String),
    #[error("verifactu client identity rejected: {0}")]
    Identity(String),
    #[error("verifactu stand-in mark is no HTTP header name: {0}")]
    Mark(String),
}

/// One AEAT endpoint, one client identity per pool.
#[derive(Clone)]
pub struct HttpVerifactuTransport {
    client: HttpWireClient,
    endpoint: http::Uri,
    timeout: Duration,
    stand_in_mark: Option<http::HeaderName>,
}

impl std::fmt::Debug for HttpVerifactuTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpVerifactuTransport")
            .field("endpoint", &self.endpoint.to_string())
            .field("stand_in_mark", &self.stand_in_mark)
            .finish_non_exhaustive()
    }
}

impl HttpVerifactuTransport {
    /// # Errors
    /// [`WireBuildError::Endpoint`] when the URL does not parse.
    pub fn new(endpoint: &str, request_timeout: Duration) -> Result<Self, WireBuildError> {
        let endpoint = endpoint
            .parse::<http::Uri>()
            .map_err(|e| WireBuildError::Endpoint(e.to_string()))?;
        Ok(Self {
            client: HttpWireClient::new(request_timeout),
            endpoint,
            timeout: request_timeout,
            stand_in_mark: None,
        })
    }

    #[must_use]
    pub fn with_roots(self, roots: rustls::RootCertStore) -> Self {
        Self {
            client: HttpWireClient::with_roots(roots, self.timeout),
            ..self
        }
    }

    /// # Errors
    /// [`WireBuildError::Identity`] when the archive refuses or rustls
    /// rejects the chain/key pair. Demands the `signing` feature (the
    /// archive parser rides the signing family).
    #[cfg(feature = "signing")]
    pub fn with_client_pkcs12(self, pfx: &[u8], password: &str) -> Result<Self, WireBuildError> {
        let identity = p12::identity_from_pkcs12(pfx, password)
            .map_err(|e| WireBuildError::Identity(e.to_string()))?;
        self.with_client_identity(identity)
    }

    /// # Errors
    /// [`WireBuildError::Identity`] when rustls rejects the chain/key
    /// pair.
    pub fn with_client_identity(self, identity: ClientIdentity) -> Result<Self, WireBuildError> {
        let (chain, key) = identity.into_parts();
        let client = self
            .client
            .with_client_cert(chain, key)
            .map_err(|e| WireBuildError::Identity(e.to_string()))?;
        Ok(Self { client, ..self })
    }

    /// The same transport, refusing every answer that carries the `mark`
    /// header: the mark a deployment's stand-ins (its fake AEAT, a
    /// bench) put on their answers, so a build that must reach AEAT never
    /// reads a stand-in's verdict as AEAT's, wherever the endpoint points.
    /// AEAT sends no such header. A refused answer is a
    /// [`TransportFailure`], as for any exchange that did not reach AEAT.
    ///
    /// # Errors
    /// [`WireBuildError::Mark`] when `mark` is no HTTP header name.
    pub fn refusing_stand_ins(self, mark: &str) -> Result<Self, WireBuildError> {
        let mark =
            http::HeaderName::try_from(mark).map_err(|e| WireBuildError::Mark(e.to_string()))?;
        Ok(Self {
            stand_in_mark: Some(mark),
            ..self
        })
    }

    #[must_use]
    pub fn recording(self) -> (Self, Recorder) {
        let (client, recorder) = self.client.recording();
        (Self { client, ..self }, recorder)
    }
}

impl VerifactuTransport for HttpVerifactuTransport {
    fn send<'a>(&'a self, soap_envelope: &'a str) -> SendFuture<'a> {
        Box::pin(async move {
            let request = http::Request::post(&self.endpoint)
                .header(http::header::CONTENT_TYPE, CONTENT_TYPE)
                .body(soap_envelope.to_owned())
                .expect("a String body with a POST builder cannot fail");
            match self.client.send(request).await {
                Err(wire) => Err(TransportFailure {
                    detail: wire.to_string(),
                }),
                Ok(response) => {
                    // First: a stand-in's answer is never AEAT's, whatever
                    // its status or body.
                    if let Some(mark) = self
                        .stand_in_mark
                        .as_ref()
                        .filter(|mark| response.headers().contains_key(*mark))
                    {
                        return Err(TransportFailure {
                            detail: format!(
                                "a stand-in answered (its {mark} header), where only AEAT may — \
                                 {} is not AEAT's door",
                                self.endpoint
                            ),
                        });
                    }
                    let status = response.status();
                    // AEAT's certificate gate: an unaccepted client
                    // certificate answers HTTP 302 to the sede error
                    // page (erro4011) with the TLS handshake complete —
                    // the operator's likeliest misconfiguration; name
                    // it, never a generic parse failure.
                    if status == StatusCode::FOUND
                        || status == StatusCode::MOVED_PERMANENTLY
                        || status == StatusCode::UNAUTHORIZED
                        || status == StatusCode::FORBIDDEN
                    {
                        let location = response
                            .headers()
                            .get(http::header::LOCATION)
                            .and_then(|value| value.to_str().ok())
                            .unwrap_or("<no location header>");
                        return Err(TransportFailure {
                            detail: format!(
                                "AEAT refused the exchange (HTTP {status} -> {location}) — the \
                                 client certificate was not accepted"
                            ),
                        });
                    }
                    let body = response.into_body();
                    match parse_soap_answer(&body) {
                        Ok(outcome) => Ok(outcome),
                        Err(detail) => Err(TransportFailure {
                            detail: format!("unparseable AEAT answer (HTTP {status}): {detail}"),
                        }),
                    }
                }
            }
        })
    }
}
