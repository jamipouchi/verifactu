//! The flat face end to end, over the engine fakes.

use crate::{
    endpoint_table, CertFamily, Environment, Obligado, ProductionOptIn, SifConfig, Verifactu,
    VerifactuError,
};

/// Every emission here reads this fixed instant (AEAT's example,
/// Madrid `2025-02-03T14:30:00+01:00`) — no test reads the wall clock.
fn clock() -> std::sync::Arc<dyn crate::Clock> {
    std::sync::Arc::new(crate::engine::FakeClock::new(crate::Timestamp(
        1_738_589_400,
    )))
}

/// The software's producer — never one of the tenants below.
fn productor() -> Obligado {
    Obligado {
        nombre_razon: String::from("PRODUCTORA DE SOFTWARE SL"),
        nif: String::from("B11223344"),
    }
}

fn sif() -> SifConfig {
    SifConfig::new(
        productor(),
        "mi-facturacion",
        "MF",
        "1.0.0",
        crate::UsoPosible::SoloVerifactu,
    )
    .expect("valid SIF")
}

fn root() -> Verifactu {
    Verifactu::builder(clock())
        .sif(sif())
        .build()
        .expect("pruebas root builds with no gate")
}

/// The gate's whole law: production demands BOTH the token (else it
/// is unrepresentable) and the deployment key (else build refuses,
/// named and fail-closed, before anything exists).
#[test]
fn the_production_gate_fails_closed_without_the_deployment_key() {
    std::env::remove_var("VERIFACTU_ALLOW_PRODUCTION");
    let error = Verifactu::builder(clock())
        .environment(Environment::production(
            ProductionOptIn::these_emissions_are_legally_binding(),
        ))
        .sif(sif())
        .build()
        .expect_err("the deployment key is absent");
    assert!(
        matches!(error, VerifactuError::ProductionGate(_)),
        "{error}"
    );

    std::env::set_var("VERIFACTU_ALLOW_PRODUCTION", "1");
    let root = Verifactu::builder(clock())
        .environment(Environment::production(
            ProductionOptIn::these_emissions_are_legally_binding(),
        ))
        .sif(sif())
        .build()
        .expect("token + key");
    assert_eq!(root.environment(), Environment::Production);
    std::env::remove_var("VERIFACTU_ALLOW_PRODUCTION");
}

#[test]
fn the_endpoint_table_is_the_documented_eight() {
    let (remission, requerimiento) =
        endpoint_table(CertFamily::PersonalOEmpresa, Environment::Pruebas);
    assert!(remission.contains("prewww1.aeat.es") && remission.ends_with("VerifactuSOAP"));
    assert!(
        requerimiento.contains("prewww1.aeat.es") && requerimiento.ends_with("RequerimientoSOAP")
    );
    let (remission, _) = endpoint_table(CertFamily::Sello, Environment::Pruebas);
    assert!(remission.contains("prewww10"), "sello rides the 10-hosts");
    let (remission, _) = endpoint_table(CertFamily::PersonalOEmpresa, Environment::Production);
    assert!(remission.contains("www1.agenciatributaria.gob.es"));
}

/// AEAT Validaciones §3.1.5, eagerly: a named system, a two-position
/// id of A-Z/0-9 (no Ñ, no lowercase), a well-formed producer.
#[test]
fn the_sif_identity_validates_eagerly() {
    let sif = |nombre: &str, id: &str| {
        SifConfig::new(
            productor(),
            nombre,
            id,
            "1.0",
            crate::UsoPosible::SoloVerifactu,
        )
    };
    assert!(sif("x", "F1").is_ok());
    for id in ["FMA", "F", "", "f1", "Ñ1", "F-"] {
        assert!(sif("x", id).is_err(), "IdSistemaInformatico {id:?}");
    }
    assert!(
        sif("", "FM").is_err(),
        "NombreSistemaInformatico is required"
    );
    assert!(
        sif("  ", "FM").is_err(),
        "NombreSistemaInformatico has content"
    );
    let unnamed = Obligado {
        nombre_razon: String::from("PRODUCTORA"),
        nif: String::from("B1122334"),
    };
    assert!(
        SifConfig::new(unnamed, "x", "FM", "1.0", crate::UsoPosible::SoloVerifactu).is_err(),
        "the producer's NIF"
    );
}

/// The `SistemaInformatico` block names the software's producer on every
/// tenant's records — never the tenant (AEAT Validaciones §3.1.5).
#[tokio::test]
async fn the_sistema_informatico_names_the_producer_not_the_tenant() {
    use crate::engine::{FakeSigner, FakeVerifactuTransport};
    use crate::{lines, CertificateSource, InvoiceDraft, Series};
    let transport = std::sync::Arc::new(FakeVerifactuTransport::new());
    let tenant = root()
        .tenant(CertificateSource::Custom {
            signer: std::sync::Arc::new(FakeSigner::new()),
            transport: std::sync::Arc::clone(&transport) as _,
        })
        .obligado(Obligado {
            nombre_razon: String::from("ANA RUIZ PEREZ"),
            nif: String::from("12345678Z"),
        })
        .connect()
        .expect("hermetic legs");
    tenant
        .emit_invoice(
            InvoiceDraft::new()
                .serie(Series::T, 1)
                .descripcion("venta mostrador")
                .line(lines::sujeta_iva(2100, 1_000)),
            None,
        )
        .await
        .expect("the invoice emits");
    let envelope = &transport.sent_envelopes()[0];
    assert!(
        envelope.contains(
            "<sum1:SistemaInformatico><sum1:NombreRazon>PRODUCTORA DE SOFTWARE SL</sum1:NombreRazon>\
             <sum1:NIF>B11223344</sum1:NIF>"
        ),
        "{envelope}"
    );
}

/// The anulación door, hermetically: a stored invoice cursor
/// cancels that invoice, the anulación chains onto the head, and
/// its returned cursor carries the annulled invoice's own
/// coordinates. An event predecessor is refused.
#[tokio::test]
async fn the_cancel_door_annuls_a_stored_cursor_and_chains() {
    use crate::engine::{cancel_draft, emit_draft, FakeSigner, FakeVerifactuTransport};
    use crate::engine::{ScriptedOutcome, VerifactuEmitter};
    use crate::{lines, InvoiceDraft, Modality, Obligado, Predecessor, Series};
    let transport = std::sync::Arc::new(FakeVerifactuTransport::new());
    transport.push_outcome(ScriptedOutcome::Correcto {
        csv: Some(String::from("1111222233334444")),
        tiempo_espera_envio: 60,
    });
    let emitter = VerifactuEmitter::new(
        Modality::Remission,
        Obligado {
            nombre_razon: String::from("ANA RUIZ PEREZ"),
            nif: String::from("12345678Z"),
        },
        crate::engine::SistemaInformaticoConfig {
            nombre_razon: String::from("ANA RUIZ PEREZ"),
            nif: String::from("12345678Z"),
            nombre_sistema_informatico: String::from("mi-facturacion"),
            id_sistema_informatico: String::from("FM"),
            version: String::from("1.2.0"),
            numero_instalacion: String::from("000042"),
            uso_posible: crate::UsoPosible::SoloVerifactu,
        },
        std::sync::Arc::new(FakeSigner::new()),
        clock(),
    )
    .expect("valid identity")
    .with_transport(transport);

    let invoice = emit_draft(
        &emitter,
        InvoiceDraft::new()
            .serie(Series::T, 7)
            .descripcion("venta mostrador")
            .line(lines::sujeta_iva(2100, 10_000)),
        None,
    )
    .await
    .expect("the invoice emits");

    // The printed QR rides the sealed record's own data — and only
    // altas carry one.
    let qr = invoice
        .qr_url(crate::Environment::Pruebas, Modality::Remission)
        .expect("an alta carries its QR");
    assert!(
        qr.starts_with(
            "https://prewww2.aeat.es/wlpl/TIKE-CONT/ValidarQR?nif=12345678Z&numserie=T00000007&fecha="
        ) && qr.ends_with("&importe=121.00"),
        "{qr}"
    );

    let anulado = cancel_draft(&emitter, &invoice.cursor, Some(&invoice.cursor))
        .await
        .expect("the anulación emits");
    let Predecessor::Factura(head) = &anulado.cursor else {
        panic!("an anulación's cursor is a Factura predecessor");
    };
    assert_eq!(
        (head.serie.clone(), head.number),
        (Series::T, 7),
        "the anulación carries the annulled invoice's own coordinates"
    );
    assert!(
        anulado
            .qr_url(crate::Environment::Pruebas, Modality::Remission)
            .is_none(),
        "the printed QR is always the factura's own — anulación kinds carry none"
    );

    let error = cancel_draft(&emitter, &Predecessor::Evento(String::new()), None)
        .await
        .expect_err("events are not annullable");
    assert!(matches!(error, crate::EmitError::InvalidRecord { .. }));
}

#[test]
fn the_vendor_door_demands_the_representante() {
    use crate::engine::{FakeSigner, FakeVerifactuTransport};
    let error = root()
        .vendor(crate::CertificateSource::Custom {
            signer: std::sync::Arc::new(FakeSigner::new()),
            transport: std::sync::Arc::new(FakeVerifactuTransport::new()),
        })
        .connect()
        .expect_err("the representante is the model's own name");
    assert!(matches!(error, VerifactuError::Identity(_)), "{error}");
}

/// The consulta doors, hermetically: the tenant lens asks plainly,
/// the vendor lens sets `IndicadorRepresentante=S` (the apoderado
/// law), and pagination merges every page in call order.
#[tokio::test]
async fn the_consulta_doors_read_back_apoderado_and_paginate() {
    use crate::engine::consulta::{ConsultaSpec, ResultadoConsulta};
    use crate::engine::{FakeSigner, FakeVerifactuTransport};
    use crate::{ConsultaFilter, Mes, Obligado};
    let obligado = Obligado {
        nombre_razon: String::from("ANA RUIZ PEREZ"),
        nif: String::from("12345678Z"),
    };
    let transport = std::sync::Arc::new(FakeVerifactuTransport::new());
    transport.push_consulta(ConsultaSpec {
        obligado_nombre: obligado.nombre_razon.clone(),
        obligado_nif: obligado.nif.clone(),
        apoderado: true,
        ejercicio: 2026,
        mes: Mes::try_new(10).expect("octubre"),
        resultado: ResultadoConsulta::ConDatos,
        paginacion_pendiente: true,
        clave_paginacion: Some(crate::fiscal::consulta::ClavePaginacion {
            id_emisor_factura: obligado.nif.clone(),
            num_serie_factura: String::from("T00000009"),
            fecha_expedicion_factura: String::from("05-10-2026"),
        }),
        registros: vec![consulta_registro_page(7)],
    });
    transport.push_consulta(ConsultaSpec {
        obligado_nombre: obligado.nombre_razon.clone(),
        obligado_nif: obligado.nif.clone(),
        apoderado: false,
        ejercicio: 2026,
        mes: Mes::try_new(10).expect("octubre"),
        resultado: ResultadoConsulta::ConDatos,
        paginacion_pendiente: false,
        clave_paginacion: None,
        registros: vec![consulta_registro_page(8)],
    });
    let tenant = root()
        .tenant(crate::CertificateSource::Custom {
            signer: std::sync::Arc::new(FakeSigner::new()),
            transport: std::sync::Arc::clone(&transport) as _,
        })
        .obligado(obligado.clone())
        .installation("000042")
        .connect()
        .expect("the custom legs wire");
    let filtro = ConsultaFilter::new(2026, Mes::try_new(10).expect("octubre"));
    let answer = tenant
        .consulta(&filtro)
        .await
        .expect("the consulta answers");
    assert_eq!(answer.registros.len(), 2, "both pages merged");
    assert_eq!(answer.registros[0].num_serie_factura, "T00000007");
    assert_eq!(
        transport.sent_envelopes()[0]
            .matches("IndicadorRepresentante")
            .count(),
        0,
        "the tenant lens never sets the apoderado flag"
    );
    assert!(
        transport.sent_envelopes()[1].contains("ClavePaginacion"),
        "page 2 resumes from the handed key"
    );

    transport.push_consulta(ConsultaSpec {
        obligado_nombre: obligado.nombre_razon.clone(),
        obligado_nif: obligado.nif.clone(),
        apoderado: true,
        ejercicio: 2026,
        mes: Mes::try_new(10).expect("octubre"),
        resultado: ResultadoConsulta::ConDatos,
        paginacion_pendiente: false,
        clave_paginacion: None,
        registros: Vec::new(),
    });
    let vendor = root()
        .vendor(crate::CertificateSource::Custom {
            signer: std::sync::Arc::new(FakeSigner::new()),
            transport: std::sync::Arc::clone(&transport) as _,
        })
        .representante(Obligado {
            nombre_razon: String::from("FACTURAS DE MANO SL"),
            nif: String::from("B87654321"),
        })
        .connect()
        .expect("vendor custody wires");
    vendor
        .consulta_for(&obligado, &filtro)
        .await
        .expect("the apoderado lens answers");
    let vendor_envelope = transport.sent_envelopes()[2].clone();
    assert!(
        vendor_envelope.contains("<sum1:IndicadorRepresentante>S</sum1:IndicadorRepresentante>"),
        "the vendor lens is the apoderado door"
    );
}

/// One huella-verifiable stored record — HS §6.1 vector 1's shape.
fn consulta_registro_page(number: u64) -> crate::fiscal::consulta::RegistroSpec {
    crate::fiscal::consulta::RegistroSpec {
        id_emisor_factura: String::from("12345678Z"),
        num_serie_factura: format!("T{number:0>8}"),
        fecha_expedicion_factura: String::from("05-10-2026"),
        estado: crate::EstadoAlmacenado::Correcto,
        timestamp_ultima_modificacion: String::from("2026-10-05T12:00:00+02:00"),
        codigo_error: None,
        descripcion_error: None,
        tipo_factura: Some(crate::TipoFactura::F2),
        cuota_total: Some(crate::Money::from_cents(210)),
        importe_total: Some(crate::Money::from_cents(1210)),
        fecha_huso_gen: Some(String::from("2026-10-05T12:00:00+02:00")),
        huella_previa: None,
        huella: Some(crate::domain::chain::huella_alta_montos(
            "12345678Z",
            &format!("T{number:0>8}"),
            "05-10-2026",
            "F2",
            "2.10",
            "12.10",
            None,
            "2026-10-05T12:00:00+02:00",
        )),
        sistema_informatico: None,
    }
}

/// The vendor door's envío carries the Cabecera `Representante`
/// (SI.xsd sequence: between `ObligadoEmision` and the remission
/// block) — the block the apoderado read lens answers; a silent
/// drop would be accepted by AEAT and only surface as `SinDatos`.
#[tokio::test]
async fn the_vendor_envio_carries_the_representante_block() {
    use crate::engine::{FakeSigner, FakeVerifactuTransport, ScriptedOutcome};
    use crate::lines;
    use crate::{CertificateSource, InvoiceDraft, Modality, Obligado, Series};
    let transport = std::sync::Arc::new(FakeVerifactuTransport::new());
    transport.push_outcome(ScriptedOutcome::Correcto {
        csv: None,
        tiempo_espera_envio: 60,
    });
    let obligado = Obligado {
        nombre_razon: String::from("ANA RUIZ PEREZ"),
        nif: String::from("12345678Z"),
    };
    let vendor = root()
        .vendor(CertificateSource::Custom {
            signer: std::sync::Arc::new(FakeSigner::new()),
            transport: std::sync::Arc::clone(&transport) as _,
        })
        .representante(Obligado {
            nombre_razon: String::from("FACTURAS DE MANO SL"),
            nif: String::from("B87654321"),
        })
        .connect()
        .expect("vendor custody wires");
    vendor
        .emit_invoice_for(
            &obligado,
            "000042",
            Modality::Remission,
            InvoiceDraft::new()
                .serie(Series::T, 9)
                .descripcion("venta por delegación")
                .line(lines::sujeta_iva(2100, 10_000)),
            None,
        )
        .await
        .expect("the vendor emission rides the injected transport");
    let envelope = &transport.sent_envelopes()[0];
    assert!(
        envelope.contains("<sum1:Representante><sum1:NombreRazon>FACTURAS DE MANO SL</sum1:NombreRazon><sum1:NIF>B87654321</sum1:NIF></sum1:Representante>"),
        "the Representante block rides the wire: {envelope}"
    );
}

/// The fake AEAT echoes the record's own `RefExterna` back — real
/// AEAT does (RS.xsd), so a consumer's "did my external reference
/// register?" demo reads the same hermetically as live.
#[tokio::test]
async fn the_fake_echoes_the_records_ref_externa() {
    use crate::engine::{FakeSigner, FakeVerifactuTransport, ScriptedOutcome};
    use crate::{lines, CertificateSource, InvoiceDraft, Series};
    let transport = std::sync::Arc::new(FakeVerifactuTransport::new());
    transport.push_outcome(ScriptedOutcome::Correcto {
        csv: Some(String::from("1111222233334444")),
        tiempo_espera_envio: 60,
    });
    let tenant = root()
        .tenant(CertificateSource::Custom {
            signer: std::sync::Arc::new(FakeSigner::new()),
            transport,
        })
        .obligado(crate::Obligado {
            nombre_razon: String::from("ANA RUIZ PEREZ"),
            nif: String::from("12345678Z"),
        })
        .connect()
        .expect("hermetic legs");
    let emitted = tenant
        .emit_invoice(
            InvoiceDraft::new()
                .serie(Series::T, 7)
                .descripcion("venta mostrador")
                .ref_externa("PEDIDO-2026-41")
                .line(lines::sujeta_iva(2100, 10_000)),
            None,
        )
        .await
        .expect("the emission rides the injected transport");
    assert_eq!(
        emitted.emission.lineas[0].ref_externa.as_deref(),
        Some("PEDIDO-2026-41"),
        "the echo answers the record's own RefExterna back"
    );
}

/// AEAT's door refusing the client certificate reaches the product
/// as the operator's, under its own code; a status with no SOAP answer
/// retries. The fake reads each through the transports' read edge.
#[tokio::test]
async fn a_refused_exchange_reaches_the_flat_face_as_the_operators() {
    use crate::engine::{FakeSigner, FakeVerifactuTransport, ScriptedOutcome};
    use crate::{lines, CertificateSource, ErrorClass, InvoiceDraft, Series};
    let transport = std::sync::Arc::new(FakeVerifactuTransport::new());
    transport.push_outcome(ScriptedOutcome::HttpStatus(302));
    transport.push_outcome(ScriptedOutcome::HttpStatus(503));
    let tenant = root()
        .tenant(CertificateSource::Custom {
            signer: std::sync::Arc::new(FakeSigner::new()),
            transport,
        })
        .obligado(crate::Obligado {
            nombre_razon: String::from("ANA RUIZ PEREZ"),
            nif: String::from("12345678Z"),
        })
        .connect()
        .expect("hermetic legs");
    let draft = || {
        InvoiceDraft::new()
            .serie(Series::T, 7)
            .descripcion("venta mostrador")
            .line(lines::sujeta_iva(2100, 10_000))
    };
    let refused = tenant
        .emit_invoice(draft(), None)
        .await
        .expect_err("the door refuses the certificate");
    assert_eq!(
        (refused.code(), refused.class()),
        ("fiscal.transport.refused", ErrorClass::OperatorAction),
        "{refused}"
    );
    let unreadable = tenant
        .emit_invoice(draft(), None)
        .await
        .expect_err("no SOAP answer to read");
    assert_eq!(
        (unreadable.code(), unreadable.class()),
        ("fiscal.transport", ErrorClass::Transient),
        "{unreadable}"
    );
}

/// A sustitutiva issued after its sale's rate ended declares the sale's
/// date: `FechaOperacion` travels in its SI.xsd place, and AEAT checks
/// the rate against it (Validaciones §15.1). Without it the draft is
/// refused before a byte leaves.
#[tokio::test]
async fn a_draft_declares_its_operation_date() {
    use crate::engine::{FakeSigner, FakeVerifactuTransport};
    use crate::{
        lines, CertificateSource, FacturaId, FechaExpedicion, IDDestinatario,
        IdentificacionDestinatario, ImporteRectificacion, InvoiceDraft, Money, Series, TipoFactura,
        TipoRectificativa,
    };
    let transport = std::sync::Arc::new(FakeVerifactuTransport::new());
    let tenant = root()
        .tenant(CertificateSource::Custom {
            signer: std::sync::Arc::new(FakeSigner::new()),
            transport: std::sync::Arc::clone(&transport) as _,
        })
        .obligado(Obligado {
            nombre_razon: String::from("ANA RUIZ PEREZ"),
            nif: String::from("12345678Z"),
        })
        .connect()
        .expect("hermetic legs");
    // The clock reads 03-02-2025; 5 % ended on 30-09-2024.
    let venta = FechaExpedicion::parse("30-09-2024").expect("a real date");
    let sustitutiva = || {
        InvoiceDraft::new()
            .serie(Series::R, 1)
            .tipo(TipoFactura::R1)
            .descripcion("sustituye la factura F00000007")
            .destinatario(IDDestinatario {
                nombre_razon: String::from("CLIENTE EJEMPLO SL"),
                identificacion: IdentificacionDestinatario::Nif(String::from("B87654321")),
            })
            .rectificativa(
                TipoRectificativa::Sustitutiva,
                vec![FacturaId {
                    nif: String::from("12345678Z"),
                    num_serie: String::from("F00000007"),
                    fecha_expedicion: venta.clone(),
                }],
                Some(ImporteRectificacion {
                    base_rectificada: Money::from_cents(100_000),
                    cuota_rectificada: Money::from_cents(5_000),
                    cuota_recargo_rectificado: None,
                }),
            )
            .line(lines::sujeta_iva(500, 100_000))
    };
    let error = tenant
        .emit_invoice(sustitutiva(), None)
        .await
        .expect_err("5 % is no rate on 03-02-2025");
    assert!(error.to_string().contains("§15.1"), "{error}");
    tenant
        .emit_invoice(sustitutiva().fecha_operacion(venta.clone()), None)
        .await
        .expect("5 % is the rate on 30-09-2024");
    let sent = transport.sent_envelopes();
    assert_eq!(sent.len(), 1, "the refused draft never left");
    assert!(
        sent[0].contains(
            "</sum1:ImporteRectificacion><sum1:FechaOperacion>30-09-2024</sum1:FechaOperacion>\
             <sum1:DescripcionOperacion>"
        ),
        "{}",
        sent[0]
    );
}

/// The bring-your-own door: both custody models wire over injected
/// legs with no HTTP stack — the emitted envelope travels the
/// injected transport, and (remission law) the alta rides unsigned.
/// Runs under BOTH feature configs: without `http` this is the only
/// door.
#[tokio::test]
async fn the_custom_door_wires_both_custody_models_over_injected_legs() {
    use crate::engine::{FakeVerifactuTransport, ScriptedOutcome};
    use crate::{
        lines, response, CertificateSource, FiscalSigner, InvoiceDraft, Modality, Obligado, Series,
        VerifactuTransport,
    };
    let obligado = Obligado {
        nombre_razon: String::from("ANA RUIZ PEREZ"),
        nif: String::from("12345678Z"),
    };
    // The documented hermetic wiring: the ENGINE fakes implement
    // the FACADE's ports — no hand-rolled stub survives.
    let fake_signer = std::sync::Arc::new(crate::engine::FakeSigner::new());
    let fake_transport = std::sync::Arc::new(FakeVerifactuTransport::new());
    fake_transport.push_outcome(ScriptedOutcome::Correcto {
        csv: Some(String::from("1111222233334444")),
        tiempo_espera_envio: 60,
    });
    let signer: std::sync::Arc<dyn FiscalSigner> = fake_signer.clone();
    let transport: std::sync::Arc<dyn VerifactuTransport> = fake_transport.clone();

    let tenant = root()
        .tenant(CertificateSource::Custom {
            signer: std::sync::Arc::clone(&signer),
            transport: std::sync::Arc::clone(&transport),
        })
        .obligado(obligado.clone())
        .installation("000042")
        .connect()
        .expect("the custom legs wire without any HTTP");
    let emitted = tenant
        .emit_invoice(
            InvoiceDraft::new()
                .serie(Series::T, 7)
                .descripcion("venta mostrador")
                .line(lines::sujeta_iva(2100, 10_000)),
            None,
        )
        .await
        .expect("the emission rides the injected transport");
    assert!(matches!(
        emitted.emission.disposition,
        response::Disposition::Complete
    ));
    let sent = fake_transport.sent_envelopes();
    assert_eq!(
        sent.len(),
        1,
        "exactly one envío traveled the injected transport"
    );
    let request = &fake_transport.sent_requests()[0];
    assert_eq!(request.obligado_nif, "12345678Z");
    assert_eq!(request.records[0].num_serie, "T00000007");
    assert_eq!(
        fake_signer.signed_count(),
        0,
        "remission altas are unsigned"
    );

    let vendor = root()
        .vendor(CertificateSource::Custom {
            signer: std::sync::Arc::clone(&signer),
            transport: std::sync::Arc::clone(&transport),
        })
        .representante(Obligado {
            nombre_razon: String::from("FACTURAS DE MANO SL"),
            nif: String::from("B87654321"),
        })
        .connect()
        .expect("vendor custody wires over custom legs too");
    fake_transport.push_outcome(ScriptedOutcome::Correcto {
        csv: Some(String::from("2222333344445555")),
        tiempo_espera_envio: 60,
    });
    let vendored = vendor
        .emit_invoice_for(
            &obligado,
            "000042",
            Modality::Remission,
            InvoiceDraft::new()
                .serie(Series::F, 1)
                .tipo(crate::TipoFactura::F1)
                .descripcion("obra fontanería")
                .destinatario(crate::IDDestinatario {
                    nombre_razon: String::from("CLIENTE EJEMPLO SL"),
                    identificacion: crate::IdentificacionDestinatario::Nif(String::from(
                        "B87654321",
                    )),
                })
                .line(lines::sujeta_iva(2100, 10_000)),
            None,
        )
        .await
        .expect("the shared injected transport mints per-tenant emitters");
    assert_eq!(vendored.num_serie, "F00000001");
    assert_eq!(
        vendored.emission.csv.as_deref(),
        Some("2222333344445555"),
        "the vendor emission rode the same injected pool"
    );
}

/// A hermetic tenant over the engine fakes with a frozen clock:
/// the draft gates, the echo, and byte-identical instants.
#[tokio::test]
async fn the_draft_gates_and_the_sealed_echo() {
    use crate::engine::Timestamp;
    use crate::engine::{FakeClock, FakeSigner, FakeVerifactuTransport, ScriptedOutcome};
    use crate::{lines, CertificateSource, InvoiceDraft, Series, TipoFactura};
    let transport = std::sync::Arc::new(FakeVerifactuTransport::new());
    transport.push_outcome(ScriptedOutcome::Correcto {
        csv: Some(String::from("1111222233334444")),
        tiempo_espera_envio: 60,
    });
    let clock = std::sync::Arc::new(FakeClock::new(Timestamp(1_738_589_400)));
    let tenant = Verifactu::builder(clock)
        .sif(sif())
        .build()
        .expect("pruebas root builds with no gate")
        .tenant(CertificateSource::Custom {
            signer: std::sync::Arc::new(FakeSigner::new()),
            transport,
        })
        .obligado(crate::Obligado {
            nombre_razon: String::from("ANA RUIZ PEREZ"),
            nif: String::from("12345678Z"),
        })
        .connect()
        .expect("hermetic legs");

    // Custom series WITH a named tipo emits (the eager-match bug
    // used to reject this); the echo carries what was sealed.
    let emitted = tenant
        .emit_invoice(
            InvoiceDraft::new()
                .serie(Series::Custom(String::from("2026E")), 1)
                .tipo(TipoFactura::F1)
                .descripcion("obra fontanería")
                .destinatario(crate::IDDestinatario {
                    nombre_razon: String::from("CLIENTE EJEMPLO SL"),
                    identificacion: crate::IdentificacionDestinatario::Nif(String::from(
                        "B87654321",
                    )),
                })
                .line(lines::sujeta_iva(2100, 10_000)),
            None,
        )
        .await
        .expect("a named tipo unlocks a custom series");
    assert_eq!(emitted.num_serie, "2026E00000001");
    assert!(matches!(
        emitted.kind,
        crate::ChainKind::Alta {
            tipo_factura: TipoFactura::F1,
            ..
        }
    ));
    assert_eq!(
        emitted.emission.lineas[0].num_serie_factura,
        "2026E00000001"
    );
    // The frozen clock reproduces AEAT's example instant.
    assert!(
        format!("{:?}", emitted.kind).contains("2025-02-03T14:30:00+01:00"),
        "the sealed instants come from the injected clock: {:?}",
        emitted.kind
    );
    // AEAT's whole answer rides the emission: the envío's state, its
    // receipt beside the CSV, and the wait it set.
    assert_eq!(
        emitted.emission.estado_envio,
        Some(crate::response::EstadoEnvio::Correcto)
    );
    assert_eq!(
        emitted.emission.datos_presentacion,
        Some(crate::response::DatosPresentacion {
            nif_presentador: String::from("12345678Z"),
            timestamp_presentacion: String::from("2025-02-03T14:30:00+01:00"),
        })
    );
    assert_eq!(emitted.emission.tiempo_espera_envio, Some(60));

    // The cursor round-trips through JSON when `serde` is on.
    #[cfg(feature = "serde")]
    {
        use crate::Predecessor;
        let json = serde_json::to_string(&emitted.cursor).expect("cursor serializes");
        let back: Predecessor = serde_json::from_str(&json).expect("cursor parses back");
        assert_eq!(back, emitted.cursor);
    }
}

/// Every draft gate fires as `InvalidRecord` (the `InvalidInput`
/// class), naming the law — never a panic.
#[test]
fn the_draft_gates_fire_as_invalid_input() {
    use crate::engine::{FakeSigner, FakeVerifactuTransport};
    use crate::lines;
    use crate::{CertificateSource, ErrorClass, InvoiceDraft, Obligado, Series};
    let tenant = root()
        .tenant(CertificateSource::Custom {
            signer: std::sync::Arc::new(FakeSigner::new()),
            transport: std::sync::Arc::new(FakeVerifactuTransport::new()),
        })
        .obligado(Obligado {
            nombre_razon: String::from("ANA RUIZ PEREZ"),
            nif: String::from("12345678Z"),
        })
        .connect()
        .expect("hermetic legs");
    let rt = tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("rt");
    let gate = |draft: InvoiceDraft| {
        let error = rt
            .block_on(tenant.emit_invoice(draft, None))
            .expect_err("the gate fires");
        assert_eq!(error.class(), ErrorClass::InvalidInput);
        error.to_string()
    };
    let cases = [
        (
            InvoiceDraft::new()
                .serie(Series::Custom(String::from("2026E")), 2)
                .descripcion("sin tipo")
                .line(lines::sujeta_iva(2100, 1_000)),
            "a custom series carries no default TipoFactura",
        ),
        (
            InvoiceDraft::new()
                .serie(Series::T, 100_000_000)
                .descripcion("9-digit correlative")
                .line(lines::sujeta_iva(2100, 1_000)),
            "99_999_999",
        ),
        (
            InvoiceDraft::new()
                .serie(Series::T, 8)
                .descripcion("")
                .line(lines::sujeta_iva(2100, 1_000)),
            "description is required",
        ),
        (
            InvoiceDraft::new()
                .serie(Series::parse(""), 9)
                .descripcion("empty custom prefix"),
            "custom series prefix is required",
        ),
        (
            InvoiceDraft::new()
                .serie(Series::parse(&"A".repeat(53)), 9)
                .descripcion("NumSerieFactura renders 61 chars")
                .line(lines::sujeta_iva(2100, 1_000)),
            "1..=60",
        ),
        (
            InvoiceDraft::new()
                .serie(Series::parse("FÁ"), 9)
                .descripcion("the printed QR admits ASCII only")
                .line(lines::sujeta_iva(2100, 1_000)),
            "printable ASCII",
        ),
        (
            InvoiceDraft::new()
                .serie(Series::parse(" F"), 9)
                .descripcion("the huella would trim the prefix")
                .line(lines::sujeta_iva(2100, 1_000)),
            "edge whitespace",
        ),
        (
            InvoiceDraft::new()
                .serie(Series::F, 2)
                .descripcion("F1 sin destinatario")
                .line(lines::sujeta_iva(2100, 1_000)),
            "AEAT 1189",
        ),
    ];
    for (draft, law) in cases {
        let error = gate(draft);
        assert!(
            error.contains(law),
            "the gate names its law ({law}): {error}"
        );
    }
}

/// Conservation is NO VERI*FACTU: a conservation tenant refuses a SIF
/// declared `SoloVerifactu`, and one declaring `TambienNoVerifactu`
/// renders `TipoUsoPosibleSoloVerifactu=N` on its records.
#[tokio::test]
async fn a_conservation_tenant_needs_a_sif_that_also_runs_no_verifactu() {
    use crate::engine::{FakeSigner, FakeVerifactuTransport};
    use crate::{lines, CertificateSource, InvoiceDraft, Modality, Series};
    let connect = |sif: SifConfig| {
        Verifactu::builder(clock())
            .sif(sif)
            .build()
            .expect("pruebas root")
            .tenant(CertificateSource::Custom {
                signer: std::sync::Arc::new(FakeSigner::new()),
                transport: std::sync::Arc::new(FakeVerifactuTransport::new()),
            })
            .obligado(Obligado {
                nombre_razon: String::from("ANA RUIZ PEREZ"),
                nif: String::from("12345678Z"),
            })
            .modality(Modality::Conservation)
            .connect()
    };
    let error = connect(sif()).expect_err("a SoloVerifactu SIF cannot conserve");
    assert!(error.to_string().contains("NO VERI*FACTU"), "{error}");

    let dual = SifConfig::new(
        productor(),
        "mi-facturacion",
        "MF",
        "1.0.0",
        crate::UsoPosible::TambienNoVerifactu,
    )
    .expect("valid SIF");
    let tenant = connect(dual).expect("a SIF that also runs NO VERI*FACTU conserves");
    let emitted = tenant
        .emit_invoice(
            InvoiceDraft::new()
                .serie(Series::T, 1)
                .descripcion("venta mostrador")
                .line(lines::sujeta_iva(2100, 1_000)),
            None,
        )
        .await
        .expect("the record is kept local");
    assert!(emitted.emission.signed_records[0]
        .contains("<sum1:TipoUsoPosibleSoloVerifactu>N</sum1:TipoUsoPosibleSoloVerifactu>"));
}
