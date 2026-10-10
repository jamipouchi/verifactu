//! The refused exchange is an HTTP 302 to the sede error page — the
//! transport must name that door, never a generic parse failure.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use axum::http::StatusCode;
use verifactu::engine::test_util;
use verifactu::engine::xml::{self, CabeceraRemision};
use verifactu::engine::EmissionContext;
use verifactu::engine::SistemaInformaticoConfig;
use verifactu::VerifactuTransport as _;
use verifactu::{DetalleDesglose, Obligado};

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
        fecha_expedicion: verifactu::FechaExpedicion::parse(&instants.fecha_expedicion)
            .expect("huso renders the dd-mm-yyyy grammar"),
        tipo_factura: verifactu::TipoFactura::F2,
        cuota_total: verifactu::Money::from_cents(210),
        importe_total: verifactu::Money::from_cents(1210),
        fecha_huso_gen: verifactu::FechaHuso::parse(&instants.fecha_huso_gen)
            .expect("huso renders the ISO-8601 grammar"),
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
        id_sistema_informatico: String::from("1"),
        version: String::from("0.1.0"),
        numero_instalacion: String::from("1"),
    };
    let desglose: &'static [DetalleDesglose] = Box::leak(Box::new([iva_super()]));
    let ctx = EmissionContext {
        descripcion_operacion: Some("Venta mostrador"),
        desglose,
        ..EmissionContext::default()
    };
    let node = xml::record_node(&record, &ctx, &obligado, &sif).expect("serializes");
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
) -> (
    String,
    rustls::RootCertStore,
    verifactu::engine::p12::ClientIdentity,
    Arc<AtomicUsize>,
) {
    let served: Arc<AtomicUsize> = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&served);
    let router = axum::Router::new().route(
        "/wlpl/TIKE-CONT/ws/SistemaFacturacion/VerifactuSOAP",
        axum::routing::post(move || {
            let counter = Arc::clone(&counter);
            let body = body.clone();
            async move {
                counter.fetch_add(1, Ordering::SeqCst);
                axum::response::Response::builder()
                    .status(status)
                    .header(axum::http::header::CONTENT_TYPE, "text/xml; charset=utf-8")
                    .body(axum::body::Body::from(body))
                    .expect("the loopback answer builds")
            }
        }),
    );
    let (address, roots, handle) = test_util::serve_tls_client_certs(router).await;
    let identity = verifactu::engine::p12::ClientIdentity::new(handle.chain(), handle.key());
    (
        format!("https://{address}/wlpl/TIKE-CONT/ws/SistemaFacturacion/VerifactuSOAP"),
        roots,
        identity,
        served,
    )
}

#[tokio::test]
async fn a_redirect_answer_is_the_named_certificate_refusal() {
    let (endpoint, roots, identity, _) = aeat_loopback(StatusCode::FOUND, String::new()).await;
    let transport =
        verifactu::engine::HttpVerifactuTransport::new(&endpoint, Duration::from_secs(10))
            .expect("endpoint parses")
            .with_roots(roots)
            .with_client_identity(identity)
            .expect("the minted identity is accepted");

    let failure = transport
        .send(&envelope_for_primer_alta())
        .await
        .expect_err("the gate refuses");
    assert!(failure.detail.contains("HTTP 302"), "{}", failure.detail);
    assert!(
        failure.detail.contains("certificate was not accepted"),
        "{}",
        failure.detail
    );
}
