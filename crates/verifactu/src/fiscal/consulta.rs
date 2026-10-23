//! The consulta operation (`ConsultaFactuSistemaFacturacion`, WSDL's
//! `sfVerifactu` binding — the SAME `VerifactuSOAP` door as remission,
//! empty `soapAction`): AEAT's read-back of the records it stores, per
//! `ConsultaLR.xsd` + `RespuestaConsultaLR.xsd` (vendored under
//! `contracts/aeat-verifactu/`).
//!
//! The cabecera's `IndicadorRepresentante=S` is the apoderado door
//! (SI.xsd: "quien realiza la consulta es el representante/asesor del
//! obligado tributario… obtener los registros de facturación en los
//! que figura como representante") — the vendor custody model's
//! reconciliation path; `consulta_for` sets it, the tenant door never
//! does.
//!
//! Each answered record carries its stored `Huella` + the fields the
//! huella input hashes — [`RegistroConsulta::huella_verifica`]
//! recomputes it, so a product can prove AEAT stores byte-what-it-sealed
//! (and audit the whole stored chain page by page).
//!
//! Asked for it, each record also carries the `SistemaInformatico` that
//! issued it ([`RegistroConsulta::sistema_informatico`]): under one NIF,
//! the records of another system or installation are not this one's.

use std::sync::Arc;

use rust_decimal::Decimal;

use crate::domain::chain::{huella_alta_montos, FechaExpedicion, TipoFactura};
use crate::domain::money::Money;
use crate::fiscal::transport::{TransportFailure, TransportOutcome, VerifactuTransport};
use crate::fiscal::xml::{self, tag, Tree};
use crate::fiscal::{
    EmitError, IdOtro, IdOtroType, IdentificacionDestinatario, Obligado, UsoPosible,
};

const NS_CONSULTA: &str = "https://www2.agenciatributaria.gob.es/static_files/common/internet/dep/aplicaciones/es/aeat/tike/cont/ws/ConsultaLR.xsd";
const P_C: &str = "sfC";

/// The `PeriodoImputacion` month (`TipoPeriodoType`: `01`..`12` —
/// the consulta knows no annual/quarterly codes).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Mes(u8);

impl Mes {
    pub const ENERO: Self = Self(1);

    /// # Errors
    /// Prose `Err` outside 1..=12.
    pub fn try_new(mes: u8) -> Result<Self, String> {
        if (1..=12).contains(&mes) {
            Ok(Self(mes))
        } else {
            Err(format!("Periodo is 01..12 monthly, got {mes}"))
        }
    }

    #[must_use]
    pub fn as_codigo(&self) -> String {
        format!("{:0>2}", self.0)
    }
}

/// `FiltroConsulta/FechaExpedicionFactura` (`FechaExpedicionConsultaType`):
/// the records issued on one day, or within a range open at either end.
/// It narrows the period's records and does not replace the period: a
/// record declaring an earlier operation lands in that operation's
/// month, before its issue date.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FechaExpedicionConsulta<'a> {
    /// `FechaExpedicionFactura`: issued that day.
    Fecha(&'a FechaExpedicion),
    /// `RangoFechaExpedicion`: issued from `desde` to `hasta`, both
    /// included; `None` leaves that end open, and with neither the
    /// range narrows nothing (live-confirmed at pruebas 2026-10-23).
    /// With both, `desde` comes first (AEAT 1250).
    Rango {
        desde: Option<&'a FechaExpedicion>,
        hasta: Option<&'a FechaExpedicion>,
    },
}

impl FechaExpedicionConsulta<'_> {
    /// AEAT 1250: "fecha desde debe ser menor que ... fecha hasta",
    /// strictly (live-confirmed at pruebas 2026-10-23: `Desde` equal to
    /// `Hasta` answers 1250); one day is [`Self::Fecha`].
    fn check(self) -> Result<(), xml::SerializeError> {
        match self {
            Self::Rango {
                desde: Some(desde),
                hasta: Some(hasta),
            } if desde.ymd() >= hasta.ymd() => Err(xml::SerializeError::Field {
                field: "Consulta/FechaExpedicionFactura/RangoFechaExpedicion",
                problem: String::from("Desde must precede Hasta (AEAT 1250)"),
            }),
            _ => Ok(()),
        }
    }

    /// The wrapper is declared in ConsultaLR.xsd, its content in SI.xsd's
    /// `FechaExpedicionConsultaType` (SW description §11.1.1.3's example).
    fn render(self, out: &mut String) {
        out.push_str("<sfC:FechaExpedicionFactura>");
        match self {
            Self::Fecha(fecha) => tag(out, "sum1:FechaExpedicionFactura", fecha.as_str()),
            Self::Rango { desde, hasta } => {
                out.push_str("<sum1:RangoFechaExpedicion>");
                if let Some(desde) = desde {
                    tag(out, "sum1:Desde", desde.as_str());
                }
                if let Some(hasta) = hasta {
                    tag(out, "sum1:Hasta", hasta.as_str());
                }
                out.push_str("</sum1:RangoFechaExpedicion>");
            }
        }
        out.push_str("</sfC:FechaExpedicionFactura>");
    }
}

/// The query window + narrowing keys (`LRFiltroRegFacturacionType`),
/// and what the answer should carry (`DatosAdicionalesRespuestaType`).
/// The period is the record's `PeriodoImputacion`, where it lands: the
/// month of its `FechaOperacion`, else of its `FechaExpedicionFactura`
/// (SW description §6.4). Live-observed 2026-10-22: a factura issued in
/// October for an operation of 1 January answered `SinDatos` under
/// October.
///
/// `FiltroConsulta/SistemaInformatico` is not offered: AEAT matches it
/// against the systems it knows and refuses a filter that matches
/// several (AEAT 1285), so read each record's system back instead
/// ([`mostrar_sistema_informatico`](method@Self::mostrar_sistema_informatico)).
#[derive(Clone, Copy, Debug)]
pub struct ConsultaFilter<'a> {
    pub ejercicio: u16,
    pub mes: Mes,
    pub num_serie: Option<&'a str>,
    /// The issue date or range, within the period
    /// ([`fecha_expedicion`](method@Self::fecha_expedicion),
    /// [`rango_fecha_expedicion`](method@Self::rango_fecha_expedicion)).
    pub fecha_expedicion: Option<FechaExpedicionConsulta<'a>>,
    pub ref_externa: Option<&'a str>,
    /// `DatosAdicionalesRespuesta/MostrarSistemaInformatico=S`: AEAT
    /// answers each record's stored `SistemaInformatico`
    /// ([`RegistroConsulta::sistema_informatico`]), at the cost of a
    /// slower answer (ConsultaLR.xsd). A consulta by Destinatario must
    /// leave it unset (AEAT 1272); the obligado and apoderado lenses,
    /// the only ones this crate opens, admit it.
    pub mostrar_sistema_informatico: bool,
}

impl<'a> ConsultaFilter<'a> {
    #[must_use]
    pub fn new(ejercicio: u16, mes: Mes) -> Self {
        Self {
            ejercicio,
            mes,
            num_serie: None,
            fecha_expedicion: None,
            ref_externa: None,
            mostrar_sistema_informatico: false,
        }
    }

    /// One factura's `NumSerieFactura` (1..60 chars).
    #[must_use]
    pub fn num_serie(mut self, num_serie: &'a str) -> Self {
        self.num_serie = Some(num_serie);
        self
    }

    /// Only the records issued on `fecha`.
    #[must_use]
    pub fn fecha_expedicion(mut self, fecha: &'a FechaExpedicion) -> Self {
        self.fecha_expedicion = Some(FechaExpedicionConsulta::Fecha(fecha));
        self
    }

    /// Only the records issued from `desde` to `hasta`, both included;
    /// `None` leaves that end open. With both, `desde` must come first
    /// (AEAT 1250).
    #[must_use]
    pub fn rango_fecha_expedicion(
        mut self,
        desde: Option<&'a FechaExpedicion>,
        hasta: Option<&'a FechaExpedicion>,
    ) -> Self {
        self.fecha_expedicion = Some(FechaExpedicionConsulta::Rango { desde, hasta });
        self
    }

    #[must_use]
    pub fn ref_externa(mut self, ref_externa: &'a str) -> Self {
        self.ref_externa = Some(ref_externa);
        self
    }

    /// Asks AEAT for each record's stored `SistemaInformatico`: sets
    /// the `mostrar_sistema_informatico` field.
    #[must_use]
    pub fn mostrar_sistema_informatico(mut self) -> Self {
        self.mostrar_sistema_informatico = true;
        self
    }
}

/// The resumed-page key AEAT hands back (`IDFacturaExpedidaBCType`).
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ClavePaginacion {
    pub id_emisor_factura: String,
    pub num_serie_factura: String,
    pub fecha_expedicion_factura: String,
}

/// The full request the doors assemble (the flat face never builds it
/// by hand): whose records, through which custody lens.
pub struct ConsultaRequest<'a> {
    pub obligado: &'a Obligado,
    /// `IndicadorRepresentante=S` — the apoderado read.
    pub apoderado: bool,
    pub filtro: &'a ConsultaFilter<'a>,
    pub clave_paginacion: Option<&'a ClavePaginacion>,
}

/// `ResultadoConsultaType`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub enum ResultadoConsulta {
    ConDatos,
    SinDatos,
}

impl ResultadoConsulta {
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "ConDatos" => Ok(Self::ConDatos),
            "SinDatos" => Ok(Self::SinDatos),
            other => Err(format!("off-vocabulary ResultadoConsulta: {other}")),
        }
    }

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ConDatos => "ConDatos",
            Self::SinDatos => "SinDatos",
        }
    }
}

/// The STORED record's estado (`EstadoRegistroType` of the consulta
/// respuesta — a different vocabulary from the submission line's).
#[derive(Clone, Copy, Debug, PartialEq, Eq, strum::EnumIter)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub enum EstadoAlmacenado {
    Correcto,
    AceptadoConErrores,
    Anulado,
}

impl EstadoAlmacenado {
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "Correcto" => Ok(Self::Correcto),
            "AceptadoConErrores" => Ok(Self::AceptadoConErrores),
            "Anulado" => Ok(Self::Anulado),
            other => Err(format!("off-vocabulary EstadoRegistro: {other}")),
        }
    }

    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Correcto => "Correcto",
            Self::AceptadoConErrores => "AceptadoConErrores",
            Self::Anulado => "Anulado",
        }
    }
}

/// One stored record as AEAT answers it: identity, stored estado, and
/// (when AEAT echoes them) the fields the huella input hashes.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct RegistroConsulta {
    pub id_emisor_factura: String,
    pub num_serie_factura: String,
    pub fecha_expedicion_factura: String,
    pub estado: EstadoAlmacenado,
    pub timestamp_ultima_modificacion: String,
    pub codigo_error: Option<i64>,
    pub descripcion_error: Option<String>,
    pub tipo_factura: Option<TipoFactura>,
    pub cuota_total: Option<Money>,
    pub importe_total: Option<Money>,
    pub fecha_huso_gen: Option<String>,
    /// The stored `Encadenamiento`: `None` = `PrimerRegistro` (the
    /// huella input's `Huella` is EMPTY), `Some` = `RegistroAnterior`'s
    /// `Huella`.
    pub huella_previa: Option<String>,
    pub huella: Option<String>,
    /// The system that issued the record, as AEAT stores it; `None`
    /// unless the consulta asked for it
    /// ([`ConsultaFilter::mostrar_sistema_informatico`]) and AEAT
    /// answered it.
    ///
    /// The producer's identification, `IdSistemaInformatico` and
    /// `NumeroInstalacion` identify an installation; `Version` changes
    /// across upgrades. Compare those three, not `Version`, to tell an
    /// installation's own records from another system's under the same
    /// NIF.
    pub sistema_informatico: Option<SistemaInformaticoAlmacenado>,
}

/// A record's stored `SistemaInformatico` block
/// (`SistemaInformaticoType`, every child required).
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct SistemaInformaticoAlmacenado {
    /// The producer of the software (Validaciones §3.1.5), not the
    /// obligado.
    pub nombre_razon: String,
    /// The producer's `NIF | IDOtro`: the type repeats SI.xsd's
    /// `PersonaFisicaJuridicaType` choice, so it reads as a
    /// destinatario's does.
    pub identificacion: IdentificacionDestinatario,
    pub nombre_sistema_informatico: String,
    pub id_sistema_informatico: String,
    pub version: String,
    pub numero_instalacion: String,
    /// `TipoUsoPosibleSoloVerifactu`.
    pub uso_posible: UsoPosible,
    /// `TipoUsoPosibleMultiOT`, `S` as `true`: the installation can
    /// serve several obligados tributarios. This crate emits `N`, one
    /// obligado per installation.
    pub tipo_uso_posible_multi_ot: bool,
    /// `IndicadorMultiplesOT`, `S` as `true`: the installation serves
    /// several obligados tributarios. This crate emits `N`.
    pub indicador_multiples_ot: bool,
}

fn parse_importe(text: &str) -> Option<Money> {
    Money::try_from_decimal(text.parse::<Decimal>().ok()?)
}

impl RegistroConsulta {
    /// Recomputes the stored `Huella` from the fields AEAT echoed (HS
    /// §3.a over the respuesta's `DatosRegistroFacturacion`):
    /// `Some(true/false)` when every input field is present, `None`
    /// when the record carries an `Anulado` estado's thinner echo (or
    /// AEAT omitted a field) — the chain-audit verdict over AEAT's own
    /// storage.
    #[must_use]
    pub fn huella_verifica(&self) -> Option<bool> {
        let huella = self.huella.as_deref()?;
        let tipo = self.tipo_factura?;
        let (cuota, importe) = (self.cuota_total?, self.importe_total?);
        let huso = self.fecha_huso_gen.as_deref()?;
        // HS §3 admits 1- and 2-decimal amount renderings — try each
        // trailing-zero trim of both, or a legally-echoed `41.4`
        // false-negatives.
        for cuota in importe_variantes(cuota) {
            for importe in importe_variantes(importe) {
                if huella_alta_montos(
                    &self.id_emisor_factura,
                    &self.num_serie_factura,
                    &self.fecha_expedicion_factura,
                    tipo.as_str(),
                    &cuota,
                    &importe,
                    self.huella_previa.as_deref(),
                    huso,
                ) == huella
                {
                    return Some(true);
                }
            }
        }
        Some(false)
    }
}

/// The echoed-amount renderings HS §3 admits for one [`Money`]:
/// 2dp, then the trailing-zero trims.
fn importe_variantes(amount: Money) -> Vec<String> {
    let cents = amount.as_cents();
    let sign = if cents < 0 { "-" } else { "" };
    let units = cents.unsigned_abs() / 100;
    let frac = cents.unsigned_abs() % 100;
    let mut out = vec![format!("{sign}{units}.{frac:02}")];
    if frac.is_multiple_of(10) {
        out.push(format!("{sign}{units}.{}", frac / 10));
    }
    if frac == 0 {
        out.push(format!("{sign}{units}"));
    }
    out
}

/// One page of AEAT's answer.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct ConsultaAnswer {
    pub resultado: ResultadoConsulta,
    /// `IndicadorPaginacion=S` — more pages follow; the next request
    /// carries [`Self::clave_paginacion`].
    pub paginacion_pendiente: bool,
    pub clave_paginacion: Option<ClavePaginacion>,
    pub registros: Vec<RegistroConsulta>,
}

// ---- render ------------------------------------------------------------

/// # Errors
/// [`EmitError::InvalidRecord`] when any SI.xsd simple type is
/// violated (NIF, text lengths, the period codes), or an issue-date
/// range's `Desde` does not precede its `Hasta` (AEAT 1250).
pub(crate) fn consulta_document(request: &ConsultaRequest<'_>) -> Result<String, EmitError> {
    xml::check_nif("Consulta/ObligadoEmision/NIF", &request.obligado.nif)?;
    xml::check_text(
        "Consulta/ObligadoEmision/NombreRazon",
        &request.obligado.nombre_razon,
        120,
    )?;
    if !(1000..=9999).contains(&request.filtro.ejercicio) {
        return Err(EmitError::InvalidRecord {
            detail: format!(
                "Ejercicio is a 4-digit year, got {}",
                request.filtro.ejercicio
            ),
        });
    }
    if let Some(num_serie) = request.filtro.num_serie {
        xml::check_num_serie("Consulta/NumSerieFactura", num_serie)?;
    }
    if let Some(fecha) = request.filtro.fecha_expedicion {
        fecha.check()?;
    }
    if let Some(ref_externa) = request.filtro.ref_externa {
        xml::check_text("Consulta/RefExterna", ref_externa, 60)?;
    }
    if let Some(clave) = request.clave_paginacion {
        xml::check_nif(
            "Consulta/ClavePaginacion/IDEmisorFactura",
            &clave.id_emisor_factura,
        )?;
        xml::check_num_serie(
            "Consulta/ClavePaginacion/NumSerieFactura",
            &clave.num_serie_factura,
        )?;
        if FechaExpedicion::parse(&clave.fecha_expedicion_factura).is_none() {
            return Err(EmitError::InvalidRecord {
                detail: String::from(
                    "Consulta/ClavePaginacion/FechaExpedicionFactura is not dd-mm-yyyy",
                ),
            });
        }
    }

    let mut out = String::with_capacity(512);
    out.push('<');
    out.push_str(P_C);
    out.push_str(":ConsultaFactuSistemaFacturacion xmlns:sfC=\"");
    out.push_str(NS_CONSULTA);
    out.push_str("\" xmlns:sum1=\"");
    out.push_str(xml::NS_SUMINISTRO);
    out.push_str("\">");
    out.push_str("<sfC:Cabecera>");
    tag(&mut out, "sum1:IDVersion", xml::ID_VERSION);
    out.push_str("<sum1:ObligadoEmision>");
    tag(&mut out, "sum1:NombreRazon", &request.obligado.nombre_razon);
    tag(&mut out, "sum1:NIF", &request.obligado.nif);
    out.push_str("</sum1:ObligadoEmision>");
    if request.apoderado {
        tag(&mut out, "sum1:IndicadorRepresentante", "S");
    }
    out.push_str("</sfC:Cabecera>");
    out.push_str("<sfC:FiltroConsulta>");
    out.push_str("<sfC:PeriodoImputacion>");
    tag(
        &mut out,
        "sum1:Ejercicio",
        &request.filtro.ejercicio.to_string(),
    );
    tag(&mut out, "sum1:Periodo", &request.filtro.mes.as_codigo());
    out.push_str("</sfC:PeriodoImputacion>");
    if let Some(num_serie) = request.filtro.num_serie {
        tag(&mut out, "sfC:NumSerieFactura", num_serie);
    }
    if let Some(fecha) = request.filtro.fecha_expedicion {
        fecha.render(&mut out);
    }
    if let Some(ref_externa) = request.filtro.ref_externa {
        tag(&mut out, "sfC:RefExterna", ref_externa);
    }
    if let Some(clave) = request.clave_paginacion {
        out.push_str("<sfC:ClavePaginacion>");
        tag(&mut out, "sum1:IDEmisorFactura", &clave.id_emisor_factura);
        tag(&mut out, "sum1:NumSerieFactura", &clave.num_serie_factura);
        tag(
            &mut out,
            "sum1:FechaExpedicionFactura",
            &clave.fecha_expedicion_factura,
        );
        out.push_str("</sfC:ClavePaginacion>");
    }
    out.push_str("</sfC:FiltroConsulta>");
    if request.filtro.mostrar_sistema_informatico {
        // Declared in ConsultaLR.xsd down to the flag (only its simple
        // type is SI's), so every element carries sfC.
        out.push_str("<sfC:DatosAdicionalesRespuesta>");
        tag(&mut out, "sfC:MostrarSistemaInformatico", "S");
        out.push_str("</sfC:DatosAdicionalesRespuesta>");
    }
    out.push_str("</sfC:ConsultaFactuSistemaFacturacion>");
    Ok(out)
}

// ---- parse -------------------------------------------------------------

/// The consulta's answer alone, read as [`parse_http_answer`] reads it
/// beside the submission's: a SOAP envelope or the bare respuesta root.
///
/// [`parse_http_answer`]: crate::fiscal::transport::parse_http_answer
///
/// # Errors
/// Prose `Err` when the document carries neither shape or holds an
/// off-vocabulary value.
pub fn parse_consulta(xml: &str) -> Result<ConsultaAnswer, String> {
    let root = xml::parse_tree(xml)?;
    let payload = if root.local == "Envelope" {
        let body = root.child("Body").ok_or("SOAP envelope without Body")?;
        body.children
            .iter()
            .find(|node| node.local == "RespuestaConsultaFactuSistemaFacturacion")
            .ok_or("SOAP Body carries no RespuestaConsultaFactuSistemaFacturacion")?
    } else if root.local == "RespuestaConsultaFactuSistemaFacturacion" {
        &root
    } else {
        return Err(format!(
            "root element is {local}, not a SOAP envelope or \
             RespuestaConsultaFactuSistemaFacturacion",
            local = root.local
        ));
    };
    consulta_of_tree(payload)
}

/// [`parse_consulta`]'s tree half — shared with the transport read edge.
///
/// # Errors
/// Prose `Err` on a missing required field or off-vocabulary value.
pub(crate) fn consulta_of_tree(payload: &Tree) -> Result<ConsultaAnswer, String> {
    let resultado = ResultadoConsulta::parse(
        &payload
            .child_text("ResultadoConsulta")
            .ok_or("respuesta without ResultadoConsulta")?,
    )?;
    let paginacion_pendiente = match &payload
        .child_text("IndicadorPaginacion")
        .ok_or("respuesta without IndicadorPaginacion")?
    {
        text if text == "S" => true,
        text if text == "N" => false,
        text => return Err(format!("off-vocabulary IndicadorPaginacion: {text}")),
    };
    let clave_paginacion = payload
        .child("ClavePaginacion")
        .map(|clave| -> Result<ClavePaginacion, String> {
            Ok(ClavePaginacion {
                id_emisor_factura: clave
                    .child_text("IDEmisorFactura")
                    .ok_or("ClavePaginacion without IDEmisorFactura")?,
                num_serie_factura: clave
                    .child_text("NumSerieFactura")
                    .ok_or("ClavePaginacion without NumSerieFactura")?,
                fecha_expedicion_factura: clave
                    .child_text("FechaExpedicionFactura")
                    .ok_or("ClavePaginacion without FechaExpedicionFactura")?,
            })
        })
        .transpose()?;
    let mut registros = Vec::new();
    for node in &payload.children {
        if node.local == "RegistroRespuestaConsultaFactuSistemaFacturacion" {
            registros.push(registro_of_tree(node)?);
        }
    }
    Ok(ConsultaAnswer {
        resultado,
        paginacion_pendiente,
        clave_paginacion,
        registros,
    })
}

/// An optional child's text with the EMPTY element read as absent: the
/// anulado's thinner echo renders `<Huella/>` (and AEAT's serializer
/// emits empty elements for omitted optionals) — a present-but-empty
/// value would be off-grammar anyway, so `None` is the honest parse.
fn optional_text(node: &Tree, local: &str) -> Option<String> {
    node.child_text(local).filter(|text| !text.is_empty())
}

/// One stored record: identity, the XSD-required
/// `DatosRegistroFacturacion` echo, and the stored estado block.
fn registro_of_tree(node: &Tree) -> Result<RegistroConsulta, String> {
    let id_factura = node
        .child("IDFactura")
        .ok_or("registro without IDFactura")?;
    let estado_node = node
        .child("EstadoRegistro")
        .ok_or("registro without EstadoRegistro")?;
    let datos = node
        .child("DatosRegistroFacturacion")
        .ok_or("registro without DatosRegistroFacturacion")?;
    Ok(RegistroConsulta {
        id_emisor_factura: id_factura
            .child_text("IDEmisorFactura")
            .ok_or("IDFactura without IDEmisorFactura")?,
        num_serie_factura: id_factura
            .child_text("NumSerieFactura")
            .ok_or("IDFactura without NumSerieFactura")?,
        fecha_expedicion_factura: id_factura
            .child_text("FechaExpedicionFactura")
            .ok_or("IDFactura without FechaExpedicionFactura")?,
        estado: EstadoAlmacenado::parse(
            &estado_node
                .child_text("EstadoRegistro")
                .ok_or("EstadoRegistro block without EstadoRegistro")?,
        )?,
        timestamp_ultima_modificacion: estado_node
            .child_text("TimestampUltimaModificacion")
            .ok_or("EstadoRegistro block without TimestampUltimaModificacion")?,
        codigo_error: optional_text(estado_node, "CodigoErrorRegistro")
            .map(|text| {
                text.parse::<i64>()
                    .map_err(|_| format!("off-grammar CodigoErrorRegistro: {text}"))
            })
            .transpose()?,
        descripcion_error: optional_text(estado_node, "DescripcionErrorRegistro"),
        tipo_factura: datos
            .child_text("TipoFactura")
            .and_then(|text| TipoFactura::parse(&text)),
        cuota_total: datos
            .child_text("CuotaTotal")
            .and_then(|text| parse_importe(&text)),
        importe_total: datos
            .child_text("ImporteTotal")
            .and_then(|text| parse_importe(&text)),
        fecha_huso_gen: optional_text(datos, "FechaHoraHusoGenRegistro"),
        huella_previa: datos.child("Encadenamiento").and_then(|encadenamiento| {
            encadenamiento
                .child("RegistroAnterior")
                .and_then(|anterior| optional_text(anterior, "Huella"))
        }),
        huella: optional_text(datos, "Huella"),
        sistema_informatico: sistema_of_tree(datos)?,
    })
}

/// The stored `SistemaInformatico` (`SistemaInformaticoType`): every
/// child is required, so a block missing one is an `Err`. The empty
/// element reads as absent, as [`optional_text`] reads an empty value.
fn sistema_of_tree(datos: &Tree) -> Result<Option<SistemaInformaticoAlmacenado>, String> {
    let Some(node) = datos.child("SistemaInformatico") else {
        return Ok(None);
    };
    if node.children.is_empty() && node.text.trim().is_empty() {
        return Ok(None);
    }
    let required = |local: &str| {
        node.child_text(local)
            .ok_or_else(|| format!("SistemaInformatico without {local}"))
    };
    let si_no = |local: &str| match required(local)?.as_str() {
        "S" => Ok(true),
        "N" => Ok(false),
        other => Err(format!(
            "off-vocabulary SistemaInformatico/{local}: {other}"
        )),
    };
    let identificacion = match (node.child("NIF"), node.child("IDOtro")) {
        (Some(nif), None) => IdentificacionDestinatario::Nif(nif.text.clone()),
        (None, Some(otro)) => {
            let id_type = otro
                .child_text("IDType")
                .ok_or("SistemaInformatico/IDOtro without IDType")?;
            IdentificacionDestinatario::Otro(IdOtro {
                codigo_pais: optional_text(otro, "CodigoPais"),
                id_type: IdOtroType::parse(&id_type).ok_or_else(|| {
                    format!("off-vocabulary SistemaInformatico/IDOtro/IDType: {id_type}")
                })?,
                id: otro
                    .child_text("ID")
                    .ok_or("SistemaInformatico/IDOtro without ID")?,
            })
        }
        _ => {
            return Err(String::from(
                "SistemaInformatico carries neither or both of NIF and IDOtro",
            ))
        }
    };
    let uso_posible = required("TipoUsoPosibleSoloVerifactu")?;
    Ok(Some(SistemaInformaticoAlmacenado {
        nombre_razon: required("NombreRazon")?,
        identificacion,
        nombre_sistema_informatico: required("NombreSistemaInformatico")?,
        id_sistema_informatico: required("IdSistemaInformatico")?,
        version: required("Version")?,
        numero_instalacion: required("NumeroInstalacion")?,
        uso_posible: UsoPosible::parse(&uso_posible).ok_or_else(|| {
            format!("off-vocabulary SistemaInformatico/TipoUsoPosibleSoloVerifactu: {uso_posible}")
        })?,
        tipo_uso_posible_multi_ot: si_no("TipoUsoPosibleMultiOT")?,
        indicador_multiples_ot: si_no("IndicadorMultiplesOT")?,
    }))
}

// ---- the wire ----------------------------------------------------------

/// One consulta envío through any transport.
///
/// # Errors
/// [`EmitError::InvalidRecord`] off-contract; [`EmitError::Transport`]
/// with the transport's failure, or with [`TransportFailure::Unreadable`]
/// on a submission-shaped answer to a consulta;
/// [`EmitError::FaultClient`]/[`EmitError::FaultServer`] on a SOAP
/// fault.
pub async fn send_once(
    transport: &Arc<dyn VerifactuTransport>,
    request: &ConsultaRequest<'_>,
) -> Result<ConsultaAnswer, EmitError> {
    let body = consulta_document(request)?;
    match transport.send(&xml::soap_envelope(&body)).await? {
        TransportOutcome::Fault(fault) => Err(fault.into()),
        TransportOutcome::Consulta(answer) => Ok(answer),
        TransportOutcome::Response(_) => Err(TransportFailure::Unreadable {
            detail: String::from("AEAT answered a submission respuesta to a consulta"),
        }
        .into()),
    }
}

/// The product door: every page, merged, in call order. Consultas ride
/// the same `VerifactuSOAP` endpoint as remission.
///
/// # Errors
/// [`EmitError`] from [`send_once`]; [`EmitError::Transport`] with
/// [`TransportFailure::Unreadable`] when AEAT signals more pages without
/// a pagination key, or past the loop's 100-page bound.
pub async fn send_all(
    transport: &Arc<dyn VerifactuTransport>,
    obligado: &Obligado,
    apoderado: bool,
    filtro: &ConsultaFilter<'_>,
) -> Result<ConsultaAnswer, EmitError> {
    let mut clave: Option<ClavePaginacion> = None;
    let mut registros = Vec::new();
    for _ in 0..100 {
        let page = send_once(
            transport,
            &ConsultaRequest {
                obligado,
                apoderado,
                filtro,
                clave_paginacion: clave.as_ref(),
            },
        )
        .await?;
        registros.extend(page.registros);
        if !page.paginacion_pendiente {
            return Ok(ConsultaAnswer {
                resultado: page.resultado,
                paginacion_pendiente: false,
                clave_paginacion: None,
                registros,
            });
        }
        if page.clave_paginacion.is_none() {
            return Err(TransportFailure::Unreadable {
                detail: String::from(
                    "AEAT signaled more pages (IndicadorPaginacion=S) without a ClavePaginacion",
                ),
            }
            .into());
        }
        clave = page.clave_paginacion;
    }
    Err(TransportFailure::Unreadable {
        detail: String::from(
            "AEAT signaled more pages (IndicadorPaginacion=S) past the consulta's 100-page bound",
        ),
    }
    .into())
}

#[cfg(any(test, feature = "fakes"))]
mod fake;
#[cfg(any(test, feature = "fakes"))]
pub use fake::{consulta_response_document, ConsultaSpec, EncadenamientoSpec, RegistroSpec};

#[cfg(test)]
mod tests {
    use super::{
        consulta_document, consulta_response_document, parse_consulta, ConsultaFilter,
        ConsultaRequest, ConsultaSpec, EstadoAlmacenado, Mes, RegistroSpec, ResultadoConsulta,
        SistemaInformaticoAlmacenado,
    };
    use crate::domain::chain::{huella_alta_montos, FechaExpedicion};
    use crate::domain::money::Money;
    use crate::fiscal::{IdOtro, IdOtroType, IdentificacionDestinatario, Obligado, UsoPosible};

    fn obligado() -> Obligado {
        Obligado {
            nombre_razon: String::from("ANA RUIZ PEREZ"),
            nif: String::from("12345678Z"),
        }
    }

    /// The request renders the ConsultaLR.xsd element law: SI-namespace
    /// children where the TYPE lives in SI.xsd, sfC where the element
    /// is declared in ConsultaLR.xsd.
    #[test]
    fn the_request_renders_the_xsd_namespace_law() {
        let obligado = obligado();
        let filtro = ConsultaFilter::new(2026, Mes::try_new(10).expect("octubre"))
            .num_serie("T00000007")
            .ref_externa("REF-1");
        let document = consulta_document(&ConsultaRequest {
            obligado: &obligado,
            apoderado: true,
            filtro: &filtro,
            clave_paginacion: None,
        })
        .expect("the request renders");
        assert!(document.contains("<sfC:Cabecera><sum1:IDVersion>1.0</sum1:IDVersion>"));
        assert!(document.contains("<sum1:IndicadorRepresentante>S</sum1:IndicadorRepresentante>"));
        assert!(document.contains("<sfC:PeriodoImputacion><sum1:Ejercicio>2026</sum1:Ejercicio><sum1:Periodo>10</sum1:Periodo>"));
        assert!(document.contains("<sfC:NumSerieFactura>T00000007</sfC:NumSerieFactura>"));
        assert!(document.contains("<sfC:RefExterna>REF-1</sfC:RefExterna>"));
        assert!(document.starts_with("<sfC:ConsultaFactuSistemaFacturacion"));
    }

    #[test]
    fn off_contract_requests_fail_early() {
        let obligado = obligado();
        let filtro = ConsultaFilter::new(26, Mes::try_new(10).expect("octubre"));
        let error = consulta_document(&ConsultaRequest {
            obligado: &obligado,
            apoderado: false,
            filtro: &filtro,
            clave_paginacion: None,
        })
        .expect_err("a 2-digit ejercicio is off-contract");
        assert!(error.to_string().contains("4-digit"));
        assert!(Mes::try_new(0).is_err() && Mes::try_new(13).is_err());
        assert_eq!(
            Mes::try_new(3).expect("marzo").as_codigo(),
            "03",
            "the period rides the 2-digit XSD codes"
        );
        // TextoIDFacturaType carries minLength 1: an empty narrowing
        // element is off-contract, not "no narrowing".
        let filtro = ConsultaFilter::new(2026, Mes::try_new(10).expect("octubre")).num_serie("");
        let error = consulta_document(&ConsultaRequest {
            obligado: &obligado,
            apoderado: false,
            filtro: &filtro,
            clave_paginacion: None,
        })
        .expect_err("an empty NumSerieFactura narrows nothing legally");
        assert!(error.to_string().contains("1..=60"), "{error}");
    }

    /// The anulado's thinner echo — every optional field absent, the
    /// empty `<Huella/>` element — parses as `None` across the board,
    /// and `huella_verifica` answers `None` (nothing to prove) rather
    /// than a false negative.
    #[test]
    fn the_anulados_thinner_echo_parses_as_absent() {
        let spec = ConsultaSpec {
            obligado_nombre: String::from("ANA RUIZ PEREZ"),
            obligado_nif: String::from("89890001K"),
            apoderado: false,
            ejercicio: 2024,
            mes: Mes::try_new(1).expect("enero"),
            resultado: ResultadoConsulta::ConDatos,
            paginacion_pendiente: false,
            clave_paginacion: None,
            registros: vec![RegistroSpec {
                id_emisor_factura: String::from("89890001K"),
                num_serie_factura: String::from("12345678/G33"),
                fecha_expedicion_factura: String::from("01-01-2024"),
                estado: EstadoAlmacenado::Anulado,
                timestamp_ultima_modificacion: String::from("2024-01-02T19:20:35+01:00"),
                codigo_error: None,
                descripcion_error: None,
                tipo_factura: None,
                cuota_total: None,
                importe_total: None,
                fecha_huso_gen: None,
                huella_previa: None,
                // None renders the empty element `<sfRC:Huella/>`.
                huella: None,
                sistema_informatico: None,
            }],
        };
        let answer =
            parse_consulta(&consulta_response_document(&spec)).expect("the respuesta parses");
        let registro = &answer.registros[0];
        assert_eq!(registro.estado, EstadoAlmacenado::Anulado);
        assert_eq!(registro.huella, None, "an empty element is absent");
        assert_eq!(
            registro.huella_verifica(),
            None,
            "no huella echo to verify — the estado itself is the verdict"
        );
    }

    /// The respuesta round-trips through the real read edge, and the
    /// stored record's huella RECOMPUTES from the echoed fields — the
    /// HS §6.1 official vector end-to-end.
    #[test]
    fn the_answer_round_trips_and_the_huella_recomputes() {
        assert_eq!(
            huella_alta_montos(
                "89890001K",
                "12345678/G33",
                "01-01-2024",
                "F1",
                "12.35",
                "123.45",
                None,
                "2024-01-01T19:20:30+01:00",
            ),
            "3C464DAF61ACB827C65FDA19F352A4E3BDC2C640E9E9FC4CC058073F38F12F60"
        );

        let spec = ConsultaSpec {
            obligado_nombre: String::from("ANA RUIZ PEREZ"),
            obligado_nif: String::from("89890001K"),
            apoderado: false,
            ejercicio: 2024,
            mes: Mes::try_new(1).expect("enero"),
            resultado: ResultadoConsulta::ConDatos,
            paginacion_pendiente: false,
            clave_paginacion: None,
            registros: vec![RegistroSpec {
                id_emisor_factura: String::from("89890001K"),
                num_serie_factura: String::from("12345678/G33"),
                fecha_expedicion_factura: String::from("01-01-2024"),
                estado: EstadoAlmacenado::Correcto,
                timestamp_ultima_modificacion: String::from("2024-01-01T19:20:35+01:00"),
                codigo_error: None,
                descripcion_error: None,
                tipo_factura: Some(crate::domain::chain::TipoFactura::F1),
                cuota_total: Some(Money::from_cents(1235)),
                importe_total: Some(Money::from_cents(12_345)),
                fecha_huso_gen: Some(String::from("2024-01-01T19:20:30+01:00")),
                huella_previa: None,
                huella: Some(String::from(
                    "3C464DAF61ACB827C65FDA19F352A4E3BDC2C640E9E9FC4CC058073F38F12F60",
                )),
                sistema_informatico: None,
            }],
        };
        let answer =
            parse_consulta(&consulta_response_document(&spec)).expect("the respuesta parses");
        assert_eq!(answer.resultado, ResultadoConsulta::ConDatos);
        assert!(!answer.paginacion_pendiente);
        let registro = &answer.registros[0];
        assert_eq!(registro.estado, EstadoAlmacenado::Correcto);
        assert_eq!(registro.num_serie_factura, "12345678/G33");
        assert_eq!(
            registro.huella_verifica(),
            Some(true),
            "the stored huella recomputes from AEAT's echoed fields"
        );
    }

    /// A corrupted stored huella (or a drifted field) FAILS the
    /// recompute — the audit verdict is a real comparison.
    #[test]
    fn a_drifted_field_fails_the_recompute() {
        let spec = ConsultaSpec {
            obligado_nombre: String::from("ANA RUIZ PEREZ"),
            obligado_nif: String::from("89890001K"),
            apoderado: false,
            ejercicio: 2024,
            mes: Mes::try_new(1).expect("enero"),
            resultado: ResultadoConsulta::ConDatos,
            paginacion_pendiente: false,
            clave_paginacion: None,
            registros: vec![RegistroSpec {
                id_emisor_factura: String::from("89890001K"),
                num_serie_factura: String::from("12345678/G33"),
                fecha_expedicion_factura: String::from("01-01-2024"),
                estado: EstadoAlmacenado::Correcto,
                timestamp_ultima_modificacion: String::from("2024-01-01T19:20:35+01:00"),
                codigo_error: None,
                descripcion_error: None,
                tipo_factura: Some(crate::domain::chain::TipoFactura::F1),
                cuota_total: Some(Money::from_cents(1235)),
                importe_total: Some(Money::from_cents(999_999)), // drifted
                fecha_huso_gen: Some(String::from("2024-01-01T19:20:30+01:00")),
                huella_previa: None,
                huella: Some(String::from(
                    "3C464DAF61ACB827C65FDA19F352A4E3BDC2C640E9E9FC4CC058073F38F12F60",
                )),
                sistema_informatico: None,
            }],
        };
        let answer =
            parse_consulta(&consulta_response_document(&spec)).expect("the respuesta parses");
        assert_eq!(answer.registros[0].huella_verifica(), Some(false));
    }

    /// `MostrarSistemaInformatico=S` rides `DatosAdicionalesRespuesta`,
    /// after `FiltroConsulta`, exactly when the filter asks for it.
    #[test]
    fn the_sistema_flag_renders_exactly_when_set() {
        let obligado = obligado();
        let render = |filtro: &ConsultaFilter<'_>| {
            consulta_document(&ConsultaRequest {
                obligado: &obligado,
                apoderado: false,
                filtro,
                clave_paginacion: None,
            })
            .expect("the request renders")
        };
        let filtro = ConsultaFilter::new(2026, Mes::try_new(10).expect("octubre"));
        assert!(!filtro.mostrar_sistema_informatico, "new leaves it unset");
        let document = render(&filtro);
        assert!(
            !document.contains("DatosAdicionalesRespuesta"),
            "{document}"
        );
        assert!(
            !document.contains("MostrarSistemaInformatico"),
            "{document}"
        );

        let document = render(&filtro.mostrar_sistema_informatico());
        assert!(
            document.ends_with(
                "</sfC:FiltroConsulta><sfC:DatosAdicionalesRespuesta>\
                 <sfC:MostrarSistemaInformatico>S</sfC:MostrarSistemaInformatico>\
                 </sfC:DatosAdicionalesRespuesta></sfC:ConsultaFactuSistemaFacturacion>"
            ),
            "{document}"
        );
    }

    /// The issue-date filter rides `FiltroConsulta` between
    /// `NumSerieFactura` and `RefExterna`, its wrapper sfC and its
    /// content SI, as AEAT's example spells it (SW description
    /// §11.1.1.3); an open end leaves its bound out.
    #[test]
    fn the_issue_date_filter_renders_in_its_xsd_place() {
        let obligado = obligado();
        let render = |filtro: &ConsultaFilter<'_>| {
            consulta_document(&ConsultaRequest {
                obligado: &obligado,
                apoderado: false,
                filtro,
                clave_paginacion: None,
            })
            .expect("the request renders")
        };
        let fecha = |text: &str| FechaExpedicion::parse(text).expect("a real date");
        let (desde, hasta) = (fecha("02-11-2024"), fecha("13-11-2024"));
        let filtro = ConsultaFilter::new(2024, Mes::try_new(11).expect("noviembre"))
            .num_serie("T00000007")
            .ref_externa("REF-1");
        assert_eq!(filtro.fecha_expedicion, None, "new leaves it unset");
        let document = render(&filtro);
        assert!(!document.contains("FechaExpedicion"), "{document}");

        let around = |inner: &str| {
            format!(
                "<sfC:NumSerieFactura>T00000007</sfC:NumSerieFactura>\
                 <sfC:FechaExpedicionFactura>{inner}</sfC:FechaExpedicionFactura>\
                 <sfC:RefExterna>REF-1</sfC:RefExterna>"
            )
        };
        for (narrowed, inner) in [
            (
                filtro.fecha_expedicion(&desde),
                "<sum1:FechaExpedicionFactura>02-11-2024</sum1:FechaExpedicionFactura>",
            ),
            (
                filtro.rango_fecha_expedicion(Some(&desde), Some(&hasta)),
                "<sum1:RangoFechaExpedicion><sum1:Desde>02-11-2024</sum1:Desde>\
                 <sum1:Hasta>13-11-2024</sum1:Hasta></sum1:RangoFechaExpedicion>",
            ),
            (
                filtro.rango_fecha_expedicion(Some(&desde), None),
                "<sum1:RangoFechaExpedicion><sum1:Desde>02-11-2024</sum1:Desde>\
                 </sum1:RangoFechaExpedicion>",
            ),
            (
                filtro.rango_fecha_expedicion(None, Some(&hasta)),
                "<sum1:RangoFechaExpedicion><sum1:Hasta>13-11-2024</sum1:Hasta>\
                 </sum1:RangoFechaExpedicion>",
            ),
        ] {
            let document = render(&narrowed);
            assert!(document.contains(&around(inner)), "{document}");
        }
    }

    /// AEAT 1250 on both edges: a range admits `Desde` a day before
    /// `Hasta` and either end open, and refuses `Desde` on or after
    /// `Hasta`, naming the field and the rule.
    #[test]
    fn a_range_runs_forward() {
        let obligado = obligado();
        let render = |filtro: &ConsultaFilter<'_>| {
            consulta_document(&ConsultaRequest {
                obligado: &obligado,
                apoderado: false,
                filtro,
                clave_paginacion: None,
            })
        };
        let fecha = |text: &str| FechaExpedicion::parse(text).expect("a real date");
        let filtro = ConsultaFilter::new(2026, Mes::try_new(10).expect("octubre"));
        // Each pair crosses a field of the date: day, month, year.
        for (antes, despues) in [
            ("14-10-2026", "15-10-2026"),
            ("31-10-2026", "01-11-2026"),
            ("31-12-2025", "01-01-2026"),
        ] {
            let (antes, despues) = (fecha(antes), fecha(despues));
            render(&filtro.rango_fecha_expedicion(Some(&antes), Some(&despues)))
                .expect("Desde a day before Hasta");
            for (desde, hasta) in [(&antes, &antes), (&despues, &antes)] {
                let error = render(&filtro.rango_fecha_expedicion(Some(desde), Some(hasta)))
                    .expect_err("Desde on or after Hasta");
                let text = error.to_string();
                assert!(
                    text.contains("RangoFechaExpedicion") && text.contains("AEAT 1250"),
                    "{text}"
                );
            }
        }
        let dia = fecha("15-10-2026");
        render(&filtro.rango_fecha_expedicion(Some(&dia), None)).expect("open Hasta");
        render(&filtro.rango_fecha_expedicion(None, Some(&dia))).expect("open Desde");
        render(&filtro.fecha_expedicion(&dia)).expect("one day");
    }

    fn sistema(identificacion: IdentificacionDestinatario) -> SistemaInformaticoAlmacenado {
        SistemaInformaticoAlmacenado {
            nombre_razon: String::from("SOFTWARE & FACTURAS SL"),
            identificacion,
            nombre_sistema_informatico: String::from("mi-facturacion"),
            id_sistema_informatico: String::from("MF"),
            version: String::from("2.3.1"),
            numero_instalacion: String::from("000042"),
            uso_posible: UsoPosible::TambienNoVerifactu,
            tipo_uso_posible_multi_ot: false,
            indicador_multiples_ot: false,
        }
    }

    fn producer_nif() -> IdentificacionDestinatario {
        IdentificacionDestinatario::Nif(String::from("B87654321"))
    }

    fn registro(
        num_serie: &str,
        sistema_informatico: Option<SistemaInformaticoAlmacenado>,
    ) -> RegistroSpec {
        RegistroSpec {
            id_emisor_factura: String::from("12345678Z"),
            num_serie_factura: num_serie.to_owned(),
            fecha_expedicion_factura: String::from("05-10-2026"),
            estado: EstadoAlmacenado::Correcto,
            timestamp_ultima_modificacion: String::from("2026-10-05T12:00:00+02:00"),
            codigo_error: None,
            descripcion_error: None,
            tipo_factura: None,
            cuota_total: None,
            importe_total: None,
            fecha_huso_gen: None,
            huella_previa: None,
            huella: None,
            sistema_informatico,
        }
    }

    fn answer_document(registros: Vec<RegistroSpec>) -> String {
        consulta_response_document(&ConsultaSpec {
            obligado_nombre: String::from("ANA RUIZ PEREZ"),
            obligado_nif: String::from("12345678Z"),
            apoderado: false,
            ejercicio: 2026,
            mes: Mes::try_new(10).expect("octubre"),
            resultado: ResultadoConsulta::ConDatos,
            paginacion_pendiente: false,
            clave_paginacion: None,
            registros,
        })
    }

    /// The stored block reads back equal through the real read edge,
    /// for a NIF and an `IDOtro` producer, with and without `CodigoPais`,
    /// across both spellings of every S/N; a record answered without
    /// it reads `None`.
    #[test]
    fn the_stored_sistema_informatico_round_trips() {
        let nif = sistema(producer_nif());
        let otro = SistemaInformaticoAlmacenado {
            uso_posible: UsoPosible::SoloVerifactu,
            tipo_uso_posible_multi_ot: true,
            indicador_multiples_ot: true,
            ..sistema(IdentificacionDestinatario::Otro(IdOtro {
                codigo_pais: Some(String::from("FR")),
                id_type: IdOtroType::IdPaisResidencia,
                id: String::from("FR-0042"),
            }))
        };
        let nif_iva = sistema(IdentificacionDestinatario::Otro(IdOtro {
            codigo_pais: None,
            id_type: IdOtroType::NifIva,
            id: String::from("DE123456789"),
        }));
        let answer = parse_consulta(&answer_document(vec![
            registro("T00000001", Some(nif.clone())),
            registro("T00000002", Some(otro.clone())),
            registro("T00000003", Some(nif_iva.clone())),
            registro("T00000004", None),
        ]))
        .expect("the respuesta parses");
        let read: Vec<_> = answer
            .registros
            .into_iter()
            .map(|registro| registro.sistema_informatico)
            .collect();
        assert_eq!(read, vec![Some(nif), Some(otro), Some(nif_iva), None]);
    }

    /// Every `SistemaInformaticoType` child is required: a block missing
    /// one, carrying both or neither identification, or holding an
    /// off-vocabulary code refuses the answer. The empty element is
    /// admitted as absent.
    #[test]
    fn a_partial_sistema_informatico_block_is_refused() {
        let document = answer_document(vec![registro("T00000001", Some(sistema(producer_nif())))]);
        let block = document
            .find("<sfRC:SistemaInformatico>")
            .expect("the block renders");
        let without = |document: &str, child: &str| {
            let (open, close) = (format!("<sum1:{child}>"), format!("</sum1:{child}>"));
            let start = block + document[block..].find(&open).expect("the child renders");
            let end = start + document[start..].find(&close).expect("closed") + close.len();
            let mut cut = document.to_owned();
            cut.replace_range(start..end, "");
            cut
        };
        for child in [
            "NombreRazon",
            "NIF",
            "NombreSistemaInformatico",
            "IdSistemaInformatico",
            "Version",
            "NumeroInstalacion",
            "TipoUsoPosibleSoloVerifactu",
            "TipoUsoPosibleMultiOT",
            "IndicadorMultiplesOT",
        ] {
            let error =
                parse_consulta(&without(&document, child)).expect_err("a missing child refuses");
            assert!(error.contains(child), "{child}: {error}");
        }

        let otro = answer_document(vec![registro(
            "T00000001",
            Some(sistema(IdentificacionDestinatario::Otro(IdOtro {
                codigo_pais: Some(String::from("FR")),
                id_type: IdOtroType::Pasaporte,
                id: String::from("12AB34567"),
            }))),
        )]);
        for child in ["IDType", "ID"] {
            let error = parse_consulta(&without(&otro, child)).expect_err("IDOtro is incomplete");
            assert!(error.contains(child), "{child}: {error}");
        }
        let error = parse_consulta(&otro.replace(
            "<sum1:IDType>03</sum1:IDType>",
            "<sum1:IDType>01</sum1:IDType>",
        ))
        .expect_err("01 is not a PersonaFisicaJuridicaIDTypeType");
        assert!(error.contains("IDType"), "{error}");

        let both = document.replace(
            "<sum1:NIF>B87654321</sum1:NIF>",
            "<sum1:NIF>B87654321</sum1:NIF><sum1:IDOtro><sum1:IDType>02</sum1:IDType>\
             <sum1:ID>DE123456789</sum1:ID></sum1:IDOtro>",
        );
        assert!(
            parse_consulta(&both).is_err(),
            "NIF and IDOtro are a choice"
        );
        for (child, wire) in [
            ("TipoUsoPosibleSoloVerifactu", "N"),
            ("TipoUsoPosibleMultiOT", "N"),
            ("IndicadorMultiplesOT", "N"),
        ] {
            let off = document.replace(
                &format!("<sum1:{child}>{wire}</sum1:{child}>"),
                &format!("<sum1:{child}>X</sum1:{child}>"),
            );
            let error = parse_consulta(&off).expect_err("S/N is closed");
            assert!(error.contains(child), "{child}: {error}");
        }

        let close = "</sfRC:SistemaInformatico>";
        let end = document.find(close).expect("closed") + close.len();
        let mut empty = document.clone();
        empty.replace_range(block..end, "<sfRC:SistemaInformatico/>");
        let answer = parse_consulta(&empty).expect("the empty element is admitted");
        assert_eq!(answer.registros[0].sistema_informatico, None);
    }
}
