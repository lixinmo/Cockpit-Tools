use super::*;
use axum::body::Bytes;
use http_body_util::{BodyExt, Full};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use tokio::net::TcpListener;

fn tls_config() -> Arc<rustls::ServerConfig> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let certificate = rcgen::generate_simple_self_signed(vec!["chatgpt.com".into()]).unwrap();
    let key = rustls::pki_types::PrivatePkcs8KeyDer::from(certificate.key_pair.serialize_der());
    let mut config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(vec![certificate.cert.der().clone()], key.into())
        .unwrap();
    config.alpn_protocols = vec![b"h2".to_vec()];
    Arc::new(config)
}

struct CountedBody {
    polls: Arc<AtomicUsize>,
    drops: Arc<AtomicUsize>,
    emitted: bool,
}
impl futures_util::Stream for CountedBody {
    type Item = Result<Bytes, io::Error>;
    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        self.polls.fetch_add(1, Ordering::SeqCst);
        if std::mem::replace(&mut self.emitted, true) {
            std::task::Poll::Ready(None)
        } else {
            std::task::Poll::Ready(Some(Ok(Bytes::from_static(b"{\"message\":\"unchanged\"}"))))
        }
    }
}
impl Drop for CountedBody {
    fn drop(&mut self) {
        self.drops.fetch_add(1, Ordering::SeqCst);
    }
}

async fn server(
    tls_failures: usize,
    reject_after_send: bool,
) -> (
    SocketAddr,
    Arc<AtomicUsize>,
    Arc<AtomicUsize>,
    tokio::task::JoinHandle<()>,
) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let connects = Arc::new(AtomicUsize::new(0));
    let requests = Arc::new(AtomicUsize::new(0));
    let connections = connects.clone();
    let received = requests.clone();
    let acceptor = tokio_rustls::TlsAcceptor::from(tls_config());
    let task = tokio::spawn(async move {
        loop {
            let (socket, _) = listener.accept().await.unwrap();
            if connections.fetch_add(1, Ordering::SeqCst) < tls_failures {
                drop(socket);
                continue;
            }
            let acceptor = acceptor.clone();
            let received = received.clone();
            tokio::spawn(async move {
                let Ok(tls) = acceptor.accept(socket).await else {
                    return;
                };
                let service = hyper::service::service_fn(
                    move |request: hyper::Request<hyper::body::Incoming>| {
                        let received = received.clone();
                        async move {
                            assert_eq!(request.method(), hyper::Method::POST);
                            assert_eq!(
                                request.headers()["authorization"],
                                "Bearer synthetic-account"
                            );
                            let bytes = request.into_body().collect().await.unwrap().to_bytes();
                            assert_eq!(&bytes[..], b"{\"message\":\"unchanged\"}");
                            received.fetch_add(1, Ordering::SeqCst);
                            if reject_after_send {
                                return Err(io::Error::from(io::ErrorKind::UnexpectedEof));
                            }
                            Ok::<_, io::Error>(hyper::Response::new(Full::new(Bytes::from_static(
                                b"ok",
                            ))))
                        }
                    },
                );
                let _ =
                    hyper::server::conn::http2::Builder::new(hyper_util::rt::TokioExecutor::new())
                        .serve_connection(hyper_util::rt::TokioIo::new(tls), service)
                        .await;
            });
        }
    });
    (address, connects, requests, task)
}

fn client(address: SocketAddr, trusted: bool, enabled: bool) -> reqwest::Client {
    let builder = reqwest::Client::builder()
        .use_rustls_tls()
        .no_proxy()
        .resolve("chatgpt.com", address)
        .danger_accept_invalid_certs(trusted)
        .connect_timeout(std::time::Duration::from_secs(2));
    let builder = if enabled {
        configure(builder, "https://chatgpt.com", false)
    } else {
        builder
    };
    builder.build().unwrap()
}

async fn request(
    client: reqwest::Client,
    address: SocketAddr,
) -> (Result<reqwest::Response, reqwest::Error>, usize, usize) {
    let polls = Arc::new(AtomicUsize::new(0));
    let drops = Arc::new(AtomicUsize::new(0));
    let result = client
        .post(format!(
            "https://chatgpt.com:{}/backend-api/codex/responses",
            address.port()
        ))
        .header("authorization", "Bearer synthetic-account")
        .body(reqwest::Body::wrap_stream(CountedBody {
            polls: polls.clone(),
            drops: drops.clone(),
            emitted: false,
        }))
        .send()
        .await;
    tokio::task::yield_now().await;
    (
        result,
        polls.load(Ordering::SeqCst),
        drops.load(Ordering::SeqCst),
    )
}

#[tokio::test]
async fn native_real_tls_setup_retries_before_one_h2_post() {
    let (address, connects, requests, task) = server(2, false).await;
    let started = std::time::Instant::now();
    let (result, polls, drops) = request(client(address, true, true), address).await;
    let response = result.unwrap();
    assert_eq!(response.version(), reqwest::Version::HTTP_2);
    assert_eq!(response.text().await.unwrap(), "ok");
    assert_eq!(connects.load(Ordering::SeqCst), 3);
    assert_eq!(requests.load(Ordering::SeqCst), 1);
    assert!(polls > 0);
    assert_eq!(drops, 1);
    assert!(started.elapsed() >= std::time::Duration::from_millis(600));
    task.abort();
}

#[tokio::test]
async fn native_final_setup_failure_never_polls_and_drops_body_once() {
    let (address, connects, requests, task) = server(usize::MAX, false).await;
    let (result, polls, drops) = request(client(address, true, true), address).await;
    assert!(result.unwrap_err().is_connect());
    assert_eq!(connects.load(Ordering::SeqCst), 3);
    assert_eq!(requests.load(Ordering::SeqCst), 0);
    assert_eq!(polls, 0);
    assert_eq!(drops, 1);
    task.abort();
}

#[tokio::test]
async fn native_tls_certificate_failure_is_permanent() {
    let (address, connects, _, task) = server(0, false).await;
    let (result, polls, drops) = request(client(address, false, true), address).await;
    assert!(result.is_err());
    assert_eq!(connects.load(Ordering::SeqCst), 1);
    assert_eq!(polls, 0);
    assert_eq!(drops, 1);
    task.abort();
}

#[tokio::test]
async fn native_sent_post_is_never_replayed_by_setup_retry() {
    let (address, connects, requests, task) = server(0, true).await;
    let (result, _, drops) = request(client(address, true, true), address).await;
    assert!(result.is_err());
    assert_eq!(connects.load(Ordering::SeqCst), 1);
    assert_eq!(requests.load(Ordering::SeqCst), 1);
    assert_eq!(drops, 1);
    task.abort();
}

#[tokio::test]
async fn native_non_opted_in_transport_keeps_one_setup_attempt() {
    let (address, connects, _, task) = server(usize::MAX, false).await;
    let (result, polls, drops) = request(client(address, true, false), address).await;
    assert!(result.is_err());
    assert_eq!(connects.load(Ordering::SeqCst), 1);
    assert_eq!(polls, 0);
    assert_eq!(drops, 1);
    task.abort();
}
