//! The `RespuestaSuministro` read path and the response classifier.
//! INTERPRETATION: a duplicate whose stored estado is `Anulada` does NOT
//! drain — its standing at AEAT is ambiguous and a human decides.

use crate::domain::error::ErrorClass;
use strum::IntoEnumIterator;

use crate::fiscal::xml::{parse_tree, Tree};

#[derive(Clone, Copy, Debug, PartialEq, Eq, strum::EnumIter)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub enum EstadoEnvio {
    Correcto,
    ParcialmenteCorrecto,
    Incorrecto,
}

impl EstadoEnvio {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Correcto => "Correcto",
            Self::ParcialmenteCorrecto => "ParcialmenteCorrecto",
            Self::Incorrecto => "Incorrecto",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, strum::EnumIter)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub enum EstadoRegistro {
    Correcto,
    AceptadoConErrores,
    Incorrecto,
}

impl EstadoRegistro {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Correcto => "Correcto",
            Self::AceptadoConErrores => "AceptadoConErrores",
            Self::Incorrecto => "Incorrecto",
        }
    }
}

/// The stored estado of a duplicate registration at AEAT.
#[derive(Clone, Copy, Debug, PartialEq, Eq, strum::EnumIter)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub enum EstadoDuplicado {
    Correcta,
    AceptadaConErrores,
    Anulada,
}

impl EstadoDuplicado {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Correcta => "Correcta",
            Self::AceptadaConErrores => "AceptadaConErrores",
            Self::Anulada => "Anulada",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, strum::EnumIter)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub enum TipoOperacion {
    Alta,
    Anulacion,
}

impl TipoOperacion {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Alta => "Alta",
            Self::Anulacion => "Anulacion",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct RegistroDuplicado {
    pub id_peticion: String,
    pub estado_duplicado: EstadoDuplicado,
    pub codigo_error: Option<i64>,
    pub descripcion_error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct RespuestaLinea {
    pub nif: String,
    pub num_serie_factura: String,
    pub fecha_expedicion_factura: String,
    pub tipo_operacion: TipoOperacion,
    pub ref_externa: Option<String>,
    pub estado_registro: EstadoRegistro,
    pub codigo_error_registro: Option<i64>,
    pub descripcion_error_registro: Option<String>,
    pub registro_duplicado: Option<RegistroDuplicado>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RespuestaSuministro {
    /// Present only when the envío was not rejected — not re-fetchable
    /// later.
    pub csv: Option<String>,
    /// AEAT's mandated wait (seconds) before the next envío — distinct
    /// from any retry backoff of the caller's own.
    pub tiempo_espera_envio: u64,
    pub estado_envio: EstadoEnvio,
    pub lineas: Vec<RespuestaLinea>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SoapFault {
    pub code: String,
    pub faultstring: String,
}

impl SoapFault {
    #[must_use]
    pub fn local_code(&self) -> &str {
        self.code.rsplit(':').next().unwrap_or(&self.code)
    }

    #[must_use]
    pub fn is_server(&self) -> bool {
        self.local_code() == "Server"
    }
}

/// One law for the read edge's decoders: the spec-owned bytes live only
/// in the `as_str` arms; parsing is the reverse lookup, so the read edge
/// can never re-spell the vocabulary.
fn vocabulary<E: IntoEnumIterator + Copy>(
    element: &str,
    text: &str,
    as_str: fn(E) -> &'static str,
) -> Result<E, String> {
    E::iter()
        .find(|&variant| as_str(variant) == text)
        .ok_or_else(|| format!("unknown {element} {text:?}"))
}

/// A non-numeric code is propagated, never degraded to `None` — the
/// classifier must never answer on a shape AEAT never sent.
fn codigo_error_of(text: Option<String>) -> Result<Option<i64>, String> {
    match text {
        None => Ok(None),
        Some(code) => code
            .parse::<i64>()
            .map(Some)
            .map_err(|_| format!("CodigoErrorRegistro is not numeric: {code:?}")),
    }
}

/// # Errors
/// A prose `Err` when the document is not the RS.xsd shape — the `bug`
/// class upstream: off-contract wire.
pub fn parse_respuesta(xml: &str) -> Result<RespuestaSuministro, String> {
    let root = parse_tree(xml)?;
    if root.local != "RespuestaRegFactuSistemaFacturacion" {
        return Err(format!(
            "root element is {}, not RespuestaRegFactuSistemaFacturacion",
            root.local
        ));
    }
    respuesta_of_tree(&root)
}

/// [`parse_respuesta`]'s tree half, shared with the envelope read edge.
pub(crate) fn respuesta_of_tree(root: &Tree) -> Result<RespuestaSuministro, String> {
    let csv = root.child_text("CSV").filter(|csv| !csv.is_empty());
    // `TiempoEsperaEnvio` may arrive EMPTY (`<TiempoEsperaEnvio/>`) — a
    // legal wire shape meaning "no new pacing from AEAT": the documented
    // initial 60s carries.
    let tiempo_espera_envio = match root.child_text("TiempoEsperaEnvio") {
        None => return Err(String::from("no TiempoEsperaEnvio")),
        Some(text) if text.is_empty() => 60,
        Some(text) => text
            .parse::<u64>()
            .map_err(|_| String::from("TiempoEsperaEnvio is not numeric"))?,
    };
    let estado_envio = vocabulary(
        "EstadoEnvio",
        &root.child_text("EstadoEnvio").ok_or("no EstadoEnvio")?,
        EstadoEnvio::as_str,
    )?;
    let mut lineas = Vec::new();
    for linea in root
        .children
        .iter()
        .filter(|node| node.local == "RespuestaLinea")
    {
        let id_factura = linea.child("IDFactura").ok_or("linea without IDFactura")?;
        let tipo_operacion = vocabulary(
            "TipoOperacion",
            &linea
                .child("Operacion")
                .and_then(|op| op.child_text("TipoOperacion"))
                .ok_or("linea without TipoOperacion")?,
            TipoOperacion::as_str,
        )?;
        let duplicado = match linea.child("RegistroDuplicado") {
            Some(dup) => {
                let estado_duplicado = vocabulary(
                    "EstadoRegistroDuplicado",
                    &dup.child_text("EstadoRegistroDuplicado")
                        .ok_or("RegistroDuplicado without EstadoRegistroDuplicado")?,
                    EstadoDuplicado::as_str,
                )?;
                Some(RegistroDuplicado {
                    id_peticion: dup
                        .child_text("IdPeticionRegistroDuplicado")
                        .unwrap_or_default(),
                    estado_duplicado,
                    codigo_error: codigo_error_of(dup.child_text("CodigoErrorRegistro"))?,
                    descripcion_error: dup.child_text("DescripcionErrorRegistro"),
                })
            }
            None => None,
        };
        lineas.push(RespuestaLinea {
            nif: id_factura
                .child_text("IDEmisorFactura")
                .ok_or("IDFactura without IDEmisorFactura")?,
            num_serie_factura: id_factura
                .child_text("NumSerieFactura")
                .ok_or("IDFactura without NumSerieFactura")?,
            fecha_expedicion_factura: id_factura
                .child_text("FechaExpedicionFactura")
                .ok_or("IDFactura without FechaExpedicionFactura")?,
            tipo_operacion,
            ref_externa: linea.child_text("RefExterna"),
            estado_registro: vocabulary(
                "EstadoRegistro",
                &linea
                    .child_text("EstadoRegistro")
                    .ok_or("linea without EstadoRegistro")?,
                EstadoRegistro::as_str,
            )?,
            codigo_error_registro: codigo_error_of(linea.child_text("CodigoErrorRegistro"))?,
            descripcion_error_registro: linea.child_text("DescripcionErrorRegistro"),
            registro_duplicado: duplicado,
        });
    }
    Ok(RespuestaSuministro {
        csv,
        tiempo_espera_envio,
        estado_envio,
        lineas,
    })
}

/// One `soapenv:Fault`'s code and string.
pub(crate) fn fault_of_tree(fault: &Tree) -> Result<SoapFault, String> {
    Ok(SoapFault {
        code: fault
            .child_text("faultcode")
            .ok_or("Fault without faultcode")?,
        faultstring: fault
            .child_text("faultstring")
            .ok_or("Fault without faultstring")?,
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub enum Disposition {
    Complete,
    /// Duplicate rejection with stored estado `Correcta`/`AceptadaConErrores`
    /// — already registered at AEAT: drains, never resends.
    DrainWithNote(String),
    /// Never auto-retried, never dropped — the operator decides
    /// subsanación (a NEW record; originals stay immutable).
    Surface(String),
}

impl Disposition {
    #[must_use]
    pub fn class(&self) -> Option<ErrorClass> {
        match self {
            Self::Complete => None,
            Self::DrainWithNote(_) | Self::Surface(_) => Some(ErrorClass::Regulatory),
        }
    }
}

/// Formats one line's AEAT error detail for a disposition note — the ONE
/// formatter for this shape (a consumer's per-line detail delegates here,
/// so the AEAT note format has a single law home).
#[must_use]
pub fn line_note(linea: &RespuestaLinea) -> String {
    let codigo = linea
        .codigo_error_registro
        .map_or_else(|| String::from("sin código"), |code| code.to_string());
    let descripcion = linea
        .descripcion_error_registro
        .as_deref()
        .unwrap_or("sin descripción");
    format!(
        "NIF {} NumSerie {} fecha {}: {} {}",
        linea.nif, linea.num_serie_factura, linea.fecha_expedicion_factura, codigo, descripcion
    )
}

/// Surfacing lines win over draining ones; a non-Correcto `EstadoEnvio`
/// that no line explains (zero lines = wholesale rejection) SURFACES,
/// never Completes.
#[must_use]
pub fn classify_response(respuesta: &RespuestaSuministro) -> Disposition {
    let mut surface: Option<String> = None;
    let mut drain: Option<String> = None;
    for linea in &respuesta.lineas {
        match linea.estado_registro {
            EstadoRegistro::Correcto => {}
            EstadoRegistro::AceptadoConErrores => {
                surface = Some(line_note(linea));
            }
            EstadoRegistro::Incorrecto => {
                let stored = linea
                    .registro_duplicado
                    .as_ref()
                    .map(|dup| dup.estado_duplicado);
                match stored {
                    Some(
                        estado @ (EstadoDuplicado::Correcta | EstadoDuplicado::AceptadaConErrores),
                    ) => {
                        drain = Some(format!(
                            "registro duplicado en AEAT (estado almacenado {}): {} — el \
                             registro ya está remitido, se purga con nota",
                            estado.as_str(),
                            line_note(linea)
                        ));
                    }
                    Some(EstadoDuplicado::Anulada) => {
                        surface = Some(format!(
                            "registro duplicado pero el almacenado está ANULADO: {} — decide \
                             un operador",
                            line_note(linea)
                        ));
                    }
                    None => surface = Some(line_note(linea)),
                }
            }
        }
    }
    if let Some(note) = surface {
        return Disposition::Surface(note);
    }
    if let Some(note) = drain {
        return Disposition::DrainWithNote(note);
    }
    if respuesta.estado_envio != EstadoEnvio::Correcto {
        return Disposition::Surface(format!(
            "EstadoEnvio {:?} sin líneas que lo expliquen — envío rechazado a nivel de \
             estructura o cabecera",
            respuesta.estado_envio
        ));
    }
    Disposition::Complete
}

/// Unknown fault codes are conservatively `OperatorAction`, never an
/// automatic retry.
#[must_use]
pub fn classify_fault(fault: &SoapFault) -> ErrorClass {
    if fault.is_server() {
        ErrorClass::Transient
    } else {
        ErrorClass::OperatorAction
    }
}
