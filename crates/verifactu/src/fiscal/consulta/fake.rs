//! The scripted consulta answers the fake transport renders — test
//! tooling (feature `fakes`).

use super::{ClavePaginacion, EstadoAlmacenado, Mes, ResultadoConsulta};
use crate::domain::chain::TipoFactura;
use crate::domain::money::Money;
use crate::fiscal::xml::{self, tag};

const NS_RESPUESTA_CONSULTA: &str = "https://www2.agenciatributaria.gob.es/static_files/common/internet/dep/aplicaciones/es/aeat/tike/cont/ws/RespuestaConsultaLR.xsd";
const P_RC: &str = "sfRC";

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
    // type is RC-local), so they carry sfRC — RegistroAnterior
    // included; only ITS children ride the SI-named
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
            out.push_str("<sfRC:RegistroAnterior>");
            tag(out, "sum1:IDEmisorFactura", &registro.id_emisor_factura);
            tag(out, "sum1:NumSerieFactura", &anterior.num_serie_factura);
            tag(
                out,
                "sum1:FechaExpedicionFactura",
                &anterior.fecha_expedicion_factura,
            );
            tag(out, "sum1:Huella", &anterior.huella);
            out.push_str("</sfRC:RegistroAnterior>");
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
    tag(&mut out, "sfRC:ResultadoConsulta", spec.resultado.as_str());
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
