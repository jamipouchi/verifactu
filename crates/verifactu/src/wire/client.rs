use std::error::Error as _;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use bytes::Bytes;
use http_body_util::BodyExt as _;
use http_body_util::Full;
use hyper_util::client::legacy::connect::Connected;
use hyper_util::client::legacy::connect::Connection;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::client::legacy::Client;
use hyper_util::rt::TokioExecutor;
use hyper_util::rt::TokioIo;

use crate::wire::recorder::{
    RecordedExchange, RecordedRequest, RecordedResponse, Recorder, RecorderShared,
};
use crate::wire::WireError;

/// Trusts the Mozilla root bundle by default — [`Self::with_roots`]
/// is the explicit-trust seam.
#[derive(Clone)]
pub struct HttpWireClient {
    client: Client<ConnectorService, Full<Bytes>>,
    timeout: Duration,
    roots: rustls::RootCertStore,
    recorder: Option<Arc<RecorderShared>>,
}

impl HttpWireClient {
    #[must_use]
    pub fn new(request_timeout: Duration) -> Self {
        let mut roots = rustls::RootCertStore::empty();
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        Self::with_roots(roots, request_timeout)
    }

    #[must_use]
    pub fn with_roots(roots: rustls::RootCertStore, request_timeout: Duration) -> Self {
        Self::build(roots, Arc::new(NoClientCert), request_timeout)
    }

    /// # Errors
    /// [`WireError::Protocol`] when rustls rejects the chain/key pair.
    pub fn with_client_cert(
        self,
        chain: Vec<rustls::pki_types::CertificateDer<'static>>,
        key: rustls::pki_types::PrivateKeyDer<'static>,
    ) -> Result<Self, WireError> {
        let certified = rustls::sign::CertifiedKey::from_der(
            chain,
            key,
            &rustls::crypto::ring::default_provider(),
        )
        .map_err(|e| WireError::Protocol {
            detail: format!("client certificate rejected at construction: {e}"),
        })?;
        Ok(self.rebuild(Arc::new(rustls::sign::SingleCertAndKey::from(certified))))
    }

    #[must_use]
    pub fn recording(mut self) -> (Self, Recorder) {
        let shared = Arc::new(RecorderShared::default());
        self.recorder = Some(Arc::clone(&shared));
        (self, Recorder::of(shared))
    }

    /// # Errors
    /// [`WireError`] — see the enum's classing law.
    pub fn send(
        &self,
        req: http::Request<String>,
    ) -> impl Future<Output = Result<http::Response<String>, WireError>> + Send {
        let client = self.client.clone();
        let timeout = self.timeout;
        let recorder = self.recorder.clone();
        async move {
            // Recorded before the send, pushed after it settles — the
            // evidence is the port's own truth.
            match recorder.as_ref() {
                Some(shared) => {
                    let tee = RecordedRequest::of(&req);
                    let outcome = exchange(client, req, timeout).await;
                    shared.push(RecordedExchange {
                        request: tee,
                        response: RecordedResponse::of(&outcome),
                    });
                    outcome
                }
                None => exchange(client, req, timeout).await,
            }
        }
    }

    fn rebuild(&self, resolver: Arc<dyn rustls::client::ResolvesClientCert>) -> Self {
        let mut rebuilt = Self::build(self.roots.clone(), resolver, self.timeout);
        rebuilt.recorder.clone_from(&self.recorder);
        rebuilt
    }

    fn build(
        roots: rustls::RootCertStore,
        resolver: Arc<dyn rustls::client::ResolvesClientCert>,
        request_timeout: Duration,
    ) -> Self {
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let mut config = rustls::ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .expect("ring supports the enabled TLS versions (workspace pin: 1.3 + 1.2)")
            .with_root_certificates(roots.clone())
            .with_client_cert_resolver(resolver);
        // ALPN: http/1.1 only — nothing else may be negotiated (the
        // hyper-util pin builds no h2 leg).
        config.alpn_protocols = vec![b"http/1.1".to_vec()];
        let tls = tokio_rustls::TlsConnector::from(Arc::new(config));
        let mut http = HttpConnector::new();
        // The TLS arm wraps this connector's dial, so it must accept
        // `https` URIs too — only the scheme check is lifted.
        http.enforce_http(false);
        // Parity with `Client::builder().build_http()`: the legacy
        // pool's 90 s idle timeout expects TCP keepalive, hand-set now
        // the connector is ours.
        http.set_keepalive(Some(Duration::from_secs(90)));
        let client = Client::builder(TokioExecutor::new()).build(ConnectorService { http, tls });
        Self {
            client,
            timeout: request_timeout,
            roots,
            recorder: None,
        }
    }
}

async fn exchange(
    client: Client<ConnectorService, Full<Bytes>>,
    req: http::Request<String>,
    timeout: Duration,
) -> Result<http::Response<String>, WireError> {
    match req.uri().scheme_str() {
        Some("http" | "https") => {}
        other => Err(WireError::Protocol {
            detail: format!("request URI must carry an explicit http(s) scheme, got {other:?}"),
        })?,
    }
    let exchanged = async {
        let (parts, body) = req.into_parts();
        let req = http::Request::from_parts(parts, Full::new(Bytes::from(body)));
        let response = client.request(req).await.map_err(|e| classify(&e))?;
        let (parts, body) = response.into_parts();
        let bytes = body.collect().await.map_err(|e| WireError::Unreachable {
            detail: format!("response body lost mid-stream: {e}"),
        })?;
        let text =
            String::from_utf8(bytes.to_bytes().to_vec()).map_err(|e| WireError::Protocol {
                detail: format!("response body is not UTF-8: {e}"),
            })?;
        Ok::<http::Response<String>, WireError>(http::Response::from_parts(parts, text))
    };
    tokio::time::timeout(timeout, exchanged)
        .await
        .map_err(|_| WireError::Timeout)?
}

#[derive(Clone)]
struct ConnectorService {
    http: HttpConnector,
    tls: tokio_rustls::TlsConnector,
}

enum DialedStream {
    Plain(<HttpConnector as tower_service::Service<http::Uri>>::Response),
    Tls(Box<TokioIo<tokio_rustls::client::TlsStream<tokio::net::TcpStream>>>),
}

impl hyper::rt::Read for DialedStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: hyper::rt::ReadBufCursor<'_>,
    ) -> Poll<Result<(), std::io::Error>> {
        match self.get_mut() {
            Self::Plain(io) => Pin::new(io).poll_read(cx, buf),
            Self::Tls(io) => Pin::new(&mut **io).poll_read(cx, buf),
        }
    }
}

impl hyper::rt::Write for DialedStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, std::io::Error>> {
        match self.get_mut() {
            Self::Plain(io) => Pin::new(io).poll_write(cx, buf),
            Self::Tls(io) => Pin::new(&mut **io).poll_write(cx, buf),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), std::io::Error>> {
        match self.get_mut() {
            Self::Plain(io) => Pin::new(io).poll_flush(cx),
            Self::Tls(io) => Pin::new(&mut **io).poll_flush(cx),
        }
    }

    fn poll_shutdown(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<(), std::io::Error>> {
        match self.get_mut() {
            Self::Plain(io) => Pin::new(io).poll_shutdown(cx),
            Self::Tls(io) => Pin::new(&mut **io).poll_shutdown(cx),
        }
    }
}

impl Connection for DialedStream {
    fn connected(&self) -> Connected {
        match self {
            Self::Plain(io) => io.connected(),
            Self::Tls(io) => io.inner().get_ref().0.connected(),
        }
    }
}

type BoxError = Box<dyn std::error::Error + Send + Sync>;

impl tower_service::Service<http::Uri> for ConnectorService {
    type Response = DialedStream;
    type Error = BoxError;
    type Future = Pin<Box<dyn Future<Output = Result<DialedStream, BoxError>> + Send + 'static>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, dst: http::Uri) -> Self::Future {
        let mut http = self.http.clone();
        let tls = self.tls.clone();
        Box::pin(async move {
            let stream = http.call(dst.clone()).await?;
            if dst.scheme_str() != Some("https") {
                return Ok(DialedStream::Plain(stream));
            }
            let host = dst.host().ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::InvalidInput, "URI carries no host")
            })?;
            let name = rustls::pki_types::ServerName::try_from(host.to_owned()).map_err(|e| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!("URI host {host:?} is no TLS server name: {e}"),
                )
            })?;
            let tls_stream = tls.connect(name, stream.into_inner()).await?;
            Ok(DialedStream::Tls(Box::new(TokioIo::new(tls_stream))))
        })
    }
}

/// Connect errors (the TLS handshake included) → `Unreachable`;
/// identified timeouts → `Timeout`; everything else → conservative
/// `Unreachable` (both transient).
fn classify(err: &hyper_util::client::legacy::Error) -> WireError {
    if err.is_connect() {
        return WireError::Unreachable {
            detail: format!("client error (Connect): {}", ChainDisplay(err.source())),
        };
    }
    let mut source: Option<&(dyn std::error::Error + 'static)> = err.source();
    while let Some(error) = source {
        if let Some(hyper_error) = error.downcast_ref::<hyper::Error>() {
            if hyper_error.is_timeout() {
                return WireError::Timeout;
            }
        }
        if let Some(io_error) = error.downcast_ref::<std::io::Error>() {
            if io_error.kind() == std::io::ErrorKind::TimedOut {
                return WireError::Timeout;
            }
        }
        source = error.source();
    }
    // Under TLS 1.3 the server's CertificateRequired alert arrives
    // after the client handshake completes, so hyper-util classes it
    // SendRequest — this arm's — and a bare `err.to_string()` would
    // drop the alert leaf the operator needs.
    let chain = ChainDisplay(err.source()).to_string();
    WireError::Unreachable {
        detail: if chain.is_empty() {
            err.to_string()
        } else {
            format!("{err}: {chain}")
        },
    }
}

struct ChainDisplay<'a>(Option<&'a (dyn std::error::Error + 'static)>);

impl std::fmt::Display for ChainDisplay<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut source = self.0;
        let mut first = true;
        while let Some(error) = source {
            write!(f, "{}{error}", if first { "" } else { ": " })?;
            first = false;
            source = error.source();
        }
        Ok(())
    }
}

/// The no-client-identity resolver — rustls's own
/// `with_no_client_auth` resolver type is private.
#[derive(Debug)]
struct NoClientCert;

impl rustls::client::ResolvesClientCert for NoClientCert {
    fn resolve(
        &self,
        _root_hint_subjects: &[&[u8]],
        _sigschemes: &[rustls::SignatureScheme],
    ) -> Option<Arc<rustls::sign::CertifiedKey>> {
        None
    }

    fn has_certs(&self) -> bool {
        false
    }
}
