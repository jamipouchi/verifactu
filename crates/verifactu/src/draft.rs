//! The invoice-level face: one `InvoiceDraft` in, one sealed, submitted
//! record and its chain cursor out.

use crate::builder::Environment;
use crate::domain::chain::{
    ChainKind, ChainRecord, FechaExpedicion, FechaHuso, Predecessor, TipoFactura,
};
use crate::domain::money::Money;
use crate::domain::series::Series;
use crate::fiscal::{
    huso, DetalleDesglose, Emission, EmissionContext, EmitError, FacturaId, IDDestinatario,
    ImporteRectificacion, Modality, SiNo, TipoRectificativa, VerifactuEmitter,
};
use crate::qr::verification_url;

/// One desglose line's 90% shapes; a hand-built
/// [`DetalleDesglose`] via [`InvoiceDraft::line`] is the full door.
pub mod lines {
    use super::{DetalleDesglose, Money};

    /// Integer math (money never floats), half up away from zero;
    /// `ImporteSgn12.2` × 100 % cannot overflow `i64` cents, so the
    /// checked product's panic is a bug, never an input.
    fn cuota_cents(rate_cents: i64, base_cents: i64) -> i64 {
        let product = rate_cents
            .checked_mul(base_cents)
            .expect("ImporteSgn12.2 × rate fits i64 cents");
        let sign = if product < 0 { -1_i64 } else { 1 };
        ((product.unsigned_abs() + 5_000) / 10_000).cast_signed() * sign
    }

    /// An IVA line at `rate` cents (2100 = 21.00 %) over `base` cents —
    /// the cuota is computed from the pair, so it can never disagree
    /// with its rate.
    #[must_use]
    pub fn sujeta_iva(rate_cents: i64, base_cents: i64) -> DetalleDesglose {
        DetalleDesglose {
            impuesto: Some(crate::fiscal::Impuesto::Iva),
            clave_regimen: Some(crate::fiscal::ClaveRegimen::C01),
            calificacion: Some(crate::fiscal::CalificacionOperacion::SujetaNoExenta),
            operacion_exenta: None,
            tipo_impositivo: Some(Money::from_cents(rate_cents)),
            base_imponible: Money::from_cents(base_cents),
            cuota_repercutida: Some(Money::from_cents(cuota_cents(rate_cents, base_cents))),
            tipo_recargo_equivalencia: None,
            cuota_recargo_equivalencia: None,
        }
    }

    /// An exempt line (`E1`..`E8`).
    #[must_use]
    pub fn exenta(exenta: crate::fiscal::OperacionExenta, base_cents: i64) -> DetalleDesglose {
        DetalleDesglose {
            impuesto: Some(crate::fiscal::Impuesto::Iva),
            clave_regimen: Some(crate::fiscal::ClaveRegimen::C01),
            calificacion: None,
            operacion_exenta: Some(exenta),
            tipo_impositivo: None,
            base_imponible: Money::from_cents(base_cents),
            cuota_repercutida: None,
            tipo_recargo_equivalencia: None,
            cuota_recargo_equivalencia: None,
        }
    }

    /// A not-subject line (`N1`/`N2`).
    #[must_use]
    pub fn no_sujeta(
        calificacion: crate::fiscal::CalificacionOperacion,
        base_cents: i64,
    ) -> DetalleDesglose {
        DetalleDesglose {
            impuesto: Some(crate::fiscal::Impuesto::Iva),
            clave_regimen: Some(crate::fiscal::ClaveRegimen::C01),
            calificacion: Some(calificacion),
            operacion_exenta: None,
            tipo_impositivo: None,
            base_imponible: Money::from_cents(base_cents),
            cuota_repercutida: None,
            tipo_recargo_equivalencia: None,
            cuota_recargo_equivalencia: None,
        }
    }
}

/// One invoice at business level — the facade seals the chain record
/// (totals from the lines, macrodato at the 100 M threshold, dates at
/// the site zone) and submits; the consumer supplies only the chain
/// state. The 1189 law (destinatarios on F1/F3/R1–R4) is enforced
/// here, before any byte serializes.
#[derive(Debug, Default)]
pub struct InvoiceDraft {
    serie: Option<(Series, u64)>,
    tipo: Option<TipoFactura>,
    descripcion: Option<String>,
    lines: Vec<DetalleDesglose>,
    destinatarios: Vec<IDDestinatario>,
    rectificativa: Option<(
        TipoRectificativa,
        Vec<FacturaId>,
        Option<ImporteRectificacion>,
    )>,
    ref_externa: Option<String>,
}

impl InvoiceDraft {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The serie and correlative (the durable invoice number).
    #[must_use]
    pub fn serie(mut self, serie: Series, number: u64) -> Self {
        self.serie = Some((serie, number));
        self
    }

    /// The `TipoFactura`; defaults to `F2` on serie `T` and `F1` on
    /// series `F`/`R` (a custom series demands it — no implied
    /// document class).
    #[must_use]
    pub fn tipo(mut self, tipo: TipoFactura) -> Self {
        self.tipo = Some(tipo);
        self
    }

    /// The free-text description (`DescripcionOperacion`).
    #[must_use]
    pub fn descripcion(mut self, descripcion: impl Into<String>) -> Self {
        self.descripcion = Some(descripcion.into());
        self
    }

    #[must_use]
    pub fn line(mut self, line: DetalleDesglose) -> Self {
        self.lines.push(line);
        self
    }

    /// The counterparty (REQUIRED on F1/F3/R1–R4 — AEAT 1189).
    #[must_use]
    pub fn destinatario(mut self, destinatario: IDDestinatario) -> Self {
        self.destinatarios.push(destinatario);
        self
    }

    /// Marks the invoice as a rectificativa over `facturas`.
    #[must_use]
    pub fn rectificativa(
        mut self,
        tipo: TipoRectificativa,
        facturas: Vec<FacturaId>,
        importe_rectificacion: Option<ImporteRectificacion>,
    ) -> Self {
        self.rectificativa = Some((tipo, facturas, importe_rectificacion));
        self
    }

    /// The optional external reference (`RefExterna`).
    #[must_use]
    pub fn ref_externa(mut self, ref_externa: impl Into<String>) -> Self {
        self.ref_externa = Some(ref_externa.into());
        self
    }

    /// The serie/number gates, answering the rendered
    /// `NumSerieFactura`: the correlative renders 8 digits, a custom
    /// prefix is named, and the rendering meets its wire law.
    ///
    /// # Errors
    /// [`EmitError::InvalidRecord`] naming the broken bound.
    fn num_serie(serie: &Series, number: u64) -> Result<String, EmitError> {
        let invalid = |detail: &str| EmitError::InvalidRecord {
            detail: detail.to_owned(),
        };
        let Some(num_serie) = crate::domain::series::checked_format(serie, number) else {
            return Err(invalid(
                "the correlative is 1..=99_999_999 — a gapless series cannot reach a 9-digit \
                 correlative (NumSerieFactura renders 8)",
            ));
        };
        if matches!(serie, Series::Custom(prefix) if prefix.is_empty()) {
            return Err(invalid(
                "a custom series prefix is required (the builtin prefixes are T/F/R)",
            ));
        }
        crate::fiscal::xml::check_num_serie("NumSerieFactura", &num_serie)?;
        Ok(num_serie)
    }

    /// Validates the draft into what [`emit_draft`] seals — the facade's
    /// one construction law: totals from the lines, macrodato at the
    /// threshold, instants from `now`, dates at the site zone. The
    /// issuer is the emitter's obligado NIF — never a second copy that
    /// could disagree.
    ///
    /// # Errors
    /// [`EmitError::InvalidRecord`] naming the broken law.
    fn build(self, issuer: &str, now: u64) -> Result<Built, EmitError> {
        let invalid = |detail: String| EmitError::InvalidRecord { detail };
        let Some((serie, number)) = self.serie else {
            return Err(invalid(String::from(
                "the invoice's serie and number are required",
            )));
        };
        let num_serie = Self::num_serie(&serie, number)?;
        let Some(descripcion) = self.descripcion.filter(|text| !text.is_empty()) else {
            return Err(invalid(String::from(
                "the invoice's description is required",
            )));
        };
        if self.lines.is_empty() {
            return Err(invalid(String::from(
                "at least one desglose line is required",
            )));
        }
        let tipo = match (self.tipo, &serie) {
            (Some(tipo), _) => tipo,
            (None, Series::T) => TipoFactura::F2,
            (None, Series::F | Series::R) => TipoFactura::F1,
            (None, Series::Custom(_)) => {
                return Err(invalid(String::from(
                    "a custom series carries no default TipoFactura — name it (.tipo(..))",
                )));
            }
        };
        if tipo.requires_destinatario() && self.destinatarios.is_empty() {
            return Err(invalid(format!(
                "TipoFactura {tipo:?} requires its destinatario (AEAT 1189)"
            )));
        }
        // Totals cohere by construction: Σ cuotas (incl. recargo) and
        // Σ base + Σ cuotas.
        let base_total: i64 = self
            .lines
            .iter()
            .map(|line| line.base_imponible.as_cents())
            .sum();
        let cuota_total: i64 = self
            .lines
            .iter()
            .flat_map(|line| [line.cuota_repercutida, line.cuota_recargo_equivalencia])
            .flatten()
            .map(Money::as_cents)
            .sum();
        let importe_total = base_total + cuota_total;

        let instants = huso::render_instants(crate::clock::zone::SiteZone::EuropeMadrid, now);
        let kind = ChainKind::Alta {
            issuer: issuer.to_owned(),
            serie,
            number,
            fecha_expedicion: FechaExpedicion::parse(&instants.fecha_expedicion)
                .expect("huso renders the dd-mm-yyyy grammar"),
            tipo_factura: tipo,
            cuota_total: Money::from_cents(cuota_total),
            importe_total: Money::from_cents(importe_total),
            fecha_huso_gen: FechaHuso::parse(&instants.fecha_huso_gen)
                .expect("huso renders the ISO-8601 grammar"),
        };
        Ok(Built {
            kind,
            num_serie,
            descripcion,
            lines: self.lines,
            destinatarios: self.destinatarios,
            rectificativa: self.rectificativa,
            ref_externa: self.ref_externa,
            macrodato: importe_total.abs() >= MACRODATO_CENTS,
        })
    }
}

/// `Macrodato=S` from an `ImporteTotal` of 100 M€ (AEAT's threshold).
const MACRODATO_CENTS: i64 = 10_000_000_000;

/// A validated draft: the alta's kind plus everything its emission
/// context borrows.
struct Built {
    kind: ChainKind,
    num_serie: String,
    descripcion: String,
    lines: Vec<DetalleDesglose>,
    destinatarios: Vec<IDDestinatario>,
    rectificativa: Option<(
        TipoRectificativa,
        Vec<FacturaId>,
        Option<ImporteRectificacion>,
    )>,
    ref_externa: Option<String>,
    macrodato: bool,
}

/// One invoice through the flat API: AEAT's verdict plus the chain
/// cursor. `cursor` is exactly what the NEXT invoice's `prev` takes —
/// persist it (it is the installation's whole durable chain state) and
/// pass it back; nothing else needs to survive the call. Hermetic
/// wiring is the same call over `CertificateSource::Custom` with
/// `engine::FakeSigner` + `engine::FakeVerifactuTransport` — no
/// hand-rolled stubs.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct EmittedInvoice {
    /// The envío's typed outcome — AEAT's per-line verdicts, the CSV,
    /// the disposition.
    pub emission: Emission,
    /// The sealed record's chain coordinates + huella — the next
    /// record's `prev` (see [`ChainRecord::predecessor`], the one
    /// constructor of the shape).
    pub cursor: Predecessor,
    /// What was sealed: the record's kind — the `TipoFactura` in force
    /// (after defaulting), the totals, the instants. An anulación's
    /// kind carries the annulled invoice's coordinates.
    pub kind: ChainKind,
    /// The rendered `NumSerieFactura` (prefix + 8-digit correlative).
    pub num_serie: String,
}

impl EmittedInvoice {
    /// The cotejo URL the printed QR encodes (QR spec §4–§5): THIS
    /// factura's four mandatory parameters over the environment ×
    /// modality base. `None` for anulación kinds — the printed QR is
    /// always the factura's own (cotejo of an annulled factura answers
    /// the annulment; no anulación QR shape exists).
    ///
    /// The image is the consumer's print pipeline, but two of its
    /// parameters are legal, not rendering, choices (art. 21 via §3):
    /// error-correction level **M** — a generic QR library's default L
    /// silently violates the spec — and 30×30–40×40 mm, with
    /// [`TRIBUTARY_CAPTION`](crate::TRIBUTARY_CAPTION) above and —
    /// remission modality — [`VERIFIABLE_CAPTION`](crate::VERIFIABLE_CAPTION)
    /// below.
    #[must_use]
    pub fn qr_url(&self, environment: Environment, modality: Modality) -> Option<String> {
        let ChainKind::Alta {
            issuer,
            fecha_expedicion,
            importe_total,
            ..
        } = &self.kind
        else {
            return None;
        };
        // The QR grammar caps the importe at 9 integer digits (spec
        // §5's `NNNNNNNNN.DD`) — a wider total has no on-grammar URL.
        if importe_total.as_cents().abs() >= 100_000_000_000 {
            return None;
        }
        Some(verification_url(
            environment,
            modality,
            issuer,
            &self.num_serie,
            fecha_expedicion,
            *importe_total,
        ))
    }
}

/// The invoice-level emit over ANY emitter (the testing seam — a
/// hermetic emitter is [`VerifactuEmitter`] + the re-exported
/// `engine::FakeSigner`/`engine::FakeVerifactuTransport`, feature `fakes`; production rides the
/// custody connections' own method).
///
/// Seals the draft's record onto `prev` (the installation's first
/// record when `None`), submits one envío, and returns the emission
/// WITH the chain cursor. The borrow of the draft's parts lives
/// exactly across this call — nothing leaks.
///
/// # Errors
/// [`EmitError::InvalidRecord`] when the draft itself is off-contract
/// (before any byte serializes); the rest of [`EmitError`] from the
/// emission pipeline.
pub async fn emit_draft(
    emitter: &VerifactuEmitter,
    draft: InvoiceDraft,
    prev: Option<&Predecessor>,
) -> Result<EmittedInvoice, EmitError> {
    let now = emitter.now_utc().0;
    let built = draft.build(&emitter.obligado().nif, now)?;
    let (tipo_rectificativa, facturas_rectificadas, importe_rectificacion) =
        match &built.rectificativa {
            Some((tipo, facturas, importe)) => (Some(*tipo), facturas.as_slice(), importe.clone()),
            None => (None, &[][..], None),
        };
    let context = EmissionContext {
        ref_externa: built.ref_externa.as_deref(),
        descripcion_operacion: Some(built.descripcion.as_str()),
        desglose: built.lines.as_slice(),
        tipo_rectificativa,
        facturas_rectificadas,
        importe_rectificacion,
        destinatarios: built.destinatarios.as_slice(),
        macrodato: built.macrodato.then_some(SiNo::Si),
        ..EmissionContext::default()
    };
    seal_and_submit(emitter, built.kind, now, prev, context, built.num_serie).await
}

/// The anulación-level emit over ANY emitter (the same testing seam as
/// [`emit_draft`]): seals a `RegistroAnulacion` for `annulled` (the
/// invoice's own stored [`Predecessor`]) onto `prev`, submits it with
/// the anulación's legal context (all-default), and returns the
/// emission WITH the anulación record's cursor — the new chain head.
///
/// # Errors
/// [`EmitError::InvalidRecord`] when `annulled` names an event record
/// (only invoices are annullable); the rest of [`EmitError`] from the
/// emission pipeline.
///
/// # Panics
/// Never on input: the instant renders come from `huso` itself.
pub async fn cancel_draft(
    emitter: &VerifactuEmitter,
    annulled: &Predecessor,
    prev: Option<&Predecessor>,
) -> Result<EmittedInvoice, EmitError> {
    let Predecessor::Factura(invoice) = annulled else {
        return Err(EmitError::InvalidRecord {
            detail: String::from("only invoices are annullable — an event record is not"),
        });
    };
    let Some(num_serie) = crate::domain::series::checked_format(&invoice.serie, invoice.number)
    else {
        return Err(EmitError::InvalidRecord {
            detail: String::from("the annulled invoice's correlative exceeds 99_999_999"),
        });
    };
    let now = emitter.now_utc().0;
    let instants = huso::render_instants(crate::clock::zone::SiteZone::EuropeMadrid, now);
    let kind = ChainKind::Anulacion {
        issuer: invoice.issuer.clone(),
        serie: invoice.serie.clone(),
        number: invoice.number,
        fecha_expedicion: invoice.fecha_expedicion.clone(),
        fecha_huso_gen: FechaHuso::parse(&instants.fecha_huso_gen)
            .expect("huso renders the ISO-8601 grammar"),
    };
    seal_and_submit(
        emitter,
        kind,
        now,
        prev,
        EmissionContext::default(),
        num_serie,
    )
    .await
}

/// Seals `kind` onto `prev` at `now`, submits the one-record envío, and
/// answers the emission with the sealed record's cursor.
async fn seal_and_submit(
    emitter: &VerifactuEmitter,
    kind: ChainKind,
    now: u64,
    prev: Option<&Predecessor>,
    context: EmissionContext<'_>,
    num_serie: String,
) -> Result<EmittedInvoice, EmitError> {
    let record = ChainRecord::seal(kind.clone(), now, prev);
    let cursor = record.predecessor();
    let emission = emitter.submit(&[(record, context)]).await?;
    Ok(EmittedInvoice {
        emission,
        cursor,
        kind,
        num_serie,
    })
}
