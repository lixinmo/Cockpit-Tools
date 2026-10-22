//! Realtime admission over real client and upstream loopback WebSockets.
use super::*;
use axum::{
    extract::ws::{Message as ServerMessage, WebSocketUpgrade},
    response::IntoResponse,
    routing::get,
};
use tokio::sync::Notify;

type Captures = Arc<Mutex<Vec<Value>>>;
type ClientSocket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn fixture(block_after_selection: bool) -> (Fixture, Captures, Arc<Notify>) {
    let upstream_calls = Arc::new(AtomicUsize::new(0));
    let captures = Arc::new(Mutex::new(Vec::new()));
    let closed = Arc::new(Notify::new());
    let calls = upstream_calls.clone();
    let seen = captures.clone();
    let done = closed.clone();
    let app = Router::new().route(
        "/v1/realtime",
        get(move |upgrade: WebSocketUpgrade| {
            let seen = seen.clone();
            let done = done.clone();
            let calls = calls.clone();
            async move {
                calls.fetch_add(1, Ordering::SeqCst);
                let mut response = upgrade
                    .on_upgrade(move |mut socket| async move {
                        socket.send(ServerMessage::Text(json!({"type":"codex.rate_limits",
                            "rate_limits":{"secondary":{"used_percent":87.5,"window_minutes":10080}}}).to_string().into())).await.unwrap();
                        while let Some(Ok(frame)) = socket.recv().await {
                            let value = match frame {
                                ServerMessage::Text(text) => serde_json::from_str::<Value>(&text).ok(),
                                ServerMessage::Binary(bytes) => serde_json::from_slice::<Value>(&bytes).ok(),
                                ServerMessage::Ping(value) => {
                                    if socket.send(ServerMessage::Pong(value)).await.is_err() { break; }
                                    continue;
                                }
                                ServerMessage::Close(_) => break,
                                ServerMessage::Pong(_) => continue,
                            };
                            if let Some(value) = value {
                                seen.lock().unwrap().push(value.clone());
                                if socket.send(ServerMessage::Text(json!({"type":"fixture.ack",
                                    "request_type":value["type"],"event_id":value["event_id"]}).to_string().into())).await.is_err() { break; }
                            }
                        }
                        done.notify_one();
                    })
                    .into_response();
                response.headers_mut().insert("x-codex-primary-used-percent", "99.5".parse().unwrap());
                response.headers_mut().insert("x-codex-primary-window-minutes", "300".parse().unwrap());
                response
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let host = Arc::new(GuardHost {
        base: format!("http://{}/v1", listener.local_addr().unwrap()),
        official: false,
        oauth: true,
        block_after_selection,
        guard: Arc::new(Guard::default()),
        prepared: AtomicUsize::new(0),
        quota_checks: AtomicUsize::new(0),
        reports: Mutex::new(Vec::new()),
    });
    let upstream = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let root = tempfile::tempdir().unwrap();
    let handle = start(
        "127.0.0.1:0".parse().unwrap(),
        host.clone(),
        root.path().join("identity.sqlite"),
        Options::default(),
    )
    .await
    .unwrap();
    (
        Fixture {
            host,
            upstream_calls,
            upstream,
            handle,
            _root: root,
        },
        captures,
        closed,
    )
}

fn request(fixture: &Fixture) -> tokio_tungstenite::tungstenite::http::Request<()> {
    let mut request = format!(
        "ws://{}/v1/realtime?model=gpt-realtime",
        fixture.handle.addr
    )
    .into_client_request()
    .unwrap();
    request
        .headers_mut()
        .insert("authorization", "Bearer fixture".parse().unwrap());
    request
}

async fn next_json(socket: &mut ClientSocket) -> Value {
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            match socket
                .next()
                .await
                .expect("Realtime stays connected")
                .unwrap()
            {
                Message::Text(text) => return serde_json::from_str(&text).unwrap(),
                Message::Ping(value) => socket.send(Message::Pong(value)).await.unwrap(),
                frame => panic!("unexpected Realtime frame: {frame:?}"),
            }
        }
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn native_admission_realtime_policy_change_after_route_blocks_upstream_handshake() {
    let (fixture, _, _) = fixture(true).await;
    let error = tokio_tungstenite::connect_async(request(&fixture))
        .await
        .err()
        .unwrap();
    let tokio_tungstenite::tungstenite::Error::Http(response) = error else {
        panic!("expected a local admission rejection, got {error:?}");
    };
    assert_eq!(response.status(), 503);
    let body: Value = serde_json::from_slice(response.body().as_ref().unwrap()).unwrap();
    assert_eq!(body["error"]["code"], "credits_policy_blocked");
    assert_eq!(fixture.upstream_calls.load(Ordering::SeqCst), 0);
    assert!(fixture.host.guard.checks.load(Ordering::SeqCst) > 0);
    assert!(fixture.host.guard.observations.lock().unwrap().is_empty());
    fixture.finish().await;
}

#[tokio::test]
async fn native_admission_realtime_next_explicit_response_is_blocked_but_audio_ping_and_close_work()
{
    for binary in [false, true] {
        let (fixture, captures, closed) = fixture(false).await;
        let (mut socket, _) = tokio_tungstenite::connect_async(request(&fixture))
            .await
            .unwrap();
        assert_eq!(next_json(&mut socket).await["type"], "codex.rate_limits");
        {
            let observed = fixture.host.guard.observations.lock().unwrap();
            assert!(observed.iter().any(|s| s
                .primary
                .as_ref()
                .is_some_and(|w| w.remaining_percent == 0.5)));
            assert!(observed.iter().any(|s| s
                .secondary
                .as_ref()
                .is_some_and(|w| w.remaining_percent == 12.5)));
        }
        socket
            .send(Message::Text(
                json!({"type":"response.create","event_id":"first"})
                    .to_string()
                    .into(),
            ))
            .await
            .unwrap();
        assert_eq!(next_json(&mut socket).await["event_id"], "first");
        fixture.host.guard.denied.store(true, Ordering::SeqCst);
        let body = json!({"type":"response.create","event_id":"blocked"}).to_string();
        socket
            .send(if binary {
                Message::Binary(body.into_bytes().into())
            } else {
                Message::Text(body.into())
            })
            .await
            .unwrap();
        let rejected = next_json(&mut socket).await;
        assert_eq!(rejected["status"], 503);
        assert_eq!(rejected["error"]["code"], "credits_policy_blocked");
        assert_eq!(
            captures.lock().unwrap().len(),
            1,
            "denied response.create must never reach upstream"
        );
        let checks_after_rejection = fixture.host.guard.checks.load(Ordering::SeqCst);
        socket
            .send(Message::Text(
                json!({"type":"input_audio_buffer.append","audio":"AA==","event_id":"audio"})
                    .to_string()
                    .into(),
            ))
            .await
            .unwrap();
        assert_eq!(next_json(&mut socket).await["event_id"], "audio");
        socket
            .send(Message::Ping(b"still-open".to_vec().into()))
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                match socket.next().await.expect("ping response").unwrap() {
                    Message::Pong(value) => {
                        assert_eq!(&value[..], b"still-open");
                        break;
                    }
                    Message::Ping(value) => socket.send(Message::Pong(value)).await.unwrap(),
                    frame => panic!("unexpected frame after ping: {frame:?}"),
                }
            }
        })
        .await
        .unwrap();
        assert_eq!(
            fixture.host.guard.checks.load(Ordering::SeqCst),
            checks_after_rejection
        );
        assert_eq!(fixture.upstream_calls.load(Ordering::SeqCst), 1);
        assert_eq!(captures.lock().unwrap().len(), 2);
        socket.close(None).await.unwrap();
        tokio::time::timeout(Duration::from_secs(3), closed.notified())
            .await
            .unwrap();
        fixture.finish().await;
    }
}
