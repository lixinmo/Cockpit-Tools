use super::*;

fn levels(values: &[&str]) -> Vec<String> { values.iter().map(|value| (*value).into()).collect() }

fn provider(model: &str) -> CodexAccount {
    let mut account = CodexAccount::new_api_key(
        "reasoning-fixture".into(), "fixture@example.invalid".into(),
        "synthetic-reasoning-key".into(), CodexApiProviderMode::Custom,
        Some("https://reasoning.example.invalid/v1".into()), None, None,
        vec![model.into()],
    );
    account.api_wire_api = Some("chat_completions".into());
    account
}

fn manual(profile: &Path, model: &str, efforts: Value, default: Value) {
    fs::write(profile.join(".cockpit-experimental-model-catalog-enabled"), "enabled\n").unwrap();
    fs::write(profile.join(".cockpit-experimental-model-catalog-config.json"), json!({
        "version":4,"models":[{"model_id":model,"display_name":model,
            "reasoning_efforts":efforts,"default_reasoning_effort":default}]
    }).to_string()).unwrap();
}

#[test]
fn native_provider_reasoning_clamps_known_levels_and_preserves_unknown_intent() {
    let three = levels(&["low", "medium", "high"]);
    for requested in ["high", "xhigh", "max", "ultra"] {
        assert_eq!(native_map_provider_effort(requested, Some(&three)).unwrap(), Some("high".into()));
    }
    let two = levels(&["low", "high"]);
    assert_eq!(native_map_provider_effort("medium", Some(&two)).unwrap(), Some("low".into()));
    let high_intent = levels(&["low", "high", "max"]);
    assert_eq!(native_map_provider_effort("xhigh", Some(&high_intent)).unwrap(), Some("max".into()));
    assert_eq!(native_map_provider_effort("max", Some(&levels(&["low", "high", "ultra"]))).unwrap(), Some("ultra".into()));
    for requested in ["none", "auto", "low", "medium", "high", "xhigh", "max", "ultra"] {
        assert_eq!(native_map_provider_effort(requested, None).unwrap(), Some(requested.into()));
    }
    for special in ["none", "auto"] {
        assert_eq!(native_map_provider_effort(special, Some(&three)).unwrap(), Some(special.into()));
        assert_eq!(native_map_provider_effort(special, Some(&[])).unwrap(), None);
    }
    assert_eq!(native_map_provider_effort("high", Some(&[])).unwrap_err().code, "provider_reasoning_not_supported");
}

#[test]
fn native_provider_reasoning_manual_unknown_model_controls_catalog_and_wire() {
    let profile = tempfile::tempdir().unwrap();
    let account = provider("vendor-new-42");
    manual(profile.path(), "vendor-new-42", json!(["low", "high"]), json!("high"));
    let source = json!({"model":"vendor-new-42","reasoning":{"effort":"ultra","summary":"auto"},
        "tools":[{"type":"function","name":"read","parameters":{"type":"object"}}],
        "input":[{"type":"reasoning","encrypted_content":"","summary":[{"text":"saved"}]},
        {"type":"function_call","name":"read","call_id":"c","arguments":"{}"},
        {"type":"function_call_output","call_id":"c","output":"kept"}]});
    let mut expected = source.clone();
    expected["reasoning"]["effort"] = json!("high");
    let body = native_prepare_provider_reasoning_body(Some(profile.path()), &account,
        "vendor-new-42", source.clone()).unwrap();
    assert_eq!(body, expected);
    let wire = native_api::responses_to_chat_for_test(&body).unwrap();
    assert_eq!(wire["reasoning_effort"], "high");
    assert_eq!(wire["messages"][1]["content"], "kept");
    let base = codex_protocol::build_provider_client_model("vendor-new-42", 0);
    let decorated: Value = serde_json::from_str(&codex_account::decorate_managed_model_catalog_for_profile(
        profile.path(), &json!({"models":[base]}).to_string()).unwrap()).unwrap();
    assert_eq!(native_provider_effort_levels(&decorated["models"][0]).unwrap(), levels(&["low", "high"]));
    assert_eq!(decorated["models"][0]["default_reasoning_level"], "high");
    assert_eq!(source["reasoning"]["effort"], "ultra");
    fs::remove_file(profile.path().join(".cockpit-experimental-model-catalog-enabled")).unwrap();
    assert_eq!(native_prepare_provider_reasoning_body(Some(profile.path()), &account,
        "vendor-new-42", source.clone()).unwrap(), source);
}

#[test]
fn native_provider_reasoning_manual_default_and_unsupported_are_explicit() {
    let profile = tempfile::tempdir().unwrap();
    let account = provider("vendor-new-42");
    manual(profile.path(), "vendor-new-42", json!(["low", "high"]), json!("low"));
    let request = json!({"model":"vendor-new-42","input":"synthetic"});
    let body = native_prepare_provider_reasoning_body(Some(profile.path()), &account,
        "vendor-new-42", request.clone()).unwrap();
    assert_eq!(body["reasoning"]["effort"], "low");
    manual(profile.path(), "vendor-new-42", json!([]), Value::Null);
    assert_eq!(native_prepare_provider_reasoning_body(Some(profile.path()), &account,
        "vendor-new-42", request.clone()).unwrap(), request);
    let rejected = json!({"reasoning":{"effort":"high"}});
    assert_eq!(native_prepare_provider_reasoning_body(Some(profile.path()), &account,
        "vendor-new-42", rejected).unwrap_err().code, "provider_reasoning_not_supported");
}

#[test]
fn native_provider_reasoning_documented_stepfun_uses_real_model_and_endpoint() {
    let mut account = provider("step-3.5-flash-2603");
    account.api_base_url = Some("https://api.stepfun.com/step_plan/v1".into());
    let body = json!({"model":"step-3.5-flash-2603","reasoning":{"effort":"ultra"}});
    assert_eq!(native_prepare_provider_reasoning_body(None, &account,
        "step-3.5-flash-2603", body.clone()).unwrap()["reasoning"]["effort"], "high");
    account.api_base_url = Some("https://reasoning.example.invalid/v1".into());
    assert_eq!(native_prepare_provider_reasoning_body(None, &account,
        "step-3.5-flash-2603", body.clone()).unwrap(), body);
    account.api_provider_mode = CodexApiProviderMode::OpenaiBuiltin;
    assert_eq!(native_prepare_provider_reasoning_body(None, &account,
        "step-3.5-flash-2603", body.clone()).unwrap(), body);
}
