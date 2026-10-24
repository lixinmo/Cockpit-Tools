use super::*;
use serde_json::json;

#[test]
fn provider_metadata_parses_explicit_model_facts_without_inventing_effort_levels() {
    let models = parse_models(&json!({"data":[
        {"id":"model-a","supported_reasoning_levels":[{"effort":"high","description":"not cached"},{"effort":"low"}],"default_reasoning_level":"high","context_window":128000,"input_modalities":["text","image"],"secret":"never cached"},
        {"id":"model-b","model_info":{"supported_reasoning_efforts":["low","medium","high"],"default_reasoning_effort":"medium","context_length":"256000","input_modalities":["text"]}},
        {"id":"model-c","supported_parameters":["reasoning_effort"],"capabilities":["thinking"]},
        {"id":"model-d","supports_reasoning":false}
    ]})).unwrap();
    assert_eq!(
        models[0].metadata,
        Some(ProviderModelMetadata {
            reasoning_efforts: Some(vec!["low".into(), "high".into()]),
            default_reasoning_effort: Some("high".into()),
            context_window: Some(128000),
            supports_vision: Some(true),
        })
    );
    assert_eq!(
        models[1].metadata.as_ref().unwrap().reasoning_efforts,
        Some(vec!["low".into(), "medium".into(), "high".into()])
    );
    assert_eq!(
        models[1].metadata.as_ref().unwrap().context_window,
        Some(256000)
    );
    assert_eq!(
        models[1].metadata.as_ref().unwrap().supports_vision,
        Some(false)
    );
    assert_eq!(models[2].metadata, None);
    assert_eq!(
        models[3].metadata.as_ref().unwrap().reasoning_efforts,
        Some(vec![])
    );
}

#[test]
fn provider_metadata_distinguishes_missing_unsupported_and_malformed_fields() {
    assert!(parse_metadata(&json!({"supported_reasoning_levels":null})).is_none());
    assert_eq!(
        parse_metadata(&json!({"supported_reasoning_levels":[]}))
            .unwrap()
            .reasoning_efforts,
        Some(vec![])
    );
    for value in [
        json!(false),
        json!(["unknown", "low"]),
        json!([{"name":"high"}]),
        json!("high"),
    ] {
        assert!(parse_metadata(&json!({"supported_reasoning_levels":value})).is_none());
    }
    assert!(parse_metadata(&json!({"supported_reasoning_levels":["low"],"model_info":{"supported_reasoning_efforts":["high"]}})).is_none());
    assert!(parse_metadata(
        &json!({"supported_reasoning_levels":["low"],"supports_reasoning":false})
    )
    .is_none());
    let known = parse_metadata(&json!({"supported_reasoning_efforts":["ULTRA", "low", "low"],"default_reasoning_effort":"high"})).unwrap();
    assert_eq!(
        known.reasoning_efforts,
        Some(vec!["low".into(), "ultra".into()])
    );
    assert_eq!(known.default_reasoning_effort, None);
    assert!(parse_metadata(&json!({"context_window":-1,"supports_vision":"true"})).is_none());
    assert!(parse_metadata(&json!({"input_modalities":["unknown"]})).is_none());
    assert!(
        parse_metadata(&json!({"supports_vision":false,"input_modalities":["image"]})).is_none()
    );
}

#[test]
fn provider_metadata_accepts_explicit_catalog_and_camel_case_variants() {
    let models = parse_models(&json!({"models":[
        {"slug":"camel","supportedReasoningEfforts":[{"reasoningEffort":"high"},{"reasoningEffort":"low"}],"defaultReasoningEffort":"high","contextWindow":32000},
        {"id":"registry","thinking":{"levels":["none","minimal","high","auto"]}},
        {"id":"disabled","reasoningDisabled":true},
        {"id":"unknown","reasoningDisabled":false,"supportsReasoning":true}
    ]})).unwrap();
    assert_eq!(
        models[0].metadata.as_ref().unwrap().reasoning_efforts,
        Some(vec!["low".into(), "high".into()])
    );
    assert_eq!(
        models[0]
            .metadata
            .as_ref()
            .unwrap()
            .default_reasoning_effort
            .as_deref(),
        Some("high")
    );
    assert_eq!(
        models[1].metadata.as_ref().unwrap().reasoning_efforts,
        Some(vec![
            "none".into(),
            "minimal".into(),
            "high".into(),
            "auto".into()
        ])
    );
    assert_eq!(
        models[2].metadata.as_ref().unwrap().reasoning_efforts,
        Some(vec![])
    );
    assert!(models[3].metadata.is_none());
    assert!(parse_metadata(
        &json!({"supportedReasoningEfforts":[{"effort":"low","reasoningEffort":"high"}]})
    )
    .is_none());
}

#[test]
fn provider_metadata_duplicate_and_size_checks_do_not_silently_choose_conflicting_facts() {
    let same = json!({"data":[{"id":"same","supported_reasoning_efforts":["low"]},{"id":"SAME","supported_reasoning_efforts":["low"]}]});
    assert_eq!(parse_models(&same).unwrap().len(), 1);
    let mut conflict = same;
    conflict["data"][1]["supported_reasoning_efforts"] = json!(["high"]);
    assert_eq!(
        parse_models(&conflict).unwrap_err(),
        "PROVIDER_MODELS_CONFLICTING_METADATA"
    );
    assert!(parse_models(&json!({"data":{}})).is_err());
    assert_eq!(
        parse_models(&json!({"data":vec![json!({"id":"m"});MAX_MODELS+1]})).unwrap_err(),
        "PROVIDER_MODELS_TOO_MANY"
    );
    assert!(
        parse_models(&json!({"data":[{"id":"bad\nname"},{"id":" "}]}))
            .unwrap()
            .is_empty()
    );
}

#[test]
fn provider_metadata_cache_scope_is_endpoint_and_credential_specific() {
    let first = scope("https://relay.example/v1", "synthetic-key").unwrap();
    assert_eq!(
        first,
        scope("https://relay.example:443/v1/", "synthetic-key").unwrap()
    );
    for (base, key) in [
        ("https://relay.example/v2", "synthetic-key"),
        ("https://other.example/v1", "synthetic-key"),
        ("https://relay.example/v1", "another-key"),
        ("http://relay.example/v1", "synthetic-key"),
        ("https://relay.example/v1?scope=other", "synthetic-key"),
    ] {
        assert_ne!(first, scope(base, key).unwrap());
    }
    assert_eq!(
        models_url("https://relay.example/step_plan/v1/").unwrap(),
        "https://relay.example/step_plan/v1/models"
    );
    assert!(scope("file:///tmp", "key").is_err());
    assert_ne!(
        scope("https://user:secret@relay.example/v1", "key").unwrap(),
        scope("https://relay.example/v1", "key").unwrap()
    );
    assert!(scope("https://relay.example/v1", " ").is_err());
}

#[test]
fn provider_metadata_cache_contains_only_sanitized_facts_and_is_private() {
    let root = tempfile::tempdir().unwrap();
    let scope = scope("https://relay.example/v1", "synthetic-secret-key").unwrap();
    let metadata = parse_metadata(&json!({"id":"test","supported_reasoning_efforts":["low","high"],"raw_response":"private message","api_key":"synthetic-secret-key"})).unwrap();
    let entry = CacheEntry {
        version: 1,
        scope: scope.clone(),
        fetched_at_ms: now_ms(),
        models: BTreeMap::from([("test".into(), metadata.clone())]),
    };
    save_cache(root.path(), &entry).unwrap();
    let file = cache_path(root.path(), &scope);
    let saved = std::fs::read_to_string(&file).unwrap();
    for forbidden in [
        "synthetic-secret-key",
        "relay.example",
        "private message",
        "raw_response",
        "api_key",
    ] {
        assert!(!saved.contains(forbidden));
    }
    assert_eq!(
        read_cache(root.path(), &scope).unwrap().models["test"],
        metadata
    );
    assert!(read_cache(root.path(), "different-scope").is_none());
    let other = cache_path(root.path(), "different-scope");
    std::fs::copy(&file, other).unwrap();
    assert!(read_cache(root.path(), "different-scope").is_none());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&file).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            std::fs::metadata(file.parent().unwrap())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
    }
    assert!(!file.with_extension("json.bak").exists());
}

#[tokio::test]
async fn provider_metadata_fetch_does_not_erase_last_good_facts_after_upstream_failure() {
    use axum::{routing::get, Router};
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    };
    let fail = Arc::new(AtomicBool::new(false));
    let flag = fail.clone();
    let app = Router::new().route("/v1/models", get(move || {
        let flag = flag.clone();
        async move {
            if flag.load(Ordering::SeqCst) {
                (axum::http::StatusCode::BAD_GATEWAY, "private upstream text".to_owned())
            } else {
                (axum::http::StatusCode::OK, json!({"data":[{"id":"test","model_info":{"supported_reasoning_efforts":["low","high"]}}]}).to_string())
            }
        }
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}/v1", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let root = tempfile::tempdir().unwrap();
    let fetched = fetch_at(&base, "test-key", root.path(), Duration::from_secs(2))
        .await
        .unwrap();
    assert_eq!(
        fetched.models[0]
            .metadata
            .as_ref()
            .unwrap()
            .reasoning_efforts,
        Some(vec!["low".into(), "high".into()])
    );
    let scope = scope(&base, "test-key").unwrap();
    let before = std::fs::read(cache_path(root.path(), &scope)).unwrap();
    fail.store(true, Ordering::SeqCst);
    let error = fetch_at(&base, "test-key", root.path(), Duration::from_secs(2))
        .await
        .err()
        .unwrap();
    assert_eq!(error, "PROVIDER_MODELS_HTTP_502");
    assert_eq!(
        std::fs::read(cache_path(root.path(), &scope)).unwrap(),
        before
    );
    task.abort();
}

#[tokio::test]
async fn provider_metadata_discovery_follows_only_same_origin_redirects() {
    use axum::{response::Redirect, routing::get, Json, Router};
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };
    let received = Arc::new(AtomicUsize::new(0));
    let observed = received.clone();
    let app = Router::new()
        .route(
            "/v1/models",
            get(|| async { Redirect::temporary("/v1/catalog") }),
        )
        .route(
            "/v1/catalog",
            get(move |headers: axum::http::HeaderMap| {
                let observed = observed.clone();
                async move {
                    assert_eq!(headers["authorization"], "Bearer test-key");
                    observed.fetch_add(1, Ordering::SeqCst);
                    Json(json!({"data":[{"id":"test","supported_reasoning_efforts":["high"]}]}))
                }
            }),
        )
        .route(
            "/cross/models",
            get(|| async { Redirect::temporary("https://unconfigured.invalid/v1/models") }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let root = tempfile::tempdir().unwrap();
    let result = fetch_at(
        &format!("http://{addr}/v1"),
        "test-key",
        root.path(),
        Duration::from_secs(2),
    )
    .await
    .unwrap();
    assert_eq!(result.models.len(), 1);
    assert_eq!(received.load(Ordering::SeqCst), 1);
    let error = fetch_at(
        &format!("http://{addr}/cross"),
        "test-key",
        root.path(),
        Duration::from_secs(2),
    )
    .await
    .err()
    .unwrap();
    assert_eq!(error, "PROVIDER_MODELS_HTTP_307");
    task.abort();
}
