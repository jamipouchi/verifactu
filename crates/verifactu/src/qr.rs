//! The printed-invoice QR: the cotejo URL AEAT's `ValidarQR` door
//! answers — pure data over what an emission already holds, no side
//! effects, no network.
//!
//! Law: `DetalleEspecificacTecnCodigoQRfactura.pdf` v0.5.0 (facts and
//! provenance in `contracts/aeat-verifactu/SOURCES.md`). The URL carries EXACTLY the four
//! mandatory parameters — `nif`, `numserie`, `fecha` (dd-mm-yyyy),
//! `importe` (2dp) — over one of four bases (§5): pruebas/production ×
//! verificable (`ValidarQR`, the remission modality) / no verificable
//! (`ValidarQRNoVerifactu`, conservation). Parameter content is ASCII
//! 32..=126 and rides URL-encoded UTF-8 (§4 — AEAT's own worked example
//! encodes a `&` inside `numserie` as `%26`). The optional `idioma` /
//! `formato` parameters (§6–§7) belong to machine cotejo (art. 20.2
//! receivers appending them to the structured e-invoice's URL), never
//! to the printed QR.
//!
//! Print law (art. 21 via §3): 30×30–40×40 mm, error-correction level
//! M, [`TRIBUTARY_CAPTION`] above, and — remission modality only —
//! [`VERIFIABLE_CAPTION`] (or its short form) below. An annulled
//! factura's printed QR is still ITS own URL: cotejo answers the
//! annulment, so no anulación QR shape exists.

use crate::domain::chain::{render_amount, FechaExpedicion};
use crate::domain::money::Money;
use crate::fiscal::Modality;
use crate::Environment;

/// `QR tributario:` — the caption that must precede every printed QR
/// (spec §3), distinguishing the fiscal QR from any other the factura
/// carries.
pub const TRIBUTARY_CAPTION: &str = "QR tributario:";

/// The verifiable phrase (spec §3), remission modality only;
/// [`VERIFIABLE_CAPTION_SHORT`] is the admitted alternative.
pub const VERIFIABLE_CAPTION: &str = "Factura verificable en la sede electrónica de la AEAT";

/// [`VERIFIABLE_CAPTION`]'s admitted short form.
pub const VERIFIABLE_CAPTION_SHORT: &str = "VERI*FACTU";

/// The four bases (spec §5.1 verifiable, §5.2 no verificable).
fn base(environment: Environment, modality: Modality) -> &'static str {
    match (environment, modality) {
        (Environment::Pruebas, Modality::Remission) => {
            "https://prewww2.aeat.es/wlpl/TIKE-CONT/ValidarQR"
        }
        (Environment::Pruebas, Modality::Conservation) => {
            "https://prewww2.aeat.es/wlpl/TIKE-CONT/ValidarQRNoVerifactu"
        }
        (Environment::Production, Modality::Remission) => {
            "https://www2.agenciatributaria.gob.es/wlpl/TIKE-CONT/ValidarQR"
        }
        (Environment::Production, Modality::Conservation) => {
            "https://www2.agenciatributaria.gob.es/wlpl/TIKE-CONT/ValidarQRNoVerifactu"
        }
    }
}

/// Percent-encode one parameter's CONTENT (spec §4): every byte outside
/// RFC 3986's unreserved set rides `%XX` (uppercase hex), so the query
/// separators and the numserie's grammar survive intact.
fn encode(value: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut out = String::with_capacity(value.len());
    for &byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            _ => {
                out.push('%');
                out.push(HEX[(byte >> 4) as usize] as char);
                out.push(HEX[(byte & 0x0f) as usize] as char);
            }
        }
    }
    out
}

/// The cotejo URL the printed QR encodes (spec §4–§5): the four
/// mandatory parameters over the environment × modality base. `fecha`
/// is the factura's `FechaExpedicionFactura` (dd-mm-yyyy), `importe`
/// its `ImporteTotal` at the wire grammar (2dp, unsigned zero).
///
/// The URL is this crate's law; the IMAGE is the consumer's — but two
/// of its parameters are legal, not rendering, choices: error
/// correction level **M** (a generic QR library's default L silently
/// violates the spec) and 30×30–40×40 mm. Print
/// [`TRIBUTARY_CAPTION`] above, and — remission modality —
/// [`VERIFIABLE_CAPTION`] below.
#[must_use]
pub fn verification_url(
    environment: Environment,
    modality: Modality,
    nif: &str,
    num_serie: &str,
    fecha_expedicion: &FechaExpedicion,
    importe_total: Money,
) -> String {
    format!(
        "{}?nif={}&numserie={}&fecha={}&importe={}",
        base(environment, modality),
        encode(nif),
        encode(num_serie),
        encode(fecha_expedicion.as_str()),
        encode(&render_amount(importe_total)),
    )
}

#[cfg(test)]
mod tests {
    use super::{verification_url, Modality, Money};
    use crate::domain::chain::FechaExpedicion;
    use crate::Environment;

    fn fecha() -> FechaExpedicion {
        FechaExpedicion::parse("01-09-2024").expect("the spec §8 example date")
    }

    /// Spec §5: each base names its environment's host and its
    /// modality's door (`ValidarQR` remission, `ValidarQRNoVerifactu`
    /// conservation); the four mandatory parameters ride the §5 order.
    #[test]
    fn the_four_bases_are_the_spec_section_five_law() {
        let url = |environment, modality| {
            verification_url(
                environment,
                modality,
                "89890001K",
                "12345678-G33",
                &fecha(),
                Money::from_cents(24_140),
            )
        };
        let remission = url(Environment::Pruebas, Modality::Remission);
        assert!(remission.contains("prewww2.aeat.es") && remission.contains("/ValidarQR?"));
        let conservation = url(Environment::Pruebas, Modality::Conservation);
        assert!(
            conservation.contains("prewww2.aeat.es")
                && conservation.contains("/ValidarQRNoVerifactu?")
        );
        let remission = url(Environment::Production, Modality::Remission);
        assert!(
            remission.contains("www2.agenciatributaria.gob.es")
                && remission.contains("/ValidarQR?")
        );
        let conservation = url(Environment::Production, Modality::Conservation);
        assert!(
            conservation.contains("www2.agenciatributaria.gob.es")
                && conservation.contains("/ValidarQRNoVerifactu?")
        );
        let at = |param: &str| remission.find(param).unwrap_or(usize::MAX);
        assert!(
            at("nif=") < at("&numserie=")
                && at("&numserie=") < at("&fecha=")
                && at("&fecha=") < at("&importe="),
            "the four parameters ride the §5 order: {remission}"
        );
    }

    /// Spec §4's own worked example: a `&` inside numserie rides `%26`;
    /// the series grammar's `/` rides `%2F`.
    #[test]
    fn parameter_content_is_percent_encoded() {
        let url = verification_url(
            Environment::Pruebas,
            Modality::Remission,
            "89890001K",
            "12345678&G33",
            &fecha(),
            Money::from_cents(24_140),
        );
        assert!(url.contains("numserie=12345678%26G33"), "{url}");
        let url = verification_url(
            Environment::Pruebas,
            Modality::Remission,
            "89890001K",
            "12345678/G33",
            &fecha(),
            Money::from_cents(24_140),
        );
        assert!(url.contains("numserie=12345678%2FG33"), "{url}");
    }
}
