//! AEAT's business validations of a `RegistroAlta` (Validaciones v1.2.2
//! §3.1.3 and §15, `contracts/aeat-verifactu/SOURCES.md`): the rules
//! whose breach REJECTS the record in remisión voluntaria — and so must
//! be refused before the record is sealed into the chain.
//!
//! Bajo requerimiento AEAT admits every business error (§4.3.2: the
//! conserved records travel as they were generated), so the emitter
//! applies these in the remission modality only. The rules AEAT answers
//! with a mere "aviso" — the `CuotaTotal` / `ImporteTotal` sums, the huella,
//! `PrimerRegistro`, `FechaHoraHusoGenRegistro` — are never refused.
//!
//! "IVA" below includes an absent `Impuesto`, as AEAT reads it.

use crate::domain::chain::{FechaExpedicion, FechaHuso, TipoFactura};
use crate::domain::money::Money;
use crate::fiscal::xml::SerializeError;
use crate::fiscal::{
    CalificacionOperacion as Calificacion, ClaveRegimen as Clave, DetalleDesglose, EmissionContext,
    IdOtroType, IdentificacionDestinatario, Impuesto, OperacionExenta as Exenta, RechazoPrevio,
    SiNo, TipoRectificativa,
};

type Ymd = (i64, u32, u32);

/// The Veri*FACTU order's entry into force (§3.1.3.1).
const EN_VIGOR: Ymd = (2024, 10, 28);

fn refuse(field: &'static str, problem: impl Into<String>) -> SerializeError {
    SerializeError::Field {
        field,
        problem: problem.into(),
    }
}

fn within(date: Ymd, from: Ymd, to: Ymd) -> bool {
    from <= date && date <= to
}

/// Every business rule of one alta.
///
/// # Errors
/// [`SerializeError::Field`] naming the field and the rule's section.
pub(crate) fn check_alta(
    ctx: &EmissionContext<'_>,
    tipo: TipoFactura,
    fecha: &FechaExpedicion,
    generado: &FechaHuso,
    num_serie: &str,
    importe_total: Money,
) -> Result<(), SerializeError> {
    let date = fecha.ymd();
    if date < EN_VIGOR {
        return Err(refuse(
            "IDFactura/FechaExpedicionFactura",
            format!("{fecha} precedes 28-10-2024, the order's entry into force (§3.1.3.1)"),
        ));
    }
    // AEAT measures "the current date" at reception, never before the
    // record was generated: a factura dated after its own record's
    // generation is refused whenever it is sent early enough.
    if date > generado.ymd() {
        return Err(refuse(
            "IDFactura/FechaExpedicionFactura",
            format!(
                "{fecha} is after the record's generation ({generado}) — a factura is never \
                 issued in the future (AEAT 1112, §3.1.3.1)"
            ),
        ));
    }
    if let Some(forbidden) = num_serie.chars().find(|c| "\"'<>=".contains(*c)) {
        return Err(refuse(
            "IDFactura/NumSerieFactura",
            format!("{forbidden:?} is not admitted in an alta's NumSerieFactura (§3.1.3.1)"),
        ));
    }
    if matches!(
        ctx.rechazo_previo,
        Some(RechazoPrevio::Rechazado | RechazoPrevio::NoRemitido)
    ) && ctx.subsanacion != Some(SiNo::Si)
    {
        return Err(refuse(
            "RechazoPrevio",
            "S or X only on a subsanación (Subsanacion=S) (§3.1.3.2)",
        ));
    }
    check_rectificacion(ctx, tipo)?;
    if importe_total.as_cents().abs() >= 10_000_000_000 && ctx.macrodato.is_none() {
        return Err(refuse(
            "Macrodato",
            "required when |ImporteTotal| ≥ 100.000.000,00 (§3.1.3.10)",
        ));
    }
    check_destinatarios(ctx, tipo)?;
    for detalle in ctx.desglose {
        check_detalle(detalle, ctx, tipo, date)?;
    }
    if tipo == TipoFactura::F2 {
        let total: i64 = ctx
            .desglose
            .iter()
            .map(|d| d.base_imponible.as_cents() + d.cuota_repercutida.map_or(0, Money::as_cents))
            .sum();
        if total > 301_000 {
            return Err(refuse(
                "Desglose",
                "an F2's Σ(base + cuota) is at most 3.000,00 (+10,00) (§15.8)",
            ));
        }
    }
    Ok(())
}

/// §3.1.3.3, §3.1.3.4, §3.1.3.6.
fn check_rectificacion(ctx: &EmissionContext<'_>, tipo: TipoFactura) -> Result<(), SerializeError> {
    if tipo.is_rectificativa() != ctx.tipo_rectificativa.is_some() {
        return Err(refuse(
            "TipoRectificativa",
            format!(
                "required on R1–R5 and only there, got TipoFactura {} (§3.1.3.3; on the flat \
                 face, InvoiceDraft::rectificativa)",
                tipo.as_str()
            ),
        ));
    }
    if !tipo.is_rectificativa() && !ctx.facturas_rectificadas.is_empty() {
        return Err(refuse("FacturasRectificadas", "only on R1–R5 (§3.1.3.4)"));
    }
    let sustitutiva = ctx.tipo_rectificativa == Some(TipoRectificativa::Sustitutiva);
    if sustitutiva != ctx.importe_rectificacion.is_some() {
        return Err(refuse(
            "ImporteRectificacion",
            "required when TipoRectificativa is S, and only then (§3.1.3.6)",
        ));
    }
    Ok(())
}

/// §3.1.3.13 and AEAT's error list (the identification rules of
/// `IDOtro` are checked in every regime, by the serializer).
fn check_destinatarios(ctx: &EmissionContext<'_>, tipo: TipoFactura) -> Result<(), SerializeError> {
    if tipo.requires_destinatario() && ctx.destinatarios.is_empty() {
        return Err(refuse(
            "Destinatarios",
            format!(
                "TipoFactura {} requires its counterparty (AEAT 1189, §3.1.3.13)",
                tipo.as_str()
            ),
        ));
    }
    if !tipo.requires_destinatario() && !ctx.destinatarios.is_empty() {
        return Err(refuse(
            "Destinatarios",
            format!(
                "TipoFactura {} (simplificada) carries no Destinatarios (§3.1.3.13)",
                tipo.as_str()
            ),
        ));
    }
    // R3 (bad debts) and R2 (concurso) name a counterparty AEAT can
    // follow: a NIF, a no-censado, and on R2 a NIF-IVA too.
    let admitted = |id_type| match tipo {
        TipoFactura::R3 => id_type == IdOtroType::NoCensado,
        TipoFactura::R2 => matches!(id_type, IdOtroType::NoCensado | IdOtroType::NifIva),
        _ => true,
    };
    for destinatario in ctx.destinatarios {
        if let IdentificacionDestinatario::Otro(otro) = &destinatario.identificacion {
            if !admitted(otro.id_type) {
                return Err(refuse(
                    "IDDestinatario/IDOtro/IDType",
                    format!(
                        "TipoFactura {} admits a NIF, IDType 07{} only (AEAT 1191, 1192)",
                        tipo.as_str(),
                        if tipo == TipoFactura::R2 {
                            " or 02"
                        } else {
                            ""
                        }
                    ),
                ));
            }
        }
    }
    Ok(())
}

/// The ten IVA rates by date (§15.1): 5 % from 01-07-2022 to 30-09-2024;
/// 2 % and 7,5 % from 01-10-2024 to 31-12-2024.
fn iva_rate_admitted(rate: i64, date: Ymd) -> bool {
    match rate {
        0 | 400 | 1000 | 2100 => true,
        500 => within(date, (2022, 7, 1), (2024, 9, 30)),
        200 | 750 => within(date, (2024, 10, 1), (2024, 12, 31)),
        _ => false,
    }
}

/// The recargo de equivalencia each IVA rate admits (§15.3).
fn recargo_admitted(rate: i64, recargo: i64, date: Ymd) -> bool {
    if ![0, 26, 50, 62, 100, 140, 175, 520].contains(&recargo) {
        return false;
    }
    match rate {
        2100 => matches!(recargo, 520 | 175),
        1000 => recargo == 140,
        750 => recargo == 100,
        500 if date <= (2022, 12, 31) => recargo == 50,
        500 => recargo == 62,
        400 => recargo == 50,
        200 => recargo == 26,
        0 if within(date, (2023, 1, 1), (2024, 9, 30)) => recargo == 0,
        _ => true,
    }
}

#[allow(clippy::too_many_lines)] // one rule per section, in the document's order
fn check_detalle(
    d: &DetalleDesglose,
    ctx: &EmissionContext<'_>,
    tipo: TipoFactura,
    date: Ymd,
) -> Result<(), SerializeError> {
    let iva = matches!(d.impuesto, None | Some(Impuesto::Iva));
    let igic = d.impuesto == Some(Impuesto::Igic);
    let rated = d.tipo_impositivo.is_some()
        || d.cuota_repercutida.is_some()
        || d.tipo_recargo_equivalencia.is_some()
        || d.cuota_recargo_equivalencia.is_some();
    let field = "DetalleDesglose";

    if d.tipo_recargo_equivalencia.is_some() != d.cuota_recargo_equivalencia.is_some() {
        return Err(refuse(
            field,
            "TipoRecargoEquivalencia and CuotaRecargoEquivalencia go together (AEAT 1284)",
        ));
    }
    // §15.1, §15.3 — the rates an IVA S1 line admits.
    if iva && d.calificacion == Some(Calificacion::SujetaNoExenta) {
        if let Some(rate) = d.tipo_impositivo.map(Money::as_cents) {
            if !iva_rate_admitted(rate, date) {
                return Err(refuse(
                    "DetalleDesglose/TipoImpositivo",
                    format!(
                        "{} % is not an IVA rate on {} (0, 4, 10, 21; 5 until 30-09-2024; 2 and \
                         7,5 in Q4 2024) (§15.1)",
                        Money::from_cents(rate),
                        date_label(date)
                    ),
                ));
            }
            if let Some(recargo) = d.tipo_recargo_equivalencia.map(Money::as_cents) {
                if !recargo_admitted(rate, recargo, date) {
                    return Err(refuse(
                        "DetalleDesglose/TipoRecargoEquivalencia",
                        format!(
                            "{} % is not the recargo IVA {} % admits (§15.3)",
                            Money::from_cents(recargo),
                            Money::from_cents(rate)
                        ),
                    ));
                }
            }
        }
    }
    // §15.4
    match d.calificacion {
        Some(Calificacion::SujetaNoExentaInversion) => {
            if !tipo.requires_destinatario() {
                return Err(refuse(field, "S2 only on F1, F3, R1–R4 (§15.4)"));
            }
            let zero = Some(Money::ZERO.as_cents());
            if d.tipo_impositivo.map(Money::as_cents) != zero
                || d.cuota_repercutida.map(Money::as_cents) != zero
            {
                return Err(refuse(
                    field,
                    "S2 carries TipoImpositivo = 0 and CuotaRepercutida = 0, both present (§15.4)",
                ));
            }
            if d.tipo_recargo_equivalencia.is_some() || d.cuota_recargo_equivalencia.is_some() {
                return Err(refuse(field, "recargo rides S1 lines only (AEAT 1281)"));
            }
        }
        Some(Calificacion::NoSujetaOtras | Calificacion::NoSujetaLocalizacion) if iva && rated => {
            return Err(refuse(
                field,
                "an IVA N1/N2 line carries no rates and no cuotas (§15.4)",
            ));
        }
        _ => {}
    }
    // §15.5, §15.5.1
    if let Some(exenta) = d.operacion_exenta {
        if iva && matches!(exenta, Exenta::E7 | Exenta::E8) {
            return Err(refuse(
                "DetalleDesglose/OperacionExenta",
                format!("{} is IGIC-only; IVA admits E1–E6 (§15.5)", exenta.wire()),
            ));
        }
        if (iva || igic)
            && d.clave_regimen == Some(Clave::C01)
            && matches!(exenta, Exenta::E2 | Exenta::E3)
        {
            return Err(refuse(
                "DetalleDesglose/OperacionExenta",
                format!(
                    "{} is not admitted under ClaveRegimen 01 (§15.5) — exports ride 02",
                    exenta.wire()
                ),
            ));
        }
        if rated {
            return Err(refuse(
                field,
                "an exempt line carries no rates and no cuotas (§15.5)",
            ));
        }
        if iva
            && exenta == Exenta::E5
            && ctx
                .destinatarios
                .iter()
                .any(|dest| matches!(dest.identificacion, IdentificacionDestinatario::Nif(_)))
        {
            return Err(refuse(
                "Destinatarios",
                "an IVA E5 line identifies its destinatarios by IDOtro only (§15.5.1)",
            ));
        }
    }
    // §15.6 — ClaveRegimen by tax, then per clave.
    match (d.impuesto, d.clave_regimen) {
        (Some(Impuesto::Otros), Some(_)) => {
            return Err(refuse(
                "DetalleDesglose/ClaveRegimen",
                "not admitted with Impuesto 05 (§15.6)",
            ));
        }
        (None | Some(Impuesto::Iva | Impuesto::Igic), None) => {
            return Err(refuse(
                "DetalleDesglose/ClaveRegimen",
                "required with IVA and IGIC (AEAT 1245, §15.6)",
            ));
        }
        (Some(Impuesto::Ipsi), clave)
            if date >= (2027, 1, 1)
                && !matches!(
                    clave,
                    Some(
                        Clave::C01 | Clave::C08 | Clave::C11 | Clave::C18 | Clave::C19 | Clave::C20
                    )
                ) =>
        {
            return Err(refuse(
                "DetalleDesglose/ClaveRegimen",
                "IPSI requires 01, 08, 11, 18, 19 or 20 from 01-01-2027 (§15.6)",
            ));
        }
        _ => {}
    }
    if iva || igic {
        check_clave(d, ctx, tipo, iva, igic)?;
    }
    // §15.7
    let cuota = d.cuota_repercutida.map(Money::as_cents);
    if cuota.is_some_and(|c| c != 0) && d.calificacion != Some(Calificacion::SujetaNoExenta) {
        return Err(refuse(
            "DetalleDesglose/CuotaRepercutida",
            "non-zero only on an S1 line (§15.7)",
        ));
    }
    if d.calificacion == Some(Calificacion::SujetaNoExenta) {
        let (Some(rate), Some(cuota)) = (d.tipo_impositivo.map(Money::as_cents), cuota) else {
            return Err(refuse(
                field,
                "an S1 line carries TipoImpositivo and CuotaRepercutida (§15.7)",
            ));
        };
        let exempt_from_ratio = ctx.tipo_rectificativa == Some(TipoRectificativa::Incremental)
            || matches!(tipo, TipoFactura::R2 | TipoFactura::R3);
        if !exempt_from_ratio {
            let base = d.base_imponible.as_cents();
            if (cuota > 0 && base < 0) || (cuota < 0 && base > 0) {
                return Err(refuse(
                    "DetalleDesglose/CuotaRepercutida",
                    "the cuota and its base share their sign (§15.7)",
                ));
            }
            // |cuota − base × rate / 100| ≤ 10,00, in cents × 10 000.
            let gap = i128::from(cuota) * 10_000 - i128::from(base) * i128::from(rate);
            if gap.abs() > 1_000 * 10_000 {
                return Err(refuse(
                    "DetalleDesglose/CuotaRepercutida",
                    format!(
                        "{} is not {} × {} % (±10,00) (§15.7)",
                        Money::from_cents(cuota),
                        d.base_imponible,
                        Money::from_cents(rate)
                    ),
                ));
            }
        }
    }
    Ok(())
}

/// §15.6.1–§15.6.10, for IVA and IGIC lines.
fn check_clave(
    d: &DetalleDesglose,
    ctx: &EmissionContext<'_>,
    tipo: TipoFactura,
    iva: bool,
    igic: bool,
) -> Result<(), SerializeError> {
    let field = "DetalleDesglose/ClaveRegimen";
    let calificacion = d.calificacion;
    let ok = match d.clave_regimen {
        Some(Clave::C02) => calificacion.is_none(),
        Some(Clave::C03) => matches!(calificacion, None | Some(Calificacion::SujetaNoExenta)),
        Some(Clave::C04) => matches!(
            calificacion,
            None | Some(Calificacion::SujetaNoExentaInversion)
        ),
        Some(Clave::C06 | Clave::C14) => {
            return Err(refuse(
                field,
                "06 requires BaseImponibleACoste and 14 FechaOperacion, which this crate does \
                 not emit (§15.6.4, §15.6.9)",
            ));
        }
        Some(Clave::C07) => {
            !matches!(
                calificacion,
                Some(
                    Calificacion::SujetaNoExentaInversion
                        | Calificacion::NoSujetaOtras
                        | Calificacion::NoSujetaLocalizacion
                )
            ) && !matches!(
                d.operacion_exenta,
                Some(Exenta::E2 | Exenta::E3 | Exenta::E4 | Exenta::E5)
            )
        }
        Some(Clave::C08) => calificacion == Some(Calificacion::NoSujetaLocalizacion),
        Some(Clave::C10) => {
            calificacion == Some(Calificacion::NoSujetaOtras)
                && tipo == TipoFactura::F1
                && ctx
                    .destinatarios
                    .iter()
                    .all(|dest| matches!(dest.identificacion, IdentificacionDestinatario::Nif(_)))
        }
        Some(Clave::C11) if iva => d.tipo_impositivo.is_none_or(|r| r.as_cents() == 2100),
        Some(Clave::C20) if igic => calificacion == Some(Calificacion::NoSujetaLocalizacion),
        // 18 (recargo de equivalencia) binds nothing: Validaciones v1.0.7
        // lifted the rule keeping recargo under 18 and v1.1.2 deleted its
        // §15.6.9 (1279 and 1280 are gone from the error list), so AEAT
        // admits recargo under 01 and 18 without it (live-confirmed:
        // pruebas answered Correcto an F1 line at 21 % with recargo 5,2
        // under 01 on 2026-10-16, and an F1 and an F2 at 21 % under 18
        // with no recargo on 2026-10-17). The key names the issuer's
        // regime, not the recargo; `lines::RegimenEmisor` is where the two
        // meet.
        _ => true,
    };
    if ok {
        return Ok(());
    }
    let clave = d.clave_regimen.map_or("", Clave::wire);
    Err(refuse(
        field,
        format!(
            "ClaveRegimen {clave} does not admit this line (calificación {:?}, exenta {:?}, \
             TipoFactura {}) (§15.6)",
            calificacion.map(Calificacion::wire),
            d.operacion_exenta.map(Exenta::wire),
            tipo.as_str()
        ),
    ))
}

fn date_label((year, month, day): Ymd) -> String {
    format!("{day:02}-{month:02}-{year}")
}

#[cfg(test)]
mod tests {
    use super::check_alta;
    use crate::domain::chain::{FechaExpedicion, FechaHuso, TipoFactura};
    use crate::domain::money::Money;
    use crate::draft::lines;
    use crate::fiscal::tests_support::{iva_super, obligado, primer_alta_record, sif};
    use crate::fiscal::{
        xml, CalificacionOperacion as Calificacion, ClaveRegimen as Clave, DetalleDesglose,
        EmissionContext, IDDestinatario, IdOtro, IdOtroType, IdentificacionDestinatario, Impuesto,
        Modality, OperacionExenta as Exenta, RechazoPrevio, SiNo, TipoRectificativa,
    };

    fn fecha(value: &str) -> FechaExpedicion {
        FechaExpedicion::parse(value).expect("a real date")
    }

    /// The record generated at noon of the factura's own day.
    fn generado(fecha: &FechaExpedicion) -> FechaHuso {
        let (year, month, day) = fecha.ymd();
        FechaHuso::parse(&format!("{year}-{month:02}-{day:02}T12:00:00+01:00"))
            .expect("an ISO instant")
    }

    fn nif_dest() -> IDDestinatario {
        IDDestinatario {
            nombre_razon: String::from("CLIENTE SL"),
            identificacion: IdentificacionDestinatario::Nif(String::from("B87654321")),
        }
    }

    /// Runs one alta through the rules: `Ok(())` or the refusal's text.
    fn verdict<'a>(
        tipo: TipoFactura,
        on: &str,
        lines: &'a [DetalleDesglose],
        with: impl FnOnce(&mut EmissionContext<'a>),
    ) -> Result<(), String> {
        let destinatarios: &'a [IDDestinatario] = Box::leak(Box::new([nif_dest()]));
        let mut ctx = EmissionContext {
            descripcion_operacion: Some("venta"),
            desglose: lines,
            destinatarios: if tipo.requires_destinatario() {
                destinatarios
            } else {
                &[]
            },
            ..EmissionContext::default()
        };
        if tipo.is_rectificativa() {
            ctx.tipo_rectificativa = Some(TipoRectificativa::Incremental);
        }
        with(&mut ctx);
        let importe = lines
            .iter()
            .map(|l| l.base_imponible.as_cents() + l.cuota_repercutida.map_or(0, Money::as_cents))
            .sum();
        check_alta(
            &ctx,
            tipo,
            &fecha(on),
            &generado(&fecha(on)),
            "T00000001",
            Money::from_cents(importe),
        )
        .map_err(|error| error.to_string())
    }

    fn refused(result: Result<(), String>, section: &str) {
        let error = result.expect_err("the rule refuses");
        assert!(error.contains(section), "cites {section}: {error}");
    }

    fn line(rate: i64, base: i64, cuota: i64) -> DetalleDesglose {
        DetalleDesglose {
            tipo_impositivo: Some(Money::from_cents(rate)),
            base_imponible: Money::from_cents(base),
            cuota_repercutida: Some(Money::from_cents(cuota)),
            ..iva_super()
        }
    }

    const F1: TipoFactura = TipoFactura::F1;
    const F2: TipoFactura = TipoFactura::F2;
    const DAY: &str = "03-02-2025";

    #[test]
    fn identification_and_dates_of_the_alta() {
        let ok = [iva_super()];
        assert_eq!(verdict(F2, "28-10-2024", &ok, |_| {}), Ok(()));
        refused(verdict(F2, "27-10-2024", &ok, |_| {}), "§3.1.3.1");
        let error = check_alta(
            &EmissionContext {
                descripcion_operacion: Some("venta"),
                desglose: &ok,
                ..EmissionContext::default()
            },
            F2,
            &fecha(DAY),
            &generado(&fecha(DAY)),
            "T<1",
            Money::from_cents(1210),
        );
        assert!(error.is_err_and(|e| e.to_string().contains("§3.1.3.1")));
        refused(
            verdict(F2, DAY, &ok, |c| {
                c.rechazo_previo = Some(RechazoPrevio::Rechazado);
            }),
            "§3.1.3.2",
        );
        assert_eq!(
            verdict(F2, DAY, &ok, |c| {
                c.rechazo_previo = Some(RechazoPrevio::NoRemitido);
                c.subsanacion = Some(SiNo::Si);
            }),
            Ok(())
        );
        let big = [line(2100, 9_000_000_000, 1_890_000_000)];
        refused(verdict(F1, DAY, &big, |_| {}), "§3.1.3.10");
        assert_eq!(
            verdict(F1, DAY, &big, |c| c.macrodato = Some(SiNo::Si)),
            Ok(())
        );
    }

    #[test]
    fn rectificativas_and_destinatarios() {
        let ok = [iva_super()];
        // FacturasRectificadas is optional (§3.1.3.4).
        assert_eq!(verdict(TipoFactura::R1, DAY, &ok, |_| {}), Ok(()));
        refused(
            verdict(F1, DAY, &ok, |c| {
                c.tipo_rectificativa = Some(TipoRectificativa::Incremental);
            }),
            "§3.1.3.3",
        );
        refused(
            verdict(TipoFactura::R1, DAY, &ok, |c| {
                c.tipo_rectificativa = Some(TipoRectificativa::Sustitutiva);
            }),
            "§3.1.3.6",
        );
        refused(verdict(F1, DAY, &ok, |c| c.destinatarios = &[]), "1189");
        let dest: &[IDDestinatario] = Box::leak(Box::new([nif_dest()]));
        refused(
            verdict(F2, DAY, &ok, |c| c.destinatarios = dest),
            "§3.1.3.13",
        );
        let otro = |id_type, pais: &str, id: &str| -> &'static [IDDestinatario] {
            Box::leak(Box::new([IDDestinatario {
                nombre_razon: String::from("CLIENTE"),
                identificacion: IdentificacionDestinatario::Otro(IdOtro {
                    codigo_pais: Some(pais.to_owned()),
                    id_type,
                    id: id.to_owned(),
                }),
            }]))
        };
        let pasaporte = otro(IdOtroType::Pasaporte, "FR", "12AB34567");
        let no_censado = otro(IdOtroType::NoCensado, "ES", "12345678Z");
        let nif_iva = otro(IdOtroType::NifIva, "DE", "DE123456789");
        let r = |tipo, dest| {
            verdict(tipo, DAY, &ok, |c| {
                c.destinatarios = dest;
                c.tipo_rectificativa = Some(TipoRectificativa::Incremental);
            })
        };
        assert_eq!(r(TipoFactura::R1, pasaporte), Ok(()));
        assert_eq!(r(TipoFactura::R3, no_censado), Ok(()));
        refused(r(TipoFactura::R3, nif_iva), "1191");
        assert_eq!(r(TipoFactura::R2, nif_iva), Ok(()));
        refused(r(TipoFactura::R2, pasaporte), "1192");
    }

    /// AEAT 1112: never dated after the record's own generation.
    #[test]
    fn a_factura_is_never_issued_in_the_future() {
        let ok = [iva_super()];
        let ctx = EmissionContext {
            descripcion_operacion: Some("venta"),
            desglose: &ok,
            ..EmissionContext::default()
        };
        let run = |on: &str, generado: &str| {
            check_alta(
                &ctx,
                F2,
                &fecha(on),
                &FechaHuso::parse(generado).expect("an ISO instant"),
                "T00000001",
                Money::from_cents(121),
            )
            .map_err(|error| error.to_string())
        };
        assert_eq!(run(DAY, "2025-02-03T23:59:59+01:00"), Ok(()));
        refused(run(DAY, "2025-02-02T23:59:59+01:00"), "1112");
    }

    #[test]
    fn rates_and_recargo_by_date() {
        assert_eq!(verdict(F2, DAY, &[line(1000, 1000, 100)], |_| {}), Ok(()));
        refused(verdict(F2, DAY, &[line(1800, 1000, 180)], |_| {}), "§15.1");
        refused(verdict(F2, DAY, &[line(500, 1000, 50)], |_| {}), "§15.1");
        assert_eq!(
            verdict(F2, "15-11-2024", &[line(200, 1000, 20)], |_| {}),
            Ok(())
        );
        // The key is the issuer's regime. A general-regime issuer
        // charging a recargo client declares 01 and carries the recargo...
        let general = |recargo_rate_cents| lines::RegimenEmisor::General { recargo_rate_cents };
        let recargo = lines::sujeta_iva_recargo(2100, general(520), 1000);
        assert_eq!(recargo.clave_regimen, Some(Clave::C01));
        assert_eq!(
            recargo.tipo_recargo_equivalencia,
            Some(Money::from_cents(520))
        );
        assert_eq!(
            recargo.cuota_recargo_equivalencia,
            Some(Money::from_cents(52))
        );
        assert_eq!(
            verdict(F1, DAY, std::slice::from_ref(&recargo), |_| {}),
            Ok(())
        );
        refused(
            verdict(
                F2,
                DAY,
                &[lines::sujeta_iva_recargo(2100, general(140), 1000)],
                |_| {},
            ),
            "§15.3",
        );
        // ...an issuer in recargo declares its own sale under 18, with none
        // (live-confirmed 2026-10-17, F1 and F2).
        let propia =
            lines::sujeta_iva_recargo(2100, lines::RegimenEmisor::RecargoEquivalencia, 1000);
        assert_eq!(propia.clave_regimen, Some(Clave::C18));
        assert_eq!(propia.tipo_recargo_equivalencia, None);
        assert_eq!(propia.cuota_recargo_equivalencia, None);
        assert_eq!(propia.cuota_repercutida, Some(Money::from_cents(210)));
        assert_eq!(verdict(F2, DAY, &[propia], |_| {}), Ok(()));
        // AEAT binds neither way, so a hand-built 18 with recargo is
        // admitted too (live-confirmed 2026-10-05, beside 01 on 2026-10-16).
        let recargo_18 = DetalleDesglose {
            clave_regimen: Some(Clave::C18),
            ..recargo.clone()
        };
        assert_eq!(verdict(F1, DAY, &[recargo_18], |_| {}), Ok(()));
        // AEAT 1284: the recargo's rate and cuota go together.
        let rate_only = DetalleDesglose {
            cuota_recargo_equivalencia: None,
            ..recargo.clone()
        };
        refused(verdict(F1, DAY, &[rate_only], |_| {}), "1284");
        let cuota_only = DetalleDesglose {
            tipo_recargo_equivalencia: None,
            ..recargo
        };
        refused(verdict(F1, DAY, &[cuota_only], |_| {}), "1284");
    }

    #[test]
    fn calificacion_exenta_and_clave() {
        let s2 = DetalleDesglose {
            calificacion: Some(Calificacion::SujetaNoExentaInversion),
            ..line(0, 1000, 0)
        };
        assert_eq!(verdict(F1, DAY, std::slice::from_ref(&s2), |_| {}), Ok(()));
        let s2_recargo = DetalleDesglose {
            tipo_recargo_equivalencia: Some(Money::from_cents(520)),
            cuota_recargo_equivalencia: Some(Money::ZERO),
            ..s2.clone()
        };
        refused(verdict(F1, DAY, &[s2_recargo], |_| {}), "1281");
        refused(verdict(F2, DAY, &[s2], |_| {}), "§15.4");
        let n1_rated = DetalleDesglose {
            calificacion: Some(Calificacion::NoSujetaOtras),
            ..iva_super()
        };
        refused(verdict(F2, DAY, &[n1_rated], |_| {}), "§15.4");
        assert_eq!(
            verdict(
                F2,
                DAY,
                &[lines::no_sujeta(Calificacion::NoSujetaOtras, 1000)],
                |_| {}
            ),
            Ok(())
        );
        for exenta in [Exenta::E1, Exenta::E2, Exenta::E3, Exenta::E6] {
            assert_eq!(
                verdict(F2, DAY, &[lines::exenta(exenta, 1000)], |_| {}),
                Ok(()),
                "the helper builds an admitted {exenta:?} line"
            );
        }
        let e2_general = DetalleDesglose {
            clave_regimen: Some(Clave::C01),
            ..lines::exenta(Exenta::E2, 1000)
        };
        refused(verdict(F2, DAY, &[e2_general], |_| {}), "§15.5");
        refused(
            verdict(F2, DAY, &[lines::exenta(Exenta::E7, 1000)], |_| {}),
            "§15.5",
        );
        let exempt_rated = DetalleDesglose {
            cuota_repercutida: Some(Money::from_cents(210)),
            ..lines::exenta(Exenta::E1, 1000)
        };
        refused(verdict(F2, DAY, &[exempt_rated], |_| {}), "§15.5");
        refused(
            verdict(F1, DAY, &[lines::exenta(Exenta::E5, 1000)], |_| {}),
            "§15.5.1",
        );
        let otros_clave = DetalleDesglose {
            impuesto: Some(Impuesto::Otros),
            ..iva_super()
        };
        refused(verdict(F2, DAY, &[otros_clave], |_| {}), "§15.6");
        let no_clave = DetalleDesglose {
            clave_regimen: None,
            ..iva_super()
        };
        refused(verdict(F2, DAY, &[no_clave], |_| {}), "1245");
        for clave in [Clave::C08, Clave::C06, Clave::C14] {
            let line = DetalleDesglose {
                clave_regimen: Some(clave),
                ..iva_super()
            };
            refused(verdict(F2, DAY, &[line], |_| {}), "§15.6");
        }
    }

    #[test]
    fn cuota_ratio_and_the_simplificada_ceiling() {
        refused(verdict(F2, DAY, &[line(2100, 1000, -210)], |_| {}), "§15.7");
        refused(
            verdict(F2, DAY, &[line(2100, 10_000, 3_101)], |_| {}),
            "§15.7",
        );
        assert_eq!(
            verdict(F2, DAY, &[line(2100, 10_000, 2_999)], |_| {}),
            Ok(())
        );
        let no_cuota = DetalleDesglose {
            cuota_repercutida: None,
            ..iva_super()
        };
        refused(verdict(F2, DAY, &[no_cuota], |_| {}), "§15.7");
        let igic_n2_cuota = DetalleDesglose {
            impuesto: Some(Impuesto::Igic),
            calificacion: Some(Calificacion::NoSujetaLocalizacion),
            tipo_impositivo: None,
            ..iva_super()
        };
        refused(verdict(F2, DAY, &[igic_n2_cuota], |_| {}), "§15.7");
        // R2/R3 and incremental rectificativas are exempt from the ratio.
        assert_eq!(
            verdict(TipoFactura::R3, DAY, &[line(2100, 10_000, 9_999)], |_| {}),
            Ok(())
        );
        // Σ(base + cuota) = 3.010,00 is the ceiling itself; one cent over is not.
        assert_eq!(
            verdict(F2, DAY, &[line(2100, 248_760, 52_240)], |_| {}),
            Ok(())
        );
        refused(
            verdict(F2, DAY, &[line(2100, 248_761, 52_240)], |_| {}),
            "§15.8",
        );
    }

    /// Bajo requerimiento AEAT admits every business error (§4.3.2): the
    /// record a remission emitter refuses, a conservation one serializes.
    #[test]
    fn the_business_rules_bind_the_remission_modality_only() {
        let lines = [iva_super()];
        let dest = [nif_dest()];
        let ctx = EmissionContext {
            descripcion_operacion: Some("venta"),
            desglose: &lines,
            destinatarios: &dest,
            ..EmissionContext::default()
        };
        let node =
            |modality| xml::record_node(&primer_alta_record(), &ctx, &obligado(), &sif(), modality);
        assert!(
            node(Modality::Remission).is_err(),
            "an F2 with destinatarios"
        );
        assert!(node(Modality::Conservation).is_ok());
    }
}
