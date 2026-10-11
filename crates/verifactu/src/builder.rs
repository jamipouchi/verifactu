//! The facade's root: where emissions go and the production gate, the
//! SIF identity, the certificate doors, and the endpoint table.

use std::sync::Arc;
#[cfg(feature = "http")]
use std::time::Duration;

#[cfg(feature = "http")]
use crate::aeat::p12::ClientIdentity;
#[cfg(feature = "http")]
use crate::aeat::HttpVerifactuTransport;
use crate::clock::{Clock, SystemClock};
use crate::custody::{TenantConnecting, VendorConnecting};
use crate::fiscal::transport::VerifactuTransport;
use crate::fiscal::{Modality, Obligado, SistemaInformaticoConfig};
use crate::signer::FiscalSigner;

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
    productor: Obligado,
    nombre_sistema_informatico: String,
    id_sistema_informatico: String,
    version: String,
}

impl SifConfig {
    /// `productor` is the person or entity that produces the invoicing
    /// software — the same on every tenant's records — then the system's
    /// name, two-position id and version.
    ///
    /// # Errors
    /// [`VerifactuError::Identity`] naming the field, per
    /// [`SistemaInformaticoConfig::validate`].
    pub fn new(
        productor: Obligado,
        nombre: &str,
        id: &str,
        version: &str,
    ) -> Result<Self, VerifactuError> {
        let sif = Self {
            productor,
            nombre_sistema_informatico: nombre.to_owned(),
            id_sistema_informatico: id.to_owned(),
            version: version.to_owned(),
        };
        if version.is_empty() {
            return Err(VerifactuError::Identity(String::from(
                "SistemaInformatico/Version is required",
            )));
        }
        sif.for_installation("1")
            .validate()
            .map_err(|error| VerifactuError::Identity(error.to_string()))?;
        Ok(sif)
    }

    /// The `SistemaInformatico` block one installation declares.
    pub(crate) fn for_installation(&self, installation: &str) -> SistemaInformaticoConfig {
        SistemaInformaticoConfig {
            nombre_razon: self.productor.nombre_razon.clone(),
            nif: self.productor.nif.clone(),
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
    /// clock). Tests inject `engine::FakeClock` (feature `fakes`) for byte-identical
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
    pub(crate) sif: SifConfig,
    #[cfg(feature = "http")]
    timeout: Duration,
    pub(crate) clock: Arc<dyn Clock>,
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
pub(crate) struct CertLegs {
    pub(crate) signer: Arc<dyn FiscalSigner>,
    pub(crate) wire: WireLeg,
}

pub(crate) enum WireLeg {
    /// The provided HTTP transport, built per modality endpoint over
    /// this mTLS identity.
    #[cfg(feature = "http")]
    Identity(ClientIdentity),
    /// Injected by the consumer.
    Transport(Arc<dyn VerifactuTransport>),
}

impl CertLegs {
    pub(crate) fn of(
        clock: &Arc<dyn Clock>,
        cert: CertificateSource<'_>,
    ) -> Result<Self, VerifactuError> {
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
    pub(crate) fn transport_of(
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
        TenantConnecting::new(self, cert)
    }

    /// Custody (b): OUR one certificate + the Cabecera
    /// `Representante`. ONE pooled transport serves every tenant; the
    /// per-tenant emitters are cached and cheap.
    #[must_use]
    pub fn vendor<'a>(&self, cert: CertificateSource<'a>) -> VendorConnecting<'a, '_> {
        VendorConnecting::new(self, cert)
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
