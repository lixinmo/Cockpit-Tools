// Exercise the account-page backend entry points, not a hand-written Codex config.
fn native_provider_wire_fixture(env: &TestEnvGuard) -> CodexAccount {
    let key = format!("synthetic-wire-{}", uuid::Uuid::new_v4());
    let mut account = CodexAccount::new_api_key(
        super::build_api_key_account_id(&key),
        "synthetic-wire@example.invalid".into(),
        key,
        CodexApiProviderMode::Custom,
        Some("https://example.invalid/step_plan/v1".into()),
        Some("synthetic_stepfun".into()),
        Some("Synthetic StepFun".into()),
        vec!["step-3.7-flash".into(), "step-5-preview".into()],
    );
    account.api_wire_api = Some("chat_completions".into());
    account.api_startup_model = Some("step-5-preview".into());
    account.egress_proxy_disabled = true;
    save_account(&account).unwrap();
    save_account_index(&build_test_account_index(&account)).unwrap();
    let template: crate::models::codex_local_access::CodexLocalAccessCollection =
        serde_json::from_value(serde_json::json!({
            "enabled":false,"port":19001,"apiKey":"synthetic-global-key",
            "accountIds":[],"createdAt":1,"updatedAt":1,
            "gatewayMode":"native","creditsPolicy":"prefer_regular",
        }))
        .unwrap();
    fs::write(
        env.home_dir
            .join(".antigravity_cockpit/codex_local_access.json"),
        serde_json::to_vec(&template).unwrap(),
    )
    .unwrap();
    account
}

fn native_provider_wire_assert_profile(profile: &Path, account: &CodexAccount) -> String {
    let doc =
        crate::modules::codex_config_format::load_codex_config_doc(&profile.join("config.toml"))
            .unwrap();
    assert_eq!(doc["model_provider"].as_str(), Some("codex_local_access"));
    let provider = &doc["model_providers"]["codex_local_access"];
    assert_eq!(provider["wire_api"].as_str(), Some("responses"));
    let base = provider["base_url"].as_str().unwrap();
    let url = reqwest::Url::parse(base).unwrap();
    assert!(matches!(
        url.host_str(),
        Some("127.0.0.1" | "localhost" | "::1")
    ));
    assert_eq!(url.path(), "/v1");
    assert_ne!(
        provider["experimental_bearer_token"].as_str(),
        account.openai_api_key.as_deref()
    );
    assert_eq!(account.api_wire_api.as_deref(), Some("chat_completions"));
    assert_eq!(
        account.api_base_url.as_deref(),
        Some("https://example.invalid/step_plan/v1")
    );
    assert_eq!(doc["model"].as_str(), Some("step-5-preview"));
    let catalog = profile.join(doc["model_catalog_json"].as_str().unwrap());
    let catalog: serde_json::Value = serde_json::from_slice(&fs::read(catalog).unwrap()).unwrap();
    assert!(catalog["models"]
        .as_array()
        .unwrap()
        .iter()
        .any(|model| model["slug"] == "step-5-preview"));
    base.to_string()
}

async fn native_provider_wire_switch(account: CodexAccount) -> Result<CodexAccount, String> {
    let id = account.id.clone();
    super::commit_account_switch_locked(&id, super::PreparedCodexAccountSwitch::Account(account))
        .await
}

#[test]
fn native_provider_wire_projection_raw_chat_is_rejected_before_any_auth_or_config_write() {
    let _lock = crate::modules::test_support::env_lock()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let env = TestEnvGuard::new("native-wire-raw-projection");
    let mut account = native_provider_wire_fixture(&env);
    for (index, wire) in ["chat_completions", "chat", " Chat_Completions "]
        .iter()
        .enumerate()
    {
        account.api_wire_api = Some((*wire).into());
        let target = env.home_dir.join(format!("raw-profile-{index}"));
        let error = super::write_prepared_account_bundle_to_dir(&target, &account).unwrap_err();
        assert!(error.contains("CODEX_CLIENT_REQUIRES_RESPONSES_GATEWAY"));
        assert!(!target.join("config.toml").exists());
        assert!(!target.join("auth.json").exists());
        assert!(!target
            .join(super::CODEX_MANAGED_MODEL_CATALOG_FILE)
            .exists());
    }
}

#[tokio::test]
async fn native_provider_wire_projection_switch_and_restart_keep_downstream_responses() {
    let _lock = crate::modules::test_support::env_lock()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let env = TestEnvGuard::new("native-wire-switch");
    let account = native_provider_wire_fixture(&env);
    let profile = env.codex_home();
    let switched = native_provider_wire_switch(account).await.unwrap();
    let base = native_provider_wire_assert_profile(&profile, &switched);
    crate::modules::codex_local_access::stop_provider_gateways_for_profile(&profile).await;
    crate::modules::codex_local_access::ensure_provider_gateway_for_dir(&profile, &switched.id)
        .await
        .unwrap();
    assert_eq!(
        native_provider_wire_assert_profile(&profile, &switched),
        base
    );
    crate::modules::codex_local_access::stop_provider_gateways_for_profile(&profile).await;
    let saved = load_account(&switched.id).unwrap();
    assert_eq!(saved.api_wire_api.as_deref(), Some("chat_completions"));
}

#[tokio::test]
async fn native_provider_wire_projection_current_credential_edit_and_provider_sync() {
    let _lock = crate::modules::test_support::env_lock()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let env = TestEnvGuard::new("native-wire-current-edit");
    let account = native_provider_wire_fixture(&env);
    let profile = env.codex_home();
    let switched = native_provider_wire_switch(account).await.unwrap();
    let initial_base = native_provider_wire_assert_profile(&profile, &switched);
    let edited = update_api_key_credentials(
        &switched.id,
        switched.openai_api_key.clone().unwrap(),
        switched.api_base_url.clone(),
        Some(CodexApiProviderMode::Custom),
        switched.api_provider_id.clone(),
        switched.api_provider_name.clone(),
        switched.api_model_catalog.clone(),
        Some(false),
        Some("chat_completions".into()),
        false,
        false,
        Default::default(),
        None,
        Some("Edited StepFun".into()),
        None,
    )
    .await
    .unwrap();
    assert_eq!(
        native_provider_wire_assert_profile(&profile, &edited),
        initial_base
    );
    let count = sync_api_key_provider_accounts(
        vec![edited.id.clone()],
        edited.api_base_url.clone(),
        Some(CodexApiProviderMode::Custom),
        edited.api_provider_id.clone(),
        edited.api_provider_name.clone(),
        vec![
            "step-5-preview".into(),
            "step-3.7-flash".into(),
            "provider-added".into(),
        ],
        Some("chat_completions".into()),
        false,
        true,
        Default::default(),
        None,
        None,
    )
    .await
    .unwrap();
    assert_eq!(count, 1);
    let saved = load_account(&edited.id).unwrap();
    assert_eq!(
        native_provider_wire_assert_profile(&profile, &saved),
        initial_base
    );
    assert!(saved
        .api_model_catalog
        .iter()
        .any(|model| model == "provider-added"));
    crate::modules::codex_local_access::stop_provider_gateways_for_profile(&profile).await;
}

#[tokio::test]
async fn native_provider_wire_projection_catalog_cleanup_and_header_refresh_preserve_gateway() {
    let _lock = crate::modules::test_support::env_lock()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let env = TestEnvGuard::new("native-wire-catalog-refresh");
    let account = native_provider_wire_fixture(&env);
    let profile = env.codex_home();
    let switched = native_provider_wire_switch(account).await.unwrap();
    let original_config = fs::read(profile.join("config.toml")).unwrap();
    let original_auth = fs::read(profile.join("auth.json")).unwrap();
    for _ in 0..2 {
        super::sync_or_cleanup_managed_model_catalog_for_dir(&profile, &switched).unwrap();
        super::refresh_api_key_provider_projection_in_dir(&profile, &switched).unwrap();
        native_provider_wire_assert_profile(&profile, &switched);
        assert_eq!(
            fs::read(profile.join("config.toml")).unwrap(),
            original_config
        );
        assert_eq!(fs::read(profile.join("auth.json")).unwrap(), original_auth);
    }
    crate::modules::codex_local_access::stop_provider_gateways_for_profile(&profile).await;
}

#[tokio::test]
async fn native_provider_wire_projection_failed_switch_preserves_previous_client_config() {
    let _lock = crate::modules::test_support::env_lock()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let env = TestEnvGuard::new("native-wire-failed-switch");
    let account = native_provider_wire_fixture(&env);
    let profile = env.codex_home();
    let switched = native_provider_wire_switch(account).await.unwrap();
    let original_config = fs::read(profile.join("config.toml")).unwrap();
    let original_auth = fs::read(profile.join("auth.json")).unwrap();
    let mut invalid = switched.clone();
    let invalid_key = "synthetic-invalid-switch-key";
    invalid.id = super::build_api_key_account_id(invalid_key);
    invalid.openai_api_key = Some(invalid_key.into());
    invalid.api_model_catalog = vec!["x".repeat(1025)];
    save_account(&invalid).unwrap();
    assert!(native_provider_wire_switch(invalid).await.is_err());
    assert_eq!(
        fs::read(profile.join("config.toml")).unwrap(),
        original_config
    );
    assert_eq!(fs::read(profile.join("auth.json")).unwrap(), original_auth);
    native_provider_wire_assert_profile(&profile, &switched);
    assert_eq!(
        load_account_index().current_account_id.as_deref(),
        Some(switched.id.as_str())
    );
    crate::modules::codex_local_access::stop_provider_gateways_for_profile(&profile).await;
}

#[tokio::test]
async fn native_provider_wire_projection_failed_current_edits_leave_client_config_valid() {
    let _lock = crate::modules::test_support::env_lock()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let env = TestEnvGuard::new("native-wire-failed-edits");
    let account = native_provider_wire_fixture(&env);
    let profile = env.codex_home();
    let switched = native_provider_wire_switch(account).await.unwrap();
    let original_config = fs::read(profile.join("config.toml")).unwrap();
    let original_auth = fs::read(profile.join("auth.json")).unwrap();
    let edited = update_api_key_credentials(
        &switched.id,
        switched.openai_api_key.clone().unwrap(),
        switched.api_base_url.clone(),
        Some(CodexApiProviderMode::Custom),
        switched.api_provider_id.clone(),
        switched.api_provider_name.clone(),
        vec!["x".repeat(1025)],
        Some(false),
        Some("chat_completions".into()),
        false,
        false,
        Default::default(),
        None,
        None,
        None,
    )
    .await;
    assert!(edited.is_err());
    assert_eq!(
        fs::read(profile.join("config.toml")).unwrap(),
        original_config
    );
    assert_eq!(fs::read(profile.join("auth.json")).unwrap(), original_auth);
    save_account(&switched).unwrap();
    let synced = sync_api_key_provider_accounts(
        vec![switched.id.clone()],
        switched.api_base_url.clone(),
        Some(CodexApiProviderMode::Custom),
        switched.api_provider_id.clone(),
        switched.api_provider_name.clone(),
        vec!["x".repeat(1025)],
        Some("chat_completions".into()),
        false,
        false,
        Default::default(),
        None,
        None,
    )
    .await;
    assert!(synced.is_err());
    assert_eq!(
        fs::read(profile.join("config.toml")).unwrap(),
        original_config
    );
    assert_eq!(fs::read(profile.join("auth.json")).unwrap(), original_auth);
    native_provider_wire_assert_profile(&profile, &switched);
    crate::modules::codex_local_access::stop_provider_gateways_for_profile(&profile).await;
}

#[tokio::test]
async fn native_provider_wire_projection_reimport_reactivates_through_gateway() {
    let _lock = crate::modules::test_support::env_lock()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let env = TestEnvGuard::new("native-wire-import");
    let account = native_provider_wire_fixture(&env);
    let profile = env.codex_home();
    let switched = native_provider_wire_switch(account).await.unwrap();
    let original_base = native_provider_wire_assert_profile(&profile, &switched);
    let mut imported = switched.clone();
    imported.api_model_catalog.reverse();
    let imported = super::import_account_struct(imported).unwrap();
    let activated = super::reactivate_if_imported_matches_current(&[imported])
        .await
        .unwrap();
    assert_eq!(
        native_provider_wire_assert_profile(&profile, &activated),
        original_base
    );
    crate::modules::codex_local_access::stop_provider_gateways_for_profile(&profile).await;
}

#[tokio::test]
async fn native_provider_wire_projection_bound_oauth_keeps_credential_identity() {
    let _lock = crate::modules::test_support::env_lock()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let env = TestEnvGuard::new("native-wire-bound-oauth");
    let mut account = native_provider_wire_fixture(&env);
    let oauth = build_test_oauth_account(make_codex_tokens(
        "wire-oauth@example.invalid",
        "wire-bound",
        "wire-org",
        "wire-bound",
        "synthetic-refresh",
    ));
    save_account(&oauth).unwrap();
    account.bound_oauth_account_id = Some(oauth.id.clone());
    save_account(&account).unwrap();
    let mut index = build_test_account_index(&account);
    index
        .accounts
        .extend(build_test_account_index(&oauth).accounts);
    save_account_index(&index).unwrap();
    let profile = env.codex_home();
    let switched = super::commit_account_switch_locked(
        &account.id,
        super::PreparedCodexAccountSwitch::ApiKeyWithOauth {
            api_key_account: account.clone(),
            oauth_account: oauth.clone(),
        },
    )
    .await
    .unwrap();
    native_provider_wire_assert_profile(&profile, &switched);
    let auth: serde_json::Value =
        serde_json::from_slice(&fs::read(profile.join("auth.json")).unwrap()).unwrap();
    assert_eq!(auth["tokens"]["access_token"], oauth.tokens.access_token);
    assert_eq!(
        load_account(&switched.id).unwrap().bound_oauth_account_id,
        Some(oauth.id)
    );
    crate::modules::codex_local_access::stop_provider_gateways_for_profile(&profile).await;
}

fn native_provider_wire_export_snapshot(profile: &Path, output: &Path, account: &CodexAccount) {
    native_provider_wire_assert_profile(profile, account);
    fs::create_dir(output).unwrap();
    for file in [
        "config.toml",
        "auth.json",
        super::CODEX_MANAGED_MODEL_CATALOG_FILE,
    ] {
        fs::copy(profile.join(file), output.join(file)).unwrap();
    }
}

// Cloud compiles this opt-in driver. Locally it exports only a synthetic profile
// generated by the real switch transaction, for the installed Codex parser to read.
#[tokio::test]
#[ignore = "requires CT_NATIVE_WIRE_PROFILE_OUTPUT pointing to a new temporary directory"]
async fn native_provider_wire_projection_export_codex_profile() {
    let _lock = crate::modules::test_support::env_lock()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let output = std::path::PathBuf::from(std::env::var("CT_NATIVE_WIRE_PROFILE_OUTPUT").unwrap());
    assert!(output.is_absolute() && !output.exists());
    let env = TestEnvGuard::new("native-wire-parser-export");
    let account = native_provider_wire_fixture(&env);
    let profile = env.codex_home();
    let switched = native_provider_wire_switch(account).await.unwrap();
    native_provider_wire_export_snapshot(&profile, &output, &switched);
    let edited = update_api_key_credentials(
        &switched.id,
        switched.openai_api_key.clone().unwrap(),
        switched.api_base_url.clone(),
        Some(CodexApiProviderMode::Custom),
        switched.api_provider_id.clone(),
        switched.api_provider_name.clone(),
        switched.api_model_catalog.clone(),
        Some(false),
        Some("chat_completions".into()),
        false,
        false,
        Default::default(),
        None,
        Some("Edited fixture".into()),
        None,
    )
    .await
    .unwrap();
    native_provider_wire_export_snapshot(&profile, &output.join("after-credential-edit"), &edited);
    sync_api_key_provider_accounts(
        vec![edited.id.clone()],
        edited.api_base_url.clone(),
        Some(CodexApiProviderMode::Custom),
        edited.api_provider_id.clone(),
        edited.api_provider_name.clone(),
        edited.api_model_catalog.clone(),
        Some("chat_completions".into()),
        false,
        false,
        Default::default(),
        None,
        None,
    )
    .await
    .unwrap();
    let saved = load_account(&edited.id).unwrap();
    native_provider_wire_export_snapshot(&profile, &output.join("after-provider-sync"), &saved);
    super::sync_or_cleanup_managed_model_catalog_for_dir(&profile, &saved).unwrap();
    super::refresh_api_key_provider_projection_in_dir(&profile, &saved).unwrap();
    native_provider_wire_export_snapshot(&profile, &output.join("after-catalog-refresh"), &saved);
    fs::write(
        output.join("manifest.json"),
        serde_json::json!({
            "schema":1,"synthetic":true,"clientWire":"responses","upstreamWire":"chat_completions",
            "model":"step-5-preview","loopbackOnly":true,
            "profiles":[".","after-credential-edit","after-provider-sync","after-catalog-refresh"],
        })
        .to_string(),
    )
    .unwrap();
    crate::modules::codex_local_access::stop_provider_gateways_for_profile(&profile).await;
}
