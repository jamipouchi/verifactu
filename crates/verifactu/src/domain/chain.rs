//! The hash-chained ledger primitive per the PUBLIC AEAT huella
//! specification (HS v0.1.2): SHA-256 over the UTF-8 bytes of a
//! `name=value&`-joined input, hex-encoded UPPERCASE — pinned by the
//! four fixed test anchors below (HS §6.1–6.3 + AEAT's own signed
//! example).

use std::fmt;

use sha2::{Digest, Sha256};
use strum::EnumIter;

use crate::domain::money::Money;
use crate::domain::series::{self, Series};

#[must_use]
pub fn huella(cadena: &str) -> String {
    hex::encode_upper(Sha256::digest(cadena.as_bytes()))
}

/// The `RegistroAlta` huella input as a free function (HS §3.a): the
/// consulta-verification half recomputes AEAT's STORED huella from the
/// fields the consulta respuesta echoes — the same push order and
/// trim law as [`ChainRecord::cadena`]'s Alta arm.
/// The `RegistroAlta` huella input (HS §3.a) over TEXT amounts: the consulta-verification half
/// recomputes from AEAT's echoed decimals verbatim — HS §3 makes 1-
/// and 2-decimal renderings equally valid, so re-rendering through
/// [`render_amount`] would false-negative a legally-echoed `41.4`.
#[allow(clippy::too_many_arguments)] // the HS §3.a input is eight named fields, by law
#[must_use]
pub fn huella_alta_montos(
    id_emisor_factura: &str,
    num_serie_factura: &str,
    fecha_expedicion_factura: &str,
    tipo_factura: &str,
    cuota_total: &str,
    importe_total: &str,
    huella_previa: Option<&str>,
    fecha_huso_gen: &str,
) -> String {
    fn push_pair(out: &mut String, name: &str, value: &str) {
        out.push_str(name);
        out.push('=');
        out.push_str(value.trim());
        out.push('&');
    }
    let mut out = String::with_capacity(256);
    push_pair(&mut out, "IDEmisorFactura", id_emisor_factura);
    push_pair(&mut out, "NumSerieFactura", num_serie_factura);
    push_pair(&mut out, "FechaExpedicionFactura", fecha_expedicion_factura);
    push_pair(&mut out, "TipoFactura", tipo_factura);
    push_pair(&mut out, "CuotaTotal", cuota_total);
    push_pair(&mut out, "ImporteTotal", importe_total);
    push_pair(&mut out, "Huella", huella_previa.unwrap_or(""));
    push_pair(&mut out, "FechaHoraHusoGenRegistro", fecha_huso_gen);
    out.truncate(out.len() - 1);
    huella(&out)
}

/// The previous invoice-chain record's identity + huella (HS §3.a/b).
/// `fecha_expedicion` is a domain extension beyond the HS input,
/// required by the wire's `RegistroAnterior` block.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct PrevRef {
    pub issuer: String,
    pub serie: Series,
    pub number: u64,
    pub fecha_expedicion: FechaExpedicion,
    pub huella: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Predecessor {
    Factura(PrevRef),
    Evento(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct EventData {
    pub nif: String,
    pub id: String,
    pub id_sistema_informatico: String,
    pub version: String,
    pub numero_instalacion: String,
    pub nif_obligado: String,
    pub tipo_evento: String,
}

/// What was sealed. Round-trips under the `serde` feature — a ledger
/// persists the echo and reads it back (the totals, the tipo in force,
/// the instants) without consumer-side mirrors of the sealing law.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum ChainKind {
    Alta {
        issuer: String,
        serie: Series,
        number: u64,
        fecha_expedicion: FechaExpedicion,
        tipo_factura: TipoFactura,
        cuota_total: Money,
        importe_total: Money,
        fecha_huso_gen: FechaHuso,
    },
    Anulacion {
        issuer: String,
        serie: Series,
        number: u64,
        fecha_expedicion: FechaExpedicion,
        fecha_huso_gen: FechaHuso,
    },
    Evento {
        event: EventData,
        fecha_huso_gen_evento: FechaHuso,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChainRecord {
    pub kind: ChainKind,
    /// UTC epoch seconds — ordering/audit only; deliberately NOT a hash
    /// input.
    pub fecha: u64,
    pub prev: Option<PrevRef>,
    pub prev_evento_huella: Option<String>,
    pub huella: String,
}

/// The closed `TipoFactura` vocabulary — SI.xsd's `ClaveTipoFacturaType`
/// enumerates exactly these eight: `F1..F3` facturas (`F2` simplificada),
/// `R1..R5` rectificativas (`R5` refunds of simplificadas); no F4/F5
/// exist.
#[derive(Copy, Clone, Debug, PartialEq, Eq, strum::EnumString, strum::AsRefStr, EnumIter)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum TipoFactura {
    F1,
    F2,
    F3,
    R1,
    R2,
    R3,
    R4,
    R5,
}

impl TipoFactura {
    #[must_use]
    pub fn as_str(&self) -> &str {
        self.as_ref()
    }

    #[must_use]
    pub fn parse(code: &str) -> Option<Self> {
        code.parse().ok()
    }

    #[must_use]
    pub const fn is_rectificativa(self) -> bool {
        matches!(self, Self::R1 | Self::R2 | Self::R3 | Self::R4 | Self::R5)
    }
}

/// Renders a huella-input amount at exactly 2 decimals ([`Money`]'s
/// ≤2dp invariant makes this a pad, never a round). ZERO renders
/// UNSIGNED: `rust_decimal` preserves `-0.00`, whose hash would stop
/// recomputing from stored cents — AEAT flags that Aceptado con
/// errores (HS §7).
#[must_use]
pub fn render_amount(amount: Money) -> String {
    let value = amount.as_decimal();
    if value.is_zero() {
        String::from("0.00")
    } else {
        format!("{value:.2}")
    }
}

/// The rendered `FechaExpedicionFactura` string (HS §3): exactly 10
/// chars, `dd-mm-yyyy` — digits at 0,1,3,4,6..9, `-` at 2,5.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct FechaExpedicion(String);

impl FechaExpedicion {
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        let bytes = value.as_bytes();
        let structural = bytes.len() == 10
            && bytes.iter().enumerate().all(|(index, &byte)| match index {
                2 | 5 => byte == b'-',
                _ => byte.is_ascii_digit(),
            });
        structural.then(|| Self(String::from(value)))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for FechaExpedicion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// The rendered `FechaHoraHusoGenRegistro`/`…Evento` string (HS §3):
/// exactly 25 chars, `yyyy-mm-ddThh:mm:ss±hh:mm` — `-` at 4,7, `T` at
/// 10, `:` at 13,16,22, sign at 19.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct FechaHuso(String);

impl FechaHuso {
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        let bytes = value.as_bytes();
        let structural = bytes.len() == 25
            && bytes.iter().enumerate().all(|(index, &byte)| match index {
                4 | 7 => byte == b'-',
                10 => byte == b'T',
                13 | 16 | 22 => byte == b':',
                19 => byte == b'+' || byte == b'-',
                _ => byte.is_ascii_digit(),
            });
        structural.then(|| Self(String::from(value)))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for FechaHuso {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl ChainRecord {
    /// The hash input for this record's huella (HS §3), the variant
    /// selecting the field subset: **Alta (8)** `IDEmisorFactura`,
    /// `NumSerieFactura` (the [`series::format`] rendering),
    /// `FechaExpedicionFactura`, `TipoFactura`, `CuotaTotal`,
    /// `ImporteTotal`, `Huella` (the predecessor's STORED huella, EMPTY
    /// on the primer), `FechaHoraHusoGenRegistro`; **Anulacion (5)** the
    /// `…Anulada` twins, `Huella`, `FechaHoraHusoGenRegistro`;
    /// **Evento (9)** `NIF` (SIF), `ID`, `IdSistemaInformatico`,
    /// `Version`, `NumeroInstalacion`, `NIF` (obligado — the name
    /// repeats, HS §3.c), `TipoEvento`, `HuellaEvento`,
    /// `FechaHoraHusoGenEvento`. Values trimmed, `name=` for empties, no
    /// trailing separator; the primer of each chain hashes with an EMPTY
    /// `Huella`/`HuellaEvento` — never a sentinel.
    ///
    /// # Panics
    ///
    /// Panics when an invoice kind's `number > 99_999_999`
    /// ([`series::format`]'s bug class).
    #[must_use]
    pub fn cadena(&self) -> String {
        fn push_pair(out: &mut String, name: &str, value: &str) {
            out.push_str(name);
            out.push('=');
            out.push_str(value.trim());
            out.push('&');
        }
        let mut out = String::with_capacity(288);
        match &self.kind {
            ChainKind::Alta {
                issuer,
                serie,
                number,
                fecha_expedicion,
                tipo_factura,
                cuota_total,
                importe_total,
                fecha_huso_gen,
            } => {
                push_pair(&mut out, "IDEmisorFactura", issuer);
                push_pair(&mut out, "NumSerieFactura", &series::format(serie, *number));
                push_pair(
                    &mut out,
                    "FechaExpedicionFactura",
                    fecha_expedicion.as_str(),
                );
                push_pair(&mut out, "TipoFactura", tipo_factura.as_str());
                push_pair(&mut out, "CuotaTotal", &render_amount(*cuota_total));
                push_pair(&mut out, "ImporteTotal", &render_amount(*importe_total));
                push_pair(
                    &mut out,
                    "Huella",
                    self.prev.as_ref().map_or("", |prev| prev.huella.as_str()),
                );
                push_pair(
                    &mut out,
                    "FechaHoraHusoGenRegistro",
                    fecha_huso_gen.as_str(),
                );
            }
            ChainKind::Anulacion {
                issuer,
                serie,
                number,
                fecha_expedicion,
                fecha_huso_gen,
            } => {
                push_pair(&mut out, "IDEmisorFacturaAnulada", issuer);
                push_pair(
                    &mut out,
                    "NumSerieFacturaAnulada",
                    &series::format(serie, *number),
                );
                push_pair(
                    &mut out,
                    "FechaExpedicionFacturaAnulada",
                    fecha_expedicion.as_str(),
                );
                push_pair(
                    &mut out,
                    "Huella",
                    self.prev.as_ref().map_or("", |prev| prev.huella.as_str()),
                );
                push_pair(
                    &mut out,
                    "FechaHoraHusoGenRegistro",
                    fecha_huso_gen.as_str(),
                );
            }
            ChainKind::Evento {
                event,
                fecha_huso_gen_evento,
            } => {
                push_pair(&mut out, "NIF", &event.nif);
                push_pair(&mut out, "ID", &event.id);
                push_pair(
                    &mut out,
                    "IdSistemaInformatico",
                    &event.id_sistema_informatico,
                );
                push_pair(&mut out, "Version", &event.version);
                push_pair(&mut out, "NumeroInstalacion", &event.numero_instalacion);
                push_pair(&mut out, "NIF", &event.nif_obligado);
                push_pair(&mut out, "TipoEvento", &event.tipo_evento);
                push_pair(
                    &mut out,
                    "HuellaEvento",
                    self.prev_evento_huella.as_deref().unwrap_or(""),
                );
                push_pair(
                    &mut out,
                    "FechaHoraHusoGenEvento",
                    fecha_huso_gen_evento.as_str(),
                );
            }
        }
        out.pop();
        out
    }

    #[must_use]
    pub fn predecessor(&self) -> Predecessor {
        match &self.kind {
            ChainKind::Alta {
                issuer,
                serie,
                number,
                fecha_expedicion,
                ..
            }
            | ChainKind::Anulacion {
                issuer,
                serie,
                number,
                fecha_expedicion,
                ..
            } => Predecessor::Factura(PrevRef {
                issuer: issuer.clone(),
                serie: serie.clone(),
                number: *number,
                fecha_expedicion: fecha_expedicion.clone(),
                huella: self.huella.clone(),
            }),
            ChainKind::Evento { .. } => Predecessor::Evento(self.huella.clone()),
        }
    }

    /// # Panics
    ///
    /// - On an evento identity that is not exactly one of NIF/ID (HS §3
    ///   excluyente).
    /// - When an invoice kind's `number > 99_999_999`
    ///   ([`series::format`]'s bug class).
    /// - When `kind` and `prev` come from different chain families — the
    ///   invoice and event chains never chain onto each other (HS §3).
    #[must_use]
    pub fn seal(kind: ChainKind, fecha: u64, prev: Option<&Predecessor>) -> ChainRecord {
        if let ChainKind::Evento { event, .. } = &kind {
            let nif_informed = !event.nif.trim().is_empty();
            let id_informed = !event.id.trim().is_empty();
            assert!(
                nif_informed != id_informed,
                "evento identity is exactly one of NIF or ID (HS §3, \
                 excluyente)"
            );
        }
        let (prev_ref, prev_evento_huella) = match (&kind, prev) {
            (
                ChainKind::Alta { .. } | ChainKind::Anulacion { .. },
                Some(Predecessor::Factura(reference)),
            ) => (Some(reference.clone()), None),
            (ChainKind::Evento { .. }, Some(Predecessor::Evento(huella))) => {
                (None, Some(huella.clone()))
            }
            (_, None) => (None, None),
            (
                ChainKind::Alta { .. } | ChainKind::Anulacion { .. },
                Some(Predecessor::Evento(_)),
            ) => {
                panic!(
                    "invoice records chain only off invoice records — an \
                     evento predecessor mixes the chain families (HS §3, \
                    )"
                );
            }
            (ChainKind::Evento { .. }, Some(Predecessor::Factura(_))) => {
                panic!(
                    "evento records chain only off evento records — an \
                     invoice predecessor mixes the chain families (HS §3, \
                    )"
                );
            }
        };
        let mut record = ChainRecord {
            kind,
            fecha,
            prev: prev_ref,
            prev_evento_huella,
            huella: String::new(),
        };
        record.huella = huella(&record.cadena());
        record
    }
}

#[cfg(test)]
mod tests {
    use super::{
        huella, ChainKind, ChainRecord, EventData, FechaExpedicion, FechaHuso, Money, Series,
        TipoFactura,
    };

    fn fecha(value: &str) -> FechaExpedicion {
        FechaExpedicion::parse(value).expect("dd-mm-yyyy grammar")
    }

    fn instante(value: &str) -> FechaHuso {
        FechaHuso::parse(value).expect("ISO-8601 grammar")
    }

    /// The sealed echo round-trips under `serde` — a ledger persists
    /// the kind and reads it back (the totals, the tipo in force, the
    /// instants) without consumer-side mirrors of the sealing law.
    /// Money's ≤2dp invariant rides along: `1.234` is refused.
    #[cfg(feature = "serde")]
    #[test]
    fn the_sealed_kind_round_trips_through_json() {
        let kind = ChainKind::Alta {
            issuer: String::from("12345678Z"),
            serie: Series::Custom(String::from("2026E")),
            number: 1,
            fecha_expedicion: fecha("05-10-2026"),
            tipo_factura: TipoFactura::F1,
            cuota_total: Money::from_cents(2_100),
            importe_total: Money::from_cents(12_100),
            fecha_huso_gen: instante("2026-10-05T12:00:00+02:00"),
        };
        let json = serde_json::to_string(&kind).expect("the kind serializes");
        let back: ChainKind = serde_json::from_str(&json).expect("the kind parses back");
        assert_eq!(back, kind);

        let refused = serde_json::from_str::<super::Money>("1.234");
        assert!(
            refused
                .err()
                .is_some_and(|error| error.to_string().contains("2 decimal")),
            "a 3-decimal amount is money this type refuses to be"
        );
    }

    // Fixed AEAT anchors: cadenas transcribed EXACTLY from HS §6.1–6.3
    // (the PDF's mid-token line breaks unwrapped byte for byte); the
    // fourth is AEAT's own signed example, whose verbatim `41.4` rides
    // the input (HS v0.1.2 §3 declares 1- and 2-decimal renderings
    // equally valid).

    #[test]
    fn aeat_anchor_hs_6_1_primer_alta_input_hashes_to_the_spec_huella() {
        assert_eq!(
            huella(
                "IDEmisorFactura=89890001K&NumSerieFactura=12345678/G33&\
                 FechaExpedicionFactura=01-01-2024&TipoFactura=F1&\
                 CuotaTotal=12.35&ImporteTotal=123.45&Huella=&\
                 FechaHoraHusoGenRegistro=2024-01-01T19:20:30+01:00"
            ),
            "3C464DAF61ACB827C65FDA19F352A4E3BDC2C640E9E9FC4CC058073F38F12F60"
        );
    }

    #[test]
    fn aeat_anchor_hs_6_2_chained_alta_input_hashes_to_the_spec_huella() {
        assert_eq!(
            huella(
                "IDEmisorFactura=89890001K&NumSerieFactura=12345679/G34&\
                 FechaExpedicionFactura=01-01-2024&TipoFactura=F1&\
                 CuotaTotal=12.35&ImporteTotal=123.45&\
                 Huella=3C464DAF61ACB827C65FDA19F352A4E3BDC2C640E9E9FC4CC058073F38F12F60&\
                 FechaHoraHusoGenRegistro=2024-01-01T19:20:35+01:00"
            ),
            "F7B94CFD8924EDFF273501B01EE5153E4CE8F259766F88CF6ACB8935802A2B97"
        );
    }

    #[test]
    fn aeat_anchor_hs_6_3_anulacion_input_hashes_to_the_spec_huella() {
        assert_eq!(
            huella(
                "IDEmisorFacturaAnulada=89890001K&\
                 NumSerieFacturaAnulada=12345679/G34&\
                 FechaExpedicionFacturaAnulada=01-01-2024&\
                 Huella=F7B94CFD8924EDFF273501B01EE5153E4CE8F259766F88CF6ACB8935802A2B97&\
                 FechaHoraHusoGenRegistro=2024-01-01T19:20:40+01:00"
            ),
            "177547C0D57AC74748561D054A9CEC14B4C4EA23D1BEFD6F2E69E3A388F90C68"
        );
    }

    #[test]
    fn aeat_anchor_signed_example_r3_input_hashes_to_the_aeat_signed_huella() {
        assert_eq!(
            huella(
                "IDEmisorFactura=89890001K&NumSerieFactura=12345678-G66&\
                 FechaExpedicionFactura=03-02-2025&TipoFactura=R3&\
                 CuotaTotal=41.4&ImporteTotal=241.4&\
                 Huella=C9AF4AF1EF5EBBA700350DE3EEF12C2D355C56AC56F13DB2A25E0031BD2B7ED5&\
                 FechaHoraHusoGenRegistro=2025-02-03T14:30:00+01:00"
            ),
            "FF954378B64ED331A9B2366AD317D86E9DEC1716B12DD0ACCB172A6DC4C105AA"
        );
    }

    /// HS §7: an all-zero refund's totals fold to `-0.00`; hashing a
    /// signed zero breaks the stored-huella recompute (Aceptado con
    /// errores).
    #[test]
    fn a_zero_total_refund_renders_unsigned_zeros_on_both_amount_fields() {
        let negative_zero = Money::from_cents(0).negated();
        assert_eq!(format!("{:.2}", negative_zero.as_decimal()), "-0.00");
        let folded: Money = [Money::ZERO, negative_zero].into_iter().sum();
        let kind = ChainKind::Alta {
            issuer: String::from("B12345678"),
            serie: Series::R,
            number: 1,
            fecha_expedicion: fecha("02-01-2024"),
            tipo_factura: TipoFactura::R5,
            cuota_total: folded,
            importe_total: negative_zero,
            fecha_huso_gen: instante("2024-01-02T10:00:00+01:00"),
        };
        let record = ChainRecord::seal(kind, 0, None);
        assert!(record
            .cadena()
            .contains("CuotaTotal=0.00&ImporteTotal=0.00"));
        assert_eq!(record.huella, huella(&record.cadena()));
    }

    #[test]
    #[should_panic(expected = "chain families")]
    fn seal_rejects_an_alta_sealing_off_an_evento_predecessor() {
        let event = ChainRecord::seal(
            ChainKind::Evento {
                event: EventData {
                    nif: String::from("89890001K"),
                    id: String::new(),
                    id_sistema_informatico: String::from("77"),
                    version: String::from("1.0.03"),
                    numero_instalacion: String::from("383"),
                    nif_obligado: String::from("89890001K"),
                    tipo_evento: String::from("OTROS_EVENTOS_VOLUNTARIOS"),
                },
                fecha_huso_gen_evento: instante("2025-02-03T14:30:00+01:00"),
            },
            0,
            None,
        );
        let _ = ChainRecord::seal(
            ChainKind::Alta {
                issuer: String::from("B12345678"),
                serie: Series::T,
                number: 42,
                fecha_expedicion: fecha("01-01-2024"),
                tipo_factura: TipoFactura::F2,
                cuota_total: Money::from_cents(1235),
                importe_total: Money::from_cents(12_450),
                fecha_huso_gen: instante("2024-01-01T19:20:30+01:00"),
            },
            0,
            Some(&event.predecessor()),
        );
    }
}
