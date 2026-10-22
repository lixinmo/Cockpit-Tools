//! Exercise the real hyper HTTP/2 preface/ready stage, not HTTP request retries.
use super::*;
use axum::body::Bytes;
use http_body_util::{BodyExt, Full};
use hyper_util::{
    client::legacy::connect::{Connected, Connection},
    rt::{TokioExecutor, TokioIo},
};
use std::{
    pin::Pin,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
    task::{Context, Poll},
    time::Duration,
};

struct OwnedIo {
    inner: TokioIo<tokio::io::DuplexStream>,
    drops: Arc<AtomicUsize>,
    stall_preface: bool,
    eof_before_ready: bool,
}
impl Connection for OwnedIo {
    fn connected(&self) -> Connected {
        Connected::new().negotiated_h2()
    }
}
impl Drop for OwnedIo {
    fn drop(&mut self) {
        self.drops.fetch_add(1, Ordering::SeqCst);
    }
}
impl hyper::rt::Read for OwnedIo {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buffer: hyper::rt::ReadBufCursor<'_>,
    ) -> Poll<Result<(), io::Error>> {
        if self.eof_before_ready {
            return Poll::Ready(Ok(()));
        }
        Pin::new(&mut self.inner).poll_read(cx, buffer)
    }
}
impl hyper::rt::Write for OwnedIo {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buffer: &[u8],
    ) -> Poll<Result<usize, io::Error>> {
        if self.stall_preface {
            return Poll::Pending;
        }
        if self.eof_before_ready {
            return Poll::Ready(Ok(buffer.len()));
        }
        Pin::new(&mut self.inner).poll_write(cx, buffer)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), io::Error>> {
        if self.eof_before_ready {
            return Poll::Ready(Ok(()));
        }
        Pin::new(&mut self.inner).poll_flush(cx)
    }
    fn poll_shutdown(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<(), io::Error>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

#[derive(Clone)]
struct Connector {
    calls: Arc<Mutex<Vec<tokio::time::Instant>>>,
    drops: Arc<AtomicUsize>,
    requests: Arc<AtomicUsize>,
    fail_count: usize,
    stall: bool,
    eof_before_ready: bool,
    response_bytes: usize,
    connect_error: Option<io::ErrorKind>,
}
impl Connector {
    fn new(fail_count: usize) -> Self {
        Self {
            calls: Default::default(),
            drops: Default::default(),
            requests: Default::default(),
            fail_count,
            stall: false,
            eof_before_ready: false,
            response_bytes: 0,
            connect_error: None,
        }
    }
}
impl tower_service::Service<hyper::Uri> for Connector {
    type Response = OwnedIo;
    type Error = io::Error;
    type Future = std::future::Ready<Result<OwnedIo, io::Error>>;
    fn poll_ready(&mut self, _: &mut Context<'_>) -> Poll<Result<(), io::Error>> {
        Poll::Ready(Ok(()))
    }
    fn call(&mut self, _: hyper::Uri) -> Self::Future {
        let index = {
            let mut calls = self.calls.lock().unwrap();
            calls.push(tokio::time::Instant::now());
            calls.len()
        };
        if let Some(kind) = self.connect_error {
            return std::future::ready(Err(io::Error::new(kind, format!("setup-attempt-{index}"))));
        }
        let (client, server) = tokio::io::duplex(4096);
        if index <= self.fail_count {
            // Closing a real byte transport makes the HTTP/2 preface write
            // fail. The connector itself has already returned successfully.
            drop(server);
        } else {
            let received = self.requests.clone();
            let response_bytes = self.response_bytes;
            tokio::spawn(async move {
                let service = hyper::service::service_fn(
                    move |request: hyper::Request<hyper::body::Incoming>| {
                        let received = received.clone();
                        async move {
                            let bytes = request.into_body().collect().await.unwrap().to_bytes();
                            assert_eq!(&bytes[..], b"once");
                            received.fetch_add(1, Ordering::SeqCst);
                            let body = if response_bytes == 0 {
                                Bytes::from_static(b"ok")
                            } else {
                                Bytes::from(vec![b'x'; response_bytes])
                            };
                            Ok::<_, io::Error>(hyper::Response::new(Full::new(body)))
                        }
                    },
                );
                let _ = hyper::server::conn::http2::Builder::new(TokioExecutor::new())
                    .serve_connection(TokioIo::new(server), service)
                    .await;
            });
        }
        std::future::ready(Ok(OwnedIo {
            inner: TokioIo::new(client),
            drops: self.drops.clone(),
            stall_preface: self.stall,
            eof_before_ready: self.eof_before_ready && index <= self.fail_count,
        }))
    }
}

struct Body {
    polls: Arc<AtomicUsize>,
    drops: Arc<AtomicUsize>,
    sent: bool,
}
impl Drop for Body {
    fn drop(&mut self) {
        self.drops.fetch_add(1, Ordering::SeqCst);
    }
}
impl hyper::body::Body for Body {
    type Data = Bytes;
    type Error = io::Error;
    fn poll_frame(
        mut self: Pin<&mut Self>,
        _: &mut Context<'_>,
    ) -> Poll<Option<Result<hyper::body::Frame<Bytes>, io::Error>>> {
        self.polls.fetch_add(1, Ordering::SeqCst);
        if std::mem::replace(&mut self.sent, true) {
            Poll::Ready(None)
        } else {
            Poll::Ready(Some(Ok(hyper::body::Frame::data(Bytes::from_static(
                b"once",
            )))))
        }
    }
}
fn request(polls: Arc<AtomicUsize>, drops: Arc<AtomicUsize>) -> hyper::Request<Body> {
    hyper::Request::post("https://chatgpt.com/responses")
        .body(Body {
            polls,
            drops,
            sent: false,
        })
        .unwrap()
}

#[tokio::test(start_paused = true)]
async fn native_h2_preface_failure_uses_same_three_attempt_budget_before_body() {
    let connector = Connector::new(2);
    let mut builder = hyper_util::client::legacy::Client::builder(TokioExecutor::new());
    builder.cockpit_connect_retry(retryable, Some(Duration::from_secs(3)));
    let client = builder.build(connector.clone());
    let polls = Arc::new(AtomicUsize::new(0));
    let drops = Arc::new(AtomicUsize::new(0));
    let response = client
        .request(request(polls.clone(), drops.clone()))
        .await
        .unwrap();
    assert_eq!(
        response.into_body().collect().await.unwrap().to_bytes(),
        "ok"
    );
    let calls = connector.calls.lock().unwrap().clone();
    assert_eq!(calls.len(), 3);
    assert_eq!(calls[1] - calls[0], Duration::from_millis(200));
    assert_eq!(calls[2] - calls[1], Duration::from_millis(400));
    assert_eq!(connector.requests.load(Ordering::SeqCst), 1);
    assert!(polls.load(Ordering::SeqCst) > 0);
    assert_eq!(drops.load(Ordering::SeqCst), 1);
    for _ in 0..10 {
        tokio::task::yield_now().await;
    }
    assert_eq!(connector.drops.load(Ordering::SeqCst), 3);
}

#[tokio::test(start_paused = true)]
async fn native_h2_cancellation_drops_partial_dispatcher_and_unread_body() {
    let mut connector = Connector::new(0);
    connector.stall = true;
    let mut builder = hyper_util::client::legacy::Client::builder(TokioExecutor::new());
    builder.cockpit_connect_retry(retryable, Some(Duration::from_secs(3)));
    let client = builder.build(connector.clone());
    let polls = Arc::new(AtomicUsize::new(0));
    let drops = Arc::new(AtomicUsize::new(0));
    let work = tokio::spawn(client.request(request(polls.clone(), drops.clone())));
    for _ in 0..10 {
        tokio::task::yield_now().await;
    }
    assert_eq!(polls.load(Ordering::SeqCst), 0);
    work.abort();
    let _ = work.await;
    for _ in 0..10 {
        tokio::task::yield_now().await;
    }
    tokio::time::advance(Duration::from_secs(1)).await;
    assert_eq!(connector.calls.lock().unwrap().len(), 1);
    assert_eq!(connector.drops.load(Ordering::SeqCst), 1);
    assert_eq!(drops.load(Ordering::SeqCst), 1);
}

#[tokio::test(start_paused = true)]
async fn native_h2_eof_after_setup_send_boundary_is_not_replayed() {
    let mut connector = Connector::new(2);
    connector.eof_before_ready = true;
    let mut builder = hyper_util::client::legacy::Client::builder(TokioExecutor::new());
    builder.cockpit_connect_retry(retryable, Some(Duration::from_secs(3)));
    let client = builder.build(connector.clone());
    let polls = Arc::new(AtomicUsize::new(0));
    let drops = Arc::new(AtomicUsize::new(0));
    // hyper has already handed this request to its sender when the remote EOF
    // is reported. An unread body is not permission to move that boundary.
    let result = client.request(request(polls.clone(), drops.clone())).await;
    assert!(result.is_err());
    assert_eq!(connector.calls.lock().unwrap().len(), 1);
    assert_eq!(connector.requests.load(Ordering::SeqCst), 0);
    assert_eq!(drops.load(Ordering::SeqCst), 1);
}

#[tokio::test(start_paused = true)]
async fn native_h2_setup_deadline_spans_attempts_and_backoff() {
    let connector = Connector::new(usize::MAX);
    let mut builder = hyper_util::client::legacy::Client::builder(TokioExecutor::new());
    builder.cockpit_connect_retry(retryable, Some(Duration::from_millis(300)));
    let client = builder.build(connector.clone());
    let polls = Arc::new(AtomicUsize::new(0));
    let drops = Arc::new(AtomicUsize::new(0));
    let start = tokio::time::Instant::now();
    let error = client
        .request(request(polls.clone(), drops.clone()))
        .await
        .unwrap_err();
    assert!(error.is_connect());
    assert_eq!(
        tokio::time::Instant::now() - start,
        Duration::from_millis(300)
    );
    assert_eq!(connector.calls.lock().unwrap().len(), 2);
    assert_eq!(polls.load(Ordering::SeqCst), 0);
    assert_eq!(drops.load(Ordering::SeqCst), 1);
    for _ in 0..10 {
        tokio::task::yield_now().await;
    }
    assert_eq!(connector.drops.load(Ordering::SeqCst), 2);
}

#[tokio::test(start_paused = true)]
async fn native_h2_non_chatgpt_and_upgrade_requests_do_not_enter_retry() {
    for (uri, upgrade) in [
        ("https://other.invalid/responses", false),
        ("http://chatgpt.com/responses", false),
        ("https://chatgpt.com/responses", true),
    ] {
        let connector = Connector::new(usize::MAX);
        let mut builder = hyper_util::client::legacy::Client::builder(TokioExecutor::new());
        builder.cockpit_connect_retry(retryable, Some(Duration::from_secs(3)));
        let client = builder.build(connector.clone());
        let mut request = hyper::Request::post(uri);
        if upgrade {
            request = request.header("upgrade", "websocket");
        }
        let result = client
            .request(
                request
                    .body(Full::new(Bytes::from_static(b"untouched")))
                    .unwrap(),
            )
            .await;
        assert!(result.is_err());
        assert_eq!(connector.calls.lock().unwrap().len(), 1);
    }
}

#[tokio::test(start_paused = true)]
async fn native_h2_drop_response_closes_its_dedicated_active_connection() {
    let mut connector = Connector::new(0);
    connector.response_bytes = 1024 * 1024;
    let mut builder = hyper_util::client::legacy::Client::builder(TokioExecutor::new());
    builder.cockpit_connect_retry(retryable, Some(Duration::from_secs(3)));
    let client = builder.build(connector.clone());
    let response = client
        .request(request(Default::default(), Default::default()))
        .await
        .unwrap();
    for _ in 0..10 {
        tokio::task::yield_now().await;
    }
    assert_eq!(
        connector.drops.load(Ordering::SeqCst),
        0,
        "active response still owns its connection"
    );
    drop(response);
    for _ in 0..20 {
        tokio::task::yield_now().await;
    }
    assert_eq!(connector.drops.load(Ordering::SeqCst), 1);
    assert_eq!(connector.calls.lock().unwrap().len(), 1);
}

#[tokio::test(start_paused = true)]
async fn native_h2_cancel_before_poll_does_not_connect_or_consume_body() {
    let connector = Connector::new(0);
    let mut builder = hyper_util::client::legacy::Client::builder(TokioExecutor::new());
    builder.cockpit_connect_retry(retryable, Some(Duration::from_secs(3)));
    let client = builder.build(connector.clone());
    let polls = Arc::new(AtomicUsize::new(0));
    let drops = Arc::new(AtomicUsize::new(0));
    drop(client.request(request(polls.clone(), drops.clone())));
    assert!(connector.calls.lock().unwrap().is_empty());
    assert_eq!(polls.load(Ordering::SeqCst), 0);
    assert_eq!(drops.load(Ordering::SeqCst), 1);
}

#[tokio::test(start_paused = true)]
async fn native_setup_deadline_wins_when_retry_backoff_is_also_ready() {
    let mut connector = Connector::new(0);
    connector.connect_error = Some(io::ErrorKind::ConnectionReset);
    let mut builder = hyper_util::client::legacy::Client::builder(TokioExecutor::new());
    builder.cockpit_connect_retry(retryable, Some(Duration::from_millis(200)));
    let client = builder.build(connector.clone());
    let error = client
        .request(request(Default::default(), Default::default()))
        .await
        .unwrap_err();
    assert_eq!(connector.calls.lock().unwrap().len(), 1);
    assert_eq!(
        error
            .source()
            .unwrap()
            .downcast_ref::<io::Error>()
            .unwrap()
            .kind(),
        io::ErrorKind::TimedOut
    );
}

#[tokio::test(start_paused = true)]
async fn native_setup_returns_original_last_error_and_stops_on_unknown() {
    for (kind, expected) in [
        (io::ErrorKind::ConnectionReset, 3),
        (io::ErrorKind::Other, 1),
        (io::ErrorKind::PermissionDenied, 1),
    ] {
        let mut connector = Connector::new(0);
        connector.connect_error = Some(kind);
        let mut builder = hyper_util::client::legacy::Client::builder(TokioExecutor::new());
        builder.cockpit_connect_retry(retryable, Some(Duration::from_secs(3)));
        let client = builder.build(connector.clone());
        let polls = Arc::new(AtomicUsize::new(0));
        let drops = Arc::new(AtomicUsize::new(0));
        let error = client
            .request(request(polls.clone(), drops.clone()))
            .await
            .unwrap_err();
        assert_eq!(connector.calls.lock().unwrap().len(), expected);
        let source = error.source().unwrap().downcast_ref::<io::Error>().unwrap();
        assert_eq!(source.kind(), kind);
        assert_eq!(source.to_string(), format!("setup-attempt-{expected}"));
        assert_eq!(polls.load(Ordering::SeqCst), 0);
        assert_eq!(drops.load(Ordering::SeqCst), 1);
    }
}
