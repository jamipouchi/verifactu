//! The failure classes a live endpoint cannot deterministically
//! produce.

#![cfg(feature = "test-util")]

use std::time::Duration;

use axum::Router;
use verifactu::engine::test_util::serve_tls;
use verifactu::engine::test_util::serve_tls_client_certs;
use verifactu::engine::HttpWireClient;
use verifactu::engine::WireError;

const DEADLINE: Duration = Duration::from_secs(30);

fn known_body_router() -> Router {
    axum::Router::new().route("/known", axum::routing::get(|| async { "the known body" }))
}

fn request(method: http::Method, uri: &str, body: &str) -> http::Request<String> {
    http::Request::builder()
        .method(method)
        .uri(uri)
        .header(http::header::CONTENT_TYPE, "text/plain")
        .body(body.to_owned())
        .expect("the test request builds")
}

#[tokio::test]
async fn request_to_never_answering_listener_elapses_the_deadline_into_timeout() {
    // The listener never accepts — only the deadline can fire.
    let deaf = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind the loopback listener");
    let address = deaf
        .local_addr()
        .expect("the owned socket reports its address");
    let client = HttpWireClient::new(Duration::from_millis(100));
    let error = client
        .send(request(
            http::Method::GET,
            &format!("http://{address}/known"),
            "",
        ))
        .await
        .expect_err("no answer ever arrives");
    assert_eq!(error, WireError::Timeout);
    drop(deaf);
}

#[tokio::test]
async fn request_to_dropped_listener_is_unreachable_with_the_refused_dial_detail() {
    let address = {
        let dropped = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind the loopback listener");
        dropped
            .local_addr()
            .expect("the owned socket reports its address")
    };
    let client = HttpWireClient::new(DEADLINE);
    let WireError::Unreachable { detail } = client
        .send(request(
            http::Method::GET,
            &format!("http://{address}/known"),
            "",
        ))
        .await
        .expect_err("the dial is refused")
    else {
        panic!("a refused dial classifies Unreachable");
    };
    assert!(
        detail.contains("refused"),
        "the detail carries the OS refusal: {detail}"
    );
}

#[tokio::test]
async fn default_trust_is_the_mozilla_bundle_so_the_owned_ca_listener_is_unreachable() {
    let (address, _minted_roots) = serve_tls(known_body_router()).await;
    let client = HttpWireClient::new(DEADLINE);
    let WireError::Unreachable { detail } = client
        .send(request(
            http::Method::GET,
            &format!("https://{address}/known"),
            "",
        ))
        .await
        .expect_err("the Mozilla bundle does not trust the throwaway CA")
    else {
        panic!("an untrusted server certificate classifies Unreachable");
    };
    assert!(
        detail.contains("certificate"),
        "the TLS failure names the certificate: {detail}"
    );
}

#[tokio::test]
async fn client_without_identity_against_cert_demanding_server_fails_unreachable() {
    let (address, roots, _identity) = serve_tls_client_certs(known_body_router()).await;
    let client = HttpWireClient::with_roots(roots, DEADLINE);
    let WireError::Unreachable { detail } = client
        .send(request(
            http::Method::GET,
            &format!("https://{address}/known"),
            "",
        ))
        .await
        .expect_err("the exchange fails without a client certificate")
    else {
        panic!("the server's certificate demand classifies Unreachable");
    };
    assert!(
        detail.contains("alert"),
        "the conservative arm carries the chain (the alert leaf): {detail}"
    );
}
