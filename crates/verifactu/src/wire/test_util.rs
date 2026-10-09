use std::sync::Arc;

const TEST_CA_CN: &str = "verifactu wire-transport loopback test CA";

pub struct ClientCertHandle {
    chain: Vec<rustls::pki_types::CertificateDer<'static>>,
    key: rustls::pki_types::PrivateKeyDer<'static>,
}

impl ClientCertHandle {
    #[must_use]
    pub fn chain(&self) -> Vec<rustls::pki_types::CertificateDer<'static>> {
        self.chain.clone()
    }

    #[must_use]
    pub fn key(&self) -> rustls::pki_types::PrivateKeyDer<'static> {
        self.key.clone_key()
    }
}

/// # Panics
/// When certificate minting or the loopback bind fails.
pub async fn serve_tls(router: axum::Router) -> (std::net::SocketAddr, rustls::RootCertStore) {
    let (ca_cert, ca_key) = mint_test_ca();
    let (server_cert, server_key) = mint_server_identity(&ca_cert, &ca_key);
    let config = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .expect("ring supports the default TLS versions")
    .with_no_client_auth()
    .with_single_cert(vec![server_cert], server_key)
    .expect("the test server config builds");
    let address =
        serve_owned_loopback(tokio_rustls::TlsAcceptor::from(Arc::new(config)), router).await;
    (address, trust_store(&ca_cert))
}

/// # Panics
/// When certificate minting, verifier construction, or the loopback
/// bind fails.
pub async fn serve_tls_client_certs(
    router: axum::Router,
) -> (
    std::net::SocketAddr,
    rustls::RootCertStore,
    ClientCertHandle,
) {
    let (ca_cert, ca_key) = mint_test_ca();
    let (server_cert, server_key) = mint_server_identity(&ca_cert, &ca_key);

    let client_key = rcgen::KeyPair::generate().expect("the test client key generates");
    let mut client_params = rcgen::CertificateParams::default();
    client_params.distinguished_name.push(
        rcgen::DnType::CommonName,
        "verifactu wire-transport test client",
    );
    client_params.extended_key_usages = vec![rcgen::ExtendedKeyUsagePurpose::ClientAuth];
    let client_cert = client_params
        .signed_by(&client_key, &ca_cert, &ca_key)
        .expect("the test client certificate issues");
    let identity = ClientCertHandle {
        chain: vec![client_cert.der().clone(), ca_cert.der().clone()],
        key: rustls::pki_types::PrivateKeyDer::Pkcs8(rustls::pki_types::PrivatePkcs8KeyDer::from(
            client_key.serialize_der(),
        )),
    };

    let verifier = rustls::server::WebPkiClientVerifier::builder(Arc::new(trust_store(&ca_cert)))
        .build()
        .expect("the client-cert verifier builds");
    let config = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .expect("ring supports the default TLS versions")
    .with_client_cert_verifier(verifier)
    .with_single_cert(vec![server_cert], server_key)
    .expect("the test server config builds");
    let address =
        serve_owned_loopback(tokio_rustls::TlsAcceptor::from(Arc::new(config)), router).await;
    (address, trust_store(&ca_cert), identity)
}

async fn serve_owned_loopback(
    acceptor: tokio_rustls::TlsAcceptor,
    router: axum::Router,
) -> std::net::SocketAddr {
    use hyper_util::rt::{TokioExecutor, TokioIo};
    use hyper_util::server::conn::auto::Builder;
    use hyper_util::service::TowerToHyperService;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind the loopback listener");
    let address = listener
        .local_addr()
        .expect("the owned socket reports its address");

    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                break;
            };
            let acceptor = acceptor.clone();
            let service = TowerToHyperService::new(router.clone());
            tokio::spawn(async move {
                if let Ok(tls) = acceptor.accept(stream).await {
                    let _ = Builder::new(TokioExecutor::new())
                        .serve_connection_with_upgrades(TokioIo::new(tls), service)
                        .await;
                }
            });
        }
    });
    address
}

fn mint_test_ca() -> (rcgen::Certificate, rcgen::KeyPair) {
    use rcgen::{BasicConstraints, CertificateParams, DnType, IsCa, KeyPair};

    let ca_key = KeyPair::generate().expect("the test CA key generates");
    let mut ca_params = CertificateParams::default();
    ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    ca_params
        .distinguished_name
        .push(DnType::CommonName, TEST_CA_CN);
    let ca_cert = ca_params.self_signed(&ca_key).expect("the test CA issues");
    (ca_cert, ca_key)
}

fn mint_server_identity(
    ca_cert: &rcgen::Certificate,
    ca_key: &rcgen::KeyPair,
) -> (
    rustls::pki_types::CertificateDer<'static>,
    rustls::pki_types::PrivateKeyDer<'static>,
) {
    use rcgen::{CertificateParams, KeyPair, SanType};

    let server_key = KeyPair::generate().expect("the test server key generates");
    let mut server_params = CertificateParams::default();
    server_params.subject_alt_names =
        vec![SanType::IpAddress(std::net::IpAddr::from([127, 0, 0, 1]))];
    let server_cert = server_params
        .signed_by(&server_key, ca_cert, ca_key)
        .expect("the test server certificate issues");
    (
        server_cert.der().clone(),
        rustls::pki_types::PrivateKeyDer::Pkcs8(rustls::pki_types::PrivatePkcs8KeyDer::from(
            server_key.serialize_der(),
        )),
    )
}

fn trust_store(ca_cert: &rcgen::Certificate) -> rustls::RootCertStore {
    let mut roots = rustls::RootCertStore::empty();
    roots
        .add(ca_cert.der().clone())
        .expect("the test CA parses as a trust anchor");
    roots
}
