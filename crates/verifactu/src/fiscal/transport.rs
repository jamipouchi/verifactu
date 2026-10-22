//! The transport port and its read edge (the stateful fake lives in
//! `fake`, feature `fakes`). Endpoint family:
//! `sfVerifactu` (remission) and `sfRequerimiento` (conservation bajo
//! requerimiento) — same XSDs, different endpoints; selection is the
//! caller's configuration.

use std::future::Future;
use std::pin::Pin;

use crate::fiscal::consulta::consulta_of_tree;
use crate::fiscal::response::{fault_of_tree, respuesta_of_tree, RespuestaSuministro, SoapFault};
use crate::fiscal::xml::parse_tree;

#[cfg(any(test, feature = "fakes"))]
mod fake;
#[cfg(any(test, feature = "fakes"))]
pub use fake::{
    fault_document, respuesta_document, DuplicadoSpec, FakeVerifactuTransport, LineaSpec,
    RespuestaSpec, ScriptedLine, ScriptedOutcome, SentRecord, SentRequest,
};

#[derive(Debug)]
pub enum TransportOutcome {
    Response(RespuestaSuministro),
    /// The consulta operation's payload root
    /// (`RespuestaConsultaFactuSistemaFacturacion`) — same read edge,
    /// different respuesta vocabulary.
    Consulta(crate::fiscal::consulta::ConsultaAnswer),
    Fault(SoapFault),
}

/// Why an exchange yielded no AEAT answer to read, typed by cause. No
/// credential reaches an error: a refusal holds its status only, never
/// what AEAT's door sent beside it (the redirect's `Location` carries
/// the gate's session).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TransportFailure {
    /// The leg failed before any answer arrived: unreachable, timed
    /// out, TLS (a certificate refused in the handshake included),
    /// protocol. `Transient`: the caller retries.
    #[error("{detail}")]
    Wire { detail: String },
    /// AEAT's door refused the exchange by status, before any SOAP
    /// answer ([`parse_http_answer`]): HTTP 301/302 (the redirect to the
    /// sede error page), 401 or 403, so the client certificate was not
    /// accepted (live-confirmed 2026-10-22: the door answers HTTP 302 to
    /// an exchange with no client certificate). `OperatorAction`: no
    /// retry heals it.
    #[error("AEAT refused the exchange (HTTP {status}): the client certificate was not accepted")]
    Refused { status: u16 },
    /// The answer carried the stand-in mark the transport refuses
    /// (`HttpVerifactuTransport::refusing_stand_ins`); `mark` is that
    /// header's name. `OperatorAction`: the endpoint is not AEAT's.
    #[error("a stand-in answered (its {mark} header), where only AEAT may")]
    StandIn { mark: String },
    /// An answer the exchange cannot read to its end: a body that does
    /// not parse, the other operation's respuesta, more pages without a
    /// `ClavePaginacion` or past a consulta's 100-page bound.
    /// `Transient`: surfaced, never guessed at.
    #[error("{detail}")]
    Unreadable { detail: String },
}

/// Boxed (not RPITIT) to keep the trait dyn-compatible. The `+ Send`
/// bound serves multi-threaded hosts (tokio, axum handlers); the
/// `edge` feature drops it so single-threaded runtimes (wasm32: a JS
/// `Promise` is structurally `!Send`) implement the port directly over
/// their runtime's fetch — no spawn/oneshot bridge at the consumer.
#[cfg(not(feature = "edge"))]
pub type SendFuture<'a> =
    Pin<Box<dyn Future<Output = Result<TransportOutcome, TransportFailure>> + Send + 'a>>;

#[cfg(feature = "edge")]
pub type SendFuture<'a> =
    Pin<Box<dyn Future<Output = Result<TransportOutcome, TransportFailure>> + 'a>>;

/// The port: one SOAP envelope out, AEAT's answer back. A transport
/// never follows a redirect (a runtime fetch: `redirect: "manual"`)
/// and hands the answer's status and body to [`parse_http_answer`], the
/// read edge the http transport shares; a leg that fails before any
/// answer is [`TransportFailure::Wire`]. No failure quotes a header, a
/// cookie or a URL's query.
pub trait VerifactuTransport: Send + Sync {
    fn send<'a>(&'a self, soap_envelope: &'a str) -> SendFuture<'a>;
}

/// The body half of [`parse_http_answer`]: a SOAP envelope whose `Body`
/// carries a `Fault` (any HTTP status) or either respuesta, or that
/// payload as a bare root. The fake's documents parse through this same
/// edge, so one read law holds offline and live.
///
/// # Errors
/// A prose `Err` when the document carries none of those shapes:
/// [`parse_http_answer`] answers it as [`TransportFailure::Unreadable`],
/// surfaced, never guessed at.
pub(crate) fn parse_soap_answer(xml: &str) -> Result<TransportOutcome, String> {
    const PAYLOADS: [&str; 3] = [
        "Fault",
        "RespuestaRegFactuSistemaFacturacion",
        "RespuestaConsultaFactuSistemaFacturacion",
    ];
    let root = parse_tree(xml)?;
    let payload = if root.local == "Envelope" {
        root.child("Body")
            .ok_or("SOAP envelope without Body")?
            .children
            .iter()
            .find(|node| PAYLOADS.contains(&node.local.as_str()))
            .ok_or("SOAP Body carries neither a Fault nor a respuesta")?
    } else {
        &root
    };
    match payload.local.as_str() {
        "Fault" => Ok(TransportOutcome::Fault(fault_of_tree(payload)?)),
        "RespuestaRegFactuSistemaFacturacion" => {
            Ok(TransportOutcome::Response(respuesta_of_tree(payload)?))
        }
        "RespuestaConsultaFactuSistemaFacturacion" => {
            Ok(TransportOutcome::Consulta(consulta_of_tree(payload)?))
        }
        other => Err(format!(
            "root element is {other}, not a SOAP envelope, Fault, or either respuesta"
        )),
    }
}

/// The read edge every transport shares, the http one and a consumer's
/// own: an answer's HTTP status and body in, AEAT's answer out. AEAT's
/// door refuses a client certificate it does not accept by status,
/// before any SOAP answer: HTTP 302 to the sede error page (erro4011),
/// the certificate gate's answer; 301 alike, a redirect away from the
/// SOAP endpoint; 401 or 403, HTTP's refusal of the credentials (RFC 9110
/// §15.5.2, §15.5.4). Only the status is read then: the redirect's
/// `Location` carries the gate's session, and no credential reaches an
/// error. At any other status the body is read: a SOAP envelope whose
/// `Body` carries a `Fault` (at any status) or either respuesta, or that
/// payload as a bare root.
///
/// # Errors
/// [`TransportFailure::Refused`] on HTTP 301, 302, 401 or 403;
/// [`TransportFailure::Unreadable`], its status in the prose, on a body
/// in none of those shapes.
pub fn parse_http_answer(status: u16, body: &str) -> Result<TransportOutcome, TransportFailure> {
    if matches!(status, 301 | 302 | 401 | 403) {
        return Err(TransportFailure::Refused { status });
    }
    parse_soap_answer(body).map_err(|detail| TransportFailure::Unreadable {
        detail: format!("unparseable AEAT answer (HTTP {status}): {detail}"),
    })
}

#[cfg(test)]
mod tests {
    use super::{fault_document, parse_http_answer, TransportFailure, TransportOutcome};

    /// The door's refusal is its status alone, whatever rides beside
    /// it; every other status, the neighbours included, reads its SOAP
    /// answer, and a body with none names its status.
    #[test]
    fn the_http_read_edge_types_the_doors_refusal_by_status_alone() {
        let fault = fault_document("soapenv:Server", "Codigo[-1].Error interno");
        for status in [301, 302, 401, 403] {
            assert_eq!(
                parse_http_answer(status, &fault).err(),
                Some(TransportFailure::Refused { status })
            );
        }
        for status in [200, 300, 303, 307, 308, 400, 402, 404, 500, 503] {
            let read = parse_http_answer(status, &fault);
            assert!(
                matches!(read, Ok(TransportOutcome::Fault(_))),
                "HTTP {status}: {read:?}"
            );
        }
        let read = parse_http_answer(500, "<html><body>Service unavailable</body></html>");
        let Err(TransportFailure::Unreadable { detail }) = read else {
            panic!("a body in no SOAP shape is unreadable: {read:?}");
        };
        assert!(detail.contains("HTTP 500"), "{detail}");
    }
}
