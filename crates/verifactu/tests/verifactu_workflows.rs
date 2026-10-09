//! The paths no live authority can answer: event records, conservation
//! without a requerimiento, the subsanación shape, the error-code
//! contract.

use std::sync::Arc;

use verifactu::engine::ChainRecord;
use verifactu::engine::FakeVerifactuTransport;
use verifactu::engine::{EmissionContext, SiNo};
use verifactu::engine::{FakeSigner, RecordSigner, SistemaInformaticoConfig, VerifactuEmitter};
use verifactu::ErrorClass;
use verifactu::Money;
use verifactu::Series;
use verifactu::{ChainKind, FechaExpedicion, FechaHuso, TipoFactura};
use verifactu::{DetalleDesglose, EmitError, Modality, Obligado};

fn fecha(value: &str) -> FechaExpedicion {
    FechaExpedicion::parse(value).expect("dd-mm-yyyy grammar")
}

fn instante(value: &str) -> FechaHuso {
    FechaHuso::parse(value).expect("ISO-8601 grammar")
}

/// AEAT's example instant: Madrid `2025-02-03T14:30:00+01:00`.
const EMISSION_INSTANT: u64 = 1_738_589_400;

fn obligado() -> Obligado {
    Obligado {
        nombre_razon: String::from("Empresa Ejemplo"),
        nif: String::from("B12345678"),
    }
}

fn sif() -> SistemaInformaticoConfig {
    SistemaInformaticoConfig {
        nombre_razon: String::from("Empresa Ejemplo"),
        nif: String::from("B12345678"),
        nombre_sistema_informatico: String::from("test-erp"),
        id_sistema_informatico: String::from("1"),
        version: String::from("0.1.0"),
        numero_instalacion: String::from("1"),
    }
}

fn iva_super() -> DetalleDesglose {
    DetalleDesglose {
        impuesto: Some(verifactu::Impuesto::Iva),
        clave_regimen: Some(verifactu::ClaveRegimen::C01),
        calificacion: Some(verifactu::CalificacionOperacion::SujetaNoExenta),
        operacion_exenta: None,
        tipo_impositivo: Some(Money::from_cents(2100)),
        base_imponible: Money::from_cents(1000),
        cuota_repercutida: Some(Money::from_cents(210)),
        tipo_recargo_equivalencia: None,
        cuota_recargo_equivalencia: None,
    }
}

fn ticket_alta_record() -> ChainRecord {
    ChainRecord::seal(
        ChainKind::Alta {
            issuer: String::from("B12345678"),
            serie: Series::T,
            number: 42,
            fecha_expedicion: fecha("03-02-2025"),
            tipo_factura: TipoFactura::F2,
            cuota_total: Money::from_cents(210),
            importe_total: Money::from_cents(1210),
            fecha_huso_gen: instante("2025-02-03T14:30:00+01:00"),
        },
        EMISSION_INSTANT,
        None,
    )
}

fn ticket_ctx() -> EmissionContext<'static> {
    let desglose: &'static [DetalleDesglose] = Box::leak(Box::new([iva_super()]));
    EmissionContext {
        descripcion_operacion: Some("Venta mostrador"),
        desglose,
        ..EmissionContext::default()
    }
}

fn emitter(modality: Modality, signer: Arc<dyn RecordSigner>) -> VerifactuEmitter {
    VerifactuEmitter::new(modality, obligado(), sif(), signer).expect("valid identity")
}

#[tokio::test]
async fn evento_records_are_always_signed_never_remitted_and_chain_through_huellas() {
    let (signer, transport) = (
        Arc::new(FakeSigner::new()),
        Arc::new(FakeVerifactuTransport::new()),
    );
    let emitter = emitter(Modality::Remission, signer.clone()).with_transport(transport.clone());
    let sif = verifactu::engine::events::EventoSif {
        nif: String::from("B12345678"),
        id: String::new(),
        id_sistema_informatico: String::from("1"),
        version: String::from("0.1.0"),
        numero_instalacion: String::from("1"),
    };
    let primer = ChainRecord::seal(
        ChainKind::Evento {
            event: verifactu::engine::events::evento_restore(&sif, "B12345678"),
            fecha_huso_gen_evento: instante("2025-02-05T08:00:00+01:00"),
        },
        1_738_745_600,
        None,
    );

    let emission = emitter
        .submit(&[(primer.clone(), EmissionContext::default())])
        .await
        .expect("the event is signed and kept local");
    assert_eq!(
        signer.signed_count(),
        1,
        "events sign in the remission modality too"
    );
    assert_eq!(
        transport.send_count(),
        0,
        "no public event remission operation exists"
    );
    let wire = &emission.signed_records[0];
    assert!(wire.starts_with("<sum1:RegistroEvento xmlns:sum1="));
    assert!(
        wire.contains(&format!(
            "<sum1:HuellaEvento>{}</sum1:HuellaEvento>",
            primer.huella
        )),
        "the event record carries its own huella"
    );
}

#[tokio::test]
async fn conservation_alta_is_signed_kept_local_and_never_sent() {
    let (signer, transport) = (
        Arc::new(FakeSigner::new()),
        Arc::new(FakeVerifactuTransport::new()),
    );
    let emitter = emitter(Modality::Conservation, signer.clone()).with_transport(transport.clone());

    let emission = emitter
        .submit(&[(ticket_alta_record(), ticket_ctx())])
        .await
        .expect("conservation keeps the record local");

    assert_eq!(signer.signed_count(), 1, "conservation signs every record");
    assert!(emission.signed_records[0].contains("<ds:Signature"));
    assert_eq!(
        transport.send_count(),
        0,
        "no voluntary remission in conservation"
    );
    assert_eq!(emission.csv, None);
    assert_eq!(emission.lineas.len(), 0);
}

#[tokio::test]
async fn subsanacion_remites_as_a_new_record_with_the_flags() {
    let (signer, transport) = (
        Arc::new(FakeSigner::new()),
        Arc::new(FakeVerifactuTransport::new()),
    );
    let emitter = emitter(Modality::Remission, signer.clone()).with_transport(transport.clone());
    let ctx = EmissionContext {
        subsanacion: Some(SiNo::Si),
        rechazo_previo: Some(verifactu::engine::RechazoPrevio::Rechazado),
        descripcion_operacion: Some("Subsanación de la venta 42"),
        desglose: ticket_ctx().desglose,
        ..EmissionContext::default()
    };
    let _emission = emitter
        .submit(&[(ticket_alta_record(), ctx)])
        .await
        .expect("the subsanación remits");

    let sent = transport.sent_envelopes();
    assert!(sent[0].contains("<sum1:Subsanacion>S</sum1:Subsanacion>"));
    assert!(sent[0].contains("<sum1:RechazoPrevio>S</sum1:RechazoPrevio>"));
    assert!(sent[0].contains("<sum1:NumSerieFactura>T00000042</sum1:NumSerieFactura>"));
}

/// Stable codes/classes — products match on these strings.
#[test]
fn emission_error_codes_are_stable_and_namespaced() {
    let table = [
        (
            EmitError::NotConfigured,
            "fiscal.not-configured",
            ErrorClass::Bug,
        ),
        (
            EmitError::Transport {
                detail: String::from("x"),
            },
            "fiscal.transport",
            ErrorClass::Transient,
        ),
        (
            EmitError::FaultServer {
                faultstring: String::from("x"),
            },
            "fiscal.fault.server",
            ErrorClass::Transient,
        ),
        (
            EmitError::FaultClient {
                faultstring: String::from("x"),
            },
            "fiscal.fault.client",
            ErrorClass::OperatorAction,
        ),
        (
            EmitError::Signing(verifactu::SignerError::CertificateUnavailable),
            "fiscal.signing.unavailable",
            ErrorClass::OperatorAction,
        ),
        (
            EmitError::Signing(verifactu::SignerError::SigningFailed(String::from("x"))),
            "fiscal.signing.failed",
            ErrorClass::OperatorAction,
        ),
        (
            EmitError::InvalidRecord {
                detail: String::from("x"),
            },
            "fiscal.invalid-record",
            // The caller's own draft, not a programmer bug: 400-exit
            // territory, never an operator ticket.
            ErrorClass::InvalidInput,
        ),
    ];
    for (error, code, class) in table {
        assert_eq!(error.code(), code);
        assert_eq!(error.class(), class);
    }
}
