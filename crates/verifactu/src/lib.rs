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
//! | `test-util` | no | owned-loopback TLS listeners + throwaway CAs |
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

use std::collections::HashMap;
use std::sync::{Arc, RwLock};
#[cfg(feature = "http")]
use std::time::Duration;

use crate::clock::{Clock, SystemClock};
use crate::domain::chain::ChainRecord;
use crate::fiscal::huso;
use crate::fiscal::{consulta, EmissionContext, SiNo, SistemaInformaticoConfig, VerifactuEmitter};

#[cfg(feature = "http")]
use crate::aeat::HttpVerifactuTransport;

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
    pub use crate::clock::{Clock, FakeClock, SystemClock, Timestamp};
    pub use crate::domain::chain::{
        huella, verify_chain, ChainBreak, ChainKind, ChainRecord, EventData, Predecessor, PrevRef,
    };
    pub use crate::emit_draft;
    pub use crate::fiscal::consulta;
    pub use crate::fiscal::events;
    pub use crate::fiscal::huso;
    pub use crate::fiscal::response;
    pub use crate::fiscal::transport;
    pub use crate::fiscal::transport::{
        FakeVerifactuTransport, ScriptedLine, ScriptedOutcome, TransportFailure, VerifactuTransport,
    };
    pub use crate::fiscal::xml;
    pub use crate::fiscal::{
        EmissionContext, RechazoPrevio, Requerimiento, SiNo, SistemaInformaticoConfig,
        VerifactuEmitter,
    };
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

const PRODUCTION_ENV_KEY: &str = "VERIFACTU_ALLOW_PRODUCTION";

#[cfg(feature = "http")]
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// Where emissions go. [`Environment::Pruebas`] is the default;
/// production demands the consent token AND the deployment key (see
/// the crate docs).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Environment {
    /// Records carry no fiscal weight; the environment the whole stack
    /// was live-qualified against (16/16 sweep `Correcto`, 2026-10-05).
    #[default]
    Pruebas,
    /// AEAT production — submissions are legally binding.
    Production,
}

/// The compile-site consent token — constructible only through its
/// self-describing constructor.
pub struct ProductionOptIn(());

impl ProductionOptIn {
    #[must_use]
    pub fn these_emissions_are_legally_binding() -> Self {
        Self(())
    }
}

impl Environment {
    /// The only production constructor; still gated by the deployment
    /// key at build time.
    #[must_use]
    pub fn production(_opt_in: ProductionOptIn) -> Self {
        Self::Production
    }
}

/// The AEAT endpoint family axis (SW doc v1.0.3, Anexos I–II):
/// personal/company certificates ride the `www1`/`prewww1` hosts,
/// SELLO (entity-seal) certificates the `www10`/`prewww10` hosts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CertFamily {
    #[default]
    PersonalOEmpresa,
    Sello,
}

/// The vendor's registered SIF identity — the product's own
/// declaration, fixed across every tenant.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SifConfig {
    nombre_sistema_informatico: String,
    id_sistema_informatico: String,
    version: String,
}

impl SifConfig {
    /// # Errors
    /// [`VerifactuError::Identity`] — the SI.xsd simple types
    /// (`TextMax30`/`TextMax2`/`TextMax50`), naming the field.
    pub fn new(nombre: &str, id: &str, version: &str) -> Result<Self, VerifactuError> {
        let check = |field: &'static str, value: &str, max: usize| {
            if value.is_empty() || value.chars().count() > max {
                Err(VerifactuError::Identity(format!(
                    "{field} must be 1..{max} chars, got {value:?}"
                )))
            } else {
                Ok(())
            }
        };
        check("NombreSistemaInformatico", nombre, 30)?;
        check("IdSistemaInformatico", id, 2)?;
        check("Version", version, 50)?;
        Ok(Self {
            nombre_sistema_informatico: nombre.to_owned(),
            id_sistema_informatico: id.to_owned(),
            version: version.to_owned(),
        })
    }

    /// The `SistemaInformatico` block one installation declares: the
    /// SIF's producer is the obligado itself in both custody models.
    fn for_installation(
        &self,
        obligado: &Obligado,
        installation: &str,
    ) -> SistemaInformaticoConfig {
        SistemaInformaticoConfig {
            nombre_razon: obligado.nombre_razon.clone(),
            nif: obligado.nif.clone(),
            nombre_sistema_informatico: self.nombre_sistema_informatico.clone(),
            id_sistema_informatico: self.id_sistema_informatico.clone(),
            version: self.version.clone(),
            numero_instalacion: installation.to_owned(),
        }
    }
}

/// The certificate input — two doors.
pub enum CertificateSource<'a> {
    /// One PKCS#12 archive, one password — feeds BOTH legs (`XAdES`
    /// signing and the mTLS identity) from the same bytes. Demands the
    /// `http` feature (default): the archive's TLS leg belongs to the
    /// provided transport.
    Pkcs12 { p12: &'a [u8], password: &'a str },
    /// Already-parsed material — the KMS/HSM door: the consumer
    /// supplies the TLS identity and its own signing implementation
    /// however it decrypted them. Demands the `http` feature (the
    /// identity type is the provided transport's).
    #[cfg(feature = "http")]
    Parsed {
        identity: ClientIdentity,
        signer: Arc<dyn FiscalSigner>,
    },
    /// The bring-your-own door: the consumer supplies BOTH legs — its
    /// own `XAdES` signer and its own transport implementation (an edge
    /// runtime riding its mTLS-certificate binding, a hermetic fake).
    /// The facade builds no HTTP stack for it; works with
    /// `default-features = false`.
    Custom {
        signer: Arc<dyn FiscalSigner>,
        transport: Arc<dyn VerifactuTransport>,
    },
}

pub struct VerifactuBuilder {
    environment: Environment,
    #[cfg(feature = "http")]
    cert_family: CertFamily,
    sif: Option<SifConfig>,
    #[cfg(feature = "http")]
    timeout: Duration,
    clock: Option<Arc<dyn Clock>>,
}

impl std::fmt::Debug for VerifactuBuilder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VerifactuBuilder")
            .field("environment", &self.environment)
            .field(
                "clock",
                &self.clock.as_ref().map_or("<system>", |_| "<injected>"),
            )
            .finish_non_exhaustive()
    }
}

impl Default for VerifactuBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl VerifactuBuilder {
    #[must_use]
    pub fn new() -> Self {
        Self {
            environment: Environment::Pruebas,
            #[cfg(feature = "http")]
            cert_family: CertFamily::PersonalOEmpresa,
            sif: None,
            #[cfg(feature = "http")]
            timeout: DEFAULT_TIMEOUT,
            clock: None,
        }
    }

    #[must_use]
    pub fn environment(mut self, environment: Environment) -> Self {
        self.environment = environment;
        self
    }

    #[cfg(feature = "http")]
    #[must_use]
    pub fn cert_family(mut self, family: CertFamily) -> Self {
        self.cert_family = family;
        self
    }

    #[must_use]
    pub fn sif(mut self, sif: SifConfig) -> Self {
        self.sif = Some(sif);
        self
    }

    #[cfg(feature = "http")]
    #[must_use]
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// The clock every emission reads (default: the system wall
    /// clock). Tests inject [`engine::FakeClock`] for byte-identical
    /// emissions; wasm builds (no `SystemTime`) inject a clock over
    /// their runtime's date API.
    #[must_use]
    pub fn clock(mut self, clock: Arc<dyn Clock>) -> Self {
        self.clock = Some(clock);
        self
    }

    /// The production gate's one enforcement point.
    ///
    /// # Errors
    /// [`VerifactuError::ProductionGate`] when the target is production
    /// and the deployment key is absent;
    /// [`VerifactuError::Identity`] when the SIF identity is missing.
    pub fn build(self) -> Result<Verifactu, VerifactuError> {
        if self.environment == Environment::Production
            && std::env::var(PRODUCTION_ENV_KEY).as_deref() != Ok("1")
        {
            return Err(VerifactuError::ProductionGate(format!(
                "production demands {PRODUCTION_ENV_KEY}=1 alongside the consent token"
            )));
        }
        Ok(Verifactu {
            environment: self.environment,
            #[cfg(feature = "http")]
            cert_family: self.cert_family,
            sif: self.sif.ok_or_else(|| {
                VerifactuError::Identity(String::from("the SIF identity is required"))
            })?,
            #[cfg(feature = "http")]
            timeout: self.timeout,
            clock: self.clock.unwrap_or_else(|| Arc::new(SystemClock)),
        })
    }
}

pub struct Verifactu {
    environment: Environment,
    #[cfg(feature = "http")]
    cert_family: CertFamily,
    sif: SifConfig,
    #[cfg(feature = "http")]
    timeout: Duration,
    clock: Arc<dyn Clock>,
}

impl std::fmt::Debug for Verifactu {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Verifactu")
            .field("environment", &self.environment)
            .field("clock", &"<clock>")
            .finish_non_exhaustive()
    }
}

/// What a [`CertificateSource`] resolves into: the `XAdES` signing leg
/// plus the wire leg. Nothing survives `connect()` beyond these — no
/// archive bytes, no password.
struct CertLegs {
    signer: Arc<dyn FiscalSigner>,
    wire: WireLeg,
}

enum WireLeg {
    /// The provided HTTP transport, built per modality endpoint over
    /// this mTLS identity.
    #[cfg(feature = "http")]
    Identity(ClientIdentity),
    /// Injected by the consumer.
    Transport(Arc<dyn VerifactuTransport>),
}

impl CertLegs {
    fn of(clock: &Arc<dyn Clock>, cert: CertificateSource<'_>) -> Result<Self, VerifactuError> {
        match cert {
            CertificateSource::Pkcs12 { p12, password } => Self::pkcs12(clock, p12, password),
            #[cfg(feature = "http")]
            CertificateSource::Parsed { identity, signer } => Ok(Self {
                signer,
                wire: WireLeg::Identity(identity),
            }),
            CertificateSource::Custom { signer, transport } => Ok(Self {
                signer,
                wire: WireLeg::Transport(transport),
            }),
        }
    }

    #[cfg(all(feature = "http", feature = "signing"))]
    fn pkcs12(clock: &Arc<dyn Clock>, p12: &[u8], password: &str) -> Result<Self, VerifactuError> {
        // The SAME injected clock drives the emission instants AND the
        // XAdES signing time — a frozen clock reproduces byte-identical
        // signed emissions.
        let signer = crate::xades::XadesSigner::from_pkcs12(Arc::clone(clock), p12, password)
            .map_err(|_| {
                VerifactuError::Certificate(
                    "the archive does not load as a signing key (wrong password?)",
                )
            })?;
        let identity = crate::aeat::p12::identity_from_pkcs12(p12, password).map_err(|_| {
            VerifactuError::Certificate(
                "the archive does not load as a TLS identity (no key/certificate?)",
            )
        })?;
        Ok(Self {
            signer: Arc::new(signer),
            wire: WireLeg::Identity(identity),
        })
    }

    #[cfg(not(all(feature = "http", feature = "signing")))]
    fn pkcs12(
        _clock: &Arc<dyn Clock>,
        _p12: &[u8],
        _password: &str,
    ) -> Result<Self, VerifactuError> {
        Err(VerifactuError::Certificate(
            "the Pkcs12 door demands the http + signing features — enable them, or supply \
             both legs via CertificateSource::Custom",
        ))
    }
}

impl Verifactu {
    #[must_use]
    pub fn builder() -> VerifactuBuilder {
        VerifactuBuilder::new()
    }

    /// Where emissions go — products assert/display this; the gate
    /// test proves the deployment key was demanded.
    #[must_use]
    pub fn environment(&self) -> Environment {
        self.environment
    }

    /// The modality's transport over the wire leg. No network traffic
    /// happens here.
    #[cfg_attr(
        not(feature = "http"),
        allow(clippy::unused_self, clippy::unnecessary_wraps, unused_variables)
    )]
    fn transport_of(
        &self,
        wire: WireLeg,
        modality: Modality,
    ) -> Result<Arc<dyn VerifactuTransport>, VerifactuError> {
        match wire {
            #[cfg(feature = "http")]
            WireLeg::Identity(identity) => {
                let (remission, requerimiento) = endpoint_table(self.cert_family, self.environment);
                let endpoint = match modality {
                    Modality::Remission => remission,
                    Modality::Conservation => requerimiento,
                };
                let transport = HttpVerifactuTransport::new(endpoint, self.timeout)?
                    .with_client_identity(identity)?;
                Ok(Arc::new(transport))
            }
            WireLeg::Transport(transport) => Ok(transport),
        }
    }

    /// Custody (a): the TENANT's own certificate. Returns the
    /// connecting builder; `connect()` parses the archive once into
    /// BOTH legs (signer + mTLS identity) and owns its pooled
    /// transport. Cache the connected handles per tenant — each owns a
    /// connection pool.
    #[must_use]
    pub fn tenant<'a>(&self, cert: CertificateSource<'a>) -> TenantConnecting<'a, '_> {
        TenantConnecting {
            root: self,
            cert,
            obligado: None,
            modality: Modality::Remission,
            installation: String::from("1"),
        }
    }

    /// Custody (b): OUR one certificate + the Cabecera
    /// `Representante`. ONE pooled transport serves every tenant; the
    /// per-tenant emitters are cached and cheap.
    #[must_use]
    pub fn vendor<'a>(&self, cert: CertificateSource<'a>) -> VendorConnecting<'a, '_> {
        VendorConnecting {
            root: self,
            cert,
            representante: None,
        }
    }
}

/// The facade's failures: door-naming, never key material.
#[derive(Debug, thiserror::Error)]
pub enum VerifactuError {
    #[error("verifactu production gate: {0}")]
    ProductionGate(String),
    #[error("verifactu certificate: {0}")]
    Certificate(&'static str),
    #[error("verifactu identity: {0}")]
    Identity(String),
    #[cfg(feature = "http")]
    #[error("verifactu wire: {0}")]
    Wire(#[from] crate::aeat::WireBuildError),
}

/// The tenant-custody connecting builder.
pub struct TenantConnecting<'a, 'r> {
    root: &'r Verifactu,
    cert: CertificateSource<'a>,
    obligado: Option<Obligado>,
    modality: Modality,
    installation: String,
}

impl TenantConnecting<'_, '_> {
    #[must_use]
    pub fn obligado(mut self, obligado: Obligado) -> Self {
        self.obligado = Some(obligado);
        self
    }

    /// The tenant's legal modality (remission by default).
    #[must_use]
    pub fn modality(mut self, modality: Modality) -> Self {
        self.modality = modality;
        self
    }

    /// The tenant's `NumeroInstalacion` — AEAT keys record state by
    /// obligado + SIF identity including this (the live-learned 2007
    /// rule), so per-tenant installations are first-class.
    #[must_use]
    pub fn installation(mut self, installation: &str) -> Self {
        installation.clone_into(&mut self.installation);
        self
    }

    /// Parses the certificate once into both legs and builds the
    /// tenant's own pooled emitter. No network traffic happens here.
    ///
    /// # Errors
    /// [`VerifactuError`] per the doors (certificate, identity, wire).
    pub fn connect(self) -> Result<TenantVerifactu, VerifactuError> {
        let obligado = self.obligado.ok_or_else(|| {
            VerifactuError::Identity(String::from("the tenant obligado is required"))
        })?;
        let legs = CertLegs::of(&self.root.clock, self.cert)?;
        let transport = self.root.transport_of(legs.wire, self.modality)?;
        let sif = self
            .root
            .sif
            .for_installation(&obligado, &self.installation);
        let emitter = VerifactuEmitter::new(self.modality, obligado, sif, legs.signer)
            .map_err(|error| VerifactuError::Identity(error.to_string()))?
            .with_clock(Arc::clone(&self.root.clock))
            .with_transport(transport);
        Ok(TenantVerifactu { emitter })
    }
}

pub struct TenantVerifactu {
    emitter: VerifactuEmitter,
}

impl TenantVerifactu {
    /// The engine face of this connection: ledger owners that seal
    /// their own records submit through the emitter directly
    /// (`engine::VerifactuEmitter::submit`); products never look.
    #[must_use]
    pub fn emitter(&self) -> &VerifactuEmitter {
        &self.emitter
    }

    /// The invoice-level emit (products without their own ledger):
    /// seal the draft onto `prev` (the installation's first record
    /// when `None`) and submit. The return carries the emission AND
    /// the chain cursor — persist `cursor` and hand it back as the
    /// next call's `prev`; it is the only durable state the product
    /// owns.
    ///
    /// A retry after an ambiguous transport failure re-seals the
    /// record at a new instant (a new huella for the same IDFactura):
    /// if the first attempt reached AEAT, the retry comes back as
    /// AEAT's duplicate answer ([`response::Disposition::DrainWithNote`])
    /// — treat that as accepted, keeping the cursor the accepted
    /// attempt returned.
    ///
    /// # Errors
    /// [`EmitError`] — the draft's gates fire inside, before any byte
    /// serializes.
    pub async fn emit_invoice(
        &self,
        draft: InvoiceDraft,
        prev: Option<&Predecessor>,
    ) -> Result<EmittedInvoice, EmitError> {
        emit_draft(&self.emitter, draft, prev).await
    }

    /// Cancels (anula) a previously emitted invoice: `annulled` is the
    /// invoice's own stored [`Predecessor`] (the cursor its emission
    /// returned), `prev` the chain head to seal onto. The return's
    /// `cursor` is the anulación record — the new chain head.
    ///
    /// # Errors
    /// [`EmitError::InvalidRecord`] when `annulled` names an event
    /// record (only invoices are annullable).
    pub async fn cancel_invoice(
        &self,
        annulled: &Predecessor,
        prev: Option<&Predecessor>,
    ) -> Result<EmittedInvoice, EmitError> {
        cancel_draft(&self.emitter, annulled, prev).await
    }

    /// The read-back of what AEAT stores for this obligado (the
    /// tenant's own lens): every page merged, each answered record
    /// carrying its stored estado + the fields its huella hashes —
    /// [`RegistroConsulta::huella_verifica`] proves AEAT stores
    /// byte-what-was-sealed.
    ///
    /// # Errors
    /// [`EmitError`] per the consulta pipeline (fault classes included).
    pub async fn consulta(&self, filtro: &ConsultaFilter<'_>) -> Result<ConsultaAnswer, EmitError> {
        self.emitter.consulta(filtro).await
    }
}

/// The vendor-custody connecting builder.
pub struct VendorConnecting<'a, 'r> {
    root: &'r Verifactu,
    cert: CertificateSource<'a>,
    representante: Option<Obligado>,
}

impl VendorConnecting<'_, '_> {
    /// MANDATORY in this model: who remits on the tenants' behalf (the
    /// Cabecera `Representante`).
    #[must_use]
    pub fn representante(mut self, representante: Obligado) -> Self {
        self.representante = Some(representante);
        self
    }

    /// Builds the shared-pool vendor connection. ONE transport serves
    /// every tenant; emitters are cached per (tenant, installation,
    /// modality).
    ///
    /// # Errors
    /// [`VerifactuError`] per the doors; [`VerifactuError::Identity`]
    /// when the representante is missing.
    pub fn connect(self) -> Result<VendorVerifactu, VerifactuError> {
        let representante = self.representante.ok_or_else(|| {
            VerifactuError::Identity(String::from(
                "the vendor model demands the representante (the Cabecera block names us)",
            ))
        })?;
        let legs = CertLegs::of(&self.root.clock, self.cert)?;
        let transport = self.root.transport_of(legs.wire, Modality::Remission)?;
        Ok(VendorVerifactu {
            signer: legs.signer,
            transport,
            representante,
            sif: self.root.sif.clone(),
            clock: Arc::clone(&self.root.clock),
            emitters: RwLock::new(HashMap::new()),
        })
    }
}

/// The vendor connection: one certificate, one pool, every tenant.
pub struct VendorVerifactu {
    signer: Arc<dyn FiscalSigner>,
    transport: Arc<dyn VerifactuTransport>,
    representante: Obligado,
    sif: SifConfig,
    clock: Arc<dyn Clock>,
    emitters: RwLock<HashMap<(String, String, Modality), Arc<VerifactuEmitter>>>,
}

impl std::fmt::Debug for VendorVerifactu {
    /// Redacted: never prints signer internals.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VendorVerifactu")
            .field("representante", &self.representante.nif)
            .finish_non_exhaustive()
    }
}

impl VendorVerifactu {
    /// The cached emitter for one tenant, as an [`EmitError`]: emitter
    /// construction only ever fails the identity checks.
    ///
    /// # Panics
    /// On a poisoned emitter-cache lock (a bug, never an input).
    ///
    /// # Errors
    /// [`EmitError::InvalidRecord`] when the tenant identity is
    /// off-contract.
    fn emitter_of(
        &self,
        tenant: &Obligado,
        installation: &str,
        modality: Modality,
    ) -> Result<Arc<VerifactuEmitter>, EmitError> {
        let key = (tenant.nif.clone(), installation.to_owned(), modality);
        if let Some(emitter) = self.emitters.read().expect("emitter cache lock").get(&key) {
            return Ok(Arc::clone(emitter));
        }
        let emitter = VerifactuEmitter::new(
            modality,
            tenant.clone(),
            self.sif.for_installation(tenant, installation),
            Arc::clone(&self.signer),
        )?
        .with_clock(Arc::clone(&self.clock))
        .with_representante(self.representante.clone())?
        .with_transport(Arc::clone(&self.transport));
        let emitter = Arc::new(emitter);
        self.emitters
            .write()
            .expect("emitter cache lock")
            .insert(key, Arc::clone(&emitter));
        Ok(emitter)
    }

    /// The invoice-level emit for one tenant (see
    /// [`TenantVerifactu::emit_invoice`], including the retry law).
    ///
    /// # Errors
    /// [`EmitError::InvalidRecord`] when the tenant identity is
    /// off-contract; the rest of [`EmitError`] from the emission itself.
    pub async fn emit_invoice_for(
        &self,
        tenant: &Obligado,
        installation: &str,
        modality: Modality,
        draft: InvoiceDraft,
        prev: Option<&Predecessor>,
    ) -> Result<EmittedInvoice, EmitError> {
        let emitter = self.emitter_of(tenant, installation, modality)?;
        emit_draft(&emitter, draft, prev).await
    }

    /// The anulación for one tenant (see
    /// [`TenantVerifactu::cancel_invoice`]).
    ///
    /// # Errors
    /// [`EmitError::InvalidRecord`] when the tenant identity is
    /// off-contract; the rest of [`EmitError`] from the emission itself.
    pub async fn cancel_invoice_for(
        &self,
        tenant: &Obligado,
        installation: &str,
        modality: Modality,
        annulled: &Predecessor,
        prev: Option<&Predecessor>,
    ) -> Result<EmittedInvoice, EmitError> {
        let emitter = self.emitter_of(tenant, installation, modality)?;
        cancel_draft(&emitter, annulled, prev).await
    }

    /// Emits one envío for one tenant — the ledger-owner's call.
    ///
    /// # Errors
    /// [`EmitError::InvalidRecord`] on emitter construction (the
    /// tenant identity checks); the rest of [`EmitError`] from the
    /// emission itself.
    pub async fn emit_for(
        &self,
        tenant: &Obligado,
        installation: &str,
        modality: Modality,
        pairs: &[(ChainRecord, EmissionContext<'_>)],
    ) -> Result<Emission, EmitError> {
        let emitter = self.emitter_of(tenant, installation, modality)?;
        emitter.submit(pairs).await
    }

    /// The read-back through the APODERADO lens (`IndicadorRepresentante=S`):
    /// AEAT answers the records of `tenant`'s obligation in which we
    /// figure as representante (SI.xsd's own words for the flag).
    ///
    /// The lens answers the AEAT-**registered** representante
    /// relationship, never the emission's `Representante` block —
    /// live-pinned 2026-10-05: a Representante-declared record answers
    /// `SinDatos` (0s/20s/60s) while the cotejo door says `Encontrada`.
    /// With no registration, reconcile through the tenant's own lens
    /// ([`TenantVerifactu::consulta`]).
    ///
    /// # Errors
    /// [`EmitError`] per the consulta pipeline (fault classes included).
    pub async fn consulta_for(
        &self,
        tenant: &Obligado,
        filtro: &ConsultaFilter<'_>,
    ) -> Result<ConsultaAnswer, EmitError> {
        consulta::send_all(&self.transport, tenant, true, filtro).await
    }
}

/// The eight documented endpoints as (remission, requerimiento) pairs
/// for each family × environment.
#[must_use]
pub fn endpoint_table(
    family: CertFamily,
    environment: Environment,
) -> (&'static str, &'static str) {
    match (family, environment) {
        (CertFamily::PersonalOEmpresa, Environment::Pruebas) => (
            "https://prewww1.aeat.es/wlpl/TIKE-CONT/ws/SistemaFacturacion/VerifactuSOAP",
            "https://prewww1.aeat.es/wlpl/TIKE-CONT/ws/SistemaFacturacion/RequerimientoSOAP",
        ),
        (CertFamily::Sello, Environment::Pruebas) => (
            "https://prewww10.aeat.es/wlpl/TIKE-CONT/ws/SistemaFacturacion/VerifactuSOAP",
            "https://prewww10.aeat.es/wlpl/TIKE-CONT/ws/SistemaFacturacion/RequerimientoSOAP",
        ),
        (CertFamily::PersonalOEmpresa, Environment::Production) => (
            "https://www1.agenciatributaria.gob.es/wlpl/TIKE-CONT/ws/SistemaFacturacion/VerifactuSOAP",
            "https://www1.agenciatributaria.gob.es/wlpl/TIKE-CONT/ws/SistemaFacturacion/RequerimientoSOAP",
        ),
        (CertFamily::Sello, Environment::Production) => (
            "https://www10.agenciatributaria.gob.es/wlpl/TIKE-CONT/ws/SistemaFacturacion/VerifactuSOAP",
            "https://www10.agenciatributaria.gob.es/wlpl/TIKE-CONT/ws/SistemaFacturacion/RequerimientoSOAP",
        ),
    }
}

/// One desglose line's 90% shapes; a hand-built
/// [`DetalleDesglose`] via [`InvoiceDraft::line`] is the full door.
pub mod lines {
    use super::{DetalleDesglose, Money};

    /// Integer math (money never floats), half up away from zero;
    /// `ImporteSgn12.2` × 100 % cannot overflow `i64` cents, so the
    /// checked product's panic is a bug, never an input.
    fn cuota_cents(rate_cents: i64, base_cents: i64) -> i64 {
        let product = rate_cents
            .checked_mul(base_cents)
            .expect("ImporteSgn12.2 × rate fits i64 cents");
        let sign = if product < 0 { -1_i64 } else { 1 };
        ((product.unsigned_abs() + 5_000) / 10_000).cast_signed() * sign
    }

    /// An IVA line at `rate` cents (2100 = 21.00 %) over `base` cents —
    /// the cuota is computed from the pair, so it can never disagree
    /// with its rate.
    #[must_use]
    pub fn sujeta_iva(rate_cents: i64, base_cents: i64) -> DetalleDesglose {
        DetalleDesglose {
            impuesto: Some(super::Impuesto::Iva),
            clave_regimen: Some(super::ClaveRegimen::C01),
            calificacion: Some(super::CalificacionOperacion::SujetaNoExenta),
            operacion_exenta: None,
            tipo_impositivo: Some(Money::from_cents(rate_cents)),
            base_imponible: Money::from_cents(base_cents),
            cuota_repercutida: Some(Money::from_cents(cuota_cents(rate_cents, base_cents))),
            tipo_recargo_equivalencia: None,
            cuota_recargo_equivalencia: None,
        }
    }

    /// An exempt line (`E1`..`E8`).
    #[must_use]
    pub fn exenta(exenta: super::OperacionExenta, base_cents: i64) -> DetalleDesglose {
        DetalleDesglose {
            impuesto: Some(super::Impuesto::Iva),
            clave_regimen: Some(super::ClaveRegimen::C01),
            calificacion: None,
            operacion_exenta: Some(exenta),
            tipo_impositivo: None,
            base_imponible: Money::from_cents(base_cents),
            cuota_repercutida: None,
            tipo_recargo_equivalencia: None,
            cuota_recargo_equivalencia: None,
        }
    }

    /// A not-subject line (`N1`/`N2`).
    #[must_use]
    pub fn no_sujeta(
        calificacion: super::CalificacionOperacion,
        base_cents: i64,
    ) -> DetalleDesglose {
        DetalleDesglose {
            impuesto: Some(super::Impuesto::Iva),
            clave_regimen: Some(super::ClaveRegimen::C01),
            calificacion: Some(calificacion),
            operacion_exenta: None,
            tipo_impositivo: None,
            base_imponible: Money::from_cents(base_cents),
            cuota_repercutida: None,
            tipo_recargo_equivalencia: None,
            cuota_recargo_equivalencia: None,
        }
    }
}

/// One invoice at business level — the facade seals the chain record
/// (totals from the lines, macrodato at the 100 M threshold, dates at
/// the site zone) and submits; the consumer supplies only the chain
/// state. The 1189 law (destinatarios on F1/F3/R1–R4) is enforced
/// here, before any byte serializes.
#[derive(Debug, Default)]
pub struct InvoiceDraft {
    serie: Option<(Series, u64)>,
    tipo: Option<TipoFactura>,
    descripcion: Option<String>,
    lines: Vec<DetalleDesglose>,
    destinatarios: Vec<IDDestinatario>,
    rectificativa: Option<(
        TipoRectificativa,
        Vec<FacturaId>,
        Option<ImporteRectificacion>,
    )>,
    ref_externa: Option<String>,
}

impl InvoiceDraft {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The serie and correlative (the durable invoice number).
    #[must_use]
    pub fn serie(mut self, serie: Series, number: u64) -> Self {
        self.serie = Some((serie, number));
        self
    }

    /// The `TipoFactura`; defaults to `F2` on serie `T` and `F1` on
    /// series `F`/`R` (a custom series demands it — no implied
    /// document class).
    #[must_use]
    pub fn tipo(mut self, tipo: TipoFactura) -> Self {
        self.tipo = Some(tipo);
        self
    }

    /// The free-text description (`DescripcionOperacion`).
    #[must_use]
    pub fn descripcion(mut self, descripcion: impl Into<String>) -> Self {
        self.descripcion = Some(descripcion.into());
        self
    }

    #[must_use]
    pub fn line(mut self, line: DetalleDesglose) -> Self {
        self.lines.push(line);
        self
    }

    /// The counterparty (REQUIRED on F1/F3/R1–R4 — AEAT 1189).
    #[must_use]
    pub fn destinatario(mut self, destinatario: IDDestinatario) -> Self {
        self.destinatarios.push(destinatario);
        self
    }

    /// Marks the invoice as a rectificativa over `facturas`.
    #[must_use]
    pub fn rectificativa(
        mut self,
        tipo: TipoRectificativa,
        facturas: Vec<FacturaId>,
        importe_rectificacion: Option<ImporteRectificacion>,
    ) -> Self {
        self.rectificativa = Some((tipo, facturas, importe_rectificacion));
        self
    }

    /// The optional external reference (`RefExterna`).
    #[must_use]
    pub fn ref_externa(mut self, ref_externa: impl Into<String>) -> Self {
        self.ref_externa = Some(ref_externa.into());
        self
    }

    /// The serie/number gates, answering the rendered
    /// `NumSerieFactura`: the correlative renders 8 digits, a custom
    /// prefix is named, and the rendering meets its wire law.
    ///
    /// # Errors
    /// [`EmitError::InvalidRecord`] naming the broken bound.
    fn num_serie(serie: &Series, number: u64) -> Result<String, EmitError> {
        let invalid = |detail: &str| EmitError::InvalidRecord {
            detail: detail.to_owned(),
        };
        let Some(num_serie) = crate::domain::series::checked_format(serie, number) else {
            return Err(invalid(
                "the correlative is 1..=99_999_999 — a gapless series cannot reach a 9-digit \
                 correlative (NumSerieFactura renders 8)",
            ));
        };
        if matches!(serie, Series::Custom(prefix) if prefix.is_empty()) {
            return Err(invalid(
                "a custom series prefix is required (the builtin prefixes are T/F/R)",
            ));
        }
        crate::fiscal::xml::check_num_serie("NumSerieFactura", &num_serie)?;
        Ok(num_serie)
    }

    /// Validates the draft into what [`emit_draft`] seals — the facade's
    /// one construction law: totals from the lines, macrodato at the
    /// threshold, instants from `now`, dates at the site zone. The
    /// issuer is the emitter's obligado NIF — never a second copy that
    /// could disagree.
    ///
    /// # Errors
    /// [`EmitError::InvalidRecord`] naming the broken law.
    fn build(self, issuer: &str, now: u64) -> Result<Built, EmitError> {
        let invalid = |detail: String| EmitError::InvalidRecord { detail };
        let Some((serie, number)) = self.serie else {
            return Err(invalid(String::from(
                "the invoice's serie and number are required",
            )));
        };
        let num_serie = Self::num_serie(&serie, number)?;
        let Some(descripcion) = self.descripcion.filter(|text| !text.is_empty()) else {
            return Err(invalid(String::from(
                "the invoice's description is required",
            )));
        };
        if self.lines.is_empty() {
            return Err(invalid(String::from(
                "at least one desglose line is required",
            )));
        }
        let tipo = match (self.tipo, &serie) {
            (Some(tipo), _) => tipo,
            (None, Series::T) => TipoFactura::F2,
            (None, Series::F | Series::R) => TipoFactura::F1,
            (None, Series::Custom(_)) => {
                return Err(invalid(String::from(
                    "a custom series carries no default TipoFactura — name it (.tipo(..))",
                )));
            }
        };
        if tipo.requires_destinatario() && self.destinatarios.is_empty() {
            return Err(invalid(format!(
                "TipoFactura {tipo:?} requires its destinatario (AEAT 1189)"
            )));
        }
        // Totals cohere by construction: Σ cuotas (incl. recargo) and
        // Σ base + Σ cuotas.
        let base_total: i64 = self
            .lines
            .iter()
            .map(|line| line.base_imponible.as_cents())
            .sum();
        let cuota_total: i64 = self
            .lines
            .iter()
            .flat_map(|line| [line.cuota_repercutida, line.cuota_recargo_equivalencia])
            .flatten()
            .map(Money::as_cents)
            .sum();
        let importe_total = base_total + cuota_total;

        let instants = huso::render_instants(crate::clock::zone::SiteZone::EuropeMadrid, now);
        let kind = ChainKind::Alta {
            issuer: issuer.to_owned(),
            serie,
            number,
            fecha_expedicion: FechaExpedicion::parse(&instants.fecha_expedicion)
                .expect("huso renders the dd-mm-yyyy grammar"),
            tipo_factura: tipo,
            cuota_total: Money::from_cents(cuota_total),
            importe_total: Money::from_cents(importe_total),
            fecha_huso_gen: FechaHuso::parse(&instants.fecha_huso_gen)
                .expect("huso renders the ISO-8601 grammar"),
        };
        Ok(Built {
            kind,
            num_serie,
            descripcion,
            lines: self.lines,
            destinatarios: self.destinatarios,
            rectificativa: self.rectificativa,
            ref_externa: self.ref_externa,
            macrodato: importe_total.abs() >= MACRODATO_CENTS,
        })
    }
}

/// `Macrodato=S` from an `ImporteTotal` of 100 M€ (AEAT's threshold).
const MACRODATO_CENTS: i64 = 10_000_000_000;

/// A validated draft: the alta's kind plus everything its emission
/// context borrows.
struct Built {
    kind: ChainKind,
    num_serie: String,
    descripcion: String,
    lines: Vec<DetalleDesglose>,
    destinatarios: Vec<IDDestinatario>,
    rectificativa: Option<(
        TipoRectificativa,
        Vec<FacturaId>,
        Option<ImporteRectificacion>,
    )>,
    ref_externa: Option<String>,
    macrodato: bool,
}

/// One invoice through the flat API: AEAT's verdict plus the chain
/// cursor. `cursor` is exactly what the NEXT invoice's `prev` takes —
/// persist it (it is the installation's whole durable chain state) and
/// pass it back; nothing else needs to survive the call. Hermetic
/// wiring is the same call over `CertificateSource::Custom` with
/// `engine::FakeSigner` + `engine::FakeVerifactuTransport` — no
/// hand-rolled stubs.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct EmittedInvoice {
    /// The envío's typed outcome — AEAT's per-line verdicts, the CSV,
    /// the disposition.
    pub emission: Emission,
    /// The sealed record's chain coordinates + huella — the next
    /// record's `prev` (see [`ChainRecord::predecessor`], the one
    /// constructor of the shape).
    pub cursor: Predecessor,
    /// What was sealed: the record's kind — the `TipoFactura` in force
    /// (after defaulting), the totals, the instants. An anulación's
    /// kind carries the annulled invoice's coordinates.
    pub kind: ChainKind,
    /// The rendered `NumSerieFactura` (prefix + 8-digit correlative).
    pub num_serie: String,
}

impl EmittedInvoice {
    /// The cotejo URL the printed QR encodes (QR spec §4–§5): THIS
    /// factura's four mandatory parameters over the environment ×
    /// modality base. `None` for anulación kinds — the printed QR is
    /// always the factura's own (cotejo of an annulled factura answers
    /// the annulment; no anulación QR shape exists).
    ///
    /// The image is the consumer's print pipeline, but two of its
    /// parameters are legal, not rendering, choices (art. 21 via §3):
    /// error-correction level **M** — a generic QR library's default L
    /// silently violates the spec — and 30×30–40×40 mm, with
    /// [`TRIBUTARY_CAPTION`] above and — remission modality —
    /// [`VERIFIABLE_CAPTION`] below.
    #[must_use]
    pub fn qr_url(&self, environment: Environment, modality: Modality) -> Option<String> {
        let ChainKind::Alta {
            issuer,
            fecha_expedicion,
            importe_total,
            ..
        } = &self.kind
        else {
            return None;
        };
        // The QR grammar caps the importe at 9 integer digits (spec
        // §5's `NNNNNNNNN.DD`) — a wider total has no on-grammar URL.
        if importe_total.as_cents().abs() >= 100_000_000_000 {
            return None;
        }
        Some(verification_url(
            environment,
            modality,
            issuer,
            &self.num_serie,
            fecha_expedicion,
            *importe_total,
        ))
    }
}

/// The invoice-level emit over ANY emitter (the testing seam — a
/// hermetic emitter is [`VerifactuEmitter`] + the re-exported
/// [`engine::FakeSigner`]/[`engine::FakeVerifactuTransport`]; production rides the
/// custody connections' own method).
///
/// Seals the draft's record onto `prev` (the installation's first
/// record when `None`), submits one envío, and returns the emission
/// WITH the chain cursor. The borrow of the draft's parts lives
/// exactly across this call — nothing leaks.
///
/// # Errors
/// [`EmitError::InvalidRecord`] when the draft itself is off-contract
/// (before any byte serializes); the rest of [`EmitError`] from the
/// emission pipeline.
pub async fn emit_draft(
    emitter: &VerifactuEmitter,
    draft: InvoiceDraft,
    prev: Option<&Predecessor>,
) -> Result<EmittedInvoice, EmitError> {
    let now = emitter.now_utc().0;
    let built = draft.build(&emitter.obligado().nif, now)?;
    let (tipo_rectificativa, facturas_rectificadas, importe_rectificacion) =
        match &built.rectificativa {
            Some((tipo, facturas, importe)) => (Some(*tipo), facturas.as_slice(), importe.clone()),
            None => (None, &[][..], None),
        };
    let context = EmissionContext {
        ref_externa: built.ref_externa.as_deref(),
        descripcion_operacion: Some(built.descripcion.as_str()),
        desglose: built.lines.as_slice(),
        tipo_rectificativa,
        facturas_rectificadas,
        importe_rectificacion,
        destinatarios: built.destinatarios.as_slice(),
        macrodato: built.macrodato.then_some(SiNo::Si),
        ..EmissionContext::default()
    };
    seal_and_submit(emitter, built.kind, now, prev, context, built.num_serie).await
}

/// The anulación-level emit over ANY emitter (the same testing seam as
/// [`emit_draft`]): seals a `RegistroAnulacion` for `annulled` (the
/// invoice's own stored [`Predecessor`]) onto `prev`, submits it with
/// the anulación's legal context (all-default), and returns the
/// emission WITH the anulación record's cursor — the new chain head.
///
/// # Errors
/// [`EmitError::InvalidRecord`] when `annulled` names an event record
/// (only invoices are annullable); the rest of [`EmitError`] from the
/// emission pipeline.
///
/// # Panics
/// Never on input: the instant renders come from `huso` itself.
pub async fn cancel_draft(
    emitter: &VerifactuEmitter,
    annulled: &Predecessor,
    prev: Option<&Predecessor>,
) -> Result<EmittedInvoice, EmitError> {
    let Predecessor::Factura(invoice) = annulled else {
        return Err(EmitError::InvalidRecord {
            detail: String::from("only invoices are annullable — an event record is not"),
        });
    };
    let Some(num_serie) = crate::domain::series::checked_format(&invoice.serie, invoice.number)
    else {
        return Err(EmitError::InvalidRecord {
            detail: String::from("the annulled invoice's correlative exceeds 99_999_999"),
        });
    };
    let now = emitter.now_utc().0;
    let instants = huso::render_instants(crate::clock::zone::SiteZone::EuropeMadrid, now);
    let kind = ChainKind::Anulacion {
        issuer: invoice.issuer.clone(),
        serie: invoice.serie.clone(),
        number: invoice.number,
        fecha_expedicion: invoice.fecha_expedicion.clone(),
        fecha_huso_gen: FechaHuso::parse(&instants.fecha_huso_gen)
            .expect("huso renders the ISO-8601 grammar"),
    };
    seal_and_submit(
        emitter,
        kind,
        now,
        prev,
        EmissionContext::default(),
        num_serie,
    )
    .await
}

/// Seals `kind` onto `prev` at `now`, submits the one-record envío, and
/// answers the emission with the sealed record's cursor.
async fn seal_and_submit(
    emitter: &VerifactuEmitter,
    kind: ChainKind,
    now: u64,
    prev: Option<&Predecessor>,
    context: EmissionContext<'_>,
    num_serie: String,
) -> Result<EmittedInvoice, EmitError> {
    let record = ChainRecord::seal(kind.clone(), now, prev);
    let cursor = record.predecessor();
    let emission = emitter.submit(&[(record, context)]).await?;
    Ok(EmittedInvoice {
        emission,
        cursor,
        kind,
        num_serie,
    })
}

#[cfg(test)]
mod tests {
    use super::{
        endpoint_table, CertFamily, Environment, ProductionOptIn, SifConfig, Verifactu,
        VerifactuError,
    };

    fn root() -> Verifactu {
        Verifactu::builder()
            .sif(SifConfig::new("mi-facturacion", "MF", "1.0.0").expect("valid SIF"))
            .build()
            .expect("pruebas root builds with no gate")
    }

    /// The gate's whole law: production demands BOTH the token (else it
    /// is unrepresentable) and the deployment key (else build refuses,
    /// named and fail-closed, before anything exists).
    #[test]
    fn the_production_gate_fails_closed_without_the_deployment_key() {
        std::env::remove_var("VERIFACTU_ALLOW_PRODUCTION");
        let error = Verifactu::builder()
            .environment(Environment::production(
                ProductionOptIn::these_emissions_are_legally_binding(),
            ))
            .sif(SifConfig::new("mi-facturacion", "MF", "1.0.0").expect("valid SIF"))
            .build()
            .expect_err("the deployment key is absent");
        assert!(
            matches!(error, VerifactuError::ProductionGate(_)),
            "{error}"
        );

        std::env::set_var("VERIFACTU_ALLOW_PRODUCTION", "1");
        let root = Verifactu::builder()
            .environment(Environment::production(
                ProductionOptIn::these_emissions_are_legally_binding(),
            ))
            .sif(SifConfig::new("mi-facturacion", "MF", "1.0.0").expect("valid SIF"))
            .build()
            .expect("token + key");
        assert_eq!(root.environment(), Environment::Production);
        std::env::remove_var("VERIFACTU_ALLOW_PRODUCTION");
    }

    #[test]
    fn the_endpoint_table_is_the_documented_eight() {
        let (remission, requerimiento) =
            endpoint_table(CertFamily::PersonalOEmpresa, Environment::Pruebas);
        assert!(remission.contains("prewww1.aeat.es") && remission.ends_with("VerifactuSOAP"));
        assert!(
            requerimiento.contains("prewww1.aeat.es")
                && requerimiento.ends_with("RequerimientoSOAP")
        );
        let (remission, _) = endpoint_table(CertFamily::Sello, Environment::Pruebas);
        assert!(remission.contains("prewww10"), "sello rides the 10-hosts");
        let (remission, _) = endpoint_table(CertFamily::PersonalOEmpresa, Environment::Production);
        assert!(remission.contains("www1.agenciatributaria.gob.es"));
    }

    #[test]
    fn the_sif_identity_validates_eagerly() {
        assert!(
            SifConfig::new("x", "FMA", "1.0").is_err(),
            "IdSistemaInformatico is TextMax2"
        );
        assert!(
            SifConfig::new("", "FM", "1.0").is_err(),
            "names are non-empty"
        );
    }

    /// The anulación door, hermetically: a stored invoice cursor
    /// cancels that invoice, the anulación chains onto the head, and
    /// its returned cursor carries the annulled invoice's own
    /// coordinates. An event predecessor is refused.
    #[tokio::test]
    async fn the_cancel_door_annuls_a_stored_cursor_and_chains() {
        use super::engine::{cancel_draft, emit_draft, FakeSigner, FakeVerifactuTransport};
        use super::engine::{ScriptedOutcome, VerifactuEmitter};
        use super::{lines, InvoiceDraft, Modality, Obligado, Predecessor, Series};
        let transport = std::sync::Arc::new(FakeVerifactuTransport::new());
        transport.push_outcome(ScriptedOutcome::Correcto {
            csv: Some(String::from("1111222233334444")),
            tiempo_espera_envio: 60,
        });
        let emitter = VerifactuEmitter::new(
            Modality::Remission,
            Obligado {
                nombre_razon: String::from("ANA RUIZ PEREZ"),
                nif: String::from("12345678Z"),
            },
            super::SistemaInformaticoConfig {
                nombre_razon: String::from("ANA RUIZ PEREZ"),
                nif: String::from("12345678Z"),
                nombre_sistema_informatico: String::from("mi-facturacion"),
                id_sistema_informatico: String::from("FM"),
                version: String::from("1.2.0"),
                numero_instalacion: String::from("000042"),
            },
            std::sync::Arc::new(FakeSigner::new()),
        )
        .expect("valid identity")
        .with_transport(transport);

        let invoice = emit_draft(
            &emitter,
            InvoiceDraft::new()
                .serie(Series::T, 7)
                .descripcion("venta mostrador")
                .line(lines::sujeta_iva(2100, 10_000)),
            None,
        )
        .await
        .expect("the invoice emits");

        // The printed QR rides the sealed record's own data — and only
        // altas carry one.
        let qr = invoice
            .qr_url(super::Environment::Pruebas, Modality::Remission)
            .expect("an alta carries its QR");
        assert!(
            qr.starts_with(
                "https://prewww2.aeat.es/wlpl/TIKE-CONT/ValidarQR?nif=12345678Z&numserie=T00000007&fecha="
            ) && qr.ends_with("&importe=121.00"),
            "{qr}"
        );

        let anulado = cancel_draft(&emitter, &invoice.cursor, Some(&invoice.cursor))
            .await
            .expect("the anulación emits");
        let Predecessor::Factura(head) = &anulado.cursor else {
            panic!("an anulación's cursor is a Factura predecessor");
        };
        assert_eq!(
            (head.serie.clone(), head.number),
            (Series::T, 7),
            "the anulación carries the annulled invoice's own coordinates"
        );
        assert!(
            anulado
                .qr_url(super::Environment::Pruebas, Modality::Remission)
                .is_none(),
            "the printed QR is always the factura's own — anulación kinds carry none"
        );

        let error = cancel_draft(&emitter, &Predecessor::Evento(String::new()), None)
            .await
            .expect_err("events are not annullable");
        assert!(matches!(error, super::EmitError::InvalidRecord { .. }));
    }

    #[test]
    fn the_vendor_door_demands_the_representante() {
        use super::engine::{FakeSigner, FakeVerifactuTransport};
        let error = root()
            .vendor(super::CertificateSource::Custom {
                signer: std::sync::Arc::new(FakeSigner::new()),
                transport: std::sync::Arc::new(FakeVerifactuTransport::new()),
            })
            .connect()
            .expect_err("the representante is the model's own name");
        assert!(matches!(error, VerifactuError::Identity(_)), "{error}");
    }

    /// The consulta doors, hermetically: the tenant lens asks plainly,
    /// the vendor lens sets `IndicadorRepresentante=S` (the apoderado
    /// law), and pagination merges every page in call order.
    #[tokio::test]
    async fn the_consulta_doors_read_back_apoderado_and_paginate() {
        use super::engine::consulta::{ConsultaSpec, ResultadoConsulta};
        use super::engine::{FakeSigner, FakeVerifactuTransport};
        use super::{ConsultaFilter, Mes, Obligado};
        let obligado = Obligado {
            nombre_razon: String::from("ANA RUIZ PEREZ"),
            nif: String::from("12345678Z"),
        };
        let transport = std::sync::Arc::new(FakeVerifactuTransport::new());
        transport.push_consulta(ConsultaSpec {
            obligado_nombre: obligado.nombre_razon.clone(),
            obligado_nif: obligado.nif.clone(),
            apoderado: true,
            ejercicio: 2026,
            mes: Mes::try_new(10).expect("octubre"),
            resultado: ResultadoConsulta::ConDatos,
            paginacion_pendiente: true,
            clave_paginacion: Some(crate::fiscal::consulta::ClavePaginacion {
                id_emisor_factura: obligado.nif.clone(),
                num_serie_factura: String::from("T00000009"),
                fecha_expedicion_factura: String::from("05-10-2026"),
            }),
            registros: vec![consulta_registro_page(7)],
        });
        transport.push_consulta(ConsultaSpec {
            obligado_nombre: obligado.nombre_razon.clone(),
            obligado_nif: obligado.nif.clone(),
            apoderado: false,
            ejercicio: 2026,
            mes: Mes::try_new(10).expect("octubre"),
            resultado: ResultadoConsulta::ConDatos,
            paginacion_pendiente: false,
            clave_paginacion: None,
            registros: vec![consulta_registro_page(8)],
        });
        let tenant = root()
            .tenant(super::CertificateSource::Custom {
                signer: std::sync::Arc::new(FakeSigner::new()),
                transport: std::sync::Arc::clone(&transport) as _,
            })
            .obligado(obligado.clone())
            .installation("000042")
            .connect()
            .expect("the custom legs wire");
        let filtro = ConsultaFilter::new(2026, Mes::try_new(10).expect("octubre"));
        let answer = tenant
            .consulta(&filtro)
            .await
            .expect("the consulta answers");
        assert_eq!(answer.registros.len(), 2, "both pages merged");
        assert_eq!(answer.registros[0].num_serie_factura, "T00000007");
        assert_eq!(
            transport.sent_envelopes()[0]
                .matches("IndicadorRepresentante")
                .count(),
            0,
            "the tenant lens never sets the apoderado flag"
        );
        assert!(
            transport.sent_envelopes()[1].contains("ClavePaginacion"),
            "page 2 resumes from the handed key"
        );

        transport.push_consulta(ConsultaSpec {
            obligado_nombre: obligado.nombre_razon.clone(),
            obligado_nif: obligado.nif.clone(),
            apoderado: true,
            ejercicio: 2026,
            mes: Mes::try_new(10).expect("octubre"),
            resultado: ResultadoConsulta::ConDatos,
            paginacion_pendiente: false,
            clave_paginacion: None,
            registros: Vec::new(),
        });
        let vendor = root()
            .vendor(super::CertificateSource::Custom {
                signer: std::sync::Arc::new(FakeSigner::new()),
                transport: std::sync::Arc::clone(&transport) as _,
            })
            .representante(Obligado {
                nombre_razon: String::from("FACTURAS DE MANO SL"),
                nif: String::from("B87654321"),
            })
            .connect()
            .expect("vendor custody wires");
        vendor
            .consulta_for(&obligado, &filtro)
            .await
            .expect("the apoderado lens answers");
        let vendor_envelope = transport.sent_envelopes()[2].clone();
        assert!(
            vendor_envelope
                .contains("<sum1:IndicadorRepresentante>S</sum1:IndicadorRepresentante>"),
            "the vendor lens is the apoderado door"
        );
    }

    /// One huella-verifiable stored record — HS §6.1 vector 1's shape.
    fn consulta_registro_page(number: u64) -> crate::fiscal::consulta::RegistroSpec {
        crate::fiscal::consulta::RegistroSpec {
            id_emisor_factura: String::from("12345678Z"),
            num_serie_factura: format!("T{number:0>8}"),
            fecha_expedicion_factura: String::from("05-10-2026"),
            estado: super::EstadoAlmacenado::Correcto,
            timestamp_ultima_modificacion: String::from("2026-10-05T12:00:00+02:00"),
            codigo_error: None,
            descripcion_error: None,
            tipo_factura: Some(super::TipoFactura::F2),
            cuota_total: Some(super::Money::from_cents(210)),
            importe_total: Some(super::Money::from_cents(1210)),
            fecha_huso_gen: Some(String::from("2026-10-05T12:00:00+02:00")),
            huella_previa: None,
            huella: Some(crate::domain::chain::huella_alta_montos(
                "12345678Z",
                &format!("T{number:0>8}"),
                "05-10-2026",
                "F2",
                "2.10",
                "12.10",
                None,
                "2026-10-05T12:00:00+02:00",
            )),
        }
    }

    /// The vendor door's envío carries the Cabecera `Representante`
    /// (SI.xsd sequence: between `ObligadoEmision` and the remission
    /// block) — the block the apoderado read lens answers; a silent
    /// drop would be accepted by AEAT and only surface as `SinDatos`.
    #[tokio::test]
    async fn the_vendor_envio_carries_the_representante_block() {
        use super::engine::{FakeSigner, FakeVerifactuTransport, ScriptedOutcome};
        use super::lines;
        use super::{CertificateSource, InvoiceDraft, Modality, Obligado, Series};
        let transport = std::sync::Arc::new(FakeVerifactuTransport::new());
        transport.push_outcome(ScriptedOutcome::Correcto {
            csv: None,
            tiempo_espera_envio: 60,
        });
        let obligado = Obligado {
            nombre_razon: String::from("ANA RUIZ PEREZ"),
            nif: String::from("12345678Z"),
        };
        let vendor = root()
            .vendor(CertificateSource::Custom {
                signer: std::sync::Arc::new(FakeSigner::new()),
                transport: std::sync::Arc::clone(&transport) as _,
            })
            .representante(Obligado {
                nombre_razon: String::from("FACTURAS DE MANO SL"),
                nif: String::from("B87654321"),
            })
            .connect()
            .expect("vendor custody wires");
        vendor
            .emit_invoice_for(
                &obligado,
                "000042",
                Modality::Remission,
                InvoiceDraft::new()
                    .serie(Series::T, 9)
                    .descripcion("venta por delegación")
                    .line(lines::sujeta_iva(2100, 10_000)),
                None,
            )
            .await
            .expect("the vendor emission rides the injected transport");
        let envelope = &transport.sent_envelopes()[0];
        assert!(
            envelope.contains("<sum1:Representante><sum1:NombreRazon>FACTURAS DE MANO SL</sum1:NombreRazon><sum1:NIF>B87654321</sum1:NIF></sum1:Representante>"),
            "the Representante block rides the wire: {envelope}"
        );
    }

    /// The fake AEAT echoes the record's own `RefExterna` back — real
    /// AEAT does (RS.xsd), so a consumer's "did my external reference
    /// register?" demo reads the same hermetically as live.
    #[tokio::test]
    async fn the_fake_echoes_the_records_ref_externa() {
        use super::engine::{FakeSigner, FakeVerifactuTransport, ScriptedOutcome};
        use super::{lines, CertificateSource, InvoiceDraft, Series};
        let transport = std::sync::Arc::new(FakeVerifactuTransport::new());
        transport.push_outcome(ScriptedOutcome::Correcto {
            csv: Some(String::from("1111222233334444")),
            tiempo_espera_envio: 60,
        });
        let tenant = root()
            .tenant(CertificateSource::Custom {
                signer: std::sync::Arc::new(FakeSigner::new()),
                transport,
            })
            .obligado(super::Obligado {
                nombre_razon: String::from("ANA RUIZ PEREZ"),
                nif: String::from("12345678Z"),
            })
            .connect()
            .expect("hermetic legs");
        let emitted = tenant
            .emit_invoice(
                InvoiceDraft::new()
                    .serie(Series::T, 7)
                    .descripcion("venta mostrador")
                    .ref_externa("PEDIDO-2026-41")
                    .line(lines::sujeta_iva(2100, 10_000)),
                None,
            )
            .await
            .expect("the emission rides the injected transport");
        assert_eq!(
            emitted.emission.lineas[0].ref_externa.as_deref(),
            Some("PEDIDO-2026-41"),
            "the echo answers the record's own RefExterna back"
        );
    }

    /// The bring-your-own door: both custody models wire over injected
    /// legs with no HTTP stack — the emitted envelope travels the
    /// injected transport, and (remission law) the alta rides unsigned.
    /// Runs under BOTH feature configs: without `http` this is the only
    /// door.
    #[tokio::test]
    async fn the_custom_door_wires_both_custody_models_over_injected_legs() {
        use super::engine::{FakeVerifactuTransport, ScriptedOutcome};
        use super::{
            lines, response, CertificateSource, FiscalSigner, InvoiceDraft, Modality, Obligado,
            Series, VerifactuTransport,
        };
        let obligado = Obligado {
            nombre_razon: String::from("ANA RUIZ PEREZ"),
            nif: String::from("12345678Z"),
        };
        // The documented hermetic wiring: the ENGINE fakes implement
        // the FACADE's ports — no hand-rolled stub survives.
        let fake_signer = std::sync::Arc::new(super::engine::FakeSigner::new());
        let fake_transport = std::sync::Arc::new(FakeVerifactuTransport::new());
        fake_transport.push_outcome(ScriptedOutcome::Correcto {
            csv: Some(String::from("1111222233334444")),
            tiempo_espera_envio: 60,
        });
        let signer: std::sync::Arc<dyn FiscalSigner> = fake_signer.clone();
        let transport: std::sync::Arc<dyn VerifactuTransport> = fake_transport.clone();

        let tenant = root()
            .tenant(CertificateSource::Custom {
                signer: std::sync::Arc::clone(&signer),
                transport: std::sync::Arc::clone(&transport),
            })
            .obligado(obligado.clone())
            .installation("000042")
            .connect()
            .expect("the custom legs wire without any HTTP");
        let emitted = tenant
            .emit_invoice(
                InvoiceDraft::new()
                    .serie(Series::T, 7)
                    .descripcion("venta mostrador")
                    .line(lines::sujeta_iva(2100, 10_000)),
                None,
            )
            .await
            .expect("the emission rides the injected transport");
        assert!(matches!(
            emitted.emission.disposition,
            response::Disposition::Complete
        ));
        let sent = fake_transport.sent_envelopes();
        assert_eq!(
            sent.len(),
            1,
            "exactly one envío traveled the injected transport"
        );
        assert!(sent[0].contains("T00000007"));
        assert_eq!(
            fake_signer.signed_count(),
            0,
            "remission altas are unsigned"
        );

        let vendor = root()
            .vendor(CertificateSource::Custom {
                signer: std::sync::Arc::clone(&signer),
                transport: std::sync::Arc::clone(&transport),
            })
            .representante(Obligado {
                nombre_razon: String::from("FACTURAS DE MANO SL"),
                nif: String::from("B87654321"),
            })
            .connect()
            .expect("vendor custody wires over custom legs too");
        fake_transport.push_outcome(ScriptedOutcome::Correcto {
            csv: Some(String::from("2222333344445555")),
            tiempo_espera_envio: 60,
        });
        let vendored = vendor
            .emit_invoice_for(
                &obligado,
                "000042",
                Modality::Remission,
                InvoiceDraft::new()
                    .serie(Series::F, 1)
                    .tipo(super::TipoFactura::F1)
                    .descripcion("obra fontanería")
                    .destinatario(super::IDDestinatario {
                        nombre_razon: String::from("CLIENTE EJEMPLO SL"),
                        identificacion: super::IdentificacionDestinatario::Nif(String::from(
                            "B87654321",
                        )),
                    })
                    .line(lines::sujeta_iva(2100, 10_000)),
                None,
            )
            .await
            .expect("the shared injected transport mints per-tenant emitters");
        assert_eq!(vendored.num_serie, "F00000001");
        assert_eq!(
            vendored.emission.csv.as_deref(),
            Some("2222333344445555"),
            "the vendor emission rode the same injected pool"
        );
    }

    /// A hermetic tenant over the engine fakes with a frozen clock:
    /// the draft gates, the echo, and byte-identical instants.
    #[tokio::test]
    async fn the_draft_gates_and_the_sealed_echo() {
        use super::engine::Timestamp;
        use super::engine::{FakeClock, FakeSigner, FakeVerifactuTransport, ScriptedOutcome};
        use super::{lines, CertificateSource, InvoiceDraft, Series, TipoFactura};
        let transport = std::sync::Arc::new(FakeVerifactuTransport::new());
        transport.push_outcome(ScriptedOutcome::Correcto {
            csv: Some(String::from("1111222233334444")),
            tiempo_espera_envio: 60,
        });
        let clock = std::sync::Arc::new(FakeClock::new(Timestamp(1_738_589_400)));
        let tenant = Verifactu::builder()
            .sif(SifConfig::new("mi-facturacion", "MF", "1.0.0").expect("valid SIF"))
            .clock(clock)
            .build()
            .expect("pruebas root builds with no gate")
            .tenant(CertificateSource::Custom {
                signer: std::sync::Arc::new(FakeSigner::new()),
                transport,
            })
            .obligado(super::Obligado {
                nombre_razon: String::from("ANA RUIZ PEREZ"),
                nif: String::from("12345678Z"),
            })
            .connect()
            .expect("hermetic legs");

        // Custom series WITH a named tipo emits (the eager-match bug
        // used to reject this); the echo carries what was sealed.
        let emitted = tenant
            .emit_invoice(
                InvoiceDraft::new()
                    .serie(Series::Custom(String::from("2026E")), 1)
                    .tipo(TipoFactura::F1)
                    .descripcion("obra fontanería")
                    .destinatario(super::IDDestinatario {
                        nombre_razon: String::from("CLIENTE EJEMPLO SL"),
                        identificacion: super::IdentificacionDestinatario::Nif(String::from(
                            "B87654321",
                        )),
                    })
                    .line(lines::sujeta_iva(2100, 10_000)),
                None,
            )
            .await
            .expect("a named tipo unlocks a custom series");
        assert_eq!(emitted.num_serie, "2026E00000001");
        assert!(matches!(
            emitted.kind,
            super::ChainKind::Alta {
                tipo_factura: TipoFactura::F1,
                ..
            }
        ));
        assert_eq!(
            emitted.emission.lineas[0].num_serie_factura,
            "2026E00000001"
        );
        // The frozen clock reproduces AEAT's example instant.
        assert!(
            format!("{:?}", emitted.kind).contains("2025-02-03T14:30:00+01:00"),
            "the sealed instants come from the injected clock: {:?}",
            emitted.kind
        );

        // The cursor round-trips through JSON when `serde` is on.
        #[cfg(feature = "serde")]
        {
            use super::Predecessor;
            let json = serde_json::to_string(&emitted.cursor).expect("cursor serializes");
            let back: Predecessor = serde_json::from_str(&json).expect("cursor parses back");
            assert_eq!(back, emitted.cursor);
        }
    }

    /// Every draft gate fires as `InvalidRecord` (the `InvalidInput`
    /// class), naming the law — never a panic.
    #[test]
    fn the_draft_gates_fire_as_invalid_input() {
        use super::engine::{FakeSigner, FakeVerifactuTransport};
        use super::lines;
        use super::{CertificateSource, ErrorClass, InvoiceDraft, Obligado, Series};
        let tenant = root()
            .tenant(CertificateSource::Custom {
                signer: std::sync::Arc::new(FakeSigner::new()),
                transport: std::sync::Arc::new(FakeVerifactuTransport::new()),
            })
            .obligado(Obligado {
                nombre_razon: String::from("ANA RUIZ PEREZ"),
                nif: String::from("12345678Z"),
            })
            .connect()
            .expect("hermetic legs");
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("rt");
        let gate = |draft: InvoiceDraft| {
            let error = rt
                .block_on(tenant.emit_invoice(draft, None))
                .expect_err("the gate fires");
            assert_eq!(error.class(), ErrorClass::InvalidInput);
            error.to_string()
        };
        let cases = [
            (
                InvoiceDraft::new()
                    .serie(Series::Custom(String::from("2026E")), 2)
                    .descripcion("sin tipo")
                    .line(lines::sujeta_iva(2100, 1_000)),
                "a custom series carries no default TipoFactura",
            ),
            (
                InvoiceDraft::new()
                    .serie(Series::T, 100_000_000)
                    .descripcion("9-digit correlative")
                    .line(lines::sujeta_iva(2100, 1_000)),
                "99_999_999",
            ),
            (
                InvoiceDraft::new()
                    .serie(Series::T, 8)
                    .descripcion("")
                    .line(lines::sujeta_iva(2100, 1_000)),
                "description is required",
            ),
            (
                InvoiceDraft::new()
                    .serie(Series::parse(""), 9)
                    .descripcion("empty custom prefix"),
                "custom series prefix is required",
            ),
            (
                InvoiceDraft::new()
                    .serie(Series::parse(&"A".repeat(53)), 9)
                    .descripcion("NumSerieFactura renders 61 chars")
                    .line(lines::sujeta_iva(2100, 1_000)),
                "1..=60",
            ),
            (
                InvoiceDraft::new()
                    .serie(Series::parse("FÁ"), 9)
                    .descripcion("the printed QR admits ASCII only")
                    .line(lines::sujeta_iva(2100, 1_000)),
                "printable ASCII",
            ),
            (
                InvoiceDraft::new()
                    .serie(Series::parse(" F"), 9)
                    .descripcion("the huella would trim the prefix")
                    .line(lines::sujeta_iva(2100, 1_000)),
                "edge whitespace",
            ),
            (
                InvoiceDraft::new()
                    .serie(Series::F, 2)
                    .descripcion("F1 sin destinatario")
                    .line(lines::sujeta_iva(2100, 1_000)),
                "AEAT 1189",
            ),
        ];
        for (draft, law) in cases {
            let error = gate(draft);
            assert!(
                error.contains(law),
                "the gate names its law ({law}): {error}"
            );
        }
    }
}
