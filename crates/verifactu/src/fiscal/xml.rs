//! The `Veri*FACTU` wire serializer. Namespaces: wrapper elements live
//! in SLR, record/payload elements in SI; each record root re-declares
//! its own `xmlns:sum1` (AEAT's own signed example's shape) so a signed
//! record stays digest-stable under DOM extraction at AEAT.

use crate::domain::chain::{render_amount, ChainKind, ChainRecord, TipoFactura};
use crate::domain::money::Money;
use crate::domain::series;

use crate::fiscal::{
    DetalleDesglose, EmissionContext, IdentificacionDestinatario, Impuesto, Obligado,
    SistemaInformaticoConfig, TipoRectificativa,
};

pub(crate) const NS_SUMINISTRO: &str = "https://www2.agenciatributaria.gob.es/static_files/common/internet/dep/aplicaciones/es/aeat/tike/cont/ws/SuministroInformacion.xsd";
const NS_LR: &str = "https://www2.agenciatributaria.gob.es/static_files/common/internet/dep/aplicaciones/es/aeat/tike/cont/ws/SuministroLR.xsd";
pub(crate) const NS_RESPUESTA: &str = "https://www2.agenciatributaria.gob.es/static_files/common/internet/dep/aplicaciones/es/aeat/tike/cont/ws/RespuestaSuministro.xsd";
pub(crate) const NS_SOAP_ENV: &str = "http://schemas.xmlsoap.org/soap/envelope/";

/// AEAT's own signed example's SI prefix.
const P_SF: &str = "sum1";
const P_LR: &str = "sumLR";
pub(crate) const P_RS: &str = "sfR";
pub(crate) const P_ENV: &str = "soapenv";

/// The only value `VersionType` admits.
pub(crate) const ID_VERSION: &str = "1.0";
/// `01` = SHA-256, the only admitted `TipoHuella` (HS §2).
const TIPO_HUELLA: &str = "01";

/// Past ~40k nesting the tree's recursive `Drop` would abort the process
/// (fuzz-found); 16 is already far past AEAT's ≤6-deep documents.
const MAX_ELEMENT_DEPTH: usize = 16;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SerializeError {
    #[error("field {field}: {problem}")]
    Field {
        field: &'static str,
        problem: String,
    },
    #[error("cardinality of {element}: {problem}")]
    Cardinality {
        element: &'static str,
        problem: String,
    },
}

/// A literal `\r` is normalized to `\n` by any XML processor (W3C XML 1.0
/// §2.11); only the `&#xD;` character reference survives a DOM round-trip
/// at AEAT byte-for-byte, so it is the only CR spelling signed text (and
/// the huella recompute) can carry.
pub(crate) fn escape_text(input: &str) -> std::borrow::Cow<'_, str> {
    if !input
        .bytes()
        .any(|b| matches!(b, b'&' | b'<' | b'>' | b'\r'))
    {
        return std::borrow::Cow::Borrowed(input);
    }
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '\r' => out.push_str("&#xD;"),
            _ => out.push(ch),
        }
    }
    std::borrow::Cow::Owned(out)
}

fn integer_digits(rendered: &str) -> usize {
    rendered
        .split('.')
        .next()
        .unwrap_or(rendered)
        .trim_start_matches(['+', '-'])
        .chars()
        .count()
}

/// Rejects before a byte renders: originals are immutable once committed
/// — an AEAT rejection of the shape post-commit has no remedy.
fn check_importe_sgn_12_2(field: &'static str, amount: Money) -> Result<(), SerializeError> {
    let rendered = render_amount(amount);
    let digits = integer_digits(&rendered);
    if digits > 12 {
        return Err(SerializeError::Field {
            field,
            problem: format!(
                "rendered {rendered}: {digits} integer digits exceed ImporteSgn12.2Type's 12"
            ),
        });
    }
    Ok(())
}

/// The integer-digit half only; the sign half lives in the caller.
fn check_tipo_2_2(field: &'static str, rate: Money) -> Result<(), SerializeError> {
    let rendered = render_amount(rate);
    let digits = integer_digits(&rendered);
    if digits > 3 {
        return Err(SerializeError::Field {
            field,
            problem: format!(
                "rendered {rendered}: {digits} integer digits exceed Tipo2.2Type's 3 (max 999)"
            ),
        });
    }
    Ok(())
}

pub(crate) fn tag(out: &mut String, name: &str, text: &str) {
    out.push('<');
    out.push_str(name);
    out.push('>');
    out.push_str(&escape_text(text));
    out.push_str("</");
    out.push_str(name);
    out.push('>');
}

/// Validated once at emitter construction, not re-validated here.
fn sistema_informatico(sif: &SistemaInformaticoConfig) -> String {
    let mut out = String::new();
    out.push('<');
    out.push_str(P_SF);
    out.push_str(":SistemaInformatico>");
    tag(&mut out, "sum1:NombreRazon", &sif.nombre_razon);
    tag(&mut out, "sum1:NIF", &sif.nif);
    tag(
        &mut out,
        "sum1:NombreSistemaInformatico",
        &sif.nombre_sistema_informatico,
    );
    tag(
        &mut out,
        "sum1:IdSistemaInformatico",
        &sif.id_sistema_informatico,
    );
    tag(&mut out, "sum1:Version", &sif.version);
    tag(&mut out, "sum1:NumeroInstalacion", &sif.numero_instalacion);
    tag(
        &mut out,
        "sum1:TipoUsoPosibleSoloVerifactu",
        TIPO_USO_POSIBLE_SOLO_VERIFACTU,
    );
    tag(
        &mut out,
        "sum1:TipoUsoPosibleMultiOT",
        TIPO_USO_POSIBLE_MULTI_OT,
    );
    tag(
        &mut out,
        "sum1:IndicadorMultiplesOT",
        INDICADOR_MULTIPLES_OT,
    );
    out.push_str("</sum1:SistemaInformatico>");
    out
}

/// `S`, FIXED BY LAW: the field's axis is the PRODUCT FAMILY, never
/// remission-vs-conservation (both are Veri*FACTU modalities).
const TIPO_USO_POSIBLE_SOLO_VERIFACTU: &str = "S";

/// `N`, FIXED BY LAW: one obligado tributario per installation.
const TIPO_USO_POSIBLE_MULTI_OT: &str = "N";

/// `N`, FIXED BY LAW: single-OT site.
const INDICADOR_MULTIPLES_OT: &str = "N";

/// Every `RegistroAnterior` field rides the record's seal-populated
/// `prev` — wire and sealed reference cannot disagree.
fn encadenamiento(record: &ChainRecord) -> Result<String, SerializeError> {
    let mut out = String::from("<sum1:Encadenamiento>");
    match &record.prev {
        None => tag(&mut out, "sum1:PrimerRegistro", "S"),
        Some(prev) => {
            check_nif("RegistroAnterior/IDEmisorFactura", &prev.issuer)?;
            let num_serie = series::format(&prev.serie, prev.number);
            check_num_serie("RegistroAnterior/NumSerieFactura", &num_serie)?;
            out.push_str("<sum1:RegistroAnterior>");
            tag(&mut out, "sum1:IDEmisorFactura", &prev.issuer);
            tag(&mut out, "sum1:NumSerieFactura", &num_serie);
            tag(
                &mut out,
                "sum1:FechaExpedicionFactura",
                prev.fecha_expedicion.as_str(),
            );
            tag(&mut out, "sum1:Huella", &prev.huella);
            out.push_str("</sum1:RegistroAnterior>");
        }
    }
    out.push_str("</sum1:Encadenamiento>");
    Ok(out)
}

/// `NIFType` is any 9 characters; every NIF, NIE and CIF is 9 ASCII
/// alphanumerics — anything else (edge whitespace the huella cadena
/// would trim, a non-ASCII character AEAT's validator counts twice) can
/// only be a typo, refused before it is hashed.
pub(crate) fn check_nif(field: &'static str, nif: &str) -> Result<(), SerializeError> {
    if nif.len() == 9 && nif.bytes().all(|byte| byte.is_ascii_alphanumeric()) {
        return Ok(());
    }
    Err(SerializeError::Field {
        field,
        problem: format!("a NIF is 9 ASCII alphanumerics (NIFType), got {nif:?}"),
    })
}

/// XSD `maxLength` counts characters (Unicode scalar values) — an emoji
/// is one. Whether AEAT's Java validator agrees for supplementary-plane
/// characters (Java strings count UTF-16 units, where an emoji is two)
/// is unverified: a ceiling-length name carrying an emoji at pruebas
/// settles it.
fn check_max(field: &'static str, value: &str, max: usize) -> Result<(), SerializeError> {
    let count = value.chars().count();
    if count > max {
        return Err(SerializeError::Field {
            field,
            problem: format!("max {max} chars, got {count}"),
        });
    }
    Ok(())
}

/// Characters outside XML 1.0's `Char` production (the C0 controls but
/// `#x9 | #xA | #xD`, `U+FFFE`, `U+FFFF`) cannot be escaped into XML
/// validity; the predicate is the read side's [`is_char_code`] — one
/// `Char` law for both edges.
fn check_xml_chars(field: &'static str, value: &str) -> Result<(), SerializeError> {
    match value.chars().find(|ch| !is_char_code(u32::from(*ch))) {
        Some(offender) => Err(SerializeError::Field {
            field,
            problem: format!("character {offender:?} is not legal XML 1.0 (Char production)"),
        }),
        None => Ok(()),
    }
}

pub(crate) fn check_text(
    field: &'static str,
    value: &str,
    max: usize,
) -> Result<(), SerializeError> {
    check_max(field, value, max)?;
    check_xml_chars(field, value)
}

/// `NumSerieFactura` (`TextoIDFacturaType`, 1..=60): printable ASCII
/// only — the printed QR carries it, and the QR law admits ASCII
/// 32..=126 — and no edge whitespace, because the huella cadena trims
/// it while the wire renders it verbatim.
pub(crate) fn check_num_serie(field: &'static str, value: &str) -> Result<(), SerializeError> {
    let problem = if let Some(offender) = value.chars().find(|ch| !matches!(ch, ' '..='~')) {
        format!("{offender:?} is not printable ASCII (32..=126)")
    } else if value.is_empty() || value.len() > 60 {
        format!("must be 1..=60 chars, got {}", value.len())
    } else if value.trim() != value {
        format!("{value:?} carries edge whitespace the huella would trim")
    } else {
        return Ok(());
    };
    Err(SerializeError::Field { field, problem })
}

/// SI.xsd's `CountryType2` enumeration, sorted (ISO 3166-1 alpha-2 as
/// AEAT admits it) — `xsd_enumerations` pins it to the schema.
pub const CODIGOS_PAIS: [&str; 246] = [
    "AD", "AE", "AF", "AG", "AI", "AL", "AM", "AO", "AQ", "AR", "AS", "AT", "AU", "AW", "AZ", "BA",
    "BB", "BD", "BE", "BF", "BG", "BH", "BI", "BJ", "BM", "BN", "BO", "BQ", "BR", "BS", "BT", "BV",
    "BW", "BY", "BZ", "CA", "CC", "CD", "CF", "CG", "CH", "CI", "CK", "CL", "CM", "CN", "CO", "CR",
    "CU", "CV", "CW", "CX", "CY", "CZ", "DE", "DJ", "DK", "DM", "DO", "DZ", "EC", "EE", "EG", "ER",
    "ES", "ET", "FI", "FJ", "FK", "FM", "FO", "FR", "GA", "GB", "GD", "GE", "GG", "GH", "GI", "GL",
    "GM", "GN", "GQ", "GR", "GS", "GT", "GU", "GW", "GY", "HK", "HM", "HN", "HR", "HT", "HU", "ID",
    "IE", "IL", "IM", "IN", "IO", "IQ", "IR", "IS", "IT", "JE", "JM", "JO", "JP", "KE", "KG", "KH",
    "KI", "KM", "KN", "KP", "KR", "KW", "KY", "KZ", "LA", "LB", "LC", "LI", "LK", "LR", "LS", "LT",
    "LU", "LV", "LY", "MA", "MC", "MD", "ME", "MG", "MH", "MK", "ML", "MM", "MN", "MO", "MP", "MR",
    "MS", "MT", "MU", "MV", "MW", "MX", "MY", "MZ", "NA", "NC", "NE", "NF", "NG", "NI", "NL", "NO",
    "NP", "NR", "NU", "NZ", "OM", "PA", "PE", "PF", "PG", "PH", "PK", "PL", "PM", "PN", "PR", "PS",
    "PT", "PW", "PY", "QA", "QU", "RE", "RO", "RS", "RU", "RW", "SA", "SB", "SC", "SD", "SE", "SG",
    "SH", "SI", "SK", "SL", "SM", "SN", "SO", "SR", "SS", "ST", "SV", "SX", "SY", "SZ", "TC", "TD",
    "TF", "TG", "TH", "TJ", "TK", "TL", "TM", "TN", "TO", "TR", "TT", "TV", "TW", "TZ", "UA", "UG",
    "UM", "US", "UY", "UZ", "VA", "VC", "VE", "VG", "VI", "VN", "VU", "WF", "WS", "XB", "XG", "XN",
    "XU", "YE", "YT", "ZA", "ZM", "ZW",
];

/// Serializes one `RegistroAlta` as a standalone document — the signer's
/// signing unit (AEAT's own signed example is exactly this document).
///
/// # Panics
/// Panics through [`series::format`] when the correlative exceeds the
/// 8-digit capacity.
#[allow(clippy::too_many_lines)]
pub(crate) fn registro_alta(
    record: &ChainRecord,
    ctx: &EmissionContext<'_>,
    obligado: &Obligado,
    sif: &SistemaInformaticoConfig,
) -> Result<String, SerializeError> {
    let ChainKind::Alta {
        issuer,
        serie,
        number,
        fecha_expedicion,
        tipo_factura,
        cuota_total,
        importe_total,
        fecha_huso_gen,
    } = &record.kind
    else {
        return Err(SerializeError::Field {
            field: "RegistroAlta",
            problem: String::from("record kind is not Alta"),
        });
    };
    check_alta(
        ctx,
        obligado,
        issuer,
        *tipo_factura,
        *cuota_total,
        *importe_total,
    )?;
    let num_serie = series::format(serie, *number);
    check_num_serie("IDFactura/NumSerieFactura", &num_serie)?;
    let descripcion = ctx
        .descripcion_operacion
        .expect("check_alta established the descripcion");

    let mut out = String::with_capacity(2048);
    out.push_str("<sum1:RegistroAlta xmlns:sum1=\"");
    out.push_str(NS_SUMINISTRO);
    out.push_str("\">");
    tag(&mut out, "sum1:IDVersion", ID_VERSION);
    out.push_str("<sum1:IDFactura>");
    tag(&mut out, "sum1:IDEmisorFactura", issuer);
    tag(&mut out, "sum1:NumSerieFactura", &num_serie);
    tag(
        &mut out,
        "sum1:FechaExpedicionFactura",
        fecha_expedicion.as_str(),
    );
    out.push_str("</sum1:IDFactura>");
    if let Some(ref_ext) = ctx.ref_externa {
        tag(&mut out, "sum1:RefExterna", ref_ext);
    }
    tag(&mut out, "sum1:NombreRazonEmisor", &obligado.nombre_razon);
    if let Some(subsanacion) = ctx.subsanacion {
        tag(&mut out, "sum1:Subsanacion", subsanacion.wire());
    }
    if let Some(rechazo) = ctx.rechazo_previo {
        tag(&mut out, "sum1:RechazoPrevio", rechazo.wire());
    }
    tag(&mut out, "sum1:TipoFactura", tipo_factura.as_str());
    if let Some(tipo_rect) = ctx.tipo_rectificativa {
        tag(&mut out, "sum1:TipoRectificativa", tipo_rect.wire());
        out.push_str("<sum1:FacturasRectificadas>");
        for rectificada in ctx.facturas_rectificadas {
            out.push_str("<sum1:IDFacturaRectificada>");
            tag(&mut out, "sum1:IDEmisorFactura", &rectificada.nif);
            tag(&mut out, "sum1:NumSerieFactura", &rectificada.num_serie);
            tag(
                &mut out,
                "sum1:FechaExpedicionFactura",
                rectificada.fecha_expedicion.as_str(),
            );
            out.push_str("</sum1:IDFacturaRectificada>");
        }
        out.push_str("</sum1:FacturasRectificadas>");
    }
    if let Some(rectificacion) = &ctx.importe_rectificacion {
        out.push_str("<sum1:ImporteRectificacion>");
        tag(
            &mut out,
            "sum1:BaseRectificada",
            &render_amount(rectificacion.base_rectificada),
        );
        tag(
            &mut out,
            "sum1:CuotaRectificada",
            &render_amount(rectificacion.cuota_rectificada),
        );
        if let Some(recargo) = rectificacion.cuota_recargo_rectificado {
            tag(
                &mut out,
                "sum1:CuotaRecargoRectificado",
                &render_amount(recargo),
            );
        }
        out.push_str("</sum1:ImporteRectificacion>");
    }
    tag(&mut out, "sum1:DescripcionOperacion", descripcion);
    if let Some(macrodato) = ctx.macrodato {
        tag(&mut out, "sum1:Macrodato", macrodato.wire());
    }
    if !ctx.destinatarios.is_empty() {
        out.push_str("<sum1:Destinatarios>");
        for destinatario in ctx.destinatarios {
            out.push_str("<sum1:IDDestinatario>");
            tag(&mut out, "sum1:NombreRazon", &destinatario.nombre_razon);
            match &destinatario.identificacion {
                IdentificacionDestinatario::Nif(nif) => {
                    tag(&mut out, "sum1:NIF", nif);
                }
                IdentificacionDestinatario::Otro(otro) => {
                    out.push_str("<sum1:IDOtro>");
                    if let Some(pais) = &otro.codigo_pais {
                        tag(&mut out, "sum1:CodigoPais", pais);
                    }
                    tag(&mut out, "sum1:IDType", otro.id_type.wire());
                    tag(&mut out, "sum1:ID", &otro.id);
                    out.push_str("</sum1:IDOtro>");
                }
            }
            out.push_str("</sum1:IDDestinatario>");
        }
        out.push_str("</sum1:Destinatarios>");
    }
    out.push_str("<sum1:Desglose>");
    for detalle in ctx.desglose {
        out.push_str(&detalle_desglose(detalle)?);
    }
    out.push_str("</sum1:Desglose>");
    tag(&mut out, "sum1:CuotaTotal", &render_amount(*cuota_total));
    tag(
        &mut out,
        "sum1:ImporteTotal",
        &render_amount(*importe_total),
    );
    out.push_str(&encadenamiento(record)?);
    out.push_str(&sistema_informatico(sif));
    tag(
        &mut out,
        "sum1:FechaHoraHusoGenRegistro",
        fecha_huso_gen.as_str(),
    );
    tag(&mut out, "sum1:TipoHuella", TIPO_HUELLA);
    tag(&mut out, "sum1:Huella", &record.huella);
    out.push_str("</sum1:RegistroAlta>");
    Ok(out)
}

/// No "not an Alta" arm here — the kind dispatch happened at
/// [`registro_alta`]'s door.
#[allow(clippy::too_many_lines)]
fn check_alta(
    ctx: &EmissionContext<'_>,
    obligado: &Obligado,
    issuer: &str,
    tipo_factura: TipoFactura,
    cuota_total: Money,
    importe_total: Money,
) -> Result<(), SerializeError> {
    check_nif("IDFactura/IDEmisorFactura", issuer)?;
    check_nif("Cabecera/ObligadoEmision/NIF", &obligado.nif)?;
    if *issuer != obligado.nif {
        return Err(SerializeError::Field {
            field: "IDFactura/IDEmisorFactura",
            problem: format!(
                "record issuer {issuer} differs from the configured obligado NIF {} — the \
                 emitter's identity and the record's must be one",
                obligado.nif
            ),
        });
    }
    // No gate needed: the kind carries the domain enum — an
    // off-vocabulary code is unrepresentable at every layer.
    let Some(descripcion) = ctx.descripcion_operacion else {
        return Err(SerializeError::Cardinality {
            element: "DescripcionOperacion",
            problem: String::from("required on every alta, missing from the emission context"),
        });
    };
    check_text("DescripcionOperacion", descripcion, 500)?;
    check_text("NombreRazonEmisor", &obligado.nombre_razon, 120)?;
    // DELIBERATE non-check: totals are NOT cross-checked against the
    // desglose — AEAT's own signed example carries cuotas summing 31.4
    // against a declared CuotaTotal of 41.4.
    check_importe_sgn_12_2("CuotaTotal", cuota_total)?;
    check_importe_sgn_12_2("ImporteTotal", importe_total)?;
    if ctx.desglose.is_empty() || ctx.desglose.len() > 12 {
        return Err(SerializeError::Cardinality {
            element: "Desglose/DetalleDesglose",
            problem: format!("1..12 detalle lines required, got {}", ctx.desglose.len()),
        });
    }
    if let Some(ref_ext) = ctx.ref_externa {
        check_text("RefExterna", ref_ext, 60)?;
    }
    let rectificativa = tipo_factura.is_rectificativa();
    if rectificativa && ctx.tipo_rectificativa.is_none() {
        return Err(SerializeError::Cardinality {
            element: "TipoRectificativa",
            problem: format!(
                "TipoFactura {} is rectificativa — the rectification block is \
                 required (TipoRectificativa S/I + the rectified invoices; on the \
                 flat face, InvoiceDraft::rectificativa)",
                tipo_factura.as_str()
            ),
        });
    }
    if rectificativa && ctx.facturas_rectificadas.is_empty() {
        return Err(SerializeError::Cardinality {
            element: "FacturasRectificadas",
            problem: String::from(
                "at least one IDFacturaRectificada is required on rectificativa records",
            ),
        });
    }
    if !rectificativa && (ctx.tipo_rectificativa.is_some() || !ctx.facturas_rectificadas.is_empty())
    {
        return Err(SerializeError::Cardinality {
            element: "TipoRectificativa",
            problem: format!(
                "TipoFactura {} is not rectificativa — no rectification data allowed",
                tipo_factura.as_str()
            ),
        });
    }
    for rectificada in ctx.facturas_rectificadas {
        check_nif("IDFacturaRectificada/IDEmisorFactura", &rectificada.nif)?;
        check_num_serie(
            "IDFacturaRectificada/NumSerieFactura",
            &rectificada.num_serie,
        )?;
    }
    // Validaciones §3.6: obligatory on Sustitutiva, forbidden otherwise.
    match (ctx.tipo_rectificativa, &ctx.importe_rectificacion) {
        (Some(TipoRectificativa::Sustitutiva), None) => {
            return Err(SerializeError::Cardinality {
                element: "ImporteRectificacion",
                problem: String::from(
                    "required when TipoRectificativa is Sustitutiva (the substituted \
                     BaseRectificada/CuotaRectificada must ride the record)",
                ),
            });
        }
        (Some(TipoRectificativa::Incremental), Some(_)) => {
            return Err(SerializeError::Cardinality {
                element: "ImporteRectificacion",
                problem: String::from(
                    "forbidden when TipoRectificativa is Incremental (only sustitutivas \
                     carry the substituted amounts)",
                ),
            });
        }
        (None, Some(_)) => {
            return Err(SerializeError::Cardinality {
                element: "ImporteRectificacion",
                problem: String::from(
                    "forbidden on non-rectificativa records (only sustitutivas carry the \
                     substituted amounts)",
                ),
            });
        }
        _ => {}
    }
    if tipo_factura.requires_destinatario() && ctx.destinatarios.is_empty() {
        return Err(SerializeError::Cardinality {
            element: "Destinatarios",
            problem: format!(
                "TipoFactura {} requires the Destinatarios block (AEAT 1189) — the \
                 counterparty must ride the record",
                tipo_factura.as_str()
            ),
        });
    }
    for destinatario in ctx.destinatarios {
        check_text(
            "IDDestinatario/NombreRazon",
            &destinatario.nombre_razon,
            120,
        )?;
        match &destinatario.identificacion {
            IdentificacionDestinatario::Nif(nif) => {
                check_nif("IDDestinatario/NIF", nif)?;
            }
            IdentificacionDestinatario::Otro(otro) => {
                if let Some(pais) = &otro.codigo_pais {
                    if CODIGOS_PAIS.binary_search(&pais.as_str()).is_err() {
                        return Err(SerializeError::Field {
                            field: "IDDestinatario/IDOtro/CodigoPais",
                            problem: format!(
                                "{pais:?} is not a CountryType2 code (uppercase ISO 3166-1 \
                                 alpha-2)"
                            ),
                        });
                    }
                }
                check_text("IDDestinatario/IDOtro/ID", &otro.id, 20)?;
            }
        }
    }
    Ok(())
}

fn check_detalle(detalle: &DetalleDesglose) -> Result<(), SerializeError> {
    if detalle.calificacion.is_some() == detalle.operacion_exenta.is_some() {
        return Err(SerializeError::Cardinality {
            element: "DetalleDesglose",
            problem: String::from(
                "exactly one of CalificacionOperacion or OperacionExenta is required (the \
                 DetalleType choice)",
            ),
        });
    }
    // AEAT validation 1245 (live-confirmed at pruebas 2026-10-05): when
    // Impuesto is ABSENT or IVA/IPSI/IGIC, ClaveRegimen is required —
    // only Impuesto `05` (otros) may ride without one.
    let clave_required = !matches!(detalle.impuesto, Some(Impuesto::Otros));
    if clave_required && detalle.clave_regimen.is_none() {
        return Err(SerializeError::Cardinality {
            element: "DetalleDesglose/ClaveRegimen",
            problem: format!(
                "required when Impuesto is absent or IVA/IPSI/IGIC (AEAT 1245), got {:?}",
                detalle.impuesto
            ),
        });
    }
    for (field, rate) in [
        ("DetalleDesglose/TipoImpositivo", &detalle.tipo_impositivo),
        (
            "DetalleDesglose/TipoRecargoEquivalencia",
            &detalle.tipo_recargo_equivalencia,
        ),
    ] {
        if let Some(rate) = rate {
            if rate.as_decimal().is_sign_negative() {
                return Err(SerializeError::Field {
                    field,
                    problem: String::from("must be non-negative (Tipo2.2Type has no sign)"),
                });
            }
            check_tipo_2_2(field, *rate)?;
        }
    }
    check_importe_sgn_12_2(
        "DetalleDesglose/BaseImponibleOimporteNoSujeto",
        detalle.base_imponible,
    )?;
    if let Some(cuota) = &detalle.cuota_repercutida {
        check_importe_sgn_12_2("DetalleDesglose/CuotaRepercutida", *cuota)?;
    }
    if let Some(cuota) = &detalle.cuota_recargo_equivalencia {
        check_importe_sgn_12_2("DetalleDesglose/CuotaRecargoEquivalencia", *cuota)?;
    }
    Ok(())
}

fn detalle_desglose(detalle: &DetalleDesglose) -> Result<String, SerializeError> {
    check_detalle(detalle)?;
    let mut out = String::from("<sum1:DetalleDesglose>");
    if let Some(impuesto) = detalle.impuesto {
        tag(&mut out, "sum1:Impuesto", impuesto.wire());
    }
    if let Some(clave) = detalle.clave_regimen {
        tag(&mut out, "sum1:ClaveRegimen", clave.wire());
    }
    if let Some(calificacion) = detalle.calificacion {
        tag(&mut out, "sum1:CalificacionOperacion", calificacion.wire());
    }
    if let Some(exenta) = detalle.operacion_exenta {
        tag(&mut out, "sum1:OperacionExenta", exenta.wire());
    }
    if let Some(tipo) = &detalle.tipo_impositivo {
        tag(&mut out, "sum1:TipoImpositivo", &render_amount(*tipo));
    }
    tag(
        &mut out,
        "sum1:BaseImponibleOimporteNoSujeto",
        &render_amount(detalle.base_imponible),
    );
    if let Some(cuota) = &detalle.cuota_repercutida {
        tag(&mut out, "sum1:CuotaRepercutida", &render_amount(*cuota));
    }
    if let Some(tipo) = &detalle.tipo_recargo_equivalencia {
        tag(
            &mut out,
            "sum1:TipoRecargoEquivalencia",
            &render_amount(*tipo),
        );
    }
    if let Some(cuota) = &detalle.cuota_recargo_equivalencia {
        tag(
            &mut out,
            "sum1:CuotaRecargoEquivalencia",
            &render_amount(*cuota),
        );
    }
    out.push_str("</sum1:DetalleDesglose>");
    Ok(out)
}

/// The 5-field-huella record shape: no amounts ride an anulación.
pub(crate) fn registro_anulacion(
    record: &ChainRecord,
    ctx: &EmissionContext<'_>,
    obligado: &Obligado,
    sif: &SistemaInformaticoConfig,
) -> Result<String, SerializeError> {
    let ChainKind::Anulacion {
        issuer,
        serie,
        number,
        fecha_expedicion,
        fecha_huso_gen,
    } = &record.kind
    else {
        return Err(SerializeError::Field {
            field: "RegistroAnulacion",
            problem: String::from("record kind is not Anulacion"),
        });
    };
    check_nif("IDFactura/IDEmisorFacturaAnulada", issuer)?;
    check_nif("Cabecera/ObligadoEmision/NIF", &obligado.nif)?;
    if *issuer != obligado.nif {
        return Err(SerializeError::Field {
            field: "IDFactura/IDEmisorFacturaAnulada",
            problem: format!(
                "anulada issuer {issuer} differs from the configured obligado NIF {}",
                obligado.nif
            ),
        });
    }
    if let Some(ref_ext) = ctx.ref_externa {
        check_text("RefExterna", ref_ext, 60)?;
    }
    let num_serie = series::format(serie, *number);
    check_num_serie("IDFactura/NumSerieFacturaAnulada", &num_serie)?;

    let mut out = String::with_capacity(1024);
    out.push_str("<sum1:RegistroAnulacion xmlns:sum1=\"");
    out.push_str(NS_SUMINISTRO);
    out.push_str("\">");
    tag(&mut out, "sum1:IDVersion", ID_VERSION);
    out.push_str("<sum1:IDFactura>");
    tag(&mut out, "sum1:IDEmisorFacturaAnulada", issuer);
    tag(&mut out, "sum1:NumSerieFacturaAnulada", &num_serie);
    tag(
        &mut out,
        "sum1:FechaExpedicionFacturaAnulada",
        fecha_expedicion.as_str(),
    );
    out.push_str("</sum1:IDFactura>");
    if let Some(ref_ext) = ctx.ref_externa {
        tag(&mut out, "sum1:RefExterna", ref_ext);
    }
    if let Some(sin_previo) = ctx.sin_registro_previo {
        tag(&mut out, "sum1:SinRegistroPrevio", sin_previo.wire());
    }
    out.push_str(&encadenamiento(record)?);
    out.push_str(&sistema_informatico(sif));
    tag(
        &mut out,
        "sum1:FechaHoraHusoGenRegistro",
        fecha_huso_gen.as_str(),
    );
    tag(&mut out, "sum1:TipoHuella", TIPO_HUELLA);
    tag(&mut out, "sum1:Huella", &record.huella);
    out.push_str("</sum1:RegistroAnulacion>");
    Ok(out)
}

/// The event record's SIGNING UNIT (FS §6.b: the signature goes INSIDE
/// `Evento`; [`registro_evento`] embeds the signed node). PUBLIC law (HS
/// §3.c/§5.c): the event's own hash rides `HuellaEvento`; chaining rides
/// `PrimerEvento=S` / the previous event's stored hash. Routing quirk:
/// `IdSistemaInformatico` sits OUTSIDE `SistemaInformatico`.
/// INTERPRETATION (no public event XSD): the element names/order are
/// data-diff-correctable when AEAT publishes the schema.
///
/// # Errors
/// [`SerializeError`] on simple-type violations.
pub fn evento_node(
    record: &ChainRecord,
    ctx: &EmissionContext<'_>,
) -> Result<String, SerializeError> {
    let ChainKind::Evento {
        event,
        fecha_huso_gen_evento,
    } = &record.kind
    else {
        return Err(SerializeError::Field {
            field: "Evento",
            problem: String::from("record kind is not Evento"),
        });
    };
    if event.nif.is_empty() {
        check_text("Evento/IDOtro/ID", &event.id, 20)?;
    } else {
        check_nif("Evento/SistemaInformatico/NIF", &event.nif)?;
    }
    check_text(
        "Evento/IdSistemaInformatico",
        &event.id_sistema_informatico,
        2,
    )?;
    check_text("Evento/SistemaInformatico/Version", &event.version, 50)?;
    check_text(
        "Evento/SistemaInformatico/NumeroInstalacion",
        &event.numero_instalacion,
        100,
    )?;
    check_text("Evento/TipoEvento", &event.tipo_evento, 100)?;
    check_nif("Evento/ObligadoEmision/NIF", &event.nif_obligado)?;
    if let Some(motivo) = ctx.motivo_evento {
        check_text("Evento/MotivoAnomalia", motivo, 500)?;
    }

    let mut out = String::with_capacity(1024);
    out.push_str("<sum1:Evento xmlns:sum1=\"");
    out.push_str(NS_SUMINISTRO);
    out.push_str("\">");
    out.push_str("<sum1:ObligadoEmision>");
    tag(&mut out, "sum1:NIF", &event.nif_obligado);
    out.push_str("</sum1:ObligadoEmision>");
    tag(&mut out, "sum1:TipoEvento", &event.tipo_evento);
    if let Some(motivo) = ctx.motivo_evento {
        tag(&mut out, "sum1:MotivoAnomalia", motivo);
    }
    // HS §3.c: IdSistemaInformatico is a SIBLING of the
    // SistemaInformatico block, not a child.
    tag(
        &mut out,
        "sum1:IdSistemaInformatico",
        &event.id_sistema_informatico,
    );
    out.push_str("<sum1:SistemaInformatico>");
    if event.nif.is_empty() {
        // IDOtroType's REQUIRED IDType is INTENTIONALLY unrendered (no
        // event XSD is public) — flagged, not silently missing.
        out.push_str("<sum1:IDOtro>");
        tag(&mut out, "sum1:ID", &event.id);
        out.push_str("</sum1:IDOtro>");
    } else {
        tag(&mut out, "sum1:NIF", &event.nif);
    }
    tag(&mut out, "sum1:Version", &event.version);
    tag(
        &mut out,
        "sum1:NumeroInstalacion",
        &event.numero_instalacion,
    );
    out.push_str("</sum1:SistemaInformatico>");
    tag(
        &mut out,
        "sum1:FechaHoraHusoGenEvento",
        fecha_huso_gen_evento.as_str(),
    );
    tag(&mut out, "sum1:TipoHuella", TIPO_HUELLA);
    tag(&mut out, "sum1:HuellaEvento", &record.huella);
    out.push_str("<sum1:Encadenamiento>");
    match &record.prev_evento_huella {
        None => tag(&mut out, "sum1:PrimerEvento", "S"),
        Some(prev_huella) => {
            out.push_str("<sum1:EventoAnterior>");
            tag(&mut out, "sum1:HuellaEvento", prev_huella);
            out.push_str("</sum1:EventoAnterior>");
        }
    }
    out.push_str("</sum1:Encadenamiento>");
    out.push_str("</sum1:Evento>");
    Ok(out)
}

/// Embeds the signed `Evento` verbatim — its own `xmlns:sum1` keeps it
/// digest-stable, the same embedding rule as a signed record inside the
/// envelope.
#[must_use]
pub fn registro_evento(signed_evento: &str) -> String {
    let mut out = String::with_capacity(signed_evento.len() + 96);
    out.push_str("<sum1:RegistroEvento xmlns:sum1=\"");
    out.push_str(NS_SUMINISTRO);
    out.push_str("\">");
    tag(&mut out, "sum1:IDVersion", ID_VERSION);
    out.push_str(signed_evento);
    out.push_str("</sum1:RegistroEvento>");
    out
}

/// # Errors
/// [`SerializeError`] per the record's kind.
pub fn record_node(
    record: &ChainRecord,
    ctx: &EmissionContext<'_>,
    obligado: &Obligado,
    sif: &SistemaInformaticoConfig,
) -> Result<String, SerializeError> {
    match &record.kind {
        ChainKind::Alta { .. } => registro_alta(record, ctx, obligado, sif),
        ChainKind::Anulacion { .. } => registro_anulacion(record, ctx, obligado, sif),
        ChainKind::Evento { .. } => Err(SerializeError::Field {
            field: "record_node",
            problem: String::from(
                "evento records serialize through evento_node + registro_evento (FS §6.b \
                 signature placement inside Evento), not record_node",
            ),
        }),
    }
}

/// Remission is always voluntary (`Incidencia=S` on outage records);
/// conservation remits only bajo requerimiento (`FinRequerimiento=S` on
/// the last batch).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CabeceraRemision<'a> {
    Voluntaria { incidencia: bool },
    Requerimiento { referencia: &'a str, fin: bool },
}

/// One whole envío (SW §6): the Cabecera — `ObligadoEmision`, the
/// optional `Representante`, the remission block, in SI.xsd sequence —
/// then each signed record in its own `RegistroFactura`, embedded
/// verbatim.
///
/// # Errors
/// [`SerializeError::Cardinality`] when empty or over the 1000-record
/// envío limit.
pub fn reg_factu_document(
    obligado: &Obligado,
    representante: Option<&Obligado>,
    remision: &CabeceraRemision<'_>,
    signed_records: &[String],
) -> Result<String, SerializeError> {
    if signed_records.is_empty() || signed_records.len() > 1000 {
        return Err(SerializeError::Cardinality {
            element: "RegistroFactura",
            problem: format!(
                "one envío carries 1..1000 records, got {}",
                signed_records.len()
            ),
        });
    }
    let total: usize = signed_records.iter().map(String::len).sum();
    let mut out = String::with_capacity(total + 512);
    out.push('<');
    out.push_str(P_LR);
    out.push_str(":RegFactuSistemaFacturacion xmlns:sumLR=\"");
    out.push_str(NS_LR);
    out.push_str("\" xmlns:sum1=\"");
    out.push_str(NS_SUMINISTRO);
    out.push_str("\">");
    out.push_str("<sumLR:Cabecera>");
    out.push_str("<sum1:ObligadoEmision>");
    tag(&mut out, "sum1:NombreRazon", &obligado.nombre_razon);
    tag(&mut out, "sum1:NIF", &obligado.nif);
    out.push_str("</sum1:ObligadoEmision>");
    if let Some(representante) = representante {
        out.push_str("<sum1:Representante>");
        tag(&mut out, "sum1:NombreRazon", &representante.nombre_razon);
        tag(&mut out, "sum1:NIF", &representante.nif);
        out.push_str("</sum1:Representante>");
    }
    match remision {
        CabeceraRemision::Voluntaria { incidencia } => {
            if *incidencia {
                out.push_str("<sum1:RemisionVoluntaria>");
                tag(&mut out, "sum1:Incidencia", "S");
                out.push_str("</sum1:RemisionVoluntaria>");
            } else {
                out.push_str("<sum1:RemisionVoluntaria/>");
            }
        }
        CabeceraRemision::Requerimiento { referencia, fin } => {
            out.push_str("<sum1:RemisionRequerimiento>");
            tag(&mut out, "sum1:RefRequerimiento", referencia);
            if *fin {
                tag(&mut out, "sum1:FinRequerimiento", "S");
            }
            out.push_str("</sum1:RemisionRequerimiento>");
        }
    }
    out.push_str("</sumLR:Cabecera>");
    for signed_record in signed_records {
        out.push_str("<sumLR:RegistroFactura>");
        out.push_str(signed_record);
        out.push_str("</sumLR:RegistroFactura>");
    }
    out.push_str("</sumLR:RegFactuSistemaFacturacion>");
    Ok(out)
}

/// SOAP 1.1 per the `sfVerifactu` binding: `soapAction` is empty — the
/// operation name travels in the body element.
#[must_use]
pub fn soap_envelope(body_document: &str) -> String {
    let mut out = String::with_capacity(body_document.len() + 160);
    out.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>");
    out.push('<');
    out.push_str(P_ENV);
    out.push_str(":Envelope xmlns:soapenv=\"");
    out.push_str(NS_SOAP_ENV);
    out.push_str("\">");
    out.push_str("<soapenv:Body>");
    out.push_str(body_document);
    out.push_str("</soapenv:Body>");
    out.push_str("</soapenv:Envelope>");
    out
}

// The name-tree scanner — prefix-agnostic and entity-decoding, exactly
// enough for AEAT-shaped documents, no more.

/// Matched by LOCAL name only — not a general XML processor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Tree {
    pub local: String,
    pub text: String,
    pub children: Vec<Tree>,
}

impl Tree {
    pub(crate) fn child(&self, local: &str) -> Option<&Tree> {
        self.children.iter().find(|node| node.local == local)
    }

    pub(crate) fn child_text(&self, local: &str) -> Option<String> {
        self.child(local).map(|node| node.text.clone())
    }
}

/// XML 1.0's `Char` production: `#x9 | #xA | #xD | [#x20-#xD7FF] |
/// [#xE000-#xFFFD] | [#x10000-#x10FFFF]`. `&#0;` parses as a number but
/// is not XML — references must denote a legal `Char`.
fn is_char_code(code: u32) -> bool {
    matches!(
        code,
        0x9 | 0xA | 0xD | 0x20..=0xD7FF | 0xE000..=0xFFFD | 0x1_0000..=0x10_FFFF
    )
}

/// A numeric reference outside `Char` is never decoded —
/// `char::from_u32` alone would admit the NUL.
fn decode_entities(input: &str) -> Result<String, String> {
    if !input.contains('&') {
        return Ok(input.to_owned());
    }
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(start) = rest.find('&') {
        out.push_str(&rest[..start]);
        rest = &rest[start..];
        let Some(end) = rest.find(';') else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let entity = &rest[1..end];
        let decoded = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ => {
                let code = if let Some(hex) = entity.strip_prefix("#x") {
                    u32::from_str_radix(hex, 16).ok()
                } else {
                    entity
                        .strip_prefix('#')
                        .and_then(|dec| dec.parse::<u32>().ok())
                };
                match code {
                    Some(code) if is_char_code(code) => char::from_u32(code),
                    Some(_) => {
                        return Err(format!(
                            "character reference &{entity}; is not a legal XML 1.0 character"
                        ))
                    }
                    None => None,
                }
            }
        };
        match decoded {
            Some(ch) => out.push(ch),
            None => out.push_str(&rest[..=end]),
        }
        rest = &rest[end + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

fn attach(stack: &mut [Tree], root: &mut Option<Tree>, node: Tree) -> Result<(), String> {
    if let Some(parent) = stack.last_mut() {
        parent.children.push(node);
        return Ok(());
    }
    if root.is_some() {
        return Err(String::from("multiple root elements"));
    }
    *root = Some(node);
    Ok(())
}

/// Parses one XML document into a [`Tree`]; a leading UTF-8 BOM is
/// stripped (real interop: AEAT-side encoders emit one).
pub(crate) fn parse_tree(xml: &str) -> Result<Tree, String> {
    let xml = xml.strip_prefix('\u{feff}').unwrap_or(xml);
    let bytes = xml.as_bytes();
    let mut pos = 0;
    let mut stack: Vec<Tree> = Vec::new();
    let mut root: Option<Tree> = None;

    while pos < bytes.len() {
        if bytes[pos] != b'<' {
            let end = xml[pos..]
                .find('<')
                .map_or(xml.len(), |offset| pos + offset);
            let chunk = &xml[pos..end];
            match stack.last_mut() {
                // Decode per text chunk at push time so adjacent raw
                // CDATA is never decoded.
                Some(top) => top.text.push_str(&decode_entities(chunk)?),
                None if !chunk.trim().is_empty() => {
                    return Err(String::from("text outside the document root"));
                }
                None => {}
            }
            pos = end;
            continue;
        }
        if xml[pos..].starts_with("<!--") {
            // Comments may contain '>' — find the real terminator.
            let Some(close) = xml[pos..].find("-->") else {
                return Err(String::from("unterminated comment"));
            };
            pos += close + 3;
            continue;
        }
        if xml[pos..].starts_with("<![CDATA[") {
            let content_start = pos + "<![CDATA[".len();
            let Some(close) = xml[content_start..].find("]]>") else {
                return Err(String::from("unterminated CDATA section"));
            };
            let content = &xml[content_start..content_start + close];
            match stack.last_mut() {
                Some(top) => top.text.push_str(content),
                None => return Err(String::from("CDATA outside the document root")),
            }
            pos = content_start + close + "]]>".len();
            continue;
        }
        let Some((tag_end, self_closing)) = find_tag_end(xml, pos) else {
            return Err(String::from("unterminated tag"));
        };
        let raw = &xml[pos + 1..tag_end];
        pos = tag_end + 1;
        if raw.starts_with('?') {
            continue;
        }
        if raw.starts_with('!') {
            return Err(String::from("DTD declaration rejected"));
        }
        let local_of = |name: &str| name.rsplit(':').next().unwrap_or(name).to_owned();
        if let Some(name) = raw.strip_prefix('/') {
            let name = name.trim();
            let node = stack
                .pop()
                .ok_or_else(|| format!("closing tag </{name}> with no open element"))?;
            if local_of(name) != node.local {
                return Err(format!(
                    "mismatched closing tag </{name}> for <{}>",
                    node.local
                ));
            }
            attach(&mut stack, &mut root, node)?;
            continue;
        }
        let name = raw
            .split(|ch: char| ch.is_whitespace() || ch == '=' || ch == '/')
            .next()
            .unwrap_or("");
        if name.is_empty() {
            return Err(String::from("empty tag name"));
        }
        if self_closing {
            let node = Tree {
                local: local_of(name),
                text: String::new(),
                children: Vec::new(),
            };
            attach(&mut stack, &mut root, node)?;
            continue;
        }
        if stack.len() >= MAX_ELEMENT_DEPTH {
            return Err(format!(
                "element <{}> nests deeper than the contract cap {} — off-contract shape",
                local_of(name),
                MAX_ELEMENT_DEPTH
            ));
        }
        stack.push(Tree {
            local: local_of(name),
            text: String::new(),
            children: Vec::new(),
        });
    }
    if !stack.is_empty() {
        return Err(format!("unclosed element <{}>", stack[0].local));
    }
    root.ok_or_else(|| String::from("no root element"))
}

fn find_tag_end(xml: &str, start: usize) -> Option<(usize, bool)> {
    let bytes = xml.as_bytes();
    let mut quote: Option<u8> = None;
    let mut index = start + 1;
    while index < bytes.len() {
        let byte = bytes[index];
        match quote {
            Some(open) if byte == open => quote = None,
            Some(_) => {}
            None => match byte {
                b'"' | b'\'' => quote = Some(byte),
                b'>' => {
                    let self_closing = index > start + 1 && bytes[index - 1] == b'/';
                    return Some((index, self_closing));
                }
                _ => {}
            },
        }
        index += 1;
    }
    None
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SentRecord {
    pub nif: String,
    pub num_serie: String,
    pub fecha: String,
    pub tipo_operacion: crate::fiscal::response::TipoOperacion,
    /// The record's `RefExterna` when it carried one — the respuesta
    /// echo must answer it back (real AEAT does; so does the fake).
    pub ref_externa: Option<String>,
}

/// What a response must echo (SW §5): the `Cabecera` obligado plus every
/// remitted record's key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SentRequest {
    pub obligado_nombre: String,
    pub obligado_nif: String,
    pub records: Vec<SentRecord>,
}

/// # Errors
/// Our own output — an envelope that does not parse is a bug, not an
/// input class.
pub(crate) fn sent_request(envelope: &str) -> Result<SentRequest, String> {
    let root = parse_tree(envelope)?;
    let body = root.child("Body").ok_or("no soapenv:Body")?;
    let reg_factu = body
        .child("RegFactuSistemaFacturacion")
        .ok_or("no RegFactuSistemaFacturacion")?;
    let cabecera = reg_factu.child("Cabecera").ok_or("no Cabecera")?;
    let obligado = cabecera
        .child("ObligadoEmision")
        .ok_or("no ObligadoEmision")?;
    let mut records = Vec::new();
    for registro_factura in reg_factu
        .children
        .iter()
        .filter(|node| node.local == "RegistroFactura")
    {
        for record in &registro_factura.children {
            let tipo_operacion = match record.local.as_str() {
                "RegistroAlta" => crate::fiscal::response::TipoOperacion::Alta,
                "RegistroAnulacion" => crate::fiscal::response::TipoOperacion::Anulacion,
                other => return Err(format!("unexpected record element <{other}>")),
            };
            let id_factura = record
                .child("IDFactura")
                .ok_or("record without IDFactura")?;
            let nif = id_factura
                .child_text("IDEmisorFactura")
                .or_else(|| id_factura.child_text("IDEmisorFacturaAnulada"))
                .ok_or("IDFactura without issuer")?;
            let num_serie = id_factura
                .child_text("NumSerieFactura")
                .or_else(|| id_factura.child_text("NumSerieFacturaAnulada"))
                .ok_or("IDFactura without NumSerieFactura")?;
            let fecha = id_factura
                .child_text("FechaExpedicionFactura")
                .or_else(|| id_factura.child_text("FechaExpedicionFacturaAnulada"))
                .ok_or("IDFactura without FechaExpedicionFactura")?;
            let ref_externa = record
                .child_text("RefExterna")
                .filter(|text| !text.is_empty());
            records.push(SentRecord {
                nif,
                num_serie,
                fecha,
                tipo_operacion,
                ref_externa,
            });
        }
    }
    Ok(SentRequest {
        obligado_nombre: obligado.child_text("NombreRazon").unwrap_or_default(),
        obligado_nif: obligado.child_text("NIF").unwrap_or_default(),
        records,
    })
}

#[cfg(test)]
mod tests {
    use proptest::prelude::Strategy as _;

    use super::{check_text, escape_text, parse_tree, registro_alta, EmissionContext, Tree};
    use crate::fiscal::tests_support::{iva_super, obligado, primer_alta_record, sif};

    /// No live bench leg exercises escaping.
    #[test]
    fn special_characters_escape_into_the_record_bytes() {
        let desglose = [iva_super()];
        let ctx = EmissionContext {
            descripcion_operacion: Some("Venta & devolución <2 uds> al contado"),
            desglose: &desglose,
            ..EmissionContext::default()
        };
        let node =
            registro_alta(&primer_alta_record(), &ctx, &obligado(), &sif()).expect("serializes");
        assert!(
            node.contains(
                "<sum1:DescripcionOperacion>Venta &amp; devolución &lt;2 uds&gt; al contado\
                           </sum1:DescripcionOperacion>"
            ),
            "escaped bytes must appear verbatim: {node}"
        );
        assert!(!node.contains("<2 uds>"), "raw \'<\' must never appear");
    }

    fn find<'a>(tree: &'a Tree, local: &str) -> Option<&'a Tree> {
        if tree.local == local {
            return Some(tree);
        }
        tree.children.iter().find_map(|child| find(child, local))
    }

    /// The independent oracle: a conformant XML 1.0 parser's reading of
    /// `text` escaped as element content — `None` when it refuses.
    fn conformant_reading(text: &str) -> Option<String> {
        let document = format!("<a>{}</a>", escape_text(text));
        let parsed = uppsala::parse(&document).ok()?;
        let root = parsed.document_element()?;
        Some(parsed.text_content_deep(root))
    }

    /// One `Char` law, three readers: what [`check_text`] admits is
    /// exactly what a conformant XML 1.0 parser accepts, and both it and
    /// our own [`parse_tree`] read the escaped bytes back verbatim.
    fn assert_char_law(text: &str) {
        let admitted = check_text("Field", text, usize::MAX).is_ok();
        let conformant = conformant_reading(text);
        assert_eq!(
            admitted,
            conformant.is_some(),
            "check_text and XML 1.0 disagree on {text:?}"
        );
        if admitted {
            assert_eq!(conformant.as_deref(), Some(text), "conformant read-back");
            let ours = parse_tree(&format!("<a>{}</a>", escape_text(text))).expect("ours parses");
            assert_eq!(ours.text, text, "parse_tree read-back");
        }
        let reference = format!(
            "<a>&#x{:X};</a>",
            text.chars().next().map_or(0x41, u32::from)
        );
        assert_eq!(
            parse_tree(&reference).is_ok(),
            uppsala::parse(&reference).is_ok(),
            "character-reference decoding disagrees with XML 1.0 on {reference}"
        );
    }

    /// Every edge of every `Char` range (U+1F338 is the emoji a client's
    /// name once carried into a refused record).
    #[test]
    fn the_char_law_agrees_with_xml_1_0_at_every_range_edge() {
        let edges = (0..=0x20)
            .chain([
                0x7F, 0x80, 0x85, 0xD7FF, 0xE000, 0xFEFF, 0xFFFD, 0xFFFE, 0xFFFF,
            ])
            .chain([0x1_0000, 0x1_F338, 0xF_FFFF, 0x10_0000, 0x10_FFFF]);
        for code in edges {
            let ch = char::from_u32(code).expect("scalar value");
            assert_char_law(&format!("a{ch}b"));
        }
    }

    proptest::proptest! {
        #![proptest_config(proptest::prelude::ProptestConfig::with_cases(512))]

        #[test]
        fn the_char_law_agrees_with_xml_1_0_on_arbitrary_text(
            text in proptest::collection::vec(proptest::char::any(), 0..24)
                .prop_map(|chars| chars.into_iter().collect::<String>()),
        ) {
            assert_char_law(&text);
        }
    }

    /// `maxLength` counts characters: a ceiling-length name carrying
    /// emoji (two UTF-16 units each) is admitted; one char more is not.
    #[test]
    fn max_length_counts_characters_not_utf16_units() {
        let ceiling: String = "Gil·li «Ñ» 🌸 ".chars().cycle().take(120).collect();
        assert!(ceiling.encode_utf16().count() > 120);
        assert!(check_text("IDDestinatario/NombreRazon", &ceiling, 120).is_ok());
        let over = format!("{ceiling}x");
        assert!(check_text("IDDestinatario/NombreRazon", &over, 120).is_err());
    }

    /// The serializer's text reaches the record verbatim: a description
    /// carrying markup, a CR and a supplementary-plane character reads
    /// back unchanged from the serialized `RegistroAlta`.
    #[test]
    fn record_text_round_trips_through_the_serializer() {
        let descripcion = "Floristería 🌸 & <rosas>\r\n\u{10_FFFF}";
        let desglose = [iva_super()];
        let ctx = EmissionContext {
            descripcion_operacion: Some(descripcion),
            desglose: &desglose,
            ..EmissionContext::default()
        };
        let node =
            registro_alta(&primer_alta_record(), &ctx, &obligado(), &sif()).expect("serializes");
        let tree = parse_tree(&node).expect("reads back");
        let read = find(&tree, "DescripcionOperacion").expect("element present");
        assert_eq!(read.text, descripcion);
    }
}
