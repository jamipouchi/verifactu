//! The `Veri*FACTU` emission engine: the closed AEAT vocabularies, the
//! emission context, and [`VerifactuEmitter`] — serialize, sign per the
//! modality's policy, remit, classify.

pub mod consulta;
pub mod events;
pub mod huso;
pub mod response;
mod rules;
pub mod transport;
pub mod xml;

use std::sync::Arc;

use strum::IntoEnumIterator;

use crate::clock::{Clock, Timestamp};
use crate::domain::chain::{ChainKind, ChainRecord, FechaExpedicion};
use crate::domain::error::ErrorClass;
use crate::domain::money::Money;
use crate::signer::{FiscalSigner, SignerError};

use crate::fiscal::response::SoapFault;
use crate::fiscal::transport::{TransportFailure, TransportOutcome, VerifactuTransport};

/// The inverse of a vocabulary's `wire()`: the spec-owned spellings
/// live only in the `wire` arms, so a parse can never re-spell them.
fn parse_wire<E: IntoEnumIterator + Copy>(code: &str, wire: fn(E) -> &'static str) -> Option<E> {
    E::iter().find(|&variant| wire(variant) == code)
}

/// How the system runs. Remission is VERI*FACTU: every record goes to
/// AEAT as it is created ("facturas verificables"). Conservation is NO
/// VERI*FACTU: records are signed and kept locally, and remitted only
/// bajo requerimiento (SW description §6.6; Validaciones §4.3.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Modality {
    Remission,
    Conservation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SiNo {
    Si,
    No,
}

impl SiNo {
    #[must_use]
    pub fn wire(self) -> &'static str {
        match self {
            Self::Si => "S",
            Self::No => "N",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, strum::EnumIter)]
pub enum TipoRectificativa {
    Sustitutiva,
    Incremental,
}

impl TipoRectificativa {
    #[must_use]
    pub fn wire(self) -> &'static str {
        match self {
            Self::Sustitutiva => "S",
            Self::Incremental => "I",
        }
    }

    /// The inverse of [`Self::wire`] — `None` on an unknown code.
    #[must_use]
    pub fn parse(code: &str) -> Option<Self> {
        parse_wire(code, Self::wire)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, strum::EnumIter)]
pub enum Impuesto {
    Iva,
    Ipsi,
    Igic,
    Otros,
}

impl Impuesto {
    #[must_use]
    pub fn wire(self) -> &'static str {
        match self {
            Self::Iva => "01",
            Self::Ipsi => "02",
            Self::Igic => "03",
            Self::Otros => "05",
        }
    }

    /// The inverse of [`Self::wire`] — `None` on an unknown code.
    #[must_use]
    pub fn parse(code: &str) -> Option<Self> {
        parse_wire(code, Self::wire)
    }
}

/// The closed 18-code special-regime vocabulary (SI.xsd); the XSD
/// documents no per-code semantics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, strum::EnumIter)]
pub enum ClaveRegimen {
    C01,
    C02,
    C03,
    C04,
    C05,
    C06,
    C07,
    C08,
    C09,
    C10,
    C11,
    C14,
    C15,
    C17,
    C18,
    C19,
    C20,
    C21,
}

impl ClaveRegimen {
    /// The inverse of [`Self::wire`] — `None` on an unknown code.
    #[must_use]
    pub fn parse(code: &str) -> Option<Self> {
        parse_wire(code, Self::wire)
    }

    #[must_use]
    pub fn wire(self) -> &'static str {
        match self {
            Self::C01 => "01",
            Self::C02 => "02",
            Self::C03 => "03",
            Self::C04 => "04",
            Self::C05 => "05",
            Self::C06 => "06",
            Self::C07 => "07",
            Self::C08 => "08",
            Self::C09 => "09",
            Self::C10 => "10",
            Self::C11 => "11",
            Self::C14 => "14",
            Self::C15 => "15",
            Self::C17 => "17",
            Self::C18 => "18",
            Self::C19 => "19",
            Self::C20 => "20",
            Self::C21 => "21",
        }
    }
}

/// The `DetalleType` choice's sujeta arm — [`TipoImpositivo`] rides only
/// on these.
///
/// [`TipoImpositivo`]: crate::fiscal::DetalleDesglose::tipo_impositivo
#[derive(Debug, Clone, Copy, PartialEq, Eq, strum::EnumIter)]
pub enum CalificacionOperacion {
    SujetaNoExenta,
    SujetaNoExentaInversion,
    NoSujetaOtras,
    NoSujetaLocalizacion,
}

impl CalificacionOperacion {
    /// The inverse of [`Self::wire`] — `None` on an unknown code.
    #[must_use]
    pub fn parse(code: &str) -> Option<Self> {
        parse_wire(code, Self::wire)
    }

    #[must_use]
    pub fn wire(self) -> &'static str {
        match self {
            Self::SujetaNoExenta => "S1",
            Self::SujetaNoExentaInversion => "S2",
            Self::NoSujetaOtras => "N1",
            Self::NoSujetaLocalizacion => "N2",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, strum::EnumIter)]
pub enum OperacionExenta {
    E1,
    E2,
    E3,
    E4,
    E5,
    E6,
    E7,
    E8,
}

impl OperacionExenta {
    /// The inverse of [`Self::wire`] — `None` on an unknown code.
    #[must_use]
    pub fn parse(code: &str) -> Option<Self> {
        parse_wire(code, Self::wire)
    }

    #[must_use]
    pub fn wire(self) -> &'static str {
        match self {
            Self::E1 => "E1",
            Self::E2 => "E2",
            Self::E3 => "E3",
            Self::E4 => "E4",
            Self::E5 => "E5",
            Self::E6 => "E6",
            Self::E7 => "E7",
            Self::E8 => "E8",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, strum::EnumIter)]
pub enum RechazoPrevio {
    Rechazado,
    NoRechazado,
    /// `X` — the record does not exist at AEAT (e.g. migrating in from a
    /// non-Veri*FACTU SIF).
    NoRemitido,
}

impl RechazoPrevio {
    #[must_use]
    pub fn wire(self) -> &'static str {
        match self {
            Self::Rechazado => "S",
            Self::NoRechazado => "N",
            Self::NoRemitido => "X",
        }
    }
}

/// A named fiscal identity: the `Cabecera`'s `ObligadoEmision` (and
/// the records' emisor), or its `Representante` — the same
/// `PersonaFisicaJuridicaESType` shape.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Obligado {
    pub nombre_razon: String,
    pub nif: String,
}

/// The `SistemaInformatico` block: `nombre_razon`/`nif` name the
/// PRODUCER of the invoicing software (AEAT Validaciones §3.1.5: "la
/// persona o entidad productora del sistema informático"), not the
/// obligado.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SistemaInformaticoConfig {
    pub nombre_razon: String,
    pub nif: String,
    pub nombre_sistema_informatico: String,
    pub id_sistema_informatico: String,
    pub version: String,
    pub numero_instalacion: String,
    /// What the software can run as (`TipoUsoPosibleSoloVerifactu`).
    pub uso_posible: UsoPosible,
}

/// What the invoicing software can run as — the producer's declaration,
/// `TipoUsoPosibleSoloVerifactu` on every record.
#[derive(Clone, Copy, Debug, PartialEq, Eq, strum::EnumIter)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub enum UsoPosible {
    /// Only VERI*FACTU (remission): `S`.
    SoloVerifactu,
    /// Also NO VERI*FACTU (conservation, remitting bajo requerimiento):
    /// `N`. A conservation emitter demands it.
    TambienNoVerifactu,
}

impl UsoPosible {
    #[must_use]
    pub fn wire(self) -> &'static str {
        match self {
            Self::SoloVerifactu => "S",
            Self::TambienNoVerifactu => "N",
        }
    }

    /// The inverse of [`Self::wire`] — `None` on an unknown code.
    #[must_use]
    pub fn parse(code: &str) -> Option<Self> {
        parse_wire(code, Self::wire)
    }
}

impl SistemaInformaticoConfig {
    /// The block's law, checkable at configuration time: SI.xsd's
    /// simple types plus AEAT Validaciones §3.1.5 (a non-empty
    /// `NombreSistemaInformatico`, a two-position `IdSistemaInformatico`).
    ///
    /// # Errors
    /// [`EmitError::InvalidRecord`] naming the field.
    pub fn validate(&self) -> Result<(), EmitError> {
        xml::check_nif("SistemaInformatico/NIF", &self.nif)?;
        xml::check_text("SistemaInformatico/NombreRazon", &self.nombre_razon, 120)?;
        if self.nombre_sistema_informatico.trim().is_empty() {
            return Err(EmitError::InvalidRecord {
                detail: String::from(
                    "SistemaInformatico/NombreSistemaInformatico is required (AEAT Validaciones \
                     §3.1.5)",
                ),
            });
        }
        xml::check_text(
            "SistemaInformatico/NombreSistemaInformatico",
            &self.nombre_sistema_informatico,
            30,
        )?;
        xml::check_id_sistema_informatico(
            "SistemaInformatico/IdSistemaInformatico",
            &self.id_sistema_informatico,
        )?;
        xml::check_text("SistemaInformatico/Version", &self.version, 50)?;
        xml::check_text(
            "SistemaInformatico/NumeroInstalacion",
            &self.numero_instalacion,
            100,
        )?;
        Ok(())
    }
}

/// `FinRequerimiento=S` marks the last batch of the requerimiento this
/// envío answers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Requerimiento {
    pub referencia: String,
    pub fin: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FacturaId {
    pub nif: String,
    pub num_serie: String,
    pub fecha_expedicion: FechaExpedicion,
}

/// Whether a foreign `id` is a member state's NIF-IVA as AEAT checks it
/// (Validaciones §3.1.3.13, Nota 1): the uppercase state prefix, then
/// that state's number. Such a destinatario rides [`IdOtroType::NifIva`]
/// and needs no `CodigoPais`; any other foreign identifier names its
/// country. `fecha` is the operation's date, which decides Northern
/// Ireland's `GB` or `XI`: the record's `FechaOperacion`, else its
/// `FechaExpedicionFactura` (AEAT 1254, 1255).
#[must_use]
pub fn is_nif_iva(id: &str, fecha: &FechaExpedicion) -> bool {
    xml::is_nif_iva(id, fecha.ymd())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, strum::EnumIter)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub enum IdOtroType {
    NifIva,
    Pasaporte,
    IdPaisResidencia,
    CertificadoResidencia,
    OtroDocumentoProbatorio,
    NoCensado,
}

impl IdOtroType {
    /// The inverse of [`Self::wire`] — `None` on an unknown code.
    #[must_use]
    pub fn parse(code: &str) -> Option<Self> {
        parse_wire(code, Self::wire)
    }

    #[must_use]
    pub const fn wire(self) -> &'static str {
        match self {
            Self::NifIva => "02",
            Self::Pasaporte => "03",
            Self::IdPaisResidencia => "04",
            Self::CertificadoResidencia => "05",
            Self::OtroDocumentoProbatorio => "06",
            Self::NoCensado => "07",
        }
    }
}

/// `CodigoPais=ES` with `01-NIFContraparte` is forbidden by the XSD —
/// Spanish counterparts ride [`IdentificacionDestinatario::Nif`].
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct IdOtro {
    pub codigo_pais: Option<String>,
    pub id_type: IdOtroType,
    pub id: String,
}

/// SI.xsd's `NIF | IDOtro` choice (`PersonaFisicaJuridicaType`): a
/// destinatario's identification, and the producer's in a stored
/// [`SistemaInformatico`](consulta::SistemaInformaticoAlmacenado)
/// block, whose type repeats the same choice.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub enum IdentificacionDestinatario {
    Nif(String),
    Otro(IdOtro),
}

/// REQUIRED on `F1`/`F3`/`R1`–`R4` altas (AEAT validation 1189,
/// live-confirmed 2026-10-05 at pruebas), optional on `F2`/`R5`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IDDestinatario {
    pub nombre_razon: String,
    pub identificacion: IdentificacionDestinatario,
}

/// Required on [`TipoRectificativa::Sustitutiva`], forbidden otherwise
/// (AEAT Validaciones §3.6).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImporteRectificacion {
    pub base_rectificada: Money,
    pub cuota_rectificada: Money,
    pub cuota_recargo_rectificado: Option<Money>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DetalleDesglose {
    pub impuesto: Option<Impuesto>,
    pub clave_regimen: Option<ClaveRegimen>,
    /// The `DetalleType` choice: exactly one of this / `operacion_exenta`.
    pub calificacion: Option<CalificacionOperacion>,
    pub operacion_exenta: Option<OperacionExenta>,
    pub tipo_impositivo: Option<Money>,
    pub base_imponible: Money,
    pub cuota_repercutida: Option<Money>,
    pub tipo_recargo_equivalencia: Option<Money>,
    pub cuota_recargo_equivalencia: Option<Money>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct EmissionContext<'a> {
    pub ref_externa: Option<&'a str>,
    pub descripcion_operacion: Option<&'a str>,
    pub desglose: &'a [DetalleDesglose],
    pub tipo_rectificativa: Option<TipoRectificativa>,
    pub facturas_rectificadas: &'a [FacturaId],
    /// Required on `Sustitutiva`, forbidden otherwise (Validaciones §3.6).
    pub importe_rectificacion: Option<ImporteRectificacion>,
    /// The operation's date (`FechaOperacion`), when it differs from the
    /// factura's expedition: a sale invoiced or rectified on a later day.
    /// The dated rules read it, else `FechaExpedicionFactura`: the IVA
    /// rates and their recargo (Validaciones §15.1, §15.3) and the UK
    /// NIF-IVA prefix (Nota 1). It postdates the expedition only under
    /// `ClaveRegimen` 14 or 15 (§3.1.3.1), and 14 requires it (§15.6.9).
    pub fecha_operacion: Option<FechaExpedicion>,
    /// REQUIRED on `F1`/`F3`/`R1`–`R4` (AEAT 1189), optional on `F2`/`R5`.
    pub destinatarios: &'a [IDDestinatario],
    /// `S` marks a subsanación: a NEW record over the same `IDFactura` —
    /// originals stay immutable.
    pub subsanacion: Option<SiNo>,
    pub rechazo_previo: Option<RechazoPrevio>,
    pub sin_registro_previo: Option<SiNo>,
    pub macrodato: Option<SiNo>,
    pub incidencia: bool,
    pub requerimiento: Option<Requerimiento>,
    /// The event's `MotivoAnomalia` — NOT a hashed field.
    pub motivo_evento: Option<&'a str>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct Emission {
    /// Signed per the modality policy — remission-modality altas travel
    /// unsigned (art. 3 RD 1007/2023). Engine evidence, never product
    /// output: skipped in JSON shapes.
    #[cfg_attr(feature = "serde", serde(skip))]
    pub signed_records: Vec<String>,
    /// AEAT's CSV (16 chars) — store durably at alta time; not
    /// re-fetchable later.
    pub csv: Option<String>,
    /// Who presented the envío and when AEAT took it, answered with the
    /// CSV — store it beside the CSV.
    pub datos_presentacion: Option<response::DatosPresentacion>,
    /// AEAT's mandated wait (seconds) before the next envío: the caller
    /// paces its remissions on it — this crate never sleeps on its behalf.
    /// `None` when AEAT set no new wait (the one the caller keeps
    /// carries), or when the emission never left the site.
    pub tiempo_espera_envio: Option<u64>,
    /// The envío as a whole, as AEAT answered it (`None` when it never
    /// left the site); [`Self::disposition`] folds it with the lines.
    pub estado_envio: Option<response::EstadoEnvio>,
    pub lineas: Vec<response::RespuestaLinea>,
    pub disposition: response::Disposition,
}

impl Emission {
    /// An emission that never left the site — conservation without a
    /// requerimiento, or event records (no public event remission
    /// operation exists).
    #[must_use]
    pub(crate) fn kept_local(signed_records: Vec<String>) -> Self {
        Self {
            signed_records,
            csv: None,
            datos_presentacion: None,
            tiempo_espera_envio: None,
            estado_envio: None,
            lineas: Vec::new(),
            disposition: response::Disposition::Complete,
        }
    }
}

/// Regulatory outcomes are NOT errors — they ride
/// [`Emission::disposition`] and are never auto-retried.
#[derive(Debug, thiserror::Error)]
pub enum EmitError {
    /// Fail-closed: emission is impossible, never a silent no-op.
    #[error("fiscal emitter has no transport configured")]
    NotConfigured,
    /// The exchange yielded no AEAT answer to read; the cause sets the
    /// class.
    #[error("fiscal transport failure: {0}")]
    Transport(TransportFailure),
    #[error("fiscal AEAT server fault: {faultstring}")]
    FaultServer { faultstring: String },
    #[error("fiscal AEAT client fault: {faultstring}")]
    FaultClient { faultstring: String },
    #[error("fiscal signing failure: {0}")]
    Signing(SignerError),
    #[error("fiscal invalid record/context: {detail}")]
    InvalidRecord { detail: String },
}

impl EmitError {
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::NotConfigured => "fiscal.not-configured",
            Self::Transport(failure) => match failure {
                TransportFailure::Wire { .. } | TransportFailure::Unreadable { .. } => {
                    "fiscal.transport"
                }
                TransportFailure::Refused { .. } => "fiscal.transport.refused",
                TransportFailure::StandIn { .. } => "fiscal.transport.stand-in",
            },
            Self::FaultServer { .. } => "fiscal.fault.server",
            Self::FaultClient { .. } => "fiscal.fault.client",
            Self::Signing(signer_error) => match signer_error {
                SignerError::CertificateUnavailable => "fiscal.signing.unavailable",
                SignerError::SigningFailed(_) => "fiscal.signing.failed",
            },
            Self::InvalidRecord { .. } => "fiscal.invalid-record",
        }
    }

    #[must_use]
    pub fn class(&self) -> ErrorClass {
        match self {
            Self::Transport(
                TransportFailure::Wire { .. } | TransportFailure::Unreadable { .. },
            )
            | Self::FaultServer { .. } => ErrorClass::Transient,
            // A refused certificate or a stand-in's answer never heals on
            // retry: the operator fixes the certificate or the endpoint.
            Self::Transport(
                TransportFailure::Refused { .. } | TransportFailure::StandIn { .. },
            )
            | Self::FaultClient { .. }
            | Self::Signing(_) => ErrorClass::OperatorAction,
            Self::InvalidRecord { .. } => ErrorClass::InvalidInput,
            Self::NotConfigured => ErrorClass::Bug,
        }
    }
}

impl From<xml::SerializeError> for EmitError {
    fn from(error: xml::SerializeError) -> Self {
        Self::InvalidRecord {
            detail: error.to_string(),
        }
    }
}

impl From<SignerError> for EmitError {
    fn from(error: SignerError) -> Self {
        Self::Signing(error)
    }
}

/// The cause rides along: [`EmitError::class`] retries a wire failure
/// or an unreadable answer and hands a refusal or a stand-in's answer
/// to the operator.
impl From<TransportFailure> for EmitError {
    fn from(failure: TransportFailure) -> Self {
        Self::Transport(failure)
    }
}

impl From<SoapFault> for EmitError {
    /// `env:Server` retries; every other fault code is the operator's.
    fn from(fault: SoapFault) -> Self {
        match response::classify_fault(&fault) {
            ErrorClass::Transient => Self::FaultServer {
                faultstring: fault.faultstring,
            },
            _ => Self::FaultClient {
                faultstring: fault.faultstring,
            },
        }
    }
}

/// No transport is legal (conservation keeps records local) but fails
/// closed on any path that must remit.
pub struct VerifactuEmitter {
    modality: Modality,
    obligado: Obligado,
    representante: Option<Obligado>,
    sif: SistemaInformaticoConfig,
    signer: Arc<dyn FiscalSigner>,
    clock: Arc<dyn Clock>,
    transport: Option<Arc<dyn VerifactuTransport>>,
}

impl std::fmt::Debug for VerifactuEmitter {
    /// Redacted: signer internals are never printed.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VerifactuEmitter")
            .field("modality", &self.modality)
            .field("obligado", &self.obligado.nif)
            .field("transport", &self.transport.is_some())
            .finish_non_exhaustive()
    }
}

impl VerifactuEmitter {
    /// The modality's signing policy (FS §2): conservation signs EVERY
    /// record; remission signs ONLY events (art. 3 RD 1007/2023).
    fn must_sign(modality: Modality, kind: &ChainKind) -> bool {
        matches!(kind, ChainKind::Evento { .. }) || modality == Modality::Conservation
    }

    /// The emitter's obligado — the one source of the records' issuer
    /// and the consulta cabecera's identity.
    #[must_use]
    pub fn obligado(&self) -> &Obligado {
        &self.obligado
    }

    /// `clock` is the emission instant source — always the caller's
    /// (see [`crate::Verifactu::builder`]).
    ///
    /// # Errors
    /// [`EmitError::InvalidRecord`] when the obligado/SIF identity
    /// violates its SI.xsd simple types.
    pub fn new(
        modality: Modality,
        obligado: Obligado,
        sif: SistemaInformaticoConfig,
        signer: Arc<dyn FiscalSigner>,
        clock: Arc<dyn Clock>,
    ) -> Result<Self, EmitError> {
        xml::check_nif("ObligadoEmision/NIF", &obligado.nif)?;
        xml::check_text("ObligadoEmision/NombreRazon", &obligado.nombre_razon, 120)?;
        sif.validate()?;
        if modality == Modality::Conservation && sif.uso_posible == UsoPosible::SoloVerifactu {
            return Err(EmitError::InvalidRecord {
                detail: String::from(
                    "conservation is NO VERI*FACTU, but the SIF declares SoloVerifactu \
                     (TipoUsoPosibleSoloVerifactu=S) — declare UsoPosible::TambienNoVerifactu",
                ),
            });
        }
        Ok(Self {
            modality,
            obligado,
            representante: None,
            sif,
            signer,
            clock,
            transport: None,
        })
    }

    #[must_use]
    pub fn with_transport(mut self, transport: Arc<dyn VerifactuTransport>) -> Self {
        self.transport = Some(transport);
        self
    }

    /// The Cabecera's `Representante`: who remits on the obligado's
    /// behalf (per AEAT, only when a representante/asesor generated the
    /// records).
    ///
    /// # Errors
    /// [`EmitError::InvalidRecord`] when the representante violates its
    /// SI.xsd simple types.
    pub fn with_representante(mut self, representante: Obligado) -> Result<Self, EmitError> {
        xml::check_nif("Representante/NIF", &representante.nif)?;
        xml::check_text(
            "Representante/NombreRazon",
            &representante.nombre_razon,
            120,
        )?;
        self.representante = Some(representante);
        Ok(self)
    }

    /// The emission instant source — the flat face's single read point
    /// (wasm builds have no `SystemTime`; they inject a runtime clock).
    #[must_use]
    pub fn now_utc(&self) -> Timestamp {
        self.clock.now_utc()
    }

    fn transport(&self) -> Result<&Arc<dyn VerifactuTransport>, EmitError> {
        self.transport.as_ref().ok_or(EmitError::NotConfigured)
    }

    /// Serializes, signs per the modality's policy, and — when the
    /// modality remits — sends one envío of 1..=1000 records.
    ///
    /// # Errors
    /// [`EmitError`] for the failing pipeline stage.
    pub async fn submit(
        &self,
        pairs: &[(ChainRecord, EmissionContext<'_>)],
    ) -> Result<Emission, EmitError> {
        if pairs.is_empty() || pairs.len() > 1000 {
            return Err(EmitError::InvalidRecord {
                detail: format!(
                    "one envío carries 1..1000 records (SW §6), got {}",
                    pairs.len()
                ),
            });
        }
        for (record, _) in pairs {
            Self::huella_holds(record)?;
        }
        if let [(record, context)] = pairs {
            if let ChainKind::Evento { .. } = &record.kind {
                return self.emit_evento(record, context);
            }
        }
        if pairs
            .iter()
            .any(|(record, _)| matches!(record.kind, ChainKind::Evento { .. }))
        {
            return Err(EmitError::InvalidRecord {
                detail: String::from(
                    "evento records emit alone — they are never remitted, so they never batch",
                ),
            });
        }
        let requerimiento = Self::requerimiento_of(pairs)?;

        let signed_records = self.serialize_and_sign(pairs)?;

        let remision = match self.modality {
            Modality::Conservation => {
                let Some(requerimiento) = requerimiento else {
                    return Ok(Emission::kept_local(signed_records));
                };
                xml::CabeceraRemision::Requerimiento {
                    referencia: &requerimiento.referencia,
                    fin: requerimiento.fin,
                }
            }
            Modality::Remission => xml::CabeceraRemision::Voluntaria {
                incidencia: pairs.iter().any(|(_, context)| context.incidencia),
            },
        };

        let transport = self.transport()?;
        let body = xml::reg_factu_document(
            &self.obligado,
            self.representante.as_ref(),
            &remision,
            &signed_records,
        )?;
        match transport.send(&xml::soap_envelope(&body)).await? {
            TransportOutcome::Fault(fault) => Err(fault.into()),
            TransportOutcome::Consulta(_) => Err(TransportFailure::Unreadable {
                detail: String::from("AEAT answered a consulta respuesta to a submission"),
            }
            .into()),
            TransportOutcome::Response(respuesta) => {
                let disposition = response::classify_response(&respuesta);
                Ok(Emission {
                    signed_records,
                    csv: respuesta.csv,
                    datos_presentacion: respuesta.datos_presentacion,
                    tiempo_espera_envio: respuesta.tiempo_espera_envio,
                    estado_envio: Some(respuesta.estado_envio),
                    lineas: respuesta.lineas,
                    disposition,
                })
            }
        }
    }

    /// Refuses `record` exactly as [`Self::submit`] would refuse it in an
    /// envío — every gate of this emitter's modality, AEAT's business
    /// rules under remission — without signing or sending anything: the
    /// check a ledger owner runs on a sealed record before committing it
    /// to its chain, where an original is immutable. The envío's own
    /// rules (1..1000 records, one requerimiento, eventos alone) stay
    /// [`Self::submit`]'s.
    ///
    /// # Errors
    /// [`EmitError::InvalidRecord`] naming the field and the law it
    /// breaks: AEAT's error code or the section of its rules.
    pub fn check(
        &self,
        record: &ChainRecord,
        context: &EmissionContext<'_>,
    ) -> Result<(), EmitError> {
        Self::huella_holds(record)?;
        if let ChainKind::Evento { .. } = &record.kind {
            Self::evento_node(record, context)?;
            return Ok(());
        }
        if let Some(requerimiento) = &context.requerimiento {
            Self::check_referencia(&requerimiento.referencia)?;
        }
        self.unsigned_node(record, context)?;
        Ok(())
    }

    /// The consulta read-back over this emitter's obligado (the tenant
    /// door): every page merged, [`crate::fiscal::consulta`] for the
    /// raw shapes.
    ///
    /// # Errors
    /// [`EmitError::NotConfigured`] without a transport; the rest of
    /// [`EmitError`] per the consulta pipeline.
    pub async fn consulta(
        &self,
        filtro: &consulta::ConsultaFilter<'_>,
    ) -> Result<consulta::ConsultaAnswer, EmitError> {
        consulta::send_all(self.transport()?, &self.obligado, false, filtro).await
    }

    /// The raw single-page door (the engine face): the request names
    /// the obligado and the lens — `ConsultaRequest::apoderado` is the
    /// caller's explicit choice here, never a default.
    ///
    /// # Errors
    /// [`EmitError::NotConfigured`] without a transport; the rest of
    /// [`EmitError`] per the consulta pipeline.
    pub async fn consulta_with(
        &self,
        request: &consulta::ConsultaRequest<'_>,
    ) -> Result<consulta::ConsultaAnswer, EmitError> {
        consulta::send_once(self.transport()?, request).await
    }

    /// The one requerimiento the envío answers, if any.
    ///
    /// # Errors
    /// [`EmitError::InvalidRecord`] when a reference is not 1..18
    /// alphanumeric (AEAT 4133), or one envío carries two different
    /// requerimientos.
    fn requerimiento_of<'p>(
        pairs: &'p [(ChainRecord, EmissionContext<'_>)],
    ) -> Result<Option<&'p Requerimiento>, EmitError> {
        let mut found: Option<&Requerimiento> = None;
        for requerimiento in pairs
            .iter()
            .filter_map(|(_, ctx)| ctx.requerimiento.as_ref())
        {
            Self::check_referencia(&requerimiento.referencia)?;
            if found.is_some_and(|first| first != requerimiento) {
                return Err(EmitError::InvalidRecord {
                    detail: String::from(
                        "one envío answers one requerimiento — all contexts must carry the same \
                         RefRequerimiento/FinRequerimiento",
                    ),
                });
            }
            found = Some(requerimiento);
        }
        Ok(found)
    }

    /// AEAT 4133 (live-confirmed 2026-10-05: a hyphen was rejected
    /// `env:Client` — an application-level rule beyond the XSD).
    fn check_referencia(referencia: &str) -> Result<(), EmitError> {
        if referencia.is_empty()
            || referencia.len() > 18
            || !referencia.chars().all(|c| c.is_ascii_alphanumeric())
        {
            return Err(EmitError::InvalidRecord {
                detail: String::from(
                    "RefRequerimiento must be 1..18 ASCII alphanumerics (TextMax18, AEAT 4133)",
                ),
            });
        }
        Ok(())
    }

    fn huella_holds(record: &ChainRecord) -> Result<(), EmitError> {
        if record.huella_recomputes() {
            Ok(())
        } else {
            Err(EmitError::InvalidRecord {
                detail: String::from("stored huella does not recompute over the cadena"),
            })
        }
    }

    fn emit_evento(
        &self,
        record: &ChainRecord,
        context: &EmissionContext<'_>,
    ) -> Result<Emission, EmitError> {
        let node = Self::evento_node(record, context)?;
        let signed = self.signer.sign_xades_epes(&node)?;
        Ok(Emission::kept_local(vec![xml::registro_evento(&signed)]))
    }

    /// An evento's unsigned node, through every gate an evento meets.
    fn evento_node(
        record: &ChainRecord,
        context: &EmissionContext<'_>,
    ) -> Result<String, EmitError> {
        let ChainKind::Evento { event, .. } = &record.kind else {
            unreachable!("only evento records are dispatched here");
        };
        if context.requerimiento.is_some() {
            return Err(EmitError::InvalidRecord {
                detail: String::from(
                    "evento records are never remitted — a requerimiento cannot apply",
                ),
            });
        }
        if !events::is_known_tipo_evento(&event.tipo_evento) {
            return Err(EmitError::InvalidRecord {
                detail: String::from(
                    "unknown TipoEvento — the vocabulary is closed at the emit boundary (a \
                     typo must never enter the event chain)",
                ),
            });
        }
        Ok(xml::evento_node(record, context)?)
    }

    fn serialize_and_sign(
        &self,
        pairs: &[(ChainRecord, EmissionContext<'_>)],
    ) -> Result<Vec<String>, EmitError> {
        let mut signed_records = Vec::with_capacity(pairs.len());
        for (record, context) in pairs {
            let unsigned = self.unsigned_node(record, context)?;
            let signed = if Self::must_sign(self.modality, &record.kind) {
                self.signer.sign_xades_epes(&unsigned)?
            } else {
                unsigned
            };
            signed_records.push(signed);
        }
        Ok(signed_records)
    }

    /// An alta's or anulación's unsigned node, through every gate of this
    /// emitter's modality.
    fn unsigned_node(
        &self,
        record: &ChainRecord,
        context: &EmissionContext<'_>,
    ) -> Result<String, EmitError> {
        if self.modality == Modality::Remission && context.requerimiento.is_some() {
            return Err(EmitError::InvalidRecord {
                detail: String::from(
                    "remission modality remits voluntarily — a requerimiento never mixes in",
                ),
            });
        }
        if self.modality == Modality::Conservation && context.incidencia {
            return Err(EmitError::InvalidRecord {
                detail: String::from(
                    "Incidencia belongs to RemisionVoluntaria; conservation remits bajo \
                     requerimiento only",
                ),
            });
        }
        Ok(xml::record_node(
            record,
            context,
            &self.obligado,
            &self.sif,
            self.modality,
        )?)
    }
}

#[cfg(test)]
pub(crate) mod tests_support {
    use crate::domain::chain::{ChainKind, ChainRecord, TipoFactura};
    use crate::domain::money::Money;
    use crate::domain::series::Series;

    use crate::fiscal::huso;
    use crate::fiscal::{DetalleDesglose, Obligado, SistemaInformaticoConfig};

    /// AEAT's example instant: Madrid `2025-02-03T14:30:00+01:00`.
    pub const EMISSION_INSTANT: u64 = 1_738_589_400;

    pub fn iva_super() -> DetalleDesglose {
        DetalleDesglose {
            impuesto: Some(super::Impuesto::Iva),
            clave_regimen: Some(super::ClaveRegimen::C01),
            calificacion: Some(super::CalificacionOperacion::SujetaNoExenta),
            operacion_exenta: None,
            tipo_impositivo: Some(Money::from_cents(2100)),
            base_imponible: Money::from_cents(1000),
            cuota_repercutida: Some(Money::from_cents(210)),
            tipo_recargo_equivalencia: None,
            cuota_recargo_equivalencia: None,
        }
    }

    pub fn obligado() -> Obligado {
        Obligado {
            nombre_razon: String::from("Empresa Ejemplo"),
            nif: String::from("B12345678"),
        }
    }

    pub fn sif() -> SistemaInformaticoConfig {
        SistemaInformaticoConfig {
            nombre_razon: String::from("Empresa Ejemplo"),
            nif: String::from("B12345678"),
            nombre_sistema_informatico: String::from("test-erp"),
            id_sistema_informatico: String::from("01"),
            version: String::from("0.1.0"),
            numero_instalacion: String::from("1"),
            uso_posible: super::UsoPosible::SoloVerifactu,
        }
    }

    pub fn primer_alta_record() -> ChainRecord {
        let instants =
            huso::render_instants(crate::clock::zone::SiteZone::EuropeMadrid, EMISSION_INSTANT);
        ChainRecord::seal(
            ChainKind::Alta {
                issuer: String::from("B12345678"),
                serie: Series::T,
                number: 42,
                fecha_expedicion: instants.fecha_expedicion,
                tipo_factura: TipoFactura::F2,
                cuota_total: Money::from_cents(210),
                importe_total: Money::from_cents(1210),
                fecha_huso_gen: instants.fecha_huso_gen,
            },
            EMISSION_INSTANT,
            None,
        )
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::tests_support::{iva_super, obligado, primer_alta_record, sif, EMISSION_INSTANT};
    use super::{DetalleDesglose, EmissionContext, Modality, Obligado, VerifactuEmitter};
    use crate::clock::{FakeClock, Timestamp};
    use crate::signer::FakeSigner;

    fn emitter(modality: Modality) -> VerifactuEmitter {
        VerifactuEmitter::new(
            modality,
            obligado(),
            sif(),
            Arc::new(FakeSigner::new()),
            Arc::new(FakeClock::new(Timestamp(EMISSION_INSTANT))),
        )
        .expect("test config is valid")
    }

    fn alta_ctx() -> EmissionContext<'static> {
        let desglose: &'static [DetalleDesglose] = Box::leak(Box::new([iva_super()]));
        EmissionContext {
            descripcion_operacion: Some("Venta mostrador"),
            desglose,
            ..EmissionContext::default()
        }
    }

    /// AEAT's own signed example: our rendering reproduces its instants
    /// and its huella (verbatim `41.4` — HS v0.1.2 §3 permits 1- and
    /// 2-decimal renderings).
    #[test]
    fn fiscal_rendering_feeds_the_aeat_fourth_anchor_bytes() {
        let instants = super::huso::render_instants(
            crate::clock::zone::SiteZone::EuropeMadrid,
            EMISSION_INSTANT,
        );
        assert_eq!(instants.fecha_expedicion.as_str(), "03-02-2025");
        assert_eq!(
            instants.fecha_huso_gen.as_str(),
            "2025-02-03T14:30:00+01:00"
        );
        let input = format!(
            "IDEmisorFactura=89890001K&NumSerieFactura=12345678-G66&\
             FechaExpedicionFactura={}&TipoFactura=R3&CuotaTotal=41.4&ImporteTotal=241.4&\
             Huella=C9AF4AF1EF5EBBA700350DE3EEF12C2D355C56AC56F13DB2A25E0031BD2B7ED5&\
             FechaHoraHusoGenRegistro={}",
            instants.fecha_expedicion, instants.fecha_huso_gen
        );
        assert_eq!(
            crate::domain::chain::huella(&input),
            "FF954378B64ED331A9B2366AD317D86E9DEC1716B12DD0ACCB172A6DC4C105AA"
        );
    }

    /// `Representante` renders between `ObligadoEmision` and the
    /// remission block (SI.xsd order).
    #[test]
    fn representante_is_gated_and_renders_in_the_xsd_order() {
        let error = emitter(Modality::Remission)
            .with_representante(Obligado {
                nombre_razon: String::from("ASESOR EJEMPLO SL"),
                nif: String::from("B8765432"),
            })
            .expect_err("NIFType is 9 chars");
        assert_eq!(error.code(), "fiscal.invalid-record");

        let (obligado, sif) = (
            super::tests_support::obligado(),
            super::tests_support::sif(),
        );
        let node = super::xml::record_node(
            &primer_alta_record(),
            &alta_ctx(),
            &obligado,
            &sif,
            Modality::Remission,
        )
        .expect("serializes");
        let doc = super::xml::reg_factu_document(
            &obligado,
            Some(&Obligado {
                nombre_razon: String::from("ASESOR EJEMPLO SL"),
                nif: String::from("B87654321"),
            }),
            &super::xml::CabeceraRemision::Voluntaria { incidencia: false },
            &[node],
        )
        .expect("wraps");
        let obligado_at = doc.find("</sum1:ObligadoEmision>").expect("obligado");
        let block = "<sum1:Representante><sum1:NombreRazon>ASESOR EJEMPLO SL\
                     </sum1:NombreRazon><sum1:NIF>B87654321</sum1:NIF></sum1:Representante>";
        let repr_at = doc.find(block).expect("the block renders verbatim");
        let remision_at = doc.find("<sum1:RemisionVoluntaria").expect("remision");
        assert!(
            obligado_at < repr_at && repr_at < remision_at,
            "the SI.xsd order: {doc}"
        );
    }

    /// `error` names `names` (a field, a code) and leaves `value` out of
    /// its text.
    fn names_never_echoes(error: &impl std::fmt::Display, names: &str, value: &str) {
        let text = error.to_string();
        assert!(text.contains(names), "names {names}: {text}");
        assert!(!text.contains(value), "echoes {value:?}: {text}");
    }

    fn fecha() -> crate::domain::chain::FechaExpedicion {
        crate::domain::chain::FechaExpedicion::parse("03-02-2025").expect("dd-mm-yyyy")
    }

    fn huso() -> crate::domain::chain::FechaHuso {
        crate::domain::chain::FechaHuso::parse("2025-02-03T14:30:00+01:00").expect("ISO-8601")
    }

    /// Through conservation's gates: identification binds every regime,
    /// and no business rule runs ahead of it.
    fn refused(
        record: &crate::domain::chain::ChainRecord,
        ctx: &EmissionContext<'_>,
        obligado: &Obligado,
    ) -> super::xml::SerializeError {
        super::xml::record_node(record, ctx, obligado, &sif(), Modality::Conservation)
            .expect_err("refused")
    }

    /// A refusal names the field and the rule, never the party: at every
    /// identifier of an alta the refused NIF, `IDOtro` or name stays out of
    /// the text a consumer logs.
    #[test]
    fn an_alta_refusal_names_the_field_never_the_party() {
        use super::{FacturaId, IDDestinatario, IdOtro, IdOtroType, IdentificacionDestinatario};
        use crate::domain::chain::{ChainKind, PrevRef};
        use crate::domain::series::Series;

        let issued_by = |nif: &str| {
            let mut record = primer_alta_record();
            if let ChainKind::Alta { issuer, .. } = &mut record.kind {
                *issuer = nif.to_owned();
            }
            record
        };
        let ctx = alta_ctx();
        let error = refused(&issued_by("1234567Z"), &ctx, &obligado());
        names_never_echoes(&error, "IDFactura/IDEmisorFactura", "1234567Z");
        let otro_obligado = Obligado {
            nif: String::from("B-2345678"),
            ..obligado()
        };
        let error = refused(&primer_alta_record(), &ctx, &otro_obligado);
        names_never_echoes(&error, "Cabecera/ObligadoEmision/NIF", "B-2345678");
        // AEAT 1108 names neither NIF.
        let error = refused(&issued_by("A87654321"), &ctx, &obligado());
        names_never_echoes(&error, "AEAT 1108", "A87654321");
        names_never_echoes(&error, "IDFactura/IDEmisorFactura", "B12345678");

        let mut chained = primer_alta_record();
        chained.prev = Some(PrevRef {
            issuer: String::from("B 2345678"),
            serie: Series::T,
            number: 41,
            fecha_expedicion: fecha(),
            huella: "0".repeat(64),
        });
        let error = refused(&chained, &ctx, &obligado());
        names_never_echoes(&error, "RegistroAnterior/IDEmisorFactura", "B 2345678");

        let rectificadas = [FacturaId {
            nif: String::from("B9876543"),
            num_serie: String::from("T00000041"),
            fecha_expedicion: fecha(),
        }];
        let rectificativa = EmissionContext {
            facturas_rectificadas: &rectificadas,
            ..alta_ctx()
        };
        let error = refused(&primer_alta_record(), &rectificativa, &obligado());
        names_never_echoes(&error, "IDFacturaRectificada/IDEmisorFactura", "B9876543");

        let destinatario = |nombre_razon: &str, identificacion| {
            let destinatarios = [IDDestinatario {
                nombre_razon: nombre_razon.to_owned(),
                identificacion,
            }];
            let ctx = EmissionContext {
                destinatarios: &destinatarios,
                ..alta_ctx()
            };
            refused(&primer_alta_record(), &ctx, &obligado())
        };
        let nif = |nif: &str| IdentificacionDestinatario::Nif(nif.to_owned());
        let otro = |id_type, pais: &str, id: &str| {
            IdentificacionDestinatario::Otro(IdOtro {
                codigo_pais: Some(pais.to_owned()),
                id_type,
                id: id.to_owned(),
            })
        };
        let error = destinatario("CLIENTE", nif("12345678-Z"));
        names_never_echoes(&error, "IDDestinatario/NIF", "12345678-Z");
        let error = destinatario("CLIENTE", otro(IdOtroType::NifIva, "DE", "DE12345678"));
        names_never_echoes(&error, "IDDestinatario/IDOtro/ID", "DE12345678");
        let error = destinatario("CLIENTE", otro(IdOtroType::NoCensado, "ES", "12345678A"));
        names_never_echoes(&error, "IDDestinatario/IDOtro/ID", "12345678A");
        names_never_echoes(&error, "AEAT 1131", "12345678A");
        let error = destinatario("CLIENTE", otro(IdOtroType::Pasaporte, "ZZ", "12AB34567"));
        names_never_echoes(&error, "IDDestinatario/IDOtro/CodigoPais", "ZZ");
        names_never_echoes(&error, "CountryType2", "12AB34567");
        let error = destinatario(&"ANA RUIZ PEREZ ".repeat(9), nif("12345678Z"));
        names_never_echoes(&error, "IDDestinatario/NombreRazon", "ANA RUIZ");
    }

    /// The anulación's and the evento's identifiers, and the evento's
    /// free-text `TipoEvento`, under the same law.
    #[test]
    fn an_anulacion_or_evento_refusal_names_the_field_never_the_party() {
        use super::events::{evento_data, evento_restore, EventoSif};
        use super::xml::evento_node;
        use crate::domain::chain::{ChainKind, ChainRecord};
        use crate::domain::series::Series;

        let anulacion = |issuer: &str| ChainRecord {
            kind: ChainKind::Anulacion {
                issuer: issuer.to_owned(),
                serie: Series::T,
                number: 42,
                fecha_expedicion: fecha(),
                fecha_huso_gen: huso(),
            },
            fecha: EMISSION_INSTANT,
            prev: None,
            prev_evento_huella: None,
            huella: String::new(),
        };
        let ctx = alta_ctx();
        let error = refused(&anulacion("1234567Z"), &ctx, &obligado());
        names_never_echoes(&error, "IDFactura/IDEmisorFacturaAnulada", "1234567Z");
        // §3.1.4.1 names neither NIF.
        let error = refused(&anulacion("A87654321"), &ctx, &obligado());
        names_never_echoes(&error, "§3.1.4.1", "A87654321");
        names_never_echoes(&error, "IDFactura/IDEmisorFacturaAnulada", "B12345678");

        let producer = |nif: &str| EventoSif {
            nif: nif.to_owned(),
            id: String::new(),
            id_sistema_informatico: String::from("01"),
            version: String::from("0.1.0"),
            numero_instalacion: String::from("1"),
        };
        let evento = |event| {
            ChainRecord::seal(
                ChainKind::Evento {
                    event,
                    fecha_huso_gen_evento: huso(),
                },
                EMISSION_INSTANT,
                None,
            )
        };
        let none = EmissionContext::default();
        let record = evento(evento_restore(&producer("B9876543"), "B12345678"));
        let error = evento_node(&record, &none).expect_err("refused");
        names_never_echoes(&error, "Evento/SistemaInformatico/NIF", "B9876543");
        let record = evento(evento_restore(&producer("B12345678"), "Q-1234567"));
        let error = evento_node(&record, &none).expect_err("refused");
        names_never_echoes(&error, "Evento/ObligadoEmision/NIF", "Q-1234567");
        let typo = evento(evento_data(
            &producer("B12345678"),
            "B12345678",
            "RESTAURACION_BACKUP",
        ));
        let error = emitter(Modality::Remission)
            .check(&typo, &none)
            .expect_err("refused");
        names_never_echoes(&error, "TipoEvento", "RESTAURACION_BACKUP");
    }

    /// The doors that take an identity or a reference before any record:
    /// the producer, the obligado, the representante, the requerimiento,
    /// the consulta.
    #[test]
    fn a_door_refusal_names_the_field_never_the_party() {
        use super::consulta::{
            consulta_document, ClavePaginacion, ConsultaFilter, ConsultaRequest,
        };
        use super::{Requerimiento, SistemaInformaticoConfig};

        let producer = SistemaInformaticoConfig {
            nif: String::from("B1234567Q9"),
            ..sif()
        };
        let error = producer.validate().expect_err("refused");
        names_never_echoes(&error, "SistemaInformatico/NIF", "B1234567Q9");
        let id_sistema = SistemaInformaticoConfig {
            id_sistema_informatico: String::from("ñ1"),
            ..sif()
        };
        let error = id_sistema.validate().expect_err("refused");
        names_never_echoes(&error, "SistemaInformatico/IdSistemaInformatico", "ñ1");

        let otro_obligado = Obligado {
            nif: String::from("B-2345678"),
            ..obligado()
        };
        let Err(error) = VerifactuEmitter::new(
            Modality::Remission,
            otro_obligado.clone(),
            sif(),
            Arc::new(FakeSigner::new()),
            Arc::new(FakeClock::new(Timestamp(EMISSION_INSTANT))),
        ) else {
            panic!("the obligado's NIF is refused");
        };
        names_never_echoes(&error, "ObligadoEmision/NIF", "B-2345678");
        let Err(error) = emitter(Modality::Remission).with_representante(Obligado {
            nombre_razon: String::from("ASESOR EJEMPLO SL"),
            nif: String::from("B8765432"),
        }) else {
            panic!("the representante's NIF is refused");
        };
        names_never_echoes(&error, "Representante/NIF", "B8765432");
        let requerido = EmissionContext {
            requerimiento: Some(Requerimiento {
                referencia: String::from("REQ-2026-001"),
                fin: true,
            }),
            ..alta_ctx()
        };
        let error = emitter(Modality::Remission)
            .check(&primer_alta_record(), &requerido)
            .expect_err("refused");
        names_never_echoes(&error, "AEAT 4133", "REQ-2026-001");

        let filtro = ConsultaFilter::new(2026, super::consulta::Mes::ENERO);
        let consulta =
            |filtro: &ConsultaFilter<'_>, obligado: &Obligado, clave: Option<&ClavePaginacion>| {
                consulta_document(&ConsultaRequest {
                    obligado,
                    apoderado: false,
                    filtro,
                    clave_paginacion: clave,
                })
                .expect_err("refused")
            };
        let error = consulta(&filtro, &otro_obligado, None);
        names_never_echoes(&error, "Consulta/ObligadoEmision/NIF", "B-2345678");
        let clave = ClavePaginacion {
            id_emisor_factura: String::from("B 2345678"),
            num_serie_factura: String::from("T00000041"),
            fecha_expedicion_factura: String::from("03-02-2025"),
        };
        let error = consulta(&filtro, &obligado(), Some(&clave));
        names_never_echoes(
            &error,
            "Consulta/ClavePaginacion/IDEmisorFactura",
            "B 2345678",
        );
        // NumSerieFactura is the caller's free text: its edge-whitespace
        // refusal leaves it out too.
        let error = consulta(&filtro.num_serie(" T00000041"), &obligado(), None);
        names_never_echoes(&error, "Consulta/NumSerieFactura", "T00000041");
        names_never_echoes(&error, "edge whitespace", "T00000041");
    }
}
