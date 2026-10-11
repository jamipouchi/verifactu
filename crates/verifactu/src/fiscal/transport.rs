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
    RespuestaSpec, ScriptedLine, ScriptedOutcome,
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

/// The wire leg failed (unreachable, timeout, refused, unparseable
/// answer) — always the `Transient` class: the caller retries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransportFailure {
    pub detail: String,
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

pub trait VerifactuTransport: Send + Sync {
    fn send<'a>(&'a self, soap_envelope: &'a str) -> SendFuture<'a>;
}

/// The production wire's read edge: a SOAP envelope whose `Body`
/// carries a `Fault` (any HTTP status) or either respuesta, or that
/// payload as a bare root — the fake's documents parse through this same
/// edge, so one read law holds offline and live.
///
/// # Errors
/// A prose `Err` when the document carries none of those shapes — the
/// `Transient` class: an unparseable answer is surfaced, never guessed
/// at.
pub fn parse_soap_answer(xml: &str) -> Result<TransportOutcome, String> {
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
