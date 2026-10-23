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
