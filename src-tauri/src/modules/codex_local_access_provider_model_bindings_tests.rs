use super::*;

fn binding(client: &str, upstream: Option<&str>) -> CodexLocalAccessProviderModelBinding {
    CodexLocalAccessProviderModelBinding {
        client_model: client.into(),
        upstream_model: upstream.map(str::to_owned),
    }
}

fn targets(slots: &[ProviderGatewayModelSlot]) -> std::collections::BTreeMap<String, String> {
    slots
        .iter()
        .map(|slot| (slot.client_model.clone(), slot.upstream_model.clone()))
        .collect()
}

fn state() -> ProviderGatewayProfileState {
    serde_json::from_value(json!({
        "apiKey":"synthetic-local-key","port":19001,"createdAt":1,"updatedAt":2,
    }))
    .unwrap()
}

fn account() -> CodexAccount {
    CodexAccount::new_api_key(
        "binding-fixture".into(),
        "fixture@example.invalid".into(),
        "synthetic-upstream".into(),
        CodexApiProviderMode::Custom,
        Some("https://example.invalid/v1".into()),
        None,
        None,
        vec!["provider-a".into(), "provider-b".into()],
    )
}

fn config_for(state: &ProviderGatewayProfileState, catalog: &str) -> String {
    format!(
        "model_provider = \"codex_local_access\"\nmodel_catalog_json = {catalog:?}\n[model_providers.codex_local_access]\nexperimental_bearer_token = {:?}\nbase_url = \"http://127.0.0.1:{}/v1\"\n",
        state.api_key, state.port.unwrap(),
    )
}

fn write_catalog(profile: &Path, state: &ProviderGatewayProfileState, target: &str) {
    fs::write(
        profile_config_path(profile),
        config_for(state, CODEX_MANAGED_MODEL_CATALOG_FILE),
    )
    .unwrap();
    let path = profile.join(CODEX_MANAGED_MODEL_CATALOG_FILE);
    fs::write(
        &path,
        json!({"models":[
            {"slug":"gpt-5.6-sol","display_name":target,"description":target},
            {"slug":"codex-auto-review","display_name":"Review","description":"Review"},
        ]})
        .to_string(),
    )
    .unwrap();
    crate::modules::codex_managed_model_catalog_version::write_managed_catalog_meta(&path).unwrap();
}

#[test]
fn native_provider_bindings_have_no_positional_limit_and_survive_list_edits() {
    let frozen = vec![
        binding("gpt-5.6-sol", Some("provider-0")),
        binding("retired-alias", Some("retired-model")),
        binding("unknown-alias", None),
    ];
    for count in [0, 1, 3, 4, 9, 1000] {
        let models = (0..count)
            .map(|index| format!("provider-{index}"))
            .collect::<Vec<_>>();
        let original = targets(&stable_provider_model_slots(&models, &frozen).unwrap());
        for model in &models {
            assert_eq!(original.get(model), Some(model));
        }
        assert_eq!(original.len(), models.len() + usize::from(count > 0));
        assert!(!original.contains_key("retired-alias"));
        assert!(!original.contains_key("unknown-alias"));
        let mut reordered = models.clone();
        reordered.reverse();
        assert_eq!(
            targets(&stable_provider_model_slots(&reordered, &frozen).unwrap()),
            original
        );
        reordered.insert(0, "new-first".into());
        reordered.retain(|model| model != "provider-0");
        let removed = targets(&stable_provider_model_slots(&reordered, &frozen).unwrap());
        assert!(!removed.contains_key("gpt-5.6-sol"));
        assert_eq!(removed["new-first"], "new-first");
        reordered.push("provider-0".into());
        let restored = targets(&stable_provider_model_slots(&reordered, &frozen).unwrap());
        assert_eq!(restored["gpt-5.6-sol"], "provider-0");
    }
    assert_eq!(frozen[1].upstream_model.as_deref(), Some("retired-model"));
}

#[test]
fn native_provider_binding_collision_never_transfers_an_old_public_identity() {
    for target in [None, Some("different-model")] {
        assert!(stable_provider_model_slots(
            &["gpt-5.6-sol".into()],
            &[binding("gpt-5.6-sol", target)]
        )
        .unwrap_err()
        .starts_with("PROVIDER_MODEL_IDENTITY_CONFLICT"));
    }
    assert!(normalize_provider_model_bindings(&[
        binding("old", Some("a")),
        binding("OLD", Some("b")),
    ])
    .is_err());
    assert!(normalize_provider_model_bindings(&[binding("old", Some(""))]).is_err());
    assert_eq!(
        normalize_provider_model_bindings(&[binding("old", Some("a")), binding("OLD", Some("A")),])
            .unwrap(),
        vec![binding("old", Some("a"))]
    );
}

#[test]
fn native_provider_descriptor_missing_bindings_is_raw_only_and_go_keeps_legacy_slots() {
    let gateway: CodexLocalAccessProviderGateway = serde_json::from_value(json!({
        "baseUrl":"https://example.invalid/v1","apiKey":"synthetic", "upstreamModel":"a",
        "upstreamModels":["a","b","c","d"],"wireApi":"chat_completions",
    }))
    .unwrap();
    let mut collection: CodexLocalAccessCollection = serde_json::from_value(json!({
        "enabled":true,"port":19001,"apiKey":"synthetic","createdAt":1,"updatedAt":1,
        "gatewayMode":"native",
    }))
    .unwrap();
    let raw = targets(&provider_gateway_model_slots_for_collection(&collection, &gateway).unwrap());
    assert_eq!(
        raw,
        ["a", "b", "c", "d"]
            .into_iter()
            .map(|id| (id.into(), id.into()))
            .collect()
    );
    collection.gateway_mode = CodexLocalAccessGatewayMode::Sidecar;
    assert_eq!(
        provider_gateway_model_slots_for_collection(&collection, &gateway).unwrap(),
        provider_gateway_model_slots(&gateway.upstream_models)
    );
}

#[test]
fn native_provider_manifest_binding_evidence_is_scoped_to_saved_key_and_account() {
    let state = state();
    let account_id = "binding-fixture";
    let manifest = json!({"apiKeys":[{
        "id":provider_gateway_api_key_id(account_id), "key":state.api_key, "enabled":true,
        "accountIds":[account_id], "providerGateway":{
            "baseUrl":"https://example.invalid/v1","apiKey":"synthetic-upstream",
            "upstreamModel":"provider-a","upstreamModels":["provider-a","removed-model"],
        }
    }], "modelAliases":[
        {"alias":"gpt-5.6-sol","sourceModel":"provider-a"},
        {"alias":"gpt-5.6-terra","sourceModel":"removed-model"},
        {"alias":"custom-user-alias","sourceModel":"provider-a"},
        {"alias":"gpt-6.1-sol","sourceModel":"provider-a"},
        {"alias":"unrelated","sourceModel":"outside-snapshot"},
    ]});
    let evidence = provider_binding_manifest_evidence(&manifest, &state, account_id, false)
        .unwrap()
        .unwrap();
    assert_eq!(
        evidence,
        vec![
            binding("gpt-5.6-sol", Some("provider-a")),
            binding("gpt-5.6-terra", Some("removed-model"))
        ]
    );
    for (field, value) in [
        ("key", json!("different-key")),
        ("accountIds", json!(["different-account"])),
        ("accountIds", json!([account_id, "additional-account"])),
        ("enabled", json!(false)),
        ("id", json!("unrelated-key")),
    ] {
        let mut wrong = manifest.clone();
        wrong["apiKeys"][0][field] = value;
        assert!(
            provider_binding_manifest_evidence(&wrong, &state, account_id, false)
                .unwrap()
                .is_none()
        );
    }
    let combined = merge_provider_binding_evidence(vec![
        binding("old", None),
        binding("old", Some("a")),
        binding("old", Some("b")),
    ])
    .unwrap();
    assert_eq!(combined, vec![binding("old", None)]);
}

#[test]
fn native_provider_grok_identities_do_not_require_a_generic_gateway_or_gpt_slots() {
    let state = state();
    let account_id = "grok-binding-fixture";
    let manifest = json!({"apiKeys":[{
        "id":provider_gateway_api_key_id(account_id),"key":state.api_key,"enabled":true,
        "accountIds":[account_id],"providerGateway":null,
    }],"accounts":[{
        "id":account_id,"provider":codex_account::GROK_PROVIDER_ID,
        "modelIds":["grok-old","grok-next"],
    }],"modelAliases":[
        {"alias":"gpt-5.6-sol","sourceModel":"grok-old"},
        {"alias":"gpt-5.6-terra","sourceModel":"grok-next"},
        {"alias":"custom-user-alias","sourceModel":"grok-old"},
    ]});
    let frozen = provider_binding_manifest_evidence(&manifest, &state, account_id, true)
        .unwrap()
        .unwrap();
    assert_eq!(
        frozen,
        vec![
            binding("gpt-5.6-sol", Some("grok-old")),
            binding("gpt-5.6-terra", Some("grok-next"))
        ]
    );
    assert!(
        provider_binding_manifest_evidence(&manifest, &state, account_id, false)
            .unwrap()
            .is_none()
    );
    let mut wrong = manifest.clone();
    wrong["accounts"][0]["provider"] = json!("unrelated-provider");
    assert!(
        provider_binding_manifest_evidence(&wrong, &state, account_id, true)
            .unwrap()
            .is_none()
    );
    let key: CodexLocalAccessApiKey = serde_json::from_value(json!({
        "id":"grok-key","label":"synthetic","key":"synthetic","accountIds":[account_id],
        "inheritAccountPool":false,"providerGateway":null,"providerModelBindings":frozen,
    }))
    .unwrap();
    assert!(key.provider_gateway.is_none());
    let mut gateway: CodexLocalAccessProviderGateway = serde_json::from_value(json!({
        "baseUrl":codex_account::GROK_CLI_CHAT_PROXY_BASE_URL,"apiKey":"",
        "upstreamModel":"grok-new","upstreamModels":["grok-new","grok-next","grok-old"],
        "wireApi":"responses","modelBindings":key.provider_model_bindings,
    }))
    .unwrap();
    let collection: CodexLocalAccessCollection = serde_json::from_value(json!({
        "enabled":true,"gatewayMode":"native","port":19001,"apiKey":"synthetic",
        "createdAt":1,"updatedAt":1,
    }))
    .unwrap();
    let first =
        targets(&provider_gateway_model_slots_for_collection(&collection, &gateway).unwrap());
    gateway.upstream_models.reverse();
    assert_eq!(
        targets(&provider_gateway_model_slots_for_collection(&collection, &gateway).unwrap()),
        first
    );
    assert_eq!(first["grok-new"], "grok-new");
    assert_eq!(first["gpt-5.6-sol"], "grok-old");
    assert_eq!(gateway.wire_api.as_deref(), Some("responses"));
    assert_eq!(
        gateway.base_url,
        codex_account::GROK_CLI_CHAT_PROXY_BASE_URL
    );
    assert!(key.provider_gateway.is_none());
}

#[test]
fn native_provider_grok_runtime_never_treats_todays_models_as_historical_evidence() {
    let state = state();
    let mut account = account();
    account.upstream_grok_account_id = Some("synthetic-grok-source".into());
    // This raw ID was added only after the old target was removed.
    account.api_model_catalog = vec!["gpt-5.6-sol".into()];
    let mut collection: CodexLocalAccessCollection = serde_json::from_value(json!({
        "enabled":true,"gatewayMode":"native","port":state.port,"apiKey":state.api_key,
        "createdAt":1,"updatedAt":1,"modelAliases":[{
            "alias":"gpt-5.6-sol","sourceModel":"grok-old",
        }],"apiKeys":[{
            "id":provider_gateway_api_key_id(&account.id),"label":"synthetic",
            "key":state.api_key,"enabled":true,"accountIds":[account.id],
            "providerGateway":null,
        }],
    }))
    .unwrap();
    assert!(
        provider_binding_runtime_collection_evidence(&collection, &account, &state)
            .unwrap()
            .is_none()
    );
    collection.api_keys[0].provider_model_bindings =
        Some(vec![binding("gpt-5.6-sol", Some("grok-old"))]);
    let proven = provider_binding_runtime_collection_evidence(&collection, &account, &state)
        .unwrap()
        .unwrap();
    assert_eq!(proven, vec![binding("gpt-5.6-sol", Some("grok-old"))]);
    assert!(stable_provider_model_slots(&account.api_model_catalog, &proven).is_err());
}

#[test]
fn native_provider_catalog_migration_requires_owned_active_catalog_and_identity_shape() {
    let profile = tempfile::tempdir().unwrap();
    let state = state();
    let allowed = vec!["provider-a".into(), "provider-b".into()];
    write_catalog(profile.path(), &state, "provider-a");
    assert_eq!(
        provider_binding_catalog_evidence(profile.path(), &state, &allowed).unwrap(),
        vec![binding("gpt-5.6-sol", Some("provider-a"))]
    );
    let catalog_path = profile.path().join(CODEX_MANAGED_MODEL_CATALOG_FILE);
    fs::write(
        &catalog_path,
        json!({"models":[{
            "slug":"gpt-5.6-sol","display_name":"provider-b","description":"different identity",
        }]})
        .to_string(),
    )
    .unwrap();
    assert_eq!(
        provider_binding_catalog_evidence(profile.path(), &state, &allowed).unwrap(),
        vec![binding("gpt-5.6-sol", None)]
    );
    write_catalog(profile.path(), &state, "provider-a");
    for changed in [
        ProviderGatewayProfileState {
            api_key: "different".into(),
            ..state.clone()
        },
        ProviderGatewayProfileState {
            port: Some(19002),
            ..state.clone()
        },
    ] {
        assert!(
            provider_binding_catalog_evidence(profile.path(), &changed, &allowed)
                .unwrap()
                .is_empty()
        );
    }
    remember_takeover_ownership(profile.path()).unwrap();
    fs::write(
        profile.path().join(TAKEOVER_OWNERSHIP_FILE),
        r#"{"connection_hash":"different-owner"}"#,
    )
    .unwrap();
    assert!(
        provider_binding_catalog_evidence(profile.path(), &state, &allowed)
            .unwrap()
            .is_empty()
    );
    remember_takeover_ownership(profile.path()).unwrap();
    write_catalog(profile.path(), &state, "removed-target");
    assert_eq!(
        provider_binding_catalog_evidence(profile.path(), &state, &allowed).unwrap(),
        vec![binding("gpt-5.6-sol", None)]
    );
    let backup_name = format!("{CODEX_MANAGED_MODEL_CATALOG_FILE}.bak");
    fs::rename(&catalog_path, profile.path().join(&backup_name)).unwrap();
    fs::write(
        profile_config_path(profile.path()),
        config_for(&state, &backup_name),
    )
    .unwrap();
    assert!(
        provider_binding_catalog_evidence(profile.path(), &state, &allowed)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn native_provider_real_legacy_writer_chain_migrates_before_port_reassignment() {
    let _lock = crate::modules::test_support::env_lock()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let _data = super::native_capability_integration_tests::NativeInstallationTestData::new();
    let profile = tempfile::tempdir().unwrap();
    let account = account();
    let legacy = state();
    save_provider_gateway_profile_state(profile.path(), &account.id, &legacy).unwrap();
    fs::write(
        profile_config_path(profile.path()),
        config_for(&legacy, CODEX_MANAGED_MODEL_CATALOG_FILE),
    )
    .unwrap();
    write_local_access_profile_model_catalog(
        profile.path(),
        false,
        &[ProfileModelDefinition::plain(
            "gpt-5.6-sol",
            "Takeover display",
        )],
    )
    .unwrap();
    write_provider_gateway_model_catalog_with_templates(
        profile.path(),
        &[ProviderGatewayModelSlot {
            client_model: "gpt-5.6-sol".into(),
            upstream_model: "provider-a".into(),
        }],
        None,
        Some(&account),
    )
    .unwrap();
    remember_takeover_ownership(profile.path()).unwrap();
    let catalog_path = profile.path().join(CODEX_MANAGED_MODEL_CATALOG_FILE);
    // This is the old writer sequence, without hand-writing a matching meta.
    assert!(
        crate::modules::codex_managed_model_catalog_version::managed_catalog_needs_rebuild(
            &catalog_path
        )
    );
    // Missing global configuration still means the default Native mode.
    assert!(!local_access_file_path().unwrap().exists());
    freeze_existing_native_provider_model_bindings(profile.path(), &account).unwrap();
    reset_instance_gateway_profile_port(profile.path(), &account.id);
    provider_gateway_profile_port(profile.path(), &account.id).unwrap();
    let after = load_provider_gateway_profile_state(profile.path(), &account.id)
        .unwrap()
        .unwrap();
    assert_eq!(after.api_key, legacy.api_key);
    assert!(after
        .model_bindings
        .as_ref()
        .unwrap()
        .contains(&binding("gpt-5.6-sol", Some("provider-a"))));
    assert_eq!(
        resolve_native_provider_model_bindings_for_profile(profile.path(), &account, Some(&after))
            .unwrap(),
        after.model_bindings.unwrap()
    );
}

#[test]
fn native_provider_does_not_freeze_editable_account_mappings_or_real_gpt6_ids() {
    let _lock = crate::modules::test_support::env_lock()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let _data = super::native_capability_integration_tests::NativeInstallationTestData::new();
    let profile = tempfile::tempdir().unwrap();
    let mut account = account();
    account.api_model_catalog.push("gpt-6.1-sol".into());
    account.api_model_mappings = vec![crate::models::codex::CodexApiModelMapping {
        client_model: "user-editable".into(),
        upstream_model: "provider-a".into(),
    }];
    let before = serde_json::to_value(&account.api_model_mappings).unwrap();
    provider_gateway_profile_port(profile.path(), &account.id).unwrap();
    let frozen =
        resolve_native_provider_model_bindings_for_profile(profile.path(), &account, None).unwrap();
    assert!(frozen.is_empty());
    assert_eq!(
        serde_json::to_value(&account.api_model_mappings).unwrap(),
        before
    );
    let slots = targets(&stable_provider_model_slots(&account.api_model_catalog, &frozen).unwrap());
    assert_eq!(slots["gpt-6.1-sol"], "gpt-6.1-sol");
    assert!(!slots.contains_key("user-editable"));
}

#[test]
fn native_provider_profile_binding_migration_is_frozen_without_key_or_port_rotation() {
    let _lock = crate::modules::test_support::env_lock()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let _data = super::native_capability_integration_tests::NativeInstallationTestData::new();
    let profile = tempfile::tempdir().unwrap();
    let mut account = account();
    let legacy = state();
    save_provider_gateway_profile_state(profile.path(), &account.id, &legacy).unwrap();
    write_catalog(profile.path(), &legacy, "provider-a");
    let frozen =
        resolve_native_provider_model_bindings_for_profile(profile.path(), &account, Some(&legacy))
            .unwrap();
    assert!(frozen.contains(&binding("gpt-5.6-sol", Some("provider-a"))));
    let saved = load_provider_gateway_profile_state(profile.path(), &account.id)
        .unwrap()
        .unwrap();
    assert_eq!(saved.api_key, legacy.api_key);
    assert_eq!(saved.port, legacy.port);
    assert_eq!(saved.created_at, legacy.created_at);
    assert_eq!(saved.model_bindings.as_ref(), Some(&frozen));
    let mut stale_writer = legacy.clone();
    stale_writer.port = Some(19003);
    save_provider_gateway_profile_state(profile.path(), &account.id, &stale_writer).unwrap();
    assert_eq!(
        load_provider_gateway_profile_state(profile.path(), &account.id)
            .unwrap()
            .unwrap()
            .model_bindings,
        Some(frozen.clone())
    );
    write_catalog(profile.path(), &legacy, "provider-b");
    account.api_model_catalog = vec!["new-first".into(), "provider-b".into()];
    assert_eq!(
        resolve_native_provider_model_bindings_for_profile(profile.path(), &account, Some(&saved))
            .unwrap(),
        frozen
    );
    assert!(
        !targets(&stable_provider_model_slots(&account.api_model_catalog, &frozen).unwrap())
            .contains_key("gpt-5.6-sol")
    );
    account.api_model_catalog.push("provider-a".into());
    assert_eq!(
        targets(&stable_provider_model_slots(&account.api_model_catalog, &frozen).unwrap())
            ["gpt-5.6-sol"],
        "provider-a"
    );
    let other_profile = tempfile::tempdir().unwrap();
    provider_gateway_profile_port(other_profile.path(), &account.id).unwrap();
    assert!(resolve_native_provider_model_bindings_for_profile(
        other_profile.path(),
        &account,
        None
    )
    .unwrap()
    .is_empty());
}

#[test]
fn native_provider_unproven_legacy_aliases_are_blocked_instead_of_guessed() {
    let _lock = crate::modules::test_support::env_lock()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let _data = super::native_capability_integration_tests::NativeInstallationTestData::new();
    let profile = tempfile::tempdir().unwrap();
    let account = account();
    let legacy = state();
    save_provider_gateway_profile_state(profile.path(), &account.id, &legacy).unwrap();
    let frozen =
        resolve_native_provider_model_bindings_for_profile(profile.path(), &account, Some(&legacy))
            .unwrap();
    assert!(frozen
        .iter()
        .all(|binding| binding.upstream_model.is_none()));
    assert!(frozen
        .iter()
        .any(|binding| binding.client_model == "gpt-5.6-sol"));
    let slots = targets(&stable_provider_model_slots(&account.api_model_catalog, &frozen).unwrap());
    assert_eq!(slots.len(), account.api_model_catalog.len());
    assert_eq!(slots["provider-a"], "provider-a");
    assert!(!slots.contains_key("gpt-5.6-sol"));
}
