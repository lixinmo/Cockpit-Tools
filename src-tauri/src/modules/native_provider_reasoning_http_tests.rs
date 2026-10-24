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

// The fixture intentionally advertises only native levels, while accepting
// additional client aliases. This is the documented DeepSeek-style server map,
// not a transformation the gateway is allowed to derive from the level list.
fn supplier_effort(body: &Value) -> &str {
    if body["model"] == "no-reasoning" { return "ignored"; }
    let effort = body["reasoning_effort"].as_str().unwrap_or("high");
    if body["model"] == "fixture-model" {
        match effort {
            "minimal" | "low" => "low",
            "medium" | "high" | "xhigh" => "high",
            "max" | "ultra" => "max",
            other => other,
        }
    } else { effort }
}

fn chat_reply(body: &Value, number: usize) -> Response {
    if body["model"] == "strict-model" && body["reasoning_effort"] == "medium" {
        return (axum::http::StatusCode::BAD_REQUEST,
            Json(json!({"error":{"type":"invalid_request_error","code":"request_params_invalid",
                "message":"This synthetic endpoint rejects medium"}}))).into_response();
    }
    let text = format!("effective:{}", supplier_effort(body));
    if body["stream"] == true {
        let chunk = json!({"id":format!("chat_effort_{number}"),"object":"chat.completion.chunk","model":body["model"],
            "choices":[{"index":0,"delta":{"role":"assistant","content":text},"finish_reason":"stop"}]});
        Response::builder()
            .header("content-type", "text/event-stream")
            .body(Body::from(format!("data: {chunk}\n\ndata: [DONE]\n\n")))
            .unwrap()
    } else {
        Json(json!({"id":format!("chat_effort_{number}"),"object":"chat.completion","model":body["model"],
            "choices":[{"index":0,"message":{"role":"assistant","content":text},"finish_reason":"stop"}]})).into_response()
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
    if requested.is_none() {
        assert!(actual.get("reasoning_effort").is_none(), "an absent effort must remain absent even with a configured client default");
    }
    let response = if matches!(delivery, Delivery::Json) { &result } else { &result["response"] };
    assert_eq!(response["output"][0]["content"][0]["text"], format!("effective:{}", supplier_effort(actual)),
        "The supplier must receive the original alias and perform its own documented interpretation");
    if matches!(model, "team/fixture-model" | "team/friendly") {
        match requested {
            Some("medium" | "xhigh") => assert_eq!(response["output"][0]["content"][0]["text"], "effective:high"),
            Some("ultra") => assert_eq!(response["output"][0]["content"][0]["text"], "effective:max"),
            None => assert_eq!(response["output"][0]["content"][0]["text"], "effective:high", "an absent request uses the supplier default, not the client menu default"),
            _ => {},
        }
    }
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
                    {"id":"fixture-model","effort":{"supported_levels":["low","high","max"],"default_level":"high"}},
                    {"id":"strict-model","supported_reasoning_efforts":["low","high","max"],"default_reasoning_effort":"high"},
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
        "strict-model",
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
        "strict-model",
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
        &["low", "high", "max"],
    );
    assert_levels(&first_catalog, "team/friendly", &["low", "high", "max"]);
    assert_levels(
        &first_catalog,
        "team/unknown-model",
        &["low", "medium", "high", "xhigh", "max", "ultra"],
    );
    assert_levels(&first_catalog, "team/no-reasoning", &[]);
    assert_levels(&first_catalog, "team/auto-model", &[]);
    for requested in ["minimal", "low", "medium", "high", "xhigh", "max", "ultra", "none", "auto"] {
        assert_wire(
            &gateway,
            &seen,
            "team/fixture-model",
            Some(requested),
            Some(requested),
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

    for requested in ["high", "none", "auto"] {
        assert_wire(&gateway, &seen, "team/no-reasoning", Some(requested), Some(requested), delivery).await;
    }
    // A different endpoint can publish the same native range but reject medium.
    // Forward its error rather than converting medium to an apparently valid low.
    let before = seen.lock().unwrap().len();
    let (success, failure) = exchange(&gateway,
        source("team/strict-model", Some("medium"), delivery, before), delivery).await;
    assert!(!success, "{failure}");
    {
        let calls = seen.lock().unwrap();
        assert_eq!(calls.len(), before + 1, "one unmodified request, no guessed retry");
        assert_eq!(calls.last().unwrap()["reasoning_effort"], "medium");
    }
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

    // Profile menu/default policy can use the authorized exposed model name.
    // It must not rewrite an explicitly supplied alias such as medium.
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
        &["low", "high", "max"],
    );
    assert_wire(
        &gateway,
        &seen,
        "team/friendly",
        Some("medium"),
        Some("medium"),
        delivery,
    )
    .await;
    fs::remove_file(
        profile
            .path()
            .join(".cockpit-experimental-model-catalog-enabled"),
    )
    .unwrap();

    // Provider-specific editor persistence configures client menus/defaults.
    // Explicit request values still belong to the caller and supplier.
    let definitions = vec![
        manual_definition("fixture-model", &["low"], Some("low")),
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
    assert_levels(&manual_catalog, "team/fixture-model", &["low"]);
    assert_levels(&manual_catalog, "team/unknown-model", &["low", "high"]);
    assert_wire(
        &gateway,
        &seen,
        "team/fixture-model",
        Some("medium"),
        Some("medium"),
        delivery,
    )
    .await;
    assert_wire(
        &gateway,
        &seen,
        "team/fixture-model",
        None,
        None,
        delivery,
    )
    .await;
    assert_wire(
        &gateway,
        &seen,
        "team/fixture-model",
        Some("ultra"),
        Some("ultra"),
        delivery,
    )
    .await;
    // Explicit null, provider-specific strings and the legacy top-level
    // spelling survive unchanged; client defaults never rewrite these values.
    for value in [Value::Null, json!(" HIGH "), json!("provider-specific"), json!(""), json!("xhigh")] {
        for nested in [true, false] {
            let before = seen.lock().unwrap().len();
            let mut body = source("team/fixture-model", None, delivery, before);
            if nested { body["reasoning"] = json!({"effort":value}); }
            else { body["reasoning_effort"] = value.clone(); }
            let original = body.clone();
            let (success, response) = exchange(&gateway, body.clone(), delivery).await;
            assert!(success, "{response}");
            let captures = seen.lock().unwrap();
            assert_eq!(captures.len(), before + 1);
            assert_eq!(captures.last().unwrap().get("reasoning_effort"), Some(&value));
            assert_eq!(body, original);
        }
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
async fn native_provider_reasoning_http_sse_websocket_preserve_supplier_effort_semantics(
) {
    for delivery in [Delivery::Json, Delivery::Sse, Delivery::WebSocket] {
        run_reasoning(delivery).await;
    }
}
