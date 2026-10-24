use super::*;

fn fixture_account() -> CodexAccount {
    let mut account = CodexAccount::new_api_key(
        format!("profile-selection-{}", uuid::Uuid::new_v4()),
        "synthetic@example.invalid".into(),
        "synthetic-provider-key".into(),
        CodexApiProviderMode::Custom,
        Some("https://example.invalid/v1".into()),
        None,
        None,
        vec![
            "provider-a".into(),
            "provider-b".into(),
            "provider-c".into(),
        ],
    );
    account.api_wire_api = Some("chat_completions".into());
    account.egress_proxy_disabled = true;
    account
}

fn fixture_collection(account: &CodexAccount) -> CodexLocalAccessCollection {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let bindings = json!([{"clientModel":"gpt-5.6-sol","upstreamModel":"provider-a"}]);
    serde_json::from_value(json!({
        "enabled":true,"gatewayMode":"native","creditsPolicy":"prefer_regular",
        "port":port,"apiKey":"synthetic-profile-key","createdAt":1,"updatedAt":1,
        "accountIds":[account.id],
        "apiKeys":[{
            "id":provider_gateway_api_key_id(&account.id),"label":"Synthetic profile",
            "key":"synthetic-profile-key","inheritAccountPool":false,"accountIds":[account.id],
            "providerModelBindings":bindings,"providerGateway":{
                "baseUrl":"https://example.invalid/v1","apiKey":"synthetic-provider-key",
                "upstreamModel":account.api_model_catalog.first(),
                "upstreamModels":account.api_model_catalog,"wireApi":"chat_completions",
                "modelBindings":bindings,
            },
        }],
    }))
    .unwrap()
}

fn selected(profile: &Path) -> String {
    crate::modules::codex_config_format::load_codex_config_doc(&profile_config_path(profile))
        .unwrap()
        .get("model")
        .and_then(|item| item.as_str())
        .unwrap()
        .to_owned()
}

async fn publish_and_stop(profile: &Path, account: &CodexAccount) {
    codex_account::save_account(account).unwrap();
    let collection = fixture_collection(account);
    let slots = provider_gateway_model_slots_for_collection(
        &collection,
        collection.api_keys[0].provider_gateway.as_ref().unwrap(),
    )
    .unwrap();
    let plan = native_model_route_plan(
        &collection, Some(&collection.api_keys[0]), "provider-a",
    ).unwrap();
    assert_eq!(native_plan_account_model(&plan, account).as_deref(), Some("provider-a"));
    assert_eq!(plan.gateway.as_ref().unwrap().wire_api.as_deref(), Some("chat_completions"));
    let published = native_ensure_provider_profile(
        profile,
        account,
        collection,
        "synthetic-profile-key",
        &slots,
    )
    .await;
    let (_, stopped) = stop_provider_gateway_runtime_checked(
        &provider_gateway_runtime_key(profile, &account.id),
        "synthetic_selection_test",
    )
    .await;
    stopped.unwrap();
    published.unwrap();
    let doc = crate::modules::codex_config_format::load_codex_config_doc(
        &profile_config_path(profile),
    ).unwrap();
    let provider = &doc["model_providers"][CODEX_LOCAL_ACCESS_RUNTIME_PROVIDER_ID];
    assert_eq!(provider["wire_api"].as_str(), Some("responses"));
    let base = reqwest::Url::parse(provider["base_url"].as_str().unwrap()).unwrap();
    assert!(matches!(base.host_str(), Some("127.0.0.1" | "localhost" | "::1")));
    assert_eq!(base.path(), "/v1");
    assert_ne!(provider["experimental_bearer_token"].as_str(), account.openai_api_key.as_deref());
    assert_eq!(native_wire_api(account).unwrap(), native_contract::WireApi::Chat);
    assert_eq!(account.api_base_url.as_deref(), Some("https://example.invalid/v1"));
}

#[tokio::test]
async fn native_provider_profile_selection_refresh_preserves_raw_and_frozen_alias() {
    let _lock = crate::modules::test_support::env_lock()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let _data = super::native_capability_integration_tests::NativeInstallationTestData::new();
    let profile = tempfile::tempdir().unwrap();
    let mut account = fixture_account();
    account.api_startup_model = Some("provider-b".into());
    publish_and_stop(profile.path(), &account).await;
    assert_eq!(
        selected(profile.path()),
        "provider-b",
        "first activation honors startup preference"
    );
    account.api_startup_model = None;
    fs::write(
        profile
            .path()
            .join(".cockpit-experimental-model-catalog-enabled"),
        "enabled\n",
    )
    .unwrap();
    fs::write(
        profile
            .path()
            .join(".cockpit-experimental-model-catalog-config.json"),
        json!({
            "version":4,"default_model_id":"provider-b","models":[{
                "model_id":"provider-b","display_name":"Provider B","context_window":16000,
            }],
        })
        .to_string(),
    )
    .unwrap();
    // Prove the saved policy would otherwise reset the selected model.
    codex_account::reapply_experimental_model_policy_if_enabled(profile.path()).unwrap();
    assert_eq!(selected(profile.path()), "provider-b");
    for chosen in ["gpt-5.6-sol", "provider-a"] {
        write_local_access_profile_model_override(profile.path(), chosen).unwrap();
        remember_takeover_ownership(profile.path()).unwrap();
        account.api_model_catalog = vec![
            "provider-c".into(),
            "provider-b".into(),
            "provider-a".into(),
        ];
        publish_and_stop(profile.path(), &account).await;
        assert_eq!(
            selected(profile.path()),
            chosen,
            "refresh must preserve a valid selection"
        );
        let catalog: Value = serde_json::from_str(
            &fs::read_to_string(profile.path().join(CODEX_MANAGED_MODEL_CATALOG_FILE)).unwrap(),
        )
        .unwrap();
        for id in ["provider-a", "provider-b", "provider-c", "gpt-5.6-sol"] {
            assert!(catalog["models"]
                .as_array()
                .unwrap()
                .iter()
                .any(|model| model["slug"] == id));
        }
        assert!(!catalog["models"]
            .as_array()
            .unwrap()
            .iter()
            .any(|model| model["slug"] == "gpt-reserve"));
    }
}

#[test]
fn native_provider_profile_selection_keeps_acl_and_does_not_guess_unresolved_aliases() {
    let profile = tempfile::tempdir().unwrap();
    let account = fixture_account();
    let mut collection = fixture_collection(&account);
    fs::write(profile_config_path(profile.path()), format!(
        "model = \"gpt-5.6-sol\"\nmodel_provider = \"codex_local_access\"\nmodel_catalog_json = {CODEX_MANAGED_MODEL_CATALOG_FILE:?}\n[model_providers.codex_local_access]\nexperimental_bearer_token = \"synthetic-profile-key\"\nbase_url = \"http://127.0.0.1:19001/v1\"\n"
    )).unwrap();
    remember_takeover_ownership(profile.path()).unwrap();
    let slots = provider_gateway_model_slots_for_collection(
        &collection,
        collection.api_keys[0].provider_gateway.as_ref().unwrap(),
    )
    .unwrap();
    collection.api_keys[0]
        .provider_model_bindings
        .as_mut()
        .unwrap()[0]
        .upstream_model = None;
    let before = fs::read_to_string(profile_config_path(profile.path())).unwrap();
    assert!(native_provider_profile_selected_model(
        profile.path(),
        &account,
        &collection,
        "synthetic-profile-key",
        &slots
    )
    .unwrap_err()
    .contains("provider_model_binding_unresolved"));
    assert_eq!(
        fs::read_to_string(profile_config_path(profile.path())).unwrap(),
        before
    );
    collection.api_keys[0].allowed_models = vec!["provider-b".into()];
    assert_eq!(
        native_provider_profile_selected_model(
            profile.path(),
            &account,
            &collection,
            "synthetic-profile-key",
            &slots
        )
        .unwrap()
        .as_deref(),
        Some("provider-b")
    );
    assert!(native_provider_profile_selected_model(
        profile.path(),
        &account,
        &collection,
        "different-key",
        &slots
    )
    .is_err());
}

#[tokio::test]
async fn native_provider_profile_selection_manual_efforts_republish_only_owned_scoped_catalog() {
    let _lock = crate::modules::test_support::env_lock().lock().unwrap_or_else(|e| e.into_inner());
    let _data = super::native_capability_integration_tests::NativeInstallationTestData::new();
    let profile = tempfile::tempdir().unwrap();
    let account = fixture_account();
    codex_account::save_account(&account).unwrap();
    let collection = fixture_collection(&account);
    let slots = provider_gateway_model_slots_for_collection(&collection,
        collection.api_keys[0].provider_gateway.as_ref().unwrap()).unwrap();
    native_ensure_provider_profile(profile.path(), &account, collection.clone(),
        "synthetic-profile-key", &slots).await.unwrap();
    let runtime_key = provider_gateway_runtime_key(profile.path(), &account.id);
    let port = provider_gateway_runtime_store().lock().await[&runtime_key].actual_port;
    let auth = fs::read(profile_auth_path(profile.path())).unwrap();
    let old_catalog = fs::read(profile.path().join(CODEX_MANAGED_MODEL_CATALOG_FILE)).unwrap();
    let definitions = vec![
        crate::models::codex::CodexExperimentalModelDefinition {
            model_id: "provider-a".into(), display_name: "Provider A".into(),
            reasoning_efforts: Some(vec!["low".into(), "high".into()]),
            default_reasoning_effort: Some("high".into()), context_window: None,
            auto_compact_token_limit: None,
        },
        crate::models::codex::CodexExperimentalModelDefinition {
            model_id: "ungranted-gpt".into(), display_name: "Unrelated saved definition".into(),
            reasoning_efforts: Some(vec!["ultra".into()]), default_reasoning_effort: None,
            context_window: None, auto_compact_token_limit: None,
        },
    ];
    codex_account::save_quick_config_for_base_dir_with_default(profile.path(), None, None,
        Some(true), Some(definitions), None).unwrap();
    assert_eq!(fs::read(profile.path().join(CODEX_MANAGED_MODEL_CATALOG_FILE)).unwrap(), old_catalog,
        "generic settings writer must not publish an unscoped intermediate catalog");
    refresh_native_provider_catalog_for_profile(profile.path()).await.unwrap();
    let catalog:Value = serde_json::from_slice(&fs::read(profile.path().join(CODEX_MANAGED_MODEL_CATALOG_FILE)).unwrap()).unwrap();
    let models = catalog["models"].as_array().unwrap();
    assert!(!models.iter().any(|model| model["slug"] == "ungranted-gpt" || model["slug"] == "gpt-reserve"));
    let model = models.iter().find(|model| model["slug"] == "provider-a").unwrap();
    assert_eq!(model["supported_reasoning_levels"].as_array().unwrap().iter().map(|level|level["effort"].as_str().unwrap()).collect::<Vec<_>>(), vec!["low", "high"]);
    assert_eq!(model["default_reasoning_level"], "high");
    assert_eq!(model["use_responses_lite"], true);
    assert_eq!(fs::read(profile_auth_path(profile.path())).unwrap(), auth);
    assert_eq!(provider_gateway_runtime_store().lock().await[&runtime_key].actual_port, port,
        "metadata refresh must not restart the listener");

    let doc = crate::modules::codex_config_format::load_codex_config_doc(&profile_config_path(profile.path())).unwrap();
    assert_eq!(doc["model_providers"][CODEX_LOCAL_ACCESS_RUNTIME_PROVIDER_ID]["wire_api"].as_str(), Some("responses"));
    codex_account::save_quick_config_for_base_dir_with_default(profile.path(), None, None,
        Some(false), None, None).unwrap();
    assert!(profile.path().join(CODEX_MANAGED_MODEL_CATALOG_FILE).is_file());
    refresh_native_provider_catalog_for_profile(profile.path()).await.unwrap();
    assert!(codex_account::profile_reasoning_override(profile.path(), "provider-a").unwrap().is_none());

    let scoped = vec![crate::models::codex::CodexExperimentalModelDefinition {
        model_id: "provider-a".into(), display_name: "Provider A".into(),
        reasoning_efforts: Some(vec!["none".into(), "minimal".into(), "high".into()]),
        default_reasoning_effort: Some("high".into()), context_window: None,
        auto_compact_token_limit: None,
    }];
    let config_before = fs::read(profile_config_path(profile.path())).unwrap();
    codex_account::save_provider_model_quick_config(profile.path(), &account, true, scoped, None).unwrap();
    assert_eq!(fs::read(profile_config_path(profile.path())).unwrap(), config_before,
        "provider policy save alone never changes the active instance config");
    refresh_native_provider_catalog_for_account(profile.path(), Some("different-provider")).await.unwrap();
    assert_eq!(fs::read(profile_config_path(profile.path())).unwrap(), config_before);
    refresh_native_provider_catalog_for_account(profile.path(), Some(&account.id)).await.unwrap();
    let catalog:Value = serde_json::from_slice(&fs::read(profile.path().join(CODEX_MANAGED_MODEL_CATALOG_FILE)).unwrap()).unwrap();
    let model = catalog["models"].as_array().unwrap().iter().find(|m|m["slug"]=="provider-a").unwrap();
    assert_eq!(model["supported_reasoning_levels"].as_array().unwrap().iter().map(|level|level["effort"].as_str().unwrap()).collect::<Vec<_>>(), vec!["none", "minimal", "high"]);
    assert_eq!(provider_gateway_runtime_store().lock().await[&runtime_key].actual_port, port);

    // A stale runtime must never reclaim a profile switched outside its endpoint.
    fs::write(profile_config_path(profile.path()), "model = \"gpt-6-astra\"\n").unwrap();
    let switched = fs::read(profile_config_path(profile.path())).unwrap();
    refresh_native_provider_catalog_for_profile(profile.path()).await.unwrap();
    assert_eq!(fs::read(profile_config_path(profile.path())).unwrap(), switched);
    let (_, stopped) = stop_provider_gateway_runtime_checked(&runtime_key, "manual-catalog-test").await;
    stopped.unwrap();
}
