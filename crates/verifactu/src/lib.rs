//! `verifactu` — the one-crate face of the Veri*FACTU stack: compose,
//! gate, and re-export. A product declares THIS crate (plus its async
//! runtime) and nothing else.
//!
//! # The two custody models
//!
//! | Model | Certificate | Cabecera | Scale shape |
//! |---|---|---|---|
//! | [`Verifactu::tenant`] | the tenant's OWN | obligado = the tenant | one pooled transport PER tenant; cache the handles |
//! | [`Verifactu::vendor`] | OUR one certificate | obligado = the tenant, `Representante` = us | ONE pool shared by every tenant; emitters are cached |
//!
//! # The production gate
//!
//! [`Environment::Pruebas`] is the default. Production demands the
//! consent token AND the deployment key `VERIFACTU_ALLOW_PRODUCTION=1`
//! — miss either and [`VerifactuBuilder::build`] fails closed before
//! any pool, emitter, or certificate parse exists. The consenting line
//! is grep-able in review:
//!
//! ```ignore
//! Environment::production(ProductionOptIn::these_emissions_are_legally_binding())
//! ```
//!
//! # Endpoints
//!
//! The eight documented AEAT endpoints (pruebas/production ×
//! personal-or-company/sello × remission/requerimiento) live HERE and
//! nowhere else; this facade exposes no raw `.endpoint(url)` door.
//!
//! # Bringing your own transport
//!
//! [`CertificateSource::Custom`] supplies both legs — your `XAdES`
//! signer and your own [`VerifactuTransport`] implementation (an edge
//! runtime riding its mTLS-certificate binding, a hermetic fake).
//! Build with `default-features = false` and neither the HTTP stack
//! nor the signing tree is compiled; supply a signer over your KMS/HSM
//! or, on remission modalities (whose altas travel unsigned by law), a
//! pass-through stub. [`endpoint_table`] still names the URLs your
//! transport dials, and `engine::transport::parse_soap_answer` is the
//! read edge it should hand the raw answer body to.
//!
//! # Edge runtimes (wasm32: Cloudflare Workers and kin)
//!
//! Three doors matter on wasm, all first-class:
//!
//! - `default-features = false` compiles no host HTTP stack and no
//!   signing tree — the custom door takes your runtime's fetch and
//!   your signer.
//! - the `edge` feature drops the `+ Send` bound from
//!   [`SendFuture`], so a runtime fetch (a JS `Promise`, structurally
//!   `!Send`) implements [`VerifactuTransport`] DIRECTLY — no
//!   spawn/oneshot bridge.
//! - wasm has no `SystemTime`: inject your runtime's date API through
//!   [`VerifactuBuilder::clock`] and the flat `emit_invoice` loop runs
//!   unmodified. Egress stays your transport: on Workers, an uploaded
//!   mTLS-certificate binding IS a `Fetcher` presenting the client
//!   cert at the TLS handshake — the AEAT door (`wrangler
//!   mtls-certificate upload` + an `mtls_certificates` binding,
//!   dialed at [`endpoint_table`]). workers-rs has no typed accessor
//!   for it yet; `env.service("AEAT_CERT")` retrieves the `Fetcher`.
//!
//! # Features
//!
//! | Feature | Default | What it compiles |
//! |---|---|---|
//! | `http` | yes | the mTLS AEAT transport (rustls + hyper-util) |
//! | `signing` | yes | XAdES-EPES over the bergshamra family: the `Pkcs12` door, `engine::xades` |
//! | `edge` | no | `?Send` transport futures for single-threaded runtimes |
//! | `serde` | no | JSON shapes for `Emission`, the durable cursors, the sealed kinds, the consulta answers |
//! | `fakes` | no | hermetic test doubles: `FakeSigner`, `FakeVerifactuTransport` (+ its scripted AEAT documents), `FakeClock` |
//! | `test-util` | no | owned-loopback TLS listeners + throwaway CAs (implies `fakes`) |
//!
//! # The product loop
//!
//! One invoice is one `emit_invoice`; the return carries the emission
//! AND the chain cursor — the only durable state a product owns:
//!
//! ```ignore
//! let first = tenant.emit_invoice(draft, None).await?;    // primer registro
//! persist(&first.cursor);                                  // YOUR storage
//! let next = tenant.emit_invoice(draft2, Some(&first.cursor)).await?;
//! // ...and an invoice's stored cursor is what later cancels it:
//! let head = tenant.cancel_invoice(&next.cursor, Some(&next.cursor)).await?;
//! ```
//!
//! The printed QR is the emission's own data: `first.qr_url(env,
//! modality)` renders the cotejo URL (the four AEAT parameters). The
//! image is the consumer's print pipeline — render it at
//! error-correction level M and 30×30–40×40 mm per the QR spec's print
//! law (a generic QR library's default level L silently violates it).
//!
//! The verdicts ride `verifactu::response` and the retry taxonomy is
//! [`EmitError::class`] over [`ErrorClass`].

// The machinery — private; the flat layer and `engine` (the two
// supported faces) are its only windows.
#[cfg(feature = "http")]
pub(crate) mod aeat;
pub(crate) mod clock;
pub(crate) mod domain;
pub(crate) mod fiscal;
pub(crate) mod qr;
pub(crate) mod signer;
#[cfg(feature = "http")]
pub(crate) mod wire;
#[cfg(feature = "signing")]
pub(crate) mod xades;

mod builder;
mod custody;
mod draft;
#[cfg(test)]
mod tests;

pub use crate::builder::{
    endpoint_table, CertFamily, CertificateSource, Environment, ProductionOptIn, SifConfig,
    Verifactu, VerifactuBuilder, VerifactuError,
};
pub use crate::custody::{TenantConnecting, TenantVerifactu, VendorConnecting, VendorVerifactu};
pub use crate::draft::{cancel_draft, emit_draft, lines, EmittedInvoice, InvoiceDraft};

// The FLAT layer re-exports the product face (builder, custody, drafts,
// the business vocabulary, results); the `engine` module below is the
// ledger-owner's surface.
#[cfg(feature = "http")]
pub use crate::aeat::p12::ClientIdentity;
pub use crate::domain::chain::{ChainKind, PrevRef};
pub use crate::domain::chain::{FechaExpedicion, FechaHuso, Predecessor, TipoFactura};
pub use crate::domain::error::ErrorClass;
pub use crate::domain::money::Money;
pub use crate::domain::series::checked_format as try_format_num_serie;
pub use crate::domain::series::format as format_num_serie;
pub use crate::domain::series::Series;
pub use crate::fiscal::consulta::{
    ConsultaAnswer, ConsultaFilter, EstadoAlmacenado, Mes, RegistroConsulta,
};
pub use crate::fiscal::response;
pub use crate::fiscal::transport::{
    SendFuture, TransportFailure, TransportOutcome, VerifactuTransport,
};
pub use crate::fiscal::{
    CalificacionOperacion, ClaveRegimen, DetalleDesglose, Emission, EmitError, FacturaId,
    IDDestinatario, IdOtro, IdOtroType, IdentificacionDestinatario, ImporteRectificacion, Impuesto,
    Modality, Obligado, OperacionExenta, TipoRectificativa,
};
pub use crate::qr::{
    verification_url, TRIBUTARY_CAPTION, VERIFIABLE_CAPTION, VERIFIABLE_CAPTION_SHORT,
};
pub use crate::signer::{FiscalSigner, SignerError};

/// The ledger-owner layer: consumers that seal their own records
/// compose here; products ride the flat layer and never look.
pub mod engine {
    #[cfg(feature = "http")]
    pub use crate::aeat::p12;
    #[cfg(feature = "http")]
    pub use crate::aeat::{HttpVerifactuTransport, WireBuildError};
    pub use crate::cancel_draft;
    pub use crate::clock::zone::SiteZone;
    #[cfg(any(test, feature = "fakes"))]
    pub use crate::clock::FakeClock;
    pub use crate::clock::{Clock, SystemClock, Timestamp};
    pub use crate::domain::chain::{
        huella, verify_chain, ChainBreak, ChainKind, ChainRecord, EventData, Predecessor, PrevRef,
    };
    pub use crate::emit_draft;
    pub use crate::fiscal::consulta;
    pub use crate::fiscal::events;
    pub use crate::fiscal::huso;
    pub use crate::fiscal::response;
    pub use crate::fiscal::transport;
    #[cfg(any(test, feature = "fakes"))]
    pub use crate::fiscal::transport::{FakeVerifactuTransport, ScriptedLine, ScriptedOutcome};
    pub use crate::fiscal::transport::{TransportFailure, VerifactuTransport};
    pub use crate::fiscal::xml;
    pub use crate::fiscal::{
        EmissionContext, RechazoPrevio, Requerimiento, SiNo, SistemaInformaticoConfig,
        VerifactuEmitter,
    };
    #[cfg(any(test, feature = "fakes"))]
    pub use crate::signer::FakeSigner;
    #[cfg(feature = "test-util")]
    pub use crate::wire::test_util;
    #[cfg(feature = "http")]
    pub use crate::wire::{
        HttpWireClient, RecordedExchange, RecordedRequest, RecordedResponse, Recorder, WireError,
    };
    #[cfg(feature = "signing")]
    pub use crate::xades::{
        enveloped, load_key_with_certificate, policy, verify, XadesError, XadesSigner,
    };
    #[cfg(feature = "http")]
    pub use rustls;
}
