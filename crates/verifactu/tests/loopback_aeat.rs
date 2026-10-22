//! The production transport against an owned loopback AEAT: the
//! refused exchange is an HTTP 302 to the sede error page — the
//! transport must type that refusal, never a generic parse failure, and
//! carry nothing the door sent beside it — and a stand-in's marked
//! answer is never read as AEAT's.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use axum::http::StatusCode;
use verifactu::engine::test_util;
use verifactu::engine::xml::{self, CabeceraRemision};
use verifactu::engine::EmissionContext;
use verifactu::engine::SistemaInformaticoConfig;
use verifactu::VerifactuTransport as _;
use verifactu::{DetalleDesglose, EmitError, ErrorClass, Obligado, TransportFailure};

const EMISSION_INSTANT: u64 = 1_738_589_400;

fn iva_super() -> DetalleDesglose {
    use verifactu::{CalificacionOperacion, ClaveRegimen, Impuesto};
    DetalleDesglose {
        impuesto: Some(Impuesto::Iva),
        clave_regimen: Some(ClaveRegimen::C01),
        calificacion: Some(CalificacionOperacion::SujetaNoExenta),
        operacion_exenta: None,
        tipo_impositivo: Some(verifactu::Money::from_cents(2100)),
        base_imponible: verifactu::Money::from_cents(1000),
        cuota_repercutida: Some(verifactu::Money::from_cents(210)),
        tipo_recargo_equivalencia: None,
        cuota_recargo_equivalencia: None,
    }
}

fn envelope_for_primer_alta() -> String {
    let instants = verifactu::engine::huso::render_instants(
        verifactu::engine::SiteZone::EuropeMadrid,
        EMISSION_INSTANT,
    );
    let kind = verifactu::ChainKind::Alta {
        issuer: String::from("B12345678"),
        serie: verifactu::Series::T,
        number: 42,
        fecha_expedicion: instants.fecha_expedicion,
        tipo_factura: verifactu::TipoFactura::F2,
        cuota_total: verifactu::Money::from_cents(210),
        importe_total: verifactu::Money::from_cents(1210),
        fecha_huso_gen: instants.fecha_huso_gen,
    };
    let record = verifactu::engine::ChainRecord::seal(kind, EMISSION_INSTANT, None);
    let obligado = Obligado {
        nombre_razon: String::from("Empresa Ejemplo"),
        nif: String::from("B12345678"),
    };
    let sif = SistemaInformaticoConfig {
        nombre_razon: String::from("TEST ERP"),
        nif: String::from("B12345678"),
        nombre_sistema_informatico: String::from("verifactu-wire-test"),
        id_sistema_informatico: String::from("01"),
        version: String::from("0.1.0"),
        numero_instalacion: String::from("1"),
        uso_posible: verifactu::UsoPosible::SoloVerifactu,
    };
    let desglose: &'static [DetalleDesglose] = Box::leak(Box::new([iva_super()]));
    let ctx = EmissionContext {
        descripcion_operacion: Some("Venta mostrador"),
        desglose,
        ..EmissionContext::default()
    };
    let node = xml::record_node(
        &record,
        &ctx,
        &obligado,
        &sif,
        verifactu::Modality::Remission,
    )
    .expect("serializes");
    let body = xml::reg_factu_document(
        &obligado,
        None,
        &CabeceraRemision::Voluntaria { incidencia: false },
        &[node],
    )
    .expect("wraps");
    xml::soap_envelope(&body)
}

async fn aeat_loopback(
    status: StatusCode,
    body: String,
    extra_headers: &'static [(&'static str, &'static str)],
) -> (
    String,
    rustls::RootCertStore,
    verifactu::engine::p12::ClientIdentity,
    Arc<AtomicUsize>,
) {
    let served: Arc<AtomicUsize> = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&served);
    // Any other path is counted too: a client that followed a
    // redirect would land here, never off the loopback.
    let stray = Arc::clone(&served);
    let router = axum::Router::new()
        .route(
            "/wlpl/TIKE-CONT/ws/SistemaFacturacion/VerifactuSOAP",
            axum::routing::post(move || {
                let counter = Arc::clone(&counter);
                let body = body.clone();
                async move {
                    counter.fetch_add(1, Ordering::SeqCst);
                    let answer = axum::response::Response::builder()
                        .status(status)
                        .header(axum::http::header::CONTENT_TYPE, "text/xml; charset=utf-8");
                    extra_headers
                        .iter()
                        .fold(answer, |answer, &(name, value)| answer.header(name, value))
                        .body(axum::body::Body::from(body))
                        .expect("the loopback answer builds")
                }
            }),
        )
        .fallback(move || {
            let stray = Arc::clone(&stray);
            async move {
                stray.fetch_add(1, Ordering::SeqCst);
                StatusCode::NOT_FOUND
            }
        });
    let (address, roots, handle) = test_util::serve_tls_client_certs(router).await;
    let identity = verifactu::engine::p12::ClientIdentity::new(handle.chain(), handle.key());
    (
        format!("https://{address}/wlpl/TIKE-CONT/ws/SistemaFacturacion/VerifactuSOAP"),
        roots,
        identity,
        served,
    )
}

/// The certificate gate's redirect: the sede error page, with the gate's
/// session in the Location's query. The Location is relative, so a
/// client that followed it would hit the loopback, counted; the page
/// links AEAT's host. The cookie stands for any other header the door
/// sends beside the status.
const GATE_LOCATION: &str =
    "/wlpl/SSII-FACT/ws/erro4011?jsessionid=SECRETSESSION&ticket=SECRETTICKET";
const GATE_HEADERS: &[(&str, &str)] = &[
    ("location", GATE_LOCATION),
    (
        "set-cookie",
        "JSESSIONID=SECRETCOOKIE; Path=/; Secure; HttpOnly",
    ),
];

/// A refusal is typed by its status alone: the Location's session, the
/// cookie and the page the door sends beside it never reach the
/// failure, nor the error it becomes.
#[tokio::test]
async fn a_refused_exchange_is_typed_and_never_carries_the_doors_session() {
    for status in [
        StatusCode::FOUND,
        StatusCode::MOVED_PERMANENTLY,
        StatusCode::UNAUTHORIZED,
        StatusCode::FORBIDDEN,
    ] {
        let page = format!(
            "<html><body><a href=\"https://www1.agenciatributaria.gob.es{GATE_LOCATION}\">\
             erro4011</a></body></html>"
        );
        let (endpoint, roots, identity, served) = aeat_loopback(status, page, GATE_HEADERS).await;

        let (transport, recorder) = transport_to(&endpoint, roots, &identity).recording();
        let failure = transport
            .send(&envelope_for_primer_alta())
            .await
            .expect_err("the gate refuses");
        assert_eq!(
            failure,
            TransportFailure::Refused {
                status: status.as_u16()
            }
        );
        assert_eq!(
            served.load(Ordering::SeqCst),
            1,
            "one exchange: neither retried nor followed"
        );
        let shown = failure.to_string();
        assert!(
            shown.contains(&format!("HTTP {}", status.as_u16()))
                && shown.contains("certificate was not accepted"),
            "{shown}"
        );

        let error = EmitError::from(failure.clone());
        for text in [
            shown,
            format!("{failure:?}"),
            error.to_string(),
            format!("{error:?}"),
        ] {
            for leak in ["SECRET", "jsessionid", "erro4011", "agenciatributaria"] {
                assert!(!text.contains(leak), "{leak} leaked into {text}");
            }
        }
        // The opt-in recording keeps the door's answer as evidence, but
        // its Debug, the channel logs use, shows only the count.
        let evidence = format!("{:?}", recorder.exchanges());
        assert!(evidence.contains("SECRETSESSION"), "{evidence}");
        let shown = format!("{recorder:?}");
        assert!(
            shown.contains("exchanges: 1") && !shown.contains("SECRET"),
            "{shown}"
        );
    }
}

/// A dial that never reaches the door is the wire leg's failure, and an
/// answer in no expected shape names its status: both retry.
#[tokio::test]
async fn a_failed_dial_and_an_unreadable_answer_keep_their_cause() {
    let address = {
        let dropped = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind the loopback listener");
        dropped
            .local_addr()
            .expect("the owned socket reports its address")
    };
    let failure = verifactu::engine::HttpVerifactuTransport::new(
        &format!("https://{address}/wlpl/TIKE-CONT/ws/SistemaFacturacion/VerifactuSOAP"),
        Duration::from_secs(10),
    )
    .expect("endpoint parses")
    .send(&envelope_for_primer_alta())
    .await
    .expect_err("the dial is refused");
    assert!(
        matches!(failure, TransportFailure::Wire { .. }),
        "{failure:?}"
    );
    assert_eq!(EmitError::from(failure).class(), ErrorClass::Transient);

    let (endpoint, roots, identity, _) = aeat_loopback(
        StatusCode::INTERNAL_SERVER_ERROR,
        String::from("<html><body>Service unavailable</body></html>"),
        &[],
    )
    .await;
    let failure = transport_to(&endpoint, roots, &identity)
        .send(&envelope_for_primer_alta())
        .await
        .expect_err("no SOAP shape to read");
    let TransportFailure::Unreadable { detail } = &failure else {
        panic!("an answer in no expected shape is unreadable: {failure:?}");
    };
    assert!(detail.contains("HTTP 500"), "{detail}");
    assert_eq!(EmitError::from(failure).class(), ErrorClass::Transient);
}

fn correcto_answer() -> String {
    use verifactu::engine::transport::{respuesta_document, RespuestaSpec};
    use verifactu::response::EstadoEnvio;
    respuesta_document(&RespuestaSpec {
        obligado_nombre: "Empresa Ejemplo",
        obligado_nif: "B12345678",
        csv: Some("A-0000000000000A"),
        datos_presentacion: None,
        tiempo_espera_envio: Some(60),
        estado_envio: EstadoEnvio::Correcto,
        lineas: Vec::new(),
    })
}

fn transport_to(
    endpoint: &str,
    roots: rustls::RootCertStore,
    identity: &verifactu::engine::p12::ClientIdentity,
) -> verifactu::engine::HttpVerifactuTransport {
    verifactu::engine::HttpVerifactuTransport::new(endpoint, Duration::from_secs(10))
        .expect("endpoint parses")
        .with_roots(roots)
        .with_client_identity(verifactu::engine::p12::ClientIdentity::new(
            identity.chain().to_vec(),
            identity.key(),
        ))
        .expect("the minted identity is accepted")
}

/// A deployment's stand-in marks its answers: a transport refusing the
/// mark (named in any case) never reads one as AEAT's verdict, whatever
/// it says; one that does not refuse it reads it as AEAT's — the
/// guard is the consumer's to set.
#[tokio::test]
async fn a_stand_ins_marked_answer_is_refused_where_only_aeat_may_answer() {
    let (endpoint, roots, identity, served) = aeat_loopback(
        StatusCode::OK,
        correcto_answer(),
        &[("x-stand-in", "fake-aeat")],
    )
    .await;

    let failure = transport_to(&endpoint, roots.clone(), &identity)
        .refusing_stand_ins("X-Stand-In")
        .expect("a header name")
        .send(&envelope_for_primer_alta())
        .await
        .expect_err("a marked answer is refused");
    assert_eq!(
        failure,
        TransportFailure::StandIn {
            mark: String::from("x-stand-in")
        }
    );

    let read = transport_to(&endpoint, roots, &identity)
        .send(&envelope_for_primer_alta())
        .await;
    assert!(
        matches!(read, Ok(verifactu::TransportOutcome::Response(_))),
        "without the guard the marked answer reads as AEAT's: {read:?}"
    );
    assert_eq!(served.load(Ordering::SeqCst), 2);
}

/// The guard refuses only the mark: AEAT's own answer reads through it,
/// and a mark that is no header name is refused at construction.
#[tokio::test]
async fn the_stand_in_guard_reads_an_unmarked_answer_and_refuses_a_bad_mark() {
    let (endpoint, roots, identity, _) =
        aeat_loopback(StatusCode::OK, correcto_answer(), &[]).await;
    let outcome = transport_to(&endpoint, roots.clone(), &identity)
        .refusing_stand_ins("x-stand-in")
        .expect("a header name")
        .send(&envelope_for_primer_alta())
        .await;
    assert!(
        matches!(outcome, Ok(verifactu::TransportOutcome::Response(_))),
        "{outcome:?}"
    );

    let error = transport_to(&endpoint, roots, &identity)
        .refusing_stand_ins("x stand in")
        .expect_err("a space is no token character");
    assert!(
        matches!(error, verifactu::engine::WireBuildError::Mark(_)),
        "{error}"
    );
}
