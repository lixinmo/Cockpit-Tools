// Included in native_catalog_tools_tests; all accounts and models are synthetic.
#[tokio::test]
async fn native_provider_profile_writer_and_refresh_share_model_metadata_and_saved_overrides() {
    let models = vec![
        "step-5-preview".into(),
        "step-3.7-flash".into(),
        "vendor-unknown".into(),
    ];
    let bindings = json!([{"clientModel":"gpt-5.6-sol","upstreamModel":"step-5-preview"}]);
    let (mut collection, mut key, mut provider) = identity_fixture(&models, bindings);
    provider.api_base_url = Some("https://api.stepfun.com/step_plan/v1".into());
    key.provider_gateway.as_mut().unwrap().base_url = provider.api_base_url.clone().unwrap();
    collection.api_keys = vec![key.clone()];
    let directory =
        std::env::temp_dir().join(format!("native-provider-profile-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&directory).unwrap();
    let path = directory.join(CODEX_MANAGED_MODEL_CATALOG_FILE);
    write_native_provider_profile_model_catalog(&directory, &collection, &key.key, &provider)
        .unwrap();
    let read = || -> Value { serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap() };
    let api = native_catalog(&collection, Some(&key), &[provider.clone()]).unwrap();
    for id in [
        "step-5-preview",
        "step-3.7-flash",
        "vendor-unknown",
        "gpt-5.6-sol",
    ] {
        for field in [
            "context_window",
            "max_context_window",
            "auto_compact_token_limit",
            "default_reasoning_level",
            "supported_reasoning_levels",
            "supports_search_tool",
            "use_responses_lite",
            "tool_mode",
            "input_modalities",
        ] {
            assert_eq!(
                entry(&read(), id)[field],
                entry(&api, id)[field],
                "{id}.{field}"
            );
        }
    }
    assert!(!read()["models"]
        .as_array()
        .unwrap()
        .iter()
        .any(|m| m["slug"] == "gpt-reserve"));
    // Account and saved profile settings are explicit inputs, unlike a stale
    // value left in a previously generated file.
    provider
        .api_model_context_windows
        .insert("step-3.7-flash".into(), 200_000);
    fs::write(
        directory.join(".cockpit-experimental-model-catalog-enabled"),
        "enabled\n",
    )
    .unwrap();
    fs::write(
        directory.join(".cockpit-experimental-model-catalog-config.json"),
        json!({
            "version":4,"models":[{"model_id":"step-5-preview","display_name":"Step 5",
                "context_window":800_000,"auto_compact_token_limit":700_000,
                "reasoning_efforts":["low","high"],"default_reasoning_effort":"high"}]
        })
        .to_string(),
    )
    .unwrap();
    write_native_provider_profile_model_catalog(&directory, &collection, &key.key, &provider)
        .unwrap();
    for _ in 0..2 {
        native_sync_profile_catalog_tool_capabilities(
            &directory,
            &collection,
            &key.key,
            &[provider.clone()],
        )
        .await
        .unwrap();
        let saved = read();
        let step5 = entry(&saved, "step-5-preview");
        assert_eq!(step5["context_window"], 800_000);
        assert_eq!(step5["auto_compact_token_limit"], 700_000);
        assert_eq!(step5["default_reasoning_level"], "high");
        assert_eq!(
            step5["supported_reasoning_levels"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        assert_eq!(entry(&saved, "step-3.7-flash")["context_window"], 200_000);
        let alias = entry(&saved, "gpt-5.6-sol");
        for field in ["context_window", "auto_compact_token_limit", "default_reasoning_level", "supported_reasoning_levels"] {
            assert_eq!(alias[field], step5[field], "The alias must inherit its actual model's explicit policy: {field}");
        }
    }
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn native_provider_public_metadata_does_not_trust_a_chat_gpt_display_name() {
    let models = vec!["gpt-6.1-sol".into()];
    let (collection, key, provider) = identity_fixture(&models, json!([]));
    let served = native_catalog(&collection, Some(&key), &[provider]).unwrap();
    let public = &served["data"][0];
    assert!(public.get("context_length").is_none());
    assert!(public["model_info"].get("supported_reasoning_efforts").is_none());
    assert!(!public["capabilities"]
        .as_array()
        .unwrap()
        .contains(&json!("thinking")));
    assert_eq!(entry(&served, &models[0])["context_window"], 128_000);
}

fn identity_fixture(
    models: &[String],
    bindings: Value,
) -> (
    CodexLocalAccessCollection,
    CodexLocalAccessApiKey,
    CodexAccount,
) {
    let mut collection = collection();
    collection.account_ids = vec!["chat".into()];
    let mut chat = account("chat", "chat_completions");
    chat.api_model_catalog = models.to_vec();
    let key = serde_json::from_value(json!({
        "id":"provider", "label":"provider", "key":"synthetic", "inheritAccountPool":false,
        "accountIds":["chat"], "providerGateway":{
            "baseUrl":"https://example.invalid/v1", "apiKey":"synthetic",
            "upstreamModel":models.first().map(String::as_str).unwrap_or(""),
            "upstreamModels":models, "wireApi":"chat_completions", "modelBindings":bindings
        }
    }))
    .unwrap();
    (collection, key, chat)
}

#[test]
fn native_provider_identity_catalog_is_complete_independent_of_count_and_order() {
    for count in [0, 1, 3, 9, 64, 257] {
        let mut models = (0..count)
            .map(|i| format!("vendor-model-{i}"))
            .collect::<Vec<_>>();
        let binding = json!([{"clientModel":"gpt-5.6-sol","upstreamModel":"vendor-model-0"}]);
        let (collection, mut key, mut chat) = identity_fixture(&models, binding);
        chat.api_model_context_windows
            .insert("vendor-model-0".into(), 16384);
        let first = native_catalog(&collection, Some(&key), &[chat.clone()]).unwrap();
        assert_eq!(
            first["models"].as_array().unwrap().len(),
            count + usize::from(count > 0)
        );
        models.reverse();
        chat.api_model_catalog = models.clone();
        let gateway = key.provider_gateway.as_mut().unwrap();
        gateway.upstream_models = models.clone();
        gateway.upstream_model = models.first().cloned().unwrap_or_default();
        let second = native_catalog(&collection, Some(&key), &[chat.clone()]).unwrap();
        let slots = stable_provider_model_slots(
            &models,
            key.provider_gateway
                .as_ref()
                .unwrap()
                .model_bindings
                .as_deref()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(slots.len(), count + usize::from(count > 0));
        for id in &models {
            let canonical = native_model_for_key(&collection, Some(&key), id).unwrap();
            assert_eq!(&canonical, id);
            assert_eq!(
                native_model_route_plan(&collection, Some(&key), &canonical)
                    .unwrap()
                    .model,
                *id
            );
            let a = entry(&first, id);
            let b = entry(&second, id);
            for field in [
                "tool_mode",
                "use_responses_lite",
                "supports_search_tool",
                "base_instructions",
                "model_messages",
                "context_window",
                "input_modalities",
            ] {
                assert_eq!(
                    a[field], b[field],
                    "count={count}, model={id}, field={field}"
                );
            }
            assert_eq!(a["use_responses_lite"], true);
            assert_eq!(a["supports_search_tool"], true);
            assert!(a["tool_mode"].is_null());
            assert_eq!(a["default_reasoning_level"], "medium");
            assert_eq!(a["supported_reasoning_levels"].as_array().unwrap().iter()
                .map(|level| level["effort"].as_str().unwrap()).collect::<Vec<_>>(),
                ["low", "medium", "high", "xhigh", "max", "ultra"]);
            assert_eq!(a["support_verbosity"], false);
            assert_eq!(a["supports_reasoning_summary_parameter"], false);
            assert!(!a["base_instructions"]
                .as_str()
                .unwrap()
                .contains("an agent based on GPT-"));
        }
        if count > 0 {
            assert_eq!(
                native_model_for_key(&collection, Some(&key), "gpt-5.6-sol").unwrap(),
                "vendor-model-0"
            );
            let original = entry(&first, "vendor-model-0");
            let alias = entry(&second, "gpt-5.6-sol");
            assert_eq!(alias["visibility"], "hide");
            assert_eq!(alias["context_window"], 16384);
            for field in [
                "tool_mode",
                "use_responses_lite",
                "supports_search_tool",
                "model_messages",
            ] {
                assert_eq!(alias[field], original[field]);
            }
        }
    }
}

#[test]
fn native_provider_identity_unknown_responses_does_not_invent_discovery_support() {
    let models = vec!["unknown-response-model".into()];
    let (collection, mut key, mut provider) = identity_fixture(&models, json!([]));
    provider.api_wire_api = Some("responses".into());
    key.provider_gateway.as_mut().unwrap().wire_api = Some("responses".into());
    let catalog = native_catalog(&collection, Some(&key), &[provider.clone()]).unwrap();
    assert_eq!(entry(&catalog, &models[0])["supports_search_tool"], false);
    assert_eq!(entry(&catalog, &models[0])["use_responses_lite"], false);
    let mut profile = json!({"models":[{"slug":models[0],"supports_search_tool":false}]});
    native_apply_profile_catalog_tool_capabilities(
        &mut profile,
        &collection,
        Some(&key),
        &[provider],
    )
    .unwrap();
    assert_eq!(profile["models"][0]["supports_search_tool"], false);
}

#[test]
fn native_provider_identity_mixed_oauth_chat_alias_uses_canonical_metadata() {
    let mut collection = collection();
    collection.account_ids = vec!["oauth".into(), "chat".into()];
    collection.model_aliases = serde_json::from_value(json!([
        {"sourceModel":"gpt-6.1-sol","alias":"explicit-provider-alias","fork":true}
    ]))
    .unwrap();
    let mut oauth = account("oauth", "responses");
    oauth.auth_mode = CodexAuthMode::OAuth;
    oauth.tokens.access_token = "synthetic-oauth-token".into();
    let chat = account("chat", "chat_completions");
    for accounts in [
        vec![oauth.clone(), chat.clone()],
        vec![chat.clone(), oauth.clone()],
    ] {
        let catalog = native_catalog(&collection, None, &accounts).unwrap();
        let raw = entry(&catalog, "gpt-6.1-sol");
        let alias = entry(&catalog, "explicit-provider-alias");
        for field in [
            "tool_mode",
            "use_responses_lite",
            "supports_search_tool",
            "base_instructions",
            "model_messages",
        ] {
            assert_eq!(raw[field], alias[field], "{field}");
        }
        assert_eq!(raw["use_responses_lite"], false);
    }
}

#[test]
fn native_provider_context_account_overrides_do_not_depend_on_hostname_or_model_name() {
    let models = vec![
        "step-5-preview".into(),
        "step-3.7-flash".into(),
        "step-3.5-flash-2603".into(),
        "vendor-unknown".into(),
    ];
    for base in [
        "https://api.stepfun.com/step_plan/v1",
        "https://api.stepfun.ai/v1",
        "https://relay.example.invalid/v1",
    ] {
        let (collection, mut key, mut provider) = identity_fixture(&models, json!([]));
        provider.api_base_url = Some(base.into());
        key.provider_gateway.as_mut().unwrap().base_url = base.into();
        for id in &models {
            assert!(native_declared_provider_model_metadata(&provider, id).is_none());
            provider.api_model_context_windows.insert(id.clone(), 32_000);
        }
        let profile = tempfile::tempdir().unwrap();
        let mut catalog = native_catalog(&collection, Some(&key), &[provider.clone()]).unwrap();
        native_decorate_provider_models(
            profile.path(), &collection, Some(&key), &[provider.clone()], &mut catalog,
        ).unwrap();
        for id in &models {
            let model = entry(&catalog, id);
            assert_eq!(model["context_window"], 32_000, "{base}: {id}");
            assert_eq!(model["max_context_window"], 32_000, "{base}: {id}");
            assert_eq!(model["auto_compact_token_limit"], 28_800, "{base}: {id}");
            assert_eq!(model["cockpit_context_known"], true, "{base}: {id}");
            let public = catalog["data"].as_array().unwrap().iter()
                .find(|model| model["id"] == id.as_str()).unwrap();
            assert_eq!(public["context_length"], 32_000);
            assert_eq!(public["model_info"]["context_length"], 32_000);
        }
    }
}

#[test]
fn native_provider_context_and_effort_defaults_do_not_depend_on_hostname_or_model_name() {
    let models = vec![
        "step-5-preview".into(),
        "step-3.7-flash".into(),
        "step-3.5-flash-2603".into(),
        "vendor-unknown".into(),
    ];
    let compatible = ["low", "medium", "high", "xhigh", "max", "ultra"];
    for base in [
        "https://api.stepfun.com/step_plan/v1",
        "https://api.stepfun.ai/v1",
        "https://relay.example.invalid/v1",
    ] {
        let (collection, mut key, mut provider) = identity_fixture(&models, json!([]));
        provider.api_base_url = Some(base.into());
        key.provider_gateway.as_mut().unwrap().base_url = base.into();
        let profile = tempfile::tempdir().unwrap();
        let mut catalog = native_catalog(&collection, Some(&key), &[provider.clone()]).unwrap();
        // Reconciliation must keep generic client defaults distinct from
        // actual context/effort capability declarations.
        native_decorate_provider_models(
            profile.path(), &collection, Some(&key), &[provider.clone()], &mut catalog,
        ).unwrap();
        for id in &models {
            let model = entry(&catalog, id);
            assert_eq!(provider_model_reasoning_levels(&provider, id), compatible, "{base}: {id}");
            assert_eq!(model["cockpit_reasoning_known"], false, "{base}: {id}");
            assert_eq!(model["default_reasoning_level"], "medium", "{base}: {id}");
            assert_eq!(model["context_window"], 128_000, "{base}: {id}");
            assert_eq!(model["max_context_window"], 128_000, "{base}: {id}");
            assert_eq!(model["auto_compact_token_limit"], 115_200, "{base}: {id}");
            assert_eq!(model["cockpit_context_known"], false, "{base}: {id}");
            assert_eq!(model["supports_reasoning_summary_parameter"], false);
            assert_eq!(model["supports_search_tool"], true);
            assert_eq!(
                model["supported_reasoning_levels"].as_array().unwrap().iter()
                    .map(|level| level["effort"].as_str().unwrap()).collect::<Vec<_>>(),
                compatible, "{base}: {id}",
            );
            let public = catalog["data"].as_array().unwrap().iter()
                .find(|model| model["id"] == id.as_str()).unwrap();
            assert!(public["model_info"].get("supported_reasoning_efforts").is_none());
            assert!(public["model_info"].get("default_reasoning_effort").is_none());
            assert!(public.get("context_length").is_none());
            assert!(public["model_info"].get("context_length").is_none());
            assert!(!public["capabilities"].as_array().unwrap().contains(&json!("thinking")));
        }
    }
}

#[test]
fn native_provider_context_and_effort_metadata_and_manual_policy_override_generic_defaults() {
    let _lock = crate::modules::test_support::env_lock()
        .lock().unwrap_or_else(|error| error.into_inner());
    let _data = super::native_capability_integration_tests::NativeInstallationTestData::new();
    let models = vec![
        "step-5-preview".into(),
        "step-3.7-flash".into(),
        "step-3.5-flash-2603".into(),
        "vendor-unknown".into(),
    ];
    for base in [
        "https://api.stepfun.com/step_plan/v1",
        "https://relay.example.invalid/v1",
    ] {
        let (collection, mut key, mut provider) = identity_fixture(&models, json!([]));
        provider.api_base_url = Some(base.into());
        // Isolate the synthetic metadata from every other test's account scope.
        provider.openai_api_key = Some(format!("synthetic-{}", uuid::Uuid::new_v4()));
        key.provider_gateway.as_mut().unwrap().base_url = base.into();
        let profile = tempfile::tempdir().unwrap();
        for id in &models {
            crate::modules::codex_provider_metadata::cache_for_test(
                &provider, id,
                crate::modules::codex_provider_metadata::ProviderModelMetadata {
                    reasoning_efforts: Some(vec!["none".into(), "high".into(), "ultra".into()]),
                    default_reasoning_effort: Some("ultra".into()),
                    context_window: Some(192_000),
                    ..Default::default()
                },
            ).unwrap();
        }
        let mut catalog = native_catalog(&collection, Some(&key), &[provider.clone()]).unwrap();
        native_decorate_provider_models(
            profile.path(), &collection, Some(&key), &[provider.clone()], &mut catalog,
        ).unwrap();
        for id in &models {
            let model = entry(&catalog, id);
            assert_eq!(provider_model_reasoning_levels(&provider, id), ["none", "high", "ultra"]);
            assert_eq!(model["cockpit_reasoning_known"], true);
            assert_eq!(model["default_reasoning_level"], "ultra");
            assert_eq!(model["context_window"], 192_000);
            assert_eq!(model["max_context_window"], 192_000);
            assert_eq!(model["auto_compact_token_limit"], 172_800);
            assert_eq!(model["cockpit_context_known"], true);
            let public = catalog["data"].as_array().unwrap().iter()
                .find(|model| model["id"] == id.as_str()).unwrap();
            assert_eq!(public["model_info"]["supported_reasoning_efforts"], json!(["none", "high", "ultra"]));
            assert_eq!(public["model_info"]["default_reasoning_effort"], "ultra");
            assert_eq!(public["context_length"], 192_000);
            assert_eq!(public["model_info"]["context_length"], 192_000);
        }

        codex_account::save_provider_model_quick_config(
            profile.path(), &provider, true,
            models.iter().map(|id| crate::models::codex::CodexExperimentalModelDefinition {
                model_id: id.clone(), display_name: id.clone(),
                reasoning_efforts: Some(vec!["low".into(), "max".into()]),
                default_reasoning_effort: Some("max".into()),
                context_window: Some(384_000), auto_compact_token_limit: Some(320_000),
            }).collect(),
            None,
        ).unwrap();
        native_decorate_provider_models(
            profile.path(), &collection, Some(&key), &[provider.clone()], &mut catalog,
        ).unwrap();
        for id in &models {
            let model = entry(&catalog, id);
            assert_eq!(model["cockpit_reasoning_known"], true);
            assert_eq!(model["default_reasoning_level"], "max");
            assert_eq!(model["context_window"], 384_000);
            assert_eq!(model["max_context_window"], 384_000);
            assert_eq!(model["auto_compact_token_limit"], 320_000);
            assert_eq!(model["cockpit_context_known"], true);
            let public = catalog["data"].as_array().unwrap().iter()
                .find(|model| model["id"] == id.as_str()).unwrap();
            assert_eq!(public["model_info"]["supported_reasoning_efforts"], json!(["low", "max"]));
            assert_eq!(public["model_info"]["default_reasoning_effort"], "max");
            assert_eq!(public["context_length"], 384_000);
            assert_eq!(public["model_info"]["context_length"], 384_000);
        }
    }
}

#[test]
fn native_provider_identity_bindings_are_key_scoped_and_never_reassigned() {
    let models = vec!["model-a".into(), "model-b".into()];
    let (mut collection, mut key, chat) = identity_fixture(
        &models,
        json!([{"clientModel":"gpt-5.6-sol","upstreamModel":"model-a"}]),
    );
    collection.model_aliases = serde_json::from_value(json!([
        {"sourceModel":"outside-provider","alias":"gpt-5.6-sol","fork":false}
    ]))
    .unwrap();
    let mut other_key = key.clone();
    other_key.provider_gateway.as_mut().unwrap().model_bindings = Some(
        serde_json::from_value(json!([
            {"clientModel":"gpt-5.6-sol","upstreamModel":"model-b"}
        ]))
        .unwrap(),
    );
    assert_eq!(
        native_model_for_key(&collection, Some(&key), "gpt-5.6-sol").unwrap(),
        "model-a"
    );
    assert_eq!(
        native_model_for_key(&collection, Some(&other_key), "gpt-5.6-sol").unwrap(),
        "model-b"
    );
    key.allowed_models = vec!["gpt-5.6-sol".into()];
    assert!(native_model_for_key(&collection, Some(&key), "model-a").is_err());
    key.excluded_models = vec!["model-a".into()];
    assert_eq!(
        native_model_for_key(&collection, Some(&key), "gpt-5.6-sol")
            .unwrap_err()
            .status,
        403
    );
    key.allowed_models.clear();
    key.excluded_models.clear();
    let gateway = key.provider_gateway.as_mut().unwrap();
    gateway.upstream_models = vec!["model-b".into(), "new-model".into()];
    gateway.upstream_model = "model-b".into();
    let canonical = native_model_for_key(&collection, Some(&key), "gpt-5.6-sol").unwrap();
    assert_eq!(canonical, "model-a");
    assert_eq!(
        native_model_route_plan(&collection, Some(&key), &canonical)
            .err()
            .unwrap()
            .code,
        "model_not_found"
    );
    assert!(
        !native_catalog(&collection, Some(&key), &[chat]).unwrap()["models"]
            .as_array()
            .unwrap()
            .iter()
            .any(|model| model["slug"] == "gpt-5.6-sol")
    );
}

#[test]
fn native_provider_identity_missing_evidence_never_guesses_from_list_position() {
    let models = vec!["first".into(), "second".into(), "third".into()];
    for bindings in [Value::Null, json!([])] {
        let (collection, key, _) = identity_fixture(&models, bindings);
        let canonical = native_model_for_key(&collection, Some(&key), "gpt-5.6-sol").unwrap();
        assert!(native_model_route_plan(&collection, Some(&key), &canonical).is_err());
    }
    let (collection, mut key, _) =
        identity_fixture(&models, json!([{"clientModel":"gpt-5.6-sol"}]));
    assert_eq!(
        native_model_for_key(&collection, Some(&key), "gpt-5.6-sol")
            .unwrap_err()
            .code,
        "provider_model_binding_unresolved"
    );
    key.provider_gateway
        .as_mut()
        .unwrap()
        .upstream_models
        .push("gpt-5.6-sol".into());
    assert!(native_model_for_key(&collection, Some(&key), "gpt-5.6-sol").is_err());
}

#[test]
fn native_provider_identity_gpt_named_chat_model_uses_provider_capabilities() {
    let models = vec!["gpt-5.6-sol".into(), "ordinary-model".into()];
    let (collection, key, chat) = identity_fixture(&models, json!([]));
    let catalog = native_catalog(&collection, Some(&key), &[chat]).unwrap();
    for id in &models {
        let item = entry(&catalog, id);
        assert!(item["tool_mode"].is_null());
        assert_eq!(item["service_tiers"], json!([]));
        assert_eq!(item["available_in_plans"], json!([]));
        assert_eq!(
            native_model_for_key(&collection, Some(&key), id).unwrap(),
            *id
        );
    }
}

#[test]
fn native_provider_identity_discovery_preserves_explicit_capability_denials() {
    let models = vec!["model-a".into()];
    let (mut collection, key, chat) = identity_fixture(&models, json!([]));
    collection.capability_fallback.overrides.push(
        serde_json::from_value(json!({
            "accountId":"chat","model":"*","capabilities":{"tool_search_client":"unsupported"}
        }))
        .unwrap(),
    );
    let catalog = native_catalog(&collection, Some(&key), &[chat.clone()]).unwrap();
    assert_eq!(entry(&catalog, "model-a")["supports_search_tool"], false);
    assert_eq!(
        native_capability_support(&collection, &chat, "model-a", "tool_search_client"),
        Some(CodexLocalAccessCapabilitySupport::Unsupported)
    );
    for hosted in [
        "web_search",
        "tool_search",
        "mcp",
        "file_search",
        "code_interpreter",
    ] {
        assert_eq!(
            native_capability_support(&collection, &chat, "model-a", hosted),
            Some(CodexLocalAccessCapabilitySupport::Unsupported)
        );
    }
}
