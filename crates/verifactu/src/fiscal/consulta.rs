//! The consulta operation (`ConsultaFactuSistemaFacturacion`, WSDL's
//! `sfVerifactu` binding — the SAME `VerifactuSOAP` door as remission,
//! empty `soapAction`): AEAT's read-back of the records it stores, per
//! `ConsultaLR.xsd` + `RespuestaConsultaLR.xsd` (pinned under
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

use std::sync::Arc;

use crate::domain::chain::{huella_alta_montos, FechaExpedicion, TipoFactura};
use crate::domain::money::Money;
use crate::fiscal::response::classify_fault;
use crate::fiscal::transport::TransportOutcome;
use crate::fiscal::xml::{self, tag, Tree};
use rust_decimal::Decimal;

use crate::fiscal::{EmitError, Obligado};

const NS_CONSULTA: &str = "https://www2.agenciatributaria.gob.es/static_files/common/internet/dep/aplicaciones/es/aeat/tike/cont/ws/ConsultaLR.xsd";
const NS_RESPUESTA_CONSULTA: &str = "https://www2.agenciatributaria.gob.es/static_files/common/internet/dep/aplicaciones/es/aeat/tike/cont/ws/RespuestaConsultaLR.xsd";
const P_C: &str = "sfC";
const P_RC: &str = "sfRC";

/// The `PeriodoImputacion` month (`TipoPeriodoType`: `01`..`12` —
/// the consulta knows no annual/quarterly codes).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Mes(u8);

impl Mes {
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

/// The query window + narrowing keys (`LRFiltroRegFacturacionType`).
/// The period is the obligado's `PeriodoImputacion` — where the record
/// LANDS, which for an emitted factura is the expedition month.
#[derive(Clone, Copy, Debug)]
pub struct ConsultaFilter<'a> {
    pub ejercicio: u16,
    pub mes: Mes,
    pub num_serie: Option<&'a str>,
    pub ref_externa: Option<&'a str>,
}

impl<'a> ConsultaFilter<'a> {
    #[must_use]
    pub fn new(ejercicio: u16, mes: Mes) -> Self {
        Self {
            ejercicio,
            mes,
            num_serie: None,
            ref_externa: None,
        }
    }

    /// One factura's `NumSerieFactura` (1..60 chars).
    #[must_use]
    pub fn num_serie(mut self, num_serie: &'a str) -> Self {
        self.num_serie = Some(num_serie);
        self
    }

    #[must_use]
    pub fn ref_externa(mut self, ref_externa: &'a str) -> Self {
        self.ref_externa = Some(ref_externa);
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
}

/// The STORED record's estado (`EstadoRegistroType` of the consulta
/// respuesta — a different vocabulary from the submission line's).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
}

fn parse_importe(text: &str) -> Option<Money> {
    let value = text.parse::<Decimal>().ok()?;
    if value.scale() <= 2 {
        Some(Money::from_decimal(value))
    } else {
        None
    }
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
/// violated (NIF, text lengths, the period codes).
pub(crate) fn consulta_document(request: &ConsultaRequest<'_>) -> Result<String, EmitError> {
    let invalid = |detail: String| EmitError::InvalidRecord { detail };
    xml::check_nif("Consulta/ObligadoEmision/NIF", &request.obligado.nif)
        .map_err(|error| invalid(error.to_string()))?;
    xml::check_text(
        "Consulta/ObligadoEmision/NombreRazon",
        &request.obligado.nombre_razon,
        120,
    )
    .map_err(|error| invalid(error.to_string()))?;
    if !(1000..=9999).contains(&request.filtro.ejercicio) {
        return Err(invalid(format!(
            "Ejercicio is a 4-digit year, got {}",
            request.filtro.ejercicio
        )));
    }
    if let Some(num_serie) = request.filtro.num_serie {
        // TextoIDFacturaType carries minLength 1 — an empty narrowing
        // element is off-contract, not "no narrowing".
        if num_serie.is_empty() {
            return Err(invalid(String::from(
                "Consulta/NumSerieFactura is 1..60 chars (TextoIDFacturaType minLength 1), \
                 got empty",
            )));
        }
        xml::check_text("Consulta/NumSerieFactura", num_serie, 60)
            .map_err(|error| invalid(error.to_string()))?;
    }
    if let Some(ref_externa) = request.filtro.ref_externa {
        xml::check_text("Consulta/RefExterna", ref_externa, 60)
            .map_err(|error| invalid(error.to_string()))?;
    }
    if let Some(clave) = request.clave_paginacion {
        xml::check_nif(
            "Consulta/ClavePaginacion/IDEmisorFactura",
            &clave.id_emisor_factura,
        )
        .map_err(|error| invalid(error.to_string()))?;
        // TextoIDFacturaType carries minLength 1 here too.
        if clave.num_serie_factura.is_empty() {
            return Err(invalid(String::from(
                "Consulta/ClavePaginacion/NumSerieFactura is 1..60 chars \
                 (TextoIDFacturaType minLength 1), got empty",
            )));
        }
        xml::check_text(
            "Consulta/ClavePaginacion/NumSerieFactura",
            &clave.num_serie_factura,
            60,
        )
        .map_err(|error| invalid(error.to_string()))?;
        if FechaExpedicion::parse(&clave.fecha_expedicion_factura).is_none() {
            return Err(invalid(String::from(
                "Consulta/ClavePaginacion/FechaExpedicionFactura is not dd-mm-yyyy",
            )));
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
    out.push_str("</sfC:ConsultaFactuSistemaFacturacion>");
    Ok(out)
}

// ---- parse -------------------------------------------------------------

/// The consulta read edge (the twin of `parse_soap_answer`'s
/// submission half): a SOAP envelope or the bare respuesta root.
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
    })
}

// ---- the wire ----------------------------------------------------------

/// One consulta envío through any transport.
///
/// # Errors
/// [`EmitError::InvalidRecord`] off-contract; [`EmitError::Transport`]
/// on the wire leg or a submission-shaped answer to a consulta;
/// [`EmitError::FaultClient`]/[`EmitError::FaultServer`] on a SOAP
/// fault.
pub async fn send_once(
    transport: &Arc<dyn crate::fiscal::transport::VerifactuTransport>,
    request: &ConsultaRequest<'_>,
) -> Result<ConsultaAnswer, EmitError> {
    let body = consulta_document(request)?;
    let envelope = xml::soap_envelope(&body);
    match transport.send(&envelope).await {
        Err(failure) => Err(EmitError::Transport {
            detail: failure.detail,
        }),
        Ok(TransportOutcome::Fault(fault)) => Err(match classify_fault(&fault) {
            crate::domain::error::ErrorClass::Transient => EmitError::FaultServer {
                faultstring: fault.faultstring,
            },
            _ => EmitError::FaultClient {
                faultstring: fault.faultstring,
            },
        }),
        Ok(TransportOutcome::Consulta(answer)) => Ok(answer),
        Ok(TransportOutcome::Response(_)) => Err(EmitError::Transport {
            detail: String::from("AEAT answered a submission respuesta to a consulta"),
        }),
    }
}

/// The product door: every page, merged, in call order. The consulta
/// modality's own transport answer rides [`Modality`] configuration —
/// consultas always dial the `VerifactuSOAP` family.
///
/// # Errors
/// [`EmitError`] from [`send_once`]; a `Transport` error when AEAT
/// signals more pages without a pagination key, or the pages exceed
/// 100 (a bug class, never an input).
pub async fn send_all(
    transport: &Arc<dyn crate::fiscal::transport::VerifactuTransport>,
    obligado: &Obligado,
    apoderado: bool,
    filtro: &ConsultaFilter<'_>,
) -> Result<ConsultaAnswer, EmitError> {
    let mut clave: Option<ClavePaginacion> = None;
    let mut registros = Vec::new();
    let mut resultado;
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
        resultado = page.resultado;
        registros.extend(page.registros);
        if !page.paginacion_pendiente {
            return Ok(ConsultaAnswer {
                resultado,
                paginacion_pendiente: false,
                clave_paginacion: None,
                registros,
            });
        }
        clave = page.clave_paginacion;
        if clave.is_none() {
            return Err(EmitError::Transport {
                detail: String::from(
                    "AEAT signaled more pages (IndicadorPaginacion=S) without a ClavePaginacion",
                ),
            });
        }
    }
    Err(EmitError::Transport {
        detail: String::from("consulta exceeded 100 pages — a bug class, never an input"),
    })
}

// ---- the scripted-answer half (tests + the fake transport) -------------

/// The respuesta document builder — the consulta twin of
/// `transport::respuesta_document`: every AEAT-vocabulary field typed.
#[derive(Clone, Debug)]
pub struct ConsultaSpec {
    pub obligado_nombre: String,
    pub obligado_nif: String,
    pub apoderado: bool,
    pub ejercicio: u16,
    pub mes: Mes,
    pub resultado: ResultadoConsulta,
    pub paginacion_pendiente: bool,
    pub clave_paginacion: Option<ClavePaginacion>,
    pub registros: Vec<RegistroSpec>,
}

#[derive(Clone, Debug)]
pub struct RegistroSpec {
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
    /// `EncadenamientoFacturaAnteriorType` demands the predecessor's
    /// whole `IDFactura` + `Huella` — the scripted fixtures stay
    /// XSD-valid.
    pub huella_previa: Option<EncadenamientoSpec>,
    pub huella: Option<String>,
}

/// The `RegistroAnterior` block's full shape (SI.xsd).
#[derive(Clone, Debug)]
pub struct EncadenamientoSpec {
    pub num_serie_factura: String,
    pub fecha_expedicion_factura: String,
    pub huella: String,
}

fn render_registro(out: &mut String, registro: &RegistroSpec) {
    // The element NAME is RespuestaConsultaLR.xsd's declaration; the
    // DatosRegistroFacturacion children are declared there too (its
    // type is RC-local), so they carry sfRC — only the
    // RegistroAnterior's children ride the SI-named
    // EncadenamientoFacturaAnteriorType.
    out.push_str("<sfRC:RegistroRespuestaConsultaFactuSistemaFacturacion>");
    out.push_str("<sfRC:IDFactura>");
    tag(out, "sum1:IDEmisorFactura", &registro.id_emisor_factura);
    tag(out, "sum1:NumSerieFactura", &registro.num_serie_factura);
    tag(
        out,
        "sum1:FechaExpedicionFactura",
        &registro.fecha_expedicion_factura,
    );
    out.push_str("</sfRC:IDFactura>");
    out.push_str("<sfRC:DatosRegistroFacturacion>");
    if let Some(tipo) = registro.tipo_factura {
        tag(out, "sfRC:TipoFactura", tipo.as_str());
    }
    if let Some(cuota) = registro.cuota_total {
        tag(
            out,
            "sfRC:CuotaTotal",
            &crate::domain::chain::render_amount(cuota),
        );
    }
    if let Some(importe) = registro.importe_total {
        tag(
            out,
            "sfRC:ImporteTotal",
            &crate::domain::chain::render_amount(importe),
        );
    }
    out.push_str("<sfRC:Encadenamiento>");
    match &registro.huella_previa {
        None => out.push_str("<sfRC:PrimerRegistro>S</sfRC:PrimerRegistro>"),
        Some(anterior) => {
            out.push_str("<sum1:RegistroAnterior>");
            tag(out, "sum1:IDEmisorFactura", &registro.id_emisor_factura);
            tag(out, "sum1:NumSerieFactura", &anterior.num_serie_factura);
            tag(
                out,
                "sum1:FechaExpedicionFactura",
                &anterior.fecha_expedicion_factura,
            );
            tag(out, "sum1:Huella", &anterior.huella);
            out.push_str("</sum1:RegistroAnterior>");
        }
    }
    out.push_str("</sfRC:Encadenamiento>");
    if let Some(huso) = &registro.fecha_huso_gen {
        tag(out, "sfRC:FechaHoraHusoGenRegistro", huso);
    }
    tag(out, "sfRC:Huella", registro.huella.as_deref().unwrap_or(""));
    out.push_str("</sfRC:DatosRegistroFacturacion>");
    out.push_str("<sfRC:EstadoRegistro>");
    tag(
        out,
        "sfRC:TimestampUltimaModificacion",
        &registro.timestamp_ultima_modificacion,
    );
    tag(out, "sfRC:EstadoRegistro", registro.estado.as_str());
    if let Some(codigo) = registro.codigo_error {
        tag(out, "sfRC:CodigoErrorRegistro", &codigo.to_string());
    }
    if let Some(descripcion) = &registro.descripcion_error {
        tag(out, "sfRC:DescripcionErrorRegistro", descripcion);
    }
    out.push_str("</sfRC:EstadoRegistro>");
    out.push_str("</sfRC:RegistroRespuestaConsultaFactuSistemaFacturacion>");
}

#[must_use]
pub fn consulta_response_document(spec: &ConsultaSpec) -> String {
    let mut out = String::with_capacity(1024);
    out.push('<');
    out.push_str(P_RC);
    out.push_str(":RespuestaConsultaFactuSistemaFacturacion xmlns:sfRC=\"");
    out.push_str(NS_RESPUESTA_CONSULTA);
    out.push_str("\" xmlns:sum1=\"");
    out.push_str(xml::NS_SUMINISTRO);
    out.push_str("\">");
    out.push_str("<sfRC:Cabecera>");
    tag(&mut out, "sum1:IDVersion", xml::ID_VERSION);
    out.push_str("<sum1:ObligadoEmision>");
    tag(&mut out, "sum1:NombreRazon", &spec.obligado_nombre);
    tag(&mut out, "sum1:NIF", &spec.obligado_nif);
    out.push_str("</sum1:ObligadoEmision>");
    if spec.apoderado {
        tag(&mut out, "sum1:IndicadorRepresentante", "S");
    }
    out.push_str("</sfRC:Cabecera>");
    // Unlike the request's SI-named PeriodoImputacionType, the
    // respuesta's is an inline RC declaration — its children are RC.
    out.push_str("<sfRC:PeriodoImputacion>");
    tag(&mut out, "sfRC:Ejercicio", &spec.ejercicio.to_string());
    tag(&mut out, "sfRC:Periodo", &spec.mes.as_codigo());
    out.push_str("</sfRC:PeriodoImputacion>");
    tag(
        &mut out,
        "sfRC:IndicadorPaginacion",
        if spec.paginacion_pendiente { "S" } else { "N" },
    );
    tag(
        &mut out,
        "sfRC:ResultadoConsulta",
        match spec.resultado {
            ResultadoConsulta::ConDatos => "ConDatos",
            ResultadoConsulta::SinDatos => "SinDatos",
        },
    );
    for registro in &spec.registros {
        render_registro(&mut out, registro);
    }
    if let Some(clave) = &spec.clave_paginacion {
        out.push_str("<sfRC:ClavePaginacion>");
        tag(&mut out, "sum1:IDEmisorFactura", &clave.id_emisor_factura);
        tag(&mut out, "sum1:NumSerieFactura", &clave.num_serie_factura);
        tag(
            &mut out,
            "sum1:FechaExpedicionFactura",
            &clave.fecha_expedicion_factura,
        );
        out.push_str("</sfRC:ClavePaginacion>");
    }
    out.push_str("</sfRC:RespuestaConsultaFactuSistemaFacturacion>");
    out
}

#[cfg(test)]
mod tests {
    use super::{
        consulta_document, consulta_response_document, parse_consulta, ConsultaFilter,
        ConsultaRequest, ConsultaSpec, EstadoAlmacenado, Mes, RegistroSpec, ResultadoConsulta,
    };
    use crate::domain::chain::huella_alta_montos;
    use crate::domain::money::Money;
    use crate::fiscal::Obligado;

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
        assert!(error.to_string().contains("minLength 1"));
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
            }],
        };
        let answer =
            parse_consulta(&consulta_response_document(&spec)).expect("the respuesta parses");
        assert_eq!(answer.registros[0].huella_verifica(), Some(false));
    }
}
