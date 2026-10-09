//! The `Veri*FACTU` emission engine. Called post-commit from the durable
//! outbox — no external call ever rides inside the sale's transaction.

pub mod consulta;
pub mod events;
pub mod huso;
pub mod response;
pub mod signer;
pub mod transport;
pub mod xml;

use std::sync::Arc;

use crate::clock::{Clock, SystemClock, Timestamp};
use crate::domain::chain::{ChainKind, ChainRecord, FechaExpedicion};
use crate::domain::error::ErrorClass;
use crate::signer::SignerError;

use crate::fiscal::signer::RecordSigner;
use crate::fiscal::transport::{TransportOutcome, VerifactuTransport};

/// Conservation exists for offline correctness: on WAN-down, records are
/// created locally and remission queues in the outbox.
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
        match code {
            "S" => Some(Self::Sustitutiva),
            "I" => Some(Self::Incremental),
            _ => None,
        }
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
        match code {
            "01" => Some(Self::Iva),
            "02" => Some(Self::Ipsi),
            "03" => Some(Self::Igic),
            "05" => Some(Self::Otros),
            _ => None,
        }
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
    /// The inverse of [`Self::wire`] over the closed 18-code set —
    /// `None` on an unknown code.
    #[must_use]
    pub fn parse(code: &str) -> Option<Self> {
        match code {
            "01" => Some(Self::C01),
            "02" => Some(Self::C02),
            "03" => Some(Self::C03),
            "04" => Some(Self::C04),
            "05" => Some(Self::C05),
            "06" => Some(Self::C06),
            "07" => Some(Self::C07),
            "08" => Some(Self::C08),
            "09" => Some(Self::C09),
            "10" => Some(Self::C10),
            "11" => Some(Self::C11),
            "14" => Some(Self::C14),
            "15" => Some(Self::C15),
            "17" => Some(Self::C17),
            "18" => Some(Self::C18),
            "19" => Some(Self::C19),
            "20" => Some(Self::C20),
            "21" => Some(Self::C21),
            _ => None,
        }
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
        match code {
            "S1" => Some(Self::SujetaNoExenta),
            "S2" => Some(Self::SujetaNoExentaInversion),
            "N1" => Some(Self::NoSujetaOtras),
            "N2" => Some(Self::NoSujetaLocalizacion),
            _ => None,
        }
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
        match code {
            "E1" => Some(Self::E1),
            "E2" => Some(Self::E2),
            "E3" => Some(Self::E3),
            "E4" => Some(Self::E4),
            "E5" => Some(Self::E5),
            "E6" => Some(Self::E6),
            "E7" => Some(Self::E7),
            "E8" => Some(Self::E8),
            _ => None,
        }
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

/// One identity for both the `Cabecera`'s `ObligadoEmision` and the
/// records' emisor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Obligado {
    pub nombre_razon: String,
    pub nif: String,
}

/// The `Cabecera`'s optional `Representante`: per AEAT, only when the
/// remitted records were generated by a representante/asesor of the
/// obligado.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Representante {
    pub nombre_razon: String,
    pub nif: String,
}

/// Only the variable identity members: the fixed-declaration members are
/// constants at the wire renderer (`xml::sistema_informatico`) — a
/// boolean whose only legal value is a constant is law wearing a
/// datatype.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SistemaInformaticoConfig {
    pub nombre_razon: String,
    pub nif: String,
    pub nombre_sistema_informatico: String,
    pub id_sistema_informatico: String,
    pub version: String,
    pub numero_instalacion: String,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, strum::EnumIter)]
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
        match code {
            "02" => Some(Self::NifIva),
            "03" => Some(Self::Pasaporte),
            "04" => Some(Self::IdPaisResidencia),
            "05" => Some(Self::CertificadoResidencia),
            "06" => Some(Self::OtroDocumentoProbatorio),
            "07" => Some(Self::NoCensado),
            _ => None,
        }
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
pub struct IdOtro {
    pub codigo_pais: Option<String>,
    pub id_type: IdOtroType,
    pub id: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
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
    pub base_rectificada: crate::domain::money::Money,
    pub cuota_rectificada: crate::domain::money::Money,
    pub cuota_recargo_rectificado: Option<crate::domain::money::Money>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DetalleDesglose {
    pub impuesto: Option<Impuesto>,
    pub clave_regimen: Option<ClaveRegimen>,
    /// The `DetalleType` choice: exactly one of this / `operacion_exenta`.
    pub calificacion: Option<CalificacionOperacion>,
    pub operacion_exenta: Option<OperacionExenta>,
    pub tipo_impositivo: Option<crate::domain::money::Money>,
    pub base_imponible: crate::domain::money::Money,
    pub cuota_repercutida: Option<crate::domain::money::Money>,
    pub tipo_recargo_equivalencia: Option<crate::domain::money::Money>,
    pub cuota_recargo_equivalencia: Option<crate::domain::money::Money>,
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
    /// AEAT's mandated backpressure (seconds). NOT yet wired: production
    /// drain pacing MUST gate on this value.
    pub tiempo_espera_envio: u64,
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
            tiempo_espera_envio: 0,
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
    #[error("fiscal transport failure: {detail}")]
    Transport { detail: String },
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
            Self::Transport { .. } => "fiscal.transport",
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
            Self::Transport { .. } | Self::FaultServer { .. } => ErrorClass::Transient,
            Self::FaultClient { .. } | Self::Signing(_) => ErrorClass::OperatorAction,
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

/// No transport is legal (conservation keeps records local) but fails
/// closed on any path that must remit.
pub struct VerifactuEmitter {
    modality: Modality,
    obligado: Obligado,
    representante: Option<Representante>,
    sif: SistemaInformaticoConfig,
    signer: Arc<dyn RecordSigner>,
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
    #[must_use]
    pub(crate) fn must_sign(modality: Modality, kind: &ChainKind) -> bool {
        matches!(kind, ChainKind::Evento { .. }) || modality == Modality::Conservation
    }

    /// One source: the emitter's construction data, never a
    /// caller-supplied copy that could disagree.
    #[must_use]
    pub fn obligado_nif(&self) -> &str {
        &self.obligado.nif
    }

    /// The emitter's obligado whole — the consulta cabecera's identity.
    #[must_use]
    pub fn obligado(&self) -> &Obligado {
        &self.obligado
    }

    fn check_config(obligado: &Obligado, sif: &SistemaInformaticoConfig) -> Result<(), EmitError> {
        let invalid = |detail: String| EmitError::InvalidRecord { detail };
        xml::check_nif("Obligado/NIF", &obligado.nif)
            .map_err(|error| invalid(error.to_string()))?;
        xml::check_text("Obligado/NombreRazon", &obligado.nombre_razon, 120)
            .map_err(|error| invalid(error.to_string()))?;
        xml::check_nif("SIF/NIF", &sif.nif).map_err(|error| invalid(error.to_string()))?;
        for (field, value, max) in [
            ("SIF/NombreRazon", &sif.nombre_razon, 120usize),
            (
                "SIF/NombreSistemaInformatico",
                &sif.nombre_sistema_informatico,
                30,
            ),
            ("SIF/IdSistemaInformatico", &sif.id_sistema_informatico, 2),
            ("SIF/Version", &sif.version, 50),
            ("SIF/NumeroInstalacion", &sif.numero_instalacion, 100),
        ] {
            xml::check_text(field, value, max).map_err(|error| invalid(error.to_string()))?;
        }
        Ok(())
    }

    /// # Errors
    /// [`EmitError::InvalidRecord`] when the obligado/SIF identity
    /// violates its SI.xsd simple types.
    pub fn new(
        modality: Modality,
        obligado: Obligado,
        sif: SistemaInformaticoConfig,
        signer: Arc<dyn RecordSigner>,
    ) -> Result<Self, EmitError> {
        Self::check_config(&obligado, &sif)?;
        Ok(Self {
            modality,
            obligado,
            representante: None,
            sif,
            signer,
            clock: Arc::new(SystemClock),
            transport: None,
        })
    }

    #[must_use]
    pub fn with_transport(mut self, transport: Arc<dyn VerifactuTransport>) -> Self {
        self.transport = Some(transport);
        self
    }

    /// The clock emissions read (default: the system wall clock) —
    /// [`crate::VerifactuBuilder::clock`] threads through here; direct
    /// engine consumers inject their own.
    #[must_use]
    pub fn with_clock(mut self, clock: Arc<dyn Clock>) -> Self {
        self.clock = clock;
        self
    }

    /// The emission instant source — the flat face's single read point
    /// (wasm builds have no `SystemTime`; they inject a runtime clock).
    #[must_use]
    pub fn now_utc(&self) -> Timestamp {
        self.clock.now_utc()
    }

    /// # Errors
    /// [`EmitError::InvalidRecord`] when the representante violates its
    /// SI.xsd simple types.
    pub fn with_representante(mut self, representante: Representante) -> Result<Self, EmitError> {
        let invalid = |detail: String| EmitError::InvalidRecord { detail };
        xml::check_nif("Representante/NIF", &representante.nif)
            .map_err(|error| invalid(error.to_string()))?;
        xml::check_text(
            "Representante/NombreRazon",
            &representante.nombre_razon,
            120,
        )
        .map_err(|error| invalid(error.to_string()))?;
        self.representante = Some(representante);
        Ok(self)
    }
}

impl VerifactuEmitter {
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
        if pairs.len() > 1
            && pairs
                .iter()
                .any(|(record, _)| matches!(record.kind, ChainKind::Evento { .. }))
        {
            return Err(EmitError::InvalidRecord {
                detail: String::from(
                    "evento records emit alone — they are never remitted, so they never batch",
                ),
            });
        }
        let requerimientos: Vec<&Requerimiento> = pairs
            .iter()
            .filter_map(|(_, context)| context.requerimiento.as_ref())
            .collect();
        Self::check_requerimientos(&requerimientos)?;

        if let [(record, context)] = pairs {
            if let ChainKind::Evento { .. } = &record.kind {
                return self.emit_evento(record, context);
            }
        }

        let signed_records = self.serialize_and_sign(pairs)?;

        let remision = match self.modality {
            Modality::Conservation => {
                let Some(requerimiento) = requerimientos.first() else {
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

        let Some(transport) = self.transport.as_ref() else {
            return Err(EmitError::NotConfigured);
        };
        let body = xml::reg_factu_document_full(
            &self.obligado,
            self.representante.as_ref(),
            &remision,
            &signed_records,
        )?;
        let envelope = xml::soap_envelope(&body);
        match transport.send(&envelope).await {
            Err(failure) => Err(EmitError::Transport {
                detail: failure.detail,
            }),
            Ok(TransportOutcome::Fault(fault)) => Err(match response::classify_fault(&fault) {
                ErrorClass::Transient => EmitError::FaultServer {
                    faultstring: fault.faultstring,
                },
                _ => EmitError::FaultClient {
                    faultstring: fault.faultstring,
                },
            }),
            Ok(TransportOutcome::Consulta(_)) => Err(EmitError::Transport {
                detail: String::from("AEAT answered a consulta respuesta to a submission"),
            }),
            Ok(TransportOutcome::Response(respuesta)) => {
                let disposition = response::classify_response(&respuesta);
                Ok(Emission {
                    signed_records,
                    csv: respuesta.csv,
                    tiempo_espera_envio: respuesta.tiempo_espera_envio,
                    lineas: respuesta.lineas,
                    disposition,
                })
            }
        }
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
        let Some(transport) = self.transport.as_ref() else {
            return Err(EmitError::NotConfigured);
        };
        consulta::send_all(transport, &self.obligado, false, filtro).await
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
        let Some(transport) = self.transport.as_ref() else {
            return Err(EmitError::NotConfigured);
        };
        consulta::send_once(transport, request).await
    }

    /// # Errors
    /// [`EmitError::InvalidRecord`] when a reference is not 1..18
    /// alphanumeric (AEAT validation 4133, live-confirmed 2026-10-05: a
    /// hyphen reached pruebas and was rejected `env:Client` — an
    /// application-level rule beyond the XSD), or one envío carries two
    /// different requerimientos.
    fn check_requerimientos(requerimientos: &[&Requerimiento]) -> Result<(), EmitError> {
        for requerimiento in requerimientos {
            if requerimiento.referencia.is_empty() || requerimiento.referencia.chars().count() > 18
            {
                return Err(EmitError::InvalidRecord {
                    detail: format!(
                        "RefRequerimiento must be 1..18 chars (TextMax18), got {:?}",
                        requerimiento.referencia
                    ),
                });
            }
            if !requerimiento
                .referencia
                .chars()
                .all(|c| c.is_ascii_alphanumeric())
            {
                return Err(EmitError::InvalidRecord {
                    detail: format!(
                        "RefRequerimiento must be alphanumeric (AEAT 4133), got {:?}",
                        requerimiento.referencia
                    ),
                });
            }
        }
        if requerimientos.len() > 1 && requerimientos.windows(2).any(|pair| pair[0] != pair[1]) {
            return Err(EmitError::InvalidRecord {
                detail: String::from(
                    "one envío answers one requerimiento — all contexts must carry the same \
                     RefRequerimiento/FinRequerimiento",
                ),
            });
        }
        Ok(())
    }

    fn emit_evento(
        &self,
        record: &ChainRecord,
        context: &EmissionContext<'_>,
    ) -> Result<Emission, EmitError> {
        let ChainKind::Evento { event, .. } = &record.kind else {
            return Err(EmitError::InvalidRecord {
                detail: String::from("record kind is not Evento"),
            });
        };
        if context.requerimiento.is_some() {
            return Err(EmitError::InvalidRecord {
                detail: String::from(
                    "evento records are never remitted — a requerimiento cannot apply",
                ),
            });
        }
        if !crate::fiscal::events::is_known_tipo_evento(&event.tipo_evento) {
            return Err(EmitError::InvalidRecord {
                detail: format!(
                    "unknown TipoEvento {:?} — the vocabulary is law at the emit boundary \
                     (L1E recovery; a typo must never enter the event chain)",
                    event.tipo_evento
                ),
            });
        }
        if record.huella != crate::domain::chain::huella(&record.cadena()) {
            return Err(EmitError::InvalidRecord {
                detail: String::from("stored huella does not recompute over the cadena"),
            });
        }
        let node = xml::evento_node(record, context)?;
        let signed = self.signer.sign_record(&node)?;
        Ok(Emission::kept_local(vec![xml::registro_evento(&signed)]))
    }

    fn serialize_and_sign(
        &self,
        pairs: &[(ChainRecord, EmissionContext<'_>)],
    ) -> Result<Vec<String>, EmitError> {
        let mut signed_records = Vec::with_capacity(pairs.len());
        for (record, context) in pairs {
            if record.huella != crate::domain::chain::huella(&record.cadena()) {
                return Err(EmitError::InvalidRecord {
                    detail: String::from("stored huella does not recompute over the cadena"),
                });
            }
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
            let unsigned = xml::record_node(record, context, &self.obligado, &self.sif)?;
            let signed = if Self::must_sign(self.modality, &record.kind) {
                self.signer.sign_record(&unsigned)?
            } else {
                unsigned
            };
            signed_records.push(signed);
        }
        Ok(signed_records)
    }
}

#[cfg(test)]
pub(crate) mod tests_support {
    use crate::domain::chain::{ChainKind, ChainRecord, FechaExpedicion, FechaHuso, TipoFactura};
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
            id_sistema_informatico: String::from("1"),
            version: String::from("0.1.0"),
            numero_instalacion: String::from("1"),
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
                fecha_expedicion: FechaExpedicion::parse(&instants.fecha_expedicion)
                    .expect("huso renders the dd-mm-yyyy grammar"),
                tipo_factura: TipoFactura::F2,
                cuota_total: Money::from_cents(210),
                importe_total: Money::from_cents(1210),
                fecha_huso_gen: FechaHuso::parse(&instants.fecha_huso_gen)
                    .expect("huso renders the ISO-8601 grammar"),
            },
            EMISSION_INSTANT,
            None,
        )
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::signer::FakeSigner;
    use super::tests_support::{iva_super, obligado, primer_alta_record, sif, EMISSION_INSTANT};
    use super::{DetalleDesglose, EmissionContext, Modality, VerifactuEmitter};

    fn emitter(modality: Modality) -> VerifactuEmitter {
        VerifactuEmitter::new(modality, obligado(), sif(), Arc::new(FakeSigner::new()))
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
        assert_eq!(instants.fecha_expedicion, "03-02-2025");
        assert_eq!(instants.fecha_huso_gen, "2025-02-03T14:30:00+01:00");
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
            .with_representante(super::Representante {
                nombre_razon: String::from("ASESOR EJEMPLO SL"),
                nif: String::from("B8765432"),
            })
            .expect_err("NIFType is 9 chars");
        assert_eq!(error.code(), "fiscal.invalid-record");

        let (obligado, sif) = (
            super::tests_support::obligado(),
            super::tests_support::sif(),
        );
        let node = super::xml::record_node(&primer_alta_record(), &alta_ctx(), &obligado, &sif)
            .expect("serializes");
        let doc = super::xml::reg_factu_document_full(
            &obligado,
            Some(&super::Representante {
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

    #[allow(dead_code)]
    fn _seam() {
        let _ = super::transport::FakeVerifactuTransport::new();
    }
}
