//! Exact production routing, metadata discovery, manual policy and wire effort.
//! Synthetic loopback providers only; no user account or external API is read.
use super::*;
use crate::models::codex::CodexExperimentalModelDefinition;
use axum::routing::get;

#[derive(Clone, Copy, Debug)]
enum Delivery {
    Json,
    Sse,
    WebSocket,
}

fn source(model: &str, effort: Option<&str>, delivery: Delivery, turn: usize) -> Value {
    let mut body = json!({"model":model,"store":false,"stream":!matches!(delivery,Delivery::Json),
    "client_metadata":{"session_id":format!("reasoning-fixture-{turn}"),"thread_id":format!("reasoning-fixture-{turn}")},
    "tools":[
        {"type":"function","name":"read_fixture","parameters":{"type":"object","properties":{"path":{"type":"string"}},"required":["path"]}},
        {"type":"custom","name":"apply_patch","format":{"type":"text"}}
    ],
    "input":[
        {"type":"message","role":"user","content":"Read and update the fixture."},
        {"type":"reasoning","encrypted_content":"","summary":[{"type":"summary_text","text":"Saved Go reasoning remains intact."}]},
        {"type":"function_call","call_id":"old_read","name":"read_fixture","arguments":"{\"path\":\"fixture.txt\"}"},
        {"type":"function_call_output","call_id":"old_read","output":"read sentinel"},
        {"type":"custom_tool_call","call_id":"old_patch","name":"apply_patch","input":"*** Begin Patch\n*** Add File: old.txt\n+sentinel\n*** End Patch"},
        {"type":"custom_tool_call_output","call_id":"old_patch","output":"patch sentinel"},
        {"type":"message","role":"assistant","content":"The saved fixture was updated."},
        {"type":"message","role":"user","content":"Continue with the complete saved history."}
    ]});
    if let Some(effort) = effort {
        body["reasoning"] = json!({"effort":effort,"summary":"auto"});
    }
    body
}

fn chat_reply(body: &Value, number: usize) -> Response {
    if body["stream"] == true {
        let chunk = json!({"id":format!("chat_effort_{number}"),"object":"chat.completion.chunk","model":body["model"],
            "choices":[{"index":0,"delta":{"role":"assistant","content":"Fixture checked."},"finish_reason":"stop"}]});
        Response::builder()
            .header("content-type", "text/event-stream")
            .body(Body::from(format!("data: {chunk}\n\ndata: [DONE]\n\n")))
            .unwrap()
    } else {
        Json(json!({"id":format!("chat_effort_{number}"),"object":"chat.completion","model":body["model"],
            "choices":[{"index":0,"message":{"role":"assistant","content":"Fixture checked."},"finish_reason":"stop"}]})).into_response()
    }
}

async fn exchange(
    gateway: &native_api::Handle,
    mut body: Value,
    delivery: Delivery,
) -> (bool, Value) {
    if matches!(delivery, Delivery::WebSocket) {
        let mut request = format!("ws://{}/v1/responses", gateway.addr)
            .into_client_request()
            .unwrap();
        request
            .headers_mut()
            .insert("authorization", "Bearer fixture-client".parse().unwrap());
        let (mut ws, _) = tokio_tungstenite::connect_async(request).await.unwrap();
        body["type"] = json!("response.create");
        ws.send(Message::Text(body.to_string().into()))
            .await
            .unwrap();
        let terminal = timeout(Duration::from_secs(10), async {
            loop {
                match ws.next().await.unwrap().unwrap() {
                    Message::Text(text) => {
                        let event: Value = serde_json::from_str(&text).unwrap();
                        if matches!(
                            event["type"].as_str(),
                            Some(
                                "response.completed"
                                    | "response.failed"
                                    | "response.incomplete"
                                    | "error"
                            )
                        ) {
                            break event;
                        }
                    }
                    Message::Ping(bytes) => ws.send(Message::Pong(bytes)).await.unwrap(),
                    _ => {}
                }
            }
        })
        .await
        .unwrap();
        let _ = ws.close(None).await;
        return (terminal["type"] == "response.completed", terminal);
    }
    let response = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap()
        .post(format!("http://{}/v1/responses", gateway.addr))
        .bearer_auth("fixture-client")
        .json(&body)
        .send()
        .await
        .unwrap();
    let status = response.status();
    let raw = response.text().await.unwrap();
    if !status.is_success() || matches!(delivery, Delivery::Json) {
        return (status.is_success(), serde_json::from_str(&raw).unwrap());
    }
    let terminal = raw
        .lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .last()
        .unwrap();
    (terminal["type"] == "response.completed", terminal)
}

async fn catalog(gateway: &native_api::Handle) -> Value {
    let response = reqwest::Client::builder()
        .no_proxy()
        .build()
        .unwrap()
        .get(format!("http://{}/v1/models", gateway.addr))
        .bearer_auth("fixture-client")
        .send()
        .await
        .unwrap();
    let status = response.status();
    let value = response.json::<Value>().await.unwrap();
    assert!(status.is_success(), "{value}");
    value
}

fn assert_levels(catalog: &Value, id: &str, expected: &[&str]) {
    let model = catalog["models"]
        .as_array()
        .unwrap()
        .iter()
        .find(|model| model["slug"] == id)
        .unwrap();
    let actual = model["supported_reasoning_levels"]
        .as_array()
        .unwrap()
        .iter()
        .map(|level| level["effort"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(actual, expected, "{id}");
}

fn manual_definition(
    model: &str,
    efforts: &[&str],
    default: Option<&str>,
) -> CodexExperimentalModelDefinition {
    CodexExperimentalModelDefinition {
        model_id: model.into(),
        display_name: model.into(),
        reasoning_efforts: Some(efforts.iter().map(|value| (*value).into()).collect()),
        default_reasoning_effort: default.map(str::to_owned),
        context_window: None,
        auto_compact_token_limit: None,
    }
}

async fn assert_wire(
    gateway: &native_api::Handle,
    seen: &Arc<Mutex<Vec<Value>>>,
    model: &str,
    requested: Option<&str>,
    expected: Option<&str>,
    delivery: Delivery,
) {
    let before = seen.lock().unwrap().len();
    let body = source(model, requested, delivery, before);
    let (success, result) = exchange(gateway, body.clone(), delivery).await;
    assert!(success, "{delivery:?}: {result}");
    let captures = seen.lock().unwrap();
    assert_eq!(captures.len(), before + 1);
    let actual = captures.last().unwrap();
    let mut canonical = body.clone();
    canonical["model"] = json!(if model == "team/friendly" {
        "fixture-model"
    } else {
        model.strip_prefix("team/").unwrap()
    });
    let expected_wire = native_api::responses_to_chat_for_test(&canonical).unwrap();
    assert_eq!(actual["model"], canonical["model"]);
    assert_eq!(actual["reasoning_effort"].as_str(), expected);
    assert_eq!(
        actual["tools"], expected_wire["tools"],
        "effort selection cannot alter tools"
    );
    assert_eq!(
        actual["messages"], expected_wire["messages"],
        "effort selection cannot alter history, calls, arguments or results"
    );
    assert_eq!(
        body["reasoning"]["effort"].as_str(),
        requested,
        "source request remains intact"
    );
}

async fn run_reasoning(delivery: Delivery) {
    let _env = crate::modules::test_support::env_lock()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let _data = NativeInstallationTestData::new();
    let seen = Arc::new(Mutex::new(Vec::<Value>::new()));
    let capture = seen.clone();
    let discovered = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let discoveries = discovered.clone();
    let router = Router::new()
        .route("/primary/v1/models", get(move || {
            let discoveries = discoveries.clone();
            async move {
                discoveries.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Json(json!({"data":[
                    {"id":"fixture-model","supported_reasoning_efforts":["low","medium","high"],"default_reasoning_effort":"medium"},
                    {"id":"unknown-model","supported_parameters":["reasoning_effort"]},
                    {"id":"no-reasoning","supported_reasoning_efforts":[]},
                    {"id":"auto-model","supported_reasoning_efforts":["auto"],"default_reasoning_effort":"auto"}
                ]}))
            }
        }))
        .route("/primary/v1/chat/completions", post(move |Json(body): Json<Value>| {
            let capture = capture.clone();
            async move {
                let number = { let mut calls = capture.lock().unwrap(); calls.push(body.clone()); calls.len() };
                chat_reply(&body, number)
            }
        }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let upstream = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    let profile = tempfile::tempdir().unwrap();
    fs::write(
        profile.path().join("config.toml"),
        "model = \"fixture-model\"\n",
    )
    .unwrap();
    let mut fixture = host(&base, false, false, true);
    fixture.inner.profile_dir = Some(profile.path().to_path_buf());
    let account = fixture.inner.account_overrides.get_mut("primary").unwrap();
    account.api_wire_api = Some("chat_completions".into());
    account.api_model_catalog = [
        "fixture-model",
        "unknown-model",
        "no-reasoning",
        "auto-model",
    ]
    .iter()
    .map(|model| (*model).into())
    .collect();
    let account = account.clone();
    let collection = fixture.inner.collection_override.as_mut().unwrap();
    collection.capability_fallback = Default::default();
    collection.model_aliases =
        vec![
            serde_json::from_value(json!({"sourceModel":"fixture-model","alias":"friendly"}))
                .unwrap(),
        ];
    let key = collection.api_keys.first_mut().unwrap();
    key.model_prefix = Some("team".into());
    key.allowed_models = [
        "fixture-model",
        "friendly",
        "unknown-model",
        "no-reasoning",
        "auto-model",
    ]
    .iter()
    .map(|model| (*model).into())
    .collect();
    crate::modules::codex_provider_metadata::refresh(&account)
        .await
        .unwrap();
    assert_eq!(discovered.load(std::sync::atomic::Ordering::SeqCst), 1);
    let host = Arc::new(fixture);
    let root = tempfile::tempdir().unwrap();
    let mut gateway = native_api::start(
        "127.0.0.1:0".parse().unwrap(),
        host.clone(),
        root.path().join("identity.sqlite"),
        native_api::Options::default(),
    )
    .await
    .unwrap();

    let first_catalog = catalog(&gateway).await;
    assert_levels(
        &first_catalog,
        "team/fixture-model",
        &["low", "medium", "high"],
    );
    assert_levels(&first_catalog, "team/friendly", &["low", "medium", "high"]);
    assert_levels(
        &first_catalog,
        "team/unknown-model",
        &["low", "medium", "high", "xhigh", "max", "ultra"],
    );
    assert_levels(&first_catalog, "team/no-reasoning", &[]);
    assert_levels(&first_catalog, "team/auto-model", &[]);
    for requested in ["high", "ultra"] {
        assert_wire(
            &gateway,
            &seen,
            "team/fixture-model",
            Some(requested),
            Some("high"),
            delivery,
        )
        .await;
    }
    assert_wire(
        &gateway,
        &seen,
        "team/unknown-model",
        Some("ultra"),
        Some("ultra"),
        delivery,
    )
    .await;
    assert_wire(
        &gateway,
        &seen,
        "team/auto-model",
        Some("auto"),
        Some("auto"),
        delivery,
    )
    .await;

    let before = seen.lock().unwrap().len();
    let (success, failure) = exchange(
        &gateway,
        source("team/no-reasoning", Some("high"), delivery, before),
        delivery,
    )
    .await;
    assert!(!success, "{failure}");
    assert_eq!(
        seen.lock().unwrap().len(),
        before,
        "explicit unsupported effort never reaches upstream"
    );
    assert_wire(
        &gateway,
        &seen,
        "team/no-reasoning",
        Some("none"),
        None,
        delivery,
    )
    .await;
    let before = seen.lock().unwrap().len();
    assert!(
        !exchange(
            &gateway,
            source("other/fixture-model", Some("ultra"), delivery, before),
            delivery
        )
        .await
        .0
    );
    assert_eq!(
        seen.lock().unwrap().len(),
        before,
        "metadata cannot expand API-key namespace"
    );

    // Profile policies can be keyed by the authorized exposed model name. The
    // routing rewrite must not lose this identity before prepare_body_for_model.
    fs::write(
        profile
            .path()
            .join(".cockpit-experimental-model-catalog-enabled"),
        "enabled\n",
    )
    .unwrap();
    fs::write(profile.path().join(".cockpit-experimental-model-catalog-config.json"), json!({"version":4,"models":[manual_definition("team/friendly", &["low","high"], Some("high"))]}).to_string()).unwrap();
    let alias_catalog = catalog(&gateway).await;
    assert_levels(&alias_catalog, "team/friendly", &["low", "high"]);
    assert_levels(
        &alias_catalog,
        "team/fixture-model",
        &["low", "medium", "high"],
    );
    assert_wire(
        &gateway,
        &seen,
        "team/friendly",
        Some("medium"),
        Some("low"),
        delivery,
    )
    .await;
    fs::remove_file(
        profile
            .path()
            .join(".cockpit-experimental-model-catalog-enabled"),
    )
    .unwrap();

    // Real provider-specific editor persistence overrides discovered facts and
    // can fill a previously unknown model, without editing the subscription list.
    let definitions = vec![
        manual_definition("fixture-model", &["low", "high"], Some("high")),
        manual_definition("unknown-model", &["low", "high"], Some("high")),
    ];
    codex_account::save_provider_model_quick_config(
        profile.path(),
        &account,
        true,
        definitions.clone(),
        None,
    )
    .unwrap();
    let manual_catalog = catalog(&gateway).await;
    assert_levels(&manual_catalog, "team/fixture-model", &["low", "high"]);
    assert_levels(&manual_catalog, "team/unknown-model", &["low", "high"]);
    assert_wire(
        &gateway,
        &seen,
        "team/fixture-model",
        Some("medium"),
        Some("low"),
        delivery,
    )
    .await;
    assert_wire(
        &gateway,
        &seen,
        "team/fixture-model",
        None,
        Some("high"),
        delivery,
    )
    .await;
    assert_wire(
        &gateway,
        &seen,
        "team/unknown-model",
        Some("ultra"),
        Some("high"),
        delivery,
    )
    .await;
    codex_account::save_provider_model_quick_config(
        profile.path(),
        &account,
        false,
        definitions,
        None,
    )
    .unwrap();
    assert_levels(
        &catalog(&gateway).await,
        "team/unknown-model",
        &["low", "medium", "high", "xhigh", "max", "ultra"],
    );
    assert_wire(
        &gateway,
        &seen,
        "team/unknown-model",
        Some("ultra"),
        Some("ultra"),
        delivery,
    )
    .await;
    assert_eq!(
        discovered.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "ordinary requests use cached facts"
    );

    gateway.shutdown(Duration::from_secs(2)).await.unwrap();
    upstream.abort();
    assert!(host
        .reports
        .lock()
        .unwrap()
        .iter()
        .all(|report| !report.success || report.account_id == "primary"));
}

#[tokio::test]
async fn native_provider_reasoning_http_sse_websocket_use_discovered_manual_and_unknown_capabilities(
) {
    for delivery in [Delivery::Json, Delivery::Sse, Delivery::WebSocket] {
        run_reasoning(delivery).await;
    }
}
