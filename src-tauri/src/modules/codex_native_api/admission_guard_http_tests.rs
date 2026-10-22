//! Real loopback requests prove admission is checked after route preparation.
use super::{contract::*, rate_limits::QuotaSnapshot, start, Handle, Options};
use axum::{body::Body, http::HeaderMap, response::Response, Router};
use futures_util::{future::BoxFuture, SinkExt, StreamExt};
use serde_json::{json, Value};
use std::{
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};
use tokio_tungstenite::tungstenite::{client::IntoClientRequest, Message};

#[path = "admission_guard_realtime_tests.rs"]
mod realtime_tests;

#[derive(Default)]
struct Guard {
    denied: AtomicBool,
    checks: AtomicUsize,
    observations: Mutex<Vec<QuotaSnapshot>>,
}

impl UpstreamAdmissionGuard for Guard {
    fn check(&self) -> Result<(), ServiceError> {
        self.checks.fetch_add(1, Ordering::SeqCst);
        if self.denied.load(Ordering::SeqCst) {
            Err(ServiceError::new(
                503,
                "credits_policy_blocked",
                "Synthetic Credits policy",
            ))
        } else {
            Ok(())
        }
    }

    fn observe_rate_limits(&self, snapshot: QuotaSnapshot, _: Instant) {
        self.observations.lock().unwrap().push(snapshot);
    }
}

struct GuardHost {
    base: String,
    official: bool,
    oauth: bool,
    block_after_selection: bool,
    guard: Arc<Guard>,
    prepared: AtomicUsize,
    quota_checks: AtomicUsize,
    reports: Mutex<Vec<RequestReport>>,
}

impl GuardHost {
    fn selected(&self, model: String, image: bool) -> Route {
        Route {
            account_id: "guarded-source".into(),
            continuity_account_id: self.official.then(|| "chatgpt_plan_fixture".into()),
            owner_id: "fixture-owner".into(),
            installation_profile: None,
            transport_identity: None,
            official_authorization_revision: self.official.then(|| "fixture-epoch".into()),
            official_registration_id: self.official.then(|| "fixture-registration".into()),
            official_fallback_registration_id: None,
            official_fallback_authorization_revision: None,
            authorization_scope_fingerprint: None,
            quota_identity: None,
            send_guard: Some(self.guard.clone()),
            official_fallback_failure: None,
            base_url: self.base.clone(),
            model,
            headers: HeaderMap::new(),
            client: reqwest::Client::builder().no_proxy().build().unwrap(),
            proxy_url: None,
            proxy_route: None,
            chatgpt: self.oauth && !image,
            chatgpt_plan: self.official && !image,
            wire_api: WireApi::Responses,
            compact_endpoint: None,
            websocket_endpoint: None,
            default_service_tier: None,
            permit: None,
        }
    }
}

impl Host for GuardHost {
    fn authorize<'a>(&'a self, _: &'a str) -> BoxFuture<'a, Result<Caller, ServiceError>> {
        Box::pin(async {
            Ok(Caller {
                id: "guard-caller".into(),
                label: "fixture".into(),
            })
        })
    }

    fn route<'a>(
        &'a self,
        _: &'a Caller,
        request: RouteRequest,
    ) -> BoxFuture<'a, Result<Route, ServiceError>> {
        Box::pin(async move {
            if request
                .excluded_account_ids
                .iter()
                .any(|id| id == "guarded-source")
            {
                return Err(ServiceError::new(
                    503,
                    "no_available_account",
                    "Fixture exhausted",
                ));
            }
            let route = self.selected(request.model, false);
            if request.auxiliary.as_deref() == Some("realtime") && self.block_after_selection {
                // Realtime has no body-preparation callback. Change the policy
                // after constructing the selected route, before dispatch.
                self.guard.denied.store(true, Ordering::SeqCst);
            }
            Ok(route)
        })
    }

    fn prepare_body<'a>(
        &'a self,
        _: &'a Caller,
        _: &'a Route,
        _: &'a HeaderMap,
        body: Value,
        _: bool,
        _: bool,
    ) -> BoxFuture<'a, Result<Value, ServiceError>> {
        Box::pin(async move {
            self.prepared.fetch_add(1, Ordering::SeqCst);
            if self.block_after_selection {
                // Selection succeeded while allowed. A policy change during
                // request preparation must be caught at the actual send gate.
                self.guard.denied.store(true, Ordering::SeqCst);
            }
            Ok(body)
        })
    }

    fn image_route<'a>(
        &'a self,
        _: &'a Caller,
        request: ImageRouteRequest,
    ) -> BoxFuture<'a, Result<ImageRoute, ServiceError>> {
        Box::pin(async move {
            if request
                .excluded_account_ids
                .iter()
                .any(|id| id == "guarded-source")
            {
                return Err(ServiceError::new(
                    503,
                    "image_source_unavailable",
                    "Fixture exhausted",
                ));
            }
            let route = self.selected("fixture-image".into(), true);
            self.guard
                .denied
                .store(self.block_after_selection, Ordering::SeqCst);
            Ok(ImageRoute {
                route,
                image_model: "fixture-image".into(),
                main_model: None,
            })
        })
    }

    fn check_quota_before_send(&self, _: &str) -> Result<(), ServiceError> {
        self.quota_checks.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn models<'a>(&'a self, _: &'a Caller) -> BoxFuture<'a, Result<Value, ServiceError>> {
        Box::pin(async { Ok(json!({"data":[]})) })
    }

    fn execution_policy(&self) -> BoxFuture<'_, Result<ExecutionPolicy, ServiceError>> {
        Box::pin(async {
            Ok(ExecutionPolicy {
                credential_retries: 1,
                send_retries: 0,
                ..Default::default()
            })
        })
    }

    fn record(&self, report: RequestReport) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            self.reports.lock().unwrap().push(report);
        })
    }
}

struct Fixture {
    host: Arc<GuardHost>,
    upstream_calls: Arc<AtomicUsize>,
    upstream: tokio::task::JoinHandle<()>,
    handle: Handle,
    _root: tempfile::TempDir,
}

impl Fixture {
    async fn new(official: bool, oauth: bool, block_after_selection: bool) -> Self {
        let upstream_calls = Arc::new(AtomicUsize::new(0));
        let calls = upstream_calls.clone();
        let app = Router::new().fallback(move || {
            let calls = calls.clone();
            async move {
                calls.fetch_add(1, Ordering::SeqCst);
                let events = [
                    json!({"type":"codex.rate_limits","rate_limits":{"secondary":{"used_percent":87.5,"window_minutes":10080}}}),
                    json!({"type":"response.completed","response":{"id":"guard-response","status":"completed","output":[{"type":"message","role":"assistant","content":[{"type":"output_text","text":"OK","annotations":[]}]}],"usage":{"input_tokens":2,"output_tokens":1,"total_tokens":3}}}),
                ];
                let body = events.iter().map(|event| format!("data: {event}\n\n")).collect::<String>();
                Response::builder().header("content-type", "text/event-stream")
                    .header("x-codex-primary-used-percent", "99.5")
                    .header("x-codex-primary-window-minutes", "300")
                    .body(Body::from(body)).unwrap()
            }
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let host = Arc::new(GuardHost {
            base: format!("http://{}/v1", listener.local_addr().unwrap()),
            official,
            oauth,
            block_after_selection,
            guard: Arc::new(Guard::default()),
            prepared: AtomicUsize::new(0),
            quota_checks: AtomicUsize::new(0),
            reports: Mutex::new(Vec::new()),
        });
        let upstream = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let root = tempfile::tempdir().unwrap();
        let handle = start(
            "127.0.0.1:0".parse().unwrap(),
            host.clone(),
            root.path().join("identity.sqlite"),
            Options::default(),
        )
        .await
        .unwrap();
        Self {
            host,
            upstream_calls,
            upstream,
            handle,
            _root: root,
        }
    }

    async fn finish(mut self) {
        self.handle.shutdown(Duration::from_secs(3)).await.unwrap();
        self.upstream.abort();
    }
}

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap()
}

#[tokio::test]
async fn native_admission_blocks_local_oauth_custom_and_official_http_before_upstream_send() {
    for (official, oauth) in [(false, true), (false, false), (true, false)] {
        for stream in [false, true] {
            let fixture = Fixture::new(official, oauth, true).await;
            let response = client()
                .post(format!("http://{}/v1/responses", fixture.handle.addr))
                .bearer_auth("fixture")
                .json(&json!({"model":"fixture-model","stream":stream,"input":"hello"}))
                .send()
                .await
                .unwrap();
            let status = response.status();
            let body = response.json::<Value>().await.unwrap();
            assert_eq!(
                status, 503,
                "official={official}, oauth={oauth}, stream={stream}: {body}"
            );
            assert_eq!(body["error"]["code"], "credits_policy_blocked", "{body}");
            assert!(fixture.host.prepared.load(Ordering::SeqCst) > 0);
            assert!(fixture.host.guard.checks.load(Ordering::SeqCst) > 0);
            assert_eq!(fixture.upstream_calls.load(Ordering::SeqCst), 0);
            assert_eq!(
                fixture.host.quota_checks.load(Ordering::SeqCst),
                0,
                "route guard runs before reserve admission"
            );
            assert!(fixture.host.guard.observations.lock().unwrap().is_empty());
            fixture.finish().await;
        }
    }
}

#[tokio::test]
async fn native_admission_receives_http_headers_and_sse_quota_for_local_and_official_routes() {
    for official in [false, true] {
        let fixture = Fixture::new(official, !official, false).await;
        let response = client()
            .post(format!("http://{}/v1/responses", fixture.handle.addr))
            .bearer_auth("fixture")
            .json(&json!({"model":"fixture-model","stream":false,"input":"hello"}))
            .send()
            .await
            .unwrap();
        let status = response.status();
        let body = response.json::<Value>().await.unwrap();
        assert_eq!(status, 200, "official={official}: {body}");
        assert_eq!(body["status"], "completed");
        assert_eq!(fixture.upstream_calls.load(Ordering::SeqCst), 1);
        assert!(fixture.host.guard.checks.load(Ordering::SeqCst) > 0);
        {
            let observed = fixture.host.guard.observations.lock().unwrap();
            assert!(
                observed.iter().any(|quota| quota
                    .primary
                    .as_ref()
                    .is_some_and(|window| window.remaining_percent == 0.5)),
                "header missing for official={official}: {observed:?}"
            );
            assert!(
                observed.iter().any(|quota| quota
                    .secondary
                    .as_ref()
                    .is_some_and(|window| window.remaining_percent == 12.5)),
                "SSE missing for official={official}: {observed:?}"
            );
        }
        // Official grants bypass OAuth reserve policy but still enforce Credits.
        assert_eq!(
            fixture.host.quota_checks.load(Ordering::SeqCst) == 0,
            official
        );
        fixture.finish().await;
    }
}

#[tokio::test]
async fn native_admission_blocks_direct_image_before_upstream_send() {
    let fixture = Fixture::new(false, false, true).await;
    let response = client()
        .post(format!(
            "http://{}/v1/images/generations",
            fixture.handle.addr
        ))
        .bearer_auth("fixture")
        .json(&json!({"model":"fixture-image","prompt":"synthetic image"}))
        .send()
        .await
        .unwrap();
    let status = response.status();
    let body = response.json::<Value>().await.unwrap();
    assert_eq!(status, 503, "{body}");
    assert_eq!(body["error"]["code"], "credits_policy_blocked", "{body}");
    assert!(fixture.host.guard.checks.load(Ordering::SeqCst) > 0);
    assert_eq!(fixture.upstream_calls.load(Ordering::SeqCst), 0);
    fixture.finish().await;
}

#[tokio::test]
async fn native_admission_blocks_websocket_before_upstream_handshake() {
    let fixture = Fixture::new(false, true, true).await;
    let mut request = format!("ws://{}/v1/responses", fixture.handle.addr)
        .into_client_request()
        .unwrap();
    request
        .headers_mut()
        .insert("authorization", "Bearer fixture".parse().unwrap());
    let (mut socket, _) = tokio_tungstenite::connect_async(request).await.unwrap();
    socket
        .send(Message::Text(
            json!({"type":"response.create","model":"fixture-model","input":[{
                "type":"message","role":"user","content":[{"type":"input_text","text":"hello"}]
            }]})
                .to_string()
                .into(),
        ))
        .await
        .unwrap();
    let event = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match socket.next().await.expect("response frame").unwrap() {
                Message::Text(text) => break serde_json::from_str::<Value>(&text).unwrap(),
                Message::Ping(payload) => socket.send(Message::Pong(payload)).await.unwrap(),
                frame => panic!("unexpected frame: {frame:?}"),
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(event["status"], 503, "{event}");
    assert_eq!(event["error"]["code"], "credits_policy_blocked", "{event}");
    assert!(fixture.host.prepared.load(Ordering::SeqCst) > 0);
    assert!(fixture.host.guard.checks.load(Ordering::SeqCst) > 0);
    assert_eq!(fixture.upstream_calls.load(Ordering::SeqCst), 0);
    let _ = socket.close(None).await;
    fixture.finish().await;
}
