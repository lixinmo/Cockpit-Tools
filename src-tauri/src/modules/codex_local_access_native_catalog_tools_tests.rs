use super::*;

fn collection() -> CodexLocalAccessCollection {
    serde_json::from_value(json!({"enabled":true,"gatewayMode":"native","port":19001,"apiKey":"synthetic-key","accountIds":["responses","chat"],"createdAt":0,"updatedAt":0})).unwrap()
}
fn account(id: &str, wire: &str) -> CodexAccount {
    let mut account = CodexAccount::new_api_key(
        id.into(),
        "synthetic@example.invalid".into(),
        "synthetic-upstream".into(),
        CodexApiProviderMode::Custom,
        Some("https://example.invalid/v1".into()),
        None,
        None,
        vec!["custom-agent".into(), "gpt-6.1-sol".into()],
    );
    account.api_wire_api = Some(wire.into());
    account
}
fn entry(catalog: &Value, id: &str) -> Value {
    catalog["models"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["slug"] == id)
        .unwrap()
        .clone()
}

#[test]
fn native_catalog_registers_visible_model_agents_despite_legacy_disabled_setting() {
    let mut collection = collection();
    collection.multi_agent.optimize = false;
    let account = account("responses", "responses");
    let catalog = native_catalog(&collection, None, &[account]).unwrap();
    for id in ["custom-agent", "gpt-6.1-sol"] {
        assert_eq!(entry(&catalog, id)["multi_agent_version"], "v2");
    }
    let plan = native_model_route_plan(&collection, None, "custom-agent").unwrap();
    let mut hidden = json!({"slug":"custom-agent","visibility":"hide","multi_agent_version":null});
    native_apply_catalog_tool_capabilities(&mut hidden, &collection, None, &plan, &[]);
    assert!(hidden["multi_agent_version"].is_null());
}

#[test]
fn native_catalog_search_considers_every_eligible_pool_member_not_first_metadata_source() {
    let collection = collection();
    for accounts in [
        vec![
            account("responses", "responses"),
            account("chat", "chat_completions"),
        ],
        vec![
            account("chat", "chat_completions"),
            account("responses", "responses"),
        ],
    ] {
        let catalog = native_catalog(&collection, None, &accounts).unwrap();
        assert_eq!(
            entry(&catalog, "custom-agent")["supports_search_tool"],
            false
        );
        assert_eq!(
            entry(&catalog, "gpt-6.1-sol")["supports_search_tool"],
            false
        );
        assert_eq!(entry(&catalog, "custom-agent")["use_responses_lite"], false);
    }
}

#[test]
fn native_catalog_search_respects_key_account_grants_and_model_prefix() {
    let collection = collection();
    let key:CodexLocalAccessApiKey=serde_json::from_value(json!({"id":"scoped","label":"scope","key":"synthetic-key","inheritAccountPool":false,"accountIds":["responses"],"modelPrefix":"team"})).unwrap();
    let accounts = vec![
        account("responses", "responses"),
        account("chat", "chat_completions"),
    ];
    let mut model =
        json!({"slug":"team/custom-agent","supports_search_tool":true,"use_responses_lite":false});
    let public = native_model_for_key(&collection, Some(&key), "team/custom-agent").unwrap();
    let plan = native_model_route_plan(&collection, Some(&key), &public).unwrap();
    native_apply_catalog_tool_capabilities(&mut model, &collection, Some(&key), &plan, &accounts);
    assert_eq!(model["supports_search_tool"], true);
    assert_eq!(model["use_responses_lite"], false);
    let catalog = native_catalog(&collection, Some(&key), &accounts).unwrap();
    assert_eq!(
        entry(&catalog, "team/custom-agent")["multi_agent_version"],
        "v2"
    );
    assert!(catalog["models"]
        .as_array()
        .unwrap()
        .iter()
        .all(|model| model["slug"].as_str().unwrap().starts_with("team/")));
}

#[test]
fn native_catalog_search_keeps_plan_and_grok_adapters_and_ignores_other_models() {
    let mut collection = collection();
    let mut plan_account = account("plan", "chat_completions");
    plan_account.api_provider_id = Some("cockpit_chatgpt_plan".into());
    let mut unrelated = account("chat", "chat_completions");
    unrelated.api_model_catalog = vec!["unrelated".into()];
    collection.account_ids = vec!["plan".into(), "chat".into()];
    let plan = native_model_route_plan(&collection, None, "custom-agent").unwrap();
    let mut model =
        json!({"slug":"custom-agent","supports_search_tool":true,"use_responses_lite":false});
    native_apply_catalog_tool_capabilities(
        &mut model,
        &collection,
        None,
        &plan,
        &[plan_account, unrelated],
    );
    assert_eq!(model["supports_search_tool"], true);
    assert_eq!(model["use_responses_lite"], false);
    let mut grok = account("grok", "chat_completions");
    grok.upstream_grok_account_id = Some("synthetic-grok".into());
    assert_eq!(
        native_wire_api(&grok).unwrap(),
        native_contract::WireApi::Responses
    );
    collection.account_ids = vec!["grok".into()];
    let plan = native_model_route_plan(&collection, None, "custom-agent").unwrap();
    native_apply_catalog_tool_capabilities(&mut model, &collection, None, &plan, &[grok]);
    assert_eq!(model["supports_search_tool"], true);
    assert_eq!(model["use_responses_lite"], false);
}

#[test]
fn native_catalog_search_follows_explicit_provider_and_namespaced_mixed_routes() {
    let collection = collection();
    let gateway = json!({"baseUrl":"https://example.invalid/v1","apiKey":"synthetic","upstreamModel":"custom-agent","upstreamModels":["custom-agent"],"wireApi":"chat_completions","supportsVision":false});
    let provider:CodexLocalAccessApiKey=serde_json::from_value(json!({"id":"provider","key":"synthetic","label":"provider","inheritAccountPool":false,"accountIds":["chat"],"providerGateway":gateway.clone()})).unwrap();
    let mixed:CodexLocalAccessApiKey=serde_json::from_value(json!({"id":"mixed","key":"synthetic","label":"mixed","inheritAccountPool":false,"accountIds":["responses"],"modelRouting":{"defaultRoute":"oauth","failurePolicy":"strict","routes":[{"id":"chat-route","namespace":"third","providerAccountId":"chat","providerGateway":gateway}]}})).unwrap();
    let accounts = vec![
        account("responses", "responses"),
        account("chat", "chat_completions"),
    ];
    assert_eq!(
        entry(
            &native_catalog(&collection, Some(&provider), &accounts).unwrap(),
            "custom-agent"
        )["supports_search_tool"],
        false
    );
    assert_eq!(
        entry(
            &native_catalog(&collection, Some(&mixed), &accounts).unwrap(),
            "third/custom-agent"
        )["supports_search_tool"],
        false
    );
}

#[tokio::test]
async fn native_profile_catalog_and_models_endpoint_share_identical_tool_capability_gates() {
    for response_wire in ["responses", "chat_completions"] {
        let mut collection = collection();
        collection.api_keys=vec![serde_json::from_value(json!({"id":"scoped","label":"scope","key":"synthetic-key","inheritAccountPool":false,"accountIds":["responses","chat"]})).unwrap()];
        let accounts = vec![
            account("responses", response_wire),
            account("chat", "chat_completions"),
        ];
        let directory =
            std::env::temp_dir().join(format!("cockpit-catalog-tools-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&directory).unwrap();
        let original = json!({"models":[{"slug":"custom-agent","display_name":"keep display","context_window":12345,"multi_agent_version":null,"supports_search_tool":true,"use_responses_lite":false}]});
        fs::write(
            directory.join(CODEX_MANAGED_MODEL_CATALOG_FILE),
            original.to_string(),
        )
        .unwrap();
        native_sync_profile_catalog_tool_capabilities(
            &directory,
            &collection,
            "synthetic-key",
            &accounts,
        )
        .await
        .unwrap();
        let saved: Value = serde_json::from_str(
            &fs::read_to_string(directory.join(CODEX_MANAGED_MODEL_CATALOG_FILE)).unwrap(),
        )
        .unwrap();
        let served = native_catalog(&collection, collection.api_keys.first(), &accounts).unwrap();
        for field in [
            "multi_agent_version",
            "supports_search_tool",
            "use_responses_lite",
        ] {
            assert_eq!(
                saved["models"][0][field],
                entry(&served, "custom-agent")[field]
            );
        }
        assert_eq!(saved["models"][0]["display_name"], "keep display");
        assert_eq!(saved["models"][0]["context_window"], 12345);
        assert_eq!(
            saved["models"][0]["use_responses_lite"],
            response_wire == "chat_completions"
        );
        fs::remove_dir_all(directory).unwrap();
    }
}

#[test]
fn native_chat_catalog_negotiates_identical_tools_for_raw_step5_and_known_shells() {
    let mut collection = collection();
    let models = vec![
        "step-3.7-flash".to_string(),
        "fixture-second".to_string(),
        "fixture-third".to_string(),
        "step-5-preview".to_string(),
    ];
    let slots = provider_gateway_model_slots(&models);
    assert_eq!(slots[0].client_model, "gpt-5.6-sol");
    assert_eq!(slots[3].client_model, "step-5-preview");
    apply_provider_gateway_model_slots(&mut collection, &models);
    let original_aliases = serde_json::to_value(&collection.model_aliases).unwrap();
    let key: CodexLocalAccessApiKey = serde_json::from_value(json!({
        "id":"provider","key":"synthetic","label":"provider",
        "inheritAccountPool":false,"accountIds":["chat"],
        "providerGateway":{"baseUrl":"https://example.invalid/v1","apiKey":"synthetic",
            "upstreamModel":models[0],"upstreamModels":models,"wireApi":"chat_completions"}
    }))
    .unwrap();
    let mut chat = account("chat", "chat_completions");
    chat.api_model_catalog = models;
    let served = native_catalog(&collection, Some(&key), &[chat.clone()]).unwrap();
    let mut profile: Value =
        serde_json::from_str(&build_provider_model_catalog_json(&slots).unwrap()).unwrap();
    let original_ids = profile["models"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["slug"].clone())
        .collect::<Vec<_>>();
    native_apply_profile_catalog_tool_capabilities(
        &mut profile,
        &collection,
        Some(&key),
        &[chat.clone()],
    );
    for id in ["gpt-5.6-sol", "step-5-preview"] {
        for catalog in [&served, &profile] {
            assert_eq!(entry(catalog, id)["use_responses_lite"], true, "{id}");
            assert_eq!(entry(catalog, id)["supports_search_tool"], false, "{id}");
        }
    }
    assert_eq!(
        profile["models"]
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| entry["slug"].clone())
            .collect::<Vec<_>>(),
        original_ids
    );
    assert_eq!(
        serde_json::to_value(&collection.model_aliases).unwrap(),
        original_aliases
    );
    // Negotiation does not grant hosted capabilities to a Chat account.
    for hosted in ["web_search", "tool_search", "mcp"] {
        assert_eq!(
            native_capability_support(&collection, &chat, "step-5-preview", hosted),
            Some(CodexLocalAccessCapabilitySupport::Unsupported)
        );
    }
}

#[test]
fn native_chat_catalog_does_not_change_responses_only_or_model_denied_routes() {
    let mut collection = collection();
    let plan = native_model_route_plan(&collection, None, "custom-agent").unwrap();
    let mut no_route = json!({"slug":"custom-agent","use_responses_lite":false});
    native_apply_catalog_tool_capabilities(&mut no_route, &collection, None, &plan, &[]);
    assert_eq!(no_route["use_responses_lite"], false);
    let unknown_wire = account("chat", "future_wire");
    native_apply_catalog_tool_capabilities(
        &mut no_route,
        &collection,
        None,
        &plan,
        &[unknown_wire],
    );
    assert_eq!(no_route["use_responses_lite"], false);
    let responses = account("responses", "responses");
    let chat = account("chat", "chat_completions");
    let responses_only = native_catalog(&collection, None, &[responses.clone()]).unwrap();
    assert_eq!(
        entry(&responses_only, "custom-agent")["use_responses_lite"],
        false
    );
    collection.account_model_rules = vec![serde_json::from_value(json!({
        "accountId":"chat","excludedModels":["custom-agent"]
    }))
    .unwrap()];
    let filtered = native_catalog(&collection, None, &[responses, chat]).unwrap();
    assert_eq!(
        entry(&filtered, "custom-agent")["use_responses_lite"],
        false
    );
    assert_eq!(
        entry(&filtered, "custom-agent")["supports_search_tool"],
        true
    );
}

#[tokio::test]
async fn native_chat_lite_catalog_still_rejects_explicit_hosted_web_search_before_upstream() {
    use axum::{routing::post, Json, Router};
    use std::sync::atomic::{AtomicUsize, Ordering};

    let _lock = crate::modules::test_support::env_lock()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let _data = super::native_capability_integration_tests::NativeInstallationTestData::new();
    let attempts = Arc::new(AtomicUsize::new(0));
    let capture = attempts.clone();
    let upstream = Router::new().route("/v1/chat/completions", post(move || {
        let capture = capture.clone();
        async move {
            capture.fetch_add(1, Ordering::SeqCst);
            Json(json!({"choices":[{"message":{"role":"assistant","content":"unexpected upstream"}}]}))
        }
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let upstream_address = listener.local_addr().unwrap();
    let upstream_task = tokio::spawn(async move { axum::serve(listener, upstream).await.unwrap() });
    let mut collection = collection();
    collection.account_ids = vec!["chat".into()];
    let mut chat = account("chat", "chat_completions");
    chat.api_model_catalog = vec!["step-5-preview".into()];
    chat.api_base_url = Some(format!("http://{upstream_address}/v1"));
    chat.egress_proxy_disabled = true;
    let mut host = NativeHostBridge::for_collection(collection);
    host.account_overrides.insert(chat.id.clone(), chat);
    let directory = tempfile::tempdir().unwrap();
    let mut handle = native_api::start(
        "127.0.0.1:0".parse().unwrap(),
        Arc::new(host),
        directory.path().join("identity.sqlite"),
        native_api::Options::default(),
    )
    .await
    .unwrap();
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap();
    let base = format!("http://{}/v1", handle.addr);
    let catalog: Value = client
        .get(format!("{base}/models"))
        .bearer_auth("synthetic-key")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        entry(&catalog, "step-5-preview")["use_responses_lite"],
        true
    );
    for nested in [false, true] {
        let mut body = json!({"model":"step-5-preview","stream":false,
            "client_metadata":{"ws_request_header_x_openai_internal_codex_responses_lite":true},
            "input":[{"role":"user","content":"Synthetic hosted capability check"}]});
        if nested {
            body["input"].as_array_mut().unwrap().push(json!({
                "type":"additional_tools","role":"developer","tools":[{"type":"web_search"}]
            }));
        } else {
            body["tools"] = json!([{"type":"web_search"}]);
        }
        let response = client
            .post(format!("{base}/responses"))
            .bearer_auth("synthetic-key")
            .header("x-openai-internal-codex-responses-lite", "true")
            .json(&body)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status().as_u16(), 503);
        let error: Value = response.json().await.unwrap();
        assert_eq!(
            error["error"]["code"],
            "capability_fallback_no_compatible_account"
        );
        assert_eq!(
            error["error"]["capability_diagnostics"]["unsupported"],
            json!(["web_search"])
        );
        assert_eq!(
            error["error"]["capability_diagnostics"]["trigger_stage"],
            "preflight"
        );
        assert_eq!(attempts.load(Ordering::SeqCst), 0);
    }
    handle.shutdown(Duration::from_secs(2)).await.unwrap();
    upstream_task.abort();
}

#[test]
fn instance_gateway_catalog_ignores_retired_template_switches() {
    let mut template = collection();
    template.multi_agent.optimize = false;
    template.multi_agent.orphan_delegation = true;
    let mut profile = collection();
    apply_provider_gateway_template_settings(&mut profile, &template);
    let catalog = native_catalog(&profile, None, &[account("responses", "responses")]).unwrap();
    assert_eq!(entry(&catalog, "custom-agent")["multi_agent_version"], "v2");
}
