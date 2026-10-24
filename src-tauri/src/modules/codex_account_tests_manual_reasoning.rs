use super::*;
use serde_json::json;

fn declaration(efforts: Option<Vec<&str>>, default: Option<&str>) -> CodexExperimentalModelDefinition {
    CodexExperimentalModelDefinition {
        model_id: "vendor/custom-model".into(),
        display_name: "Custom provider".into(),
        reasoning_efforts: efforts.map(|values| values.into_iter().map(str::to_owned).collect()),
        default_reasoning_effort: default.map(str::to_owned),
        context_window: None,
        auto_compact_token_limit: None,
    }
}

fn saved(profile: &Path, definition: &CodexExperimentalModelDefinition) {
    fs::write(experimental_model_policy_path(profile), "enabled\n").unwrap();
    fs::write(experimental_model_config_path(profile), json!({
        "version": EXPERIMENTAL_MODEL_CATALOG_CONFIG_VERSION,
        "models": [definition],
        "migrations": [GPT_6_ASTRA_MODEL_CATALOG_MIGRATION_ID, GPT_6_SOL_LUNA_MODEL_CATALOG_MIGRATION_ID, GPT_6_1_SOL_MODEL_CATALOG_MIGRATION_ID]
    }).to_string()).unwrap();
}

#[test]
fn manual_provider_reasoning_adds_actual_capabilities_to_unknown_catalog_and_runtime() {
    let dir = tempfile::tempdir().unwrap();
    let definition = declaration(Some(vec!["high", "low"]), Some("high"));
    saved(dir.path(), &definition);
    let before = fs::read(experimental_model_config_path(dir.path())).unwrap();
    let source = json!({"models":[
        {"slug":"vendor/custom-model","supported_reasoning_levels":[],"default_reasoning_level":null,"use_responses_lite":true,"tools":"preserve"},
        {"slug":"unrelated","supported_reasoning_levels":[{"effort":"medium"}],"default_reasoning_level":"medium"}
    ]});
    let rendered: serde_json::Value = serde_json::from_str(
        &decorate_managed_model_catalog_for_profile(dir.path(), &source.to_string()).unwrap()).unwrap();
    assert_eq!(rendered["models"].as_array().unwrap().len(), 2);
    assert_eq!(rendered["models"][0]["supported_reasoning_levels"], json!([
        {"effort":"low","description":"low"},{"effort":"high","description":"high"}
    ]));
    assert_eq!(rendered["models"][0]["default_reasoning_level"], "high");
    assert_eq!(rendered["models"][0]["use_responses_lite"], true);
    assert_eq!(rendered["models"][0]["tools"], "preserve");
    assert_eq!(rendered["models"][1], source["models"][1]);
    let policy = profile_reasoning_override(dir.path(), "VENDOR/CUSTOM-MODEL").unwrap().unwrap();
    assert_eq!(policy.reasoning_efforts.unwrap(), vec!["low", "high"]);
    assert_eq!(policy.default_reasoning_effort.as_deref(), Some("high"));
    assert_eq!(fs::read(experimental_model_config_path(dir.path())).unwrap(), before);
}

#[test]
fn manual_provider_reasoning_empty_and_inherit_are_distinct_and_profile_scoped() {
    let dir = tempfile::tempdir().unwrap();
    let empty = declaration(Some(Vec::new()), None);
    saved(dir.path(), &empty);
    let source = json!({"models":[{"slug":"vendor/custom-model","supported_reasoning_levels":[{"effort":"high"}],"default_reasoning_level":"high"}]});
    let rendered: serde_json::Value = serde_json::from_str(
        &decorate_managed_model_catalog_for_profile(dir.path(), &source.to_string()).unwrap()).unwrap();
    assert_eq!(rendered["models"][0]["supported_reasoning_levels"], json!([]));
    assert!(rendered["models"][0]["default_reasoning_level"].is_null());
    assert_eq!(profile_reasoning_override(dir.path(), "vendor/custom-model").unwrap().unwrap().reasoning_efforts, Some(Vec::new()));
    assert!(profile_reasoning_override(dir.path(), "other-model").unwrap().is_none());
    saved(dir.path(), &declaration(None, None));
    assert!(profile_reasoning_override(dir.path(), "vendor/custom-model").unwrap().is_none());
    let inherited: serde_json::Value = serde_json::from_str(
        &decorate_managed_model_catalog_for_profile(dir.path(), &source.to_string()).unwrap()).unwrap();
    assert_eq!(inherited, source);
    saved(dir.path(), &empty);
    fs::remove_file(experimental_model_policy_path(dir.path())).unwrap();
    assert!(profile_reasoning_override(dir.path(), "vendor/custom-model").unwrap().is_none());
}

#[test]
fn manual_provider_reasoning_defaults_are_validated_against_the_manual_list() {
    let custom = declaration(Some(vec!["low", "ultra"]), Some("ultra"));
    assert!(normalize_experimental_model_definitions(vec![custom]).is_ok());
    for invalid in [
        declaration(Some(vec!["low"]), Some("high")),
        declaration(Some(vec![]), Some("high")),
        declaration(Some(vec!["invented"]), None),
    ] {
        assert!(normalize_experimental_model_definitions(vec![invalid]).is_err());
    }
    let dir = tempfile::tempdir().unwrap();
    let custom = declaration(Some(vec!["low", "high"]), Some("high"));
    fs::write(experimental_model_catalog_path(dir.path()), json!({"models":[{
        "slug":"vendor/custom-model","supported_reasoning_levels":[],"default_reasoning_level":null
    }]}).to_string()).unwrap();
    assert!(validate_profile_default_reasoning_efforts(dir.path(), &[custom.clone()]).is_ok());
    assert_eq!(model_reasoning_efforts_for_profile(dir.path(), &[custom])["vendor/custom-model"], vec!["low", "high"]);
}

fn provider(id: &str, key: &str) -> CodexAccount {
    CodexAccount::new_api_key(id.into(), "synthetic@example.invalid".into(), key.into(),
        CodexApiProviderMode::Custom, Some("https://relay.example/v1".into()), None, None,
        vec!["vendor/custom-model".into(), "vendor/second-model".into()])
}

#[test]
fn manual_provider_reasoning_scoped_save_preserves_inactive_subscription_profile_and_other_provider() {
    let dir = tempfile::tempdir().unwrap();
    let account = provider("provider-a", "synthetic-key-a");
    let other = provider("provider-b", "synthetic-key-b");
    let protected = [
        ("config.toml", "model = \"gpt-6-astra\"\nmodel_reasoning_effort = \"ultra\"\n"),
        ("auth.json", "{\"synthetic\":\"original-account\"}"),
        (CODEX_MANAGED_MODEL_CATALOG_FILE, "{\"models\":[{\"slug\":\"gpt-6-astra\"}]}"),
        (CODEX_EXPERIMENTAL_MODEL_POLICY_FILE, "enabled\n"),
        (CODEX_EXPERIMENTAL_MODEL_CONFIG_FILE, "{\"version\":4,\"models\":[{\"model_id\":\"gpt-6-astra\",\"display_name\":\"GPT\"}]}"),
    ];
    for (name, value) in protected { fs::write(dir.path().join(name), value).unwrap(); }
    let view = read_provider_model_quick_config(dir.path(), &account).unwrap();
    assert!(!view.experimental_model_catalog_enabled);
    assert_eq!(view.experimental_model_catalog_models.iter().map(|m|m.model_id.as_str()).collect::<Vec<_>>(), vec!["vendor/custom-model", "vendor/second-model"]);
    let manual = declaration(Some(vec!["low", "high"]), Some("high"));
    save_provider_model_quick_config(dir.path(), &account, true, vec![manual.clone()], Some(manual.model_id.clone())).unwrap();
    assert_eq!(provider_reasoning_override(dir.path(), &account, &manual.model_id).unwrap().unwrap().reasoning_efforts, manual.reasoning_efforts);
    assert!(provider_reasoning_override(dir.path(), &other, &manual.model_id).unwrap().is_none());
    assert_eq!(provider_default_model_override(dir.path(), &account).unwrap().as_deref(), Some("vendor/custom-model"));
    assert!(provider_default_model_override(dir.path(), &other).unwrap().is_none());
    let mut reduced = account.clone(); reduced.api_model_catalog = vec!["vendor/second-model".into()];
    let reduced_catalog = json!({"models":[{"slug":"vendor/second-model","supported_reasoning_levels":[]}]});
    assert_eq!(decorate_managed_model_catalog_for_provider(dir.path(), &reduced, &reduced_catalog.to_string()).unwrap(), reduced_catalog.to_string());
    assert!(provider_reasoning_override(dir.path(), &reduced, "vendor/custom-model").unwrap().is_none());
    let mut moved = account.clone(); moved.api_base_url = Some("https://another.example/v1".into());
    assert!(provider_reasoning_override(dir.path(), &moved, &manual.model_id).unwrap().is_none());
    moved = account.clone(); moved.openai_api_key = Some("synthetic-new-key".into());
    assert!(provider_reasoning_override(dir.path(), &moved, &manual.model_id).unwrap().is_none());
    for (name, value) in protected { assert_eq!(fs::read_to_string(dir.path().join(name)).unwrap(), value, "{name}"); }
    let cache = fs::read_to_string(provider_model_policy_path(dir.path(), &account).unwrap()).unwrap();
    assert!(!cache.contains("synthetic-key-a") && !cache.contains("relay.example"));
}

#[test]
fn manual_provider_reasoning_scoped_unknown_default_and_none_minimal_are_effective() {
    let dir = tempfile::tempdir().unwrap();
    let account = provider("provider", "synthetic-key");
    let default_only = declaration(None, Some("ultra"));
    save_provider_model_quick_config(dir.path(), &account, true, vec![default_only], None).unwrap();
    let parsed = provider_reasoning_override(dir.path(), &account, "vendor/custom-model").unwrap().unwrap();
    assert_eq!(parsed.default_reasoning_effort.as_deref(), Some("ultra"));
    assert!(parsed.reasoning_efforts.is_none());
    let manual = declaration(Some(vec!["minimal", "none", "high"]), Some("none"));
    save_provider_model_quick_config(dir.path(), &account, true, vec![manual], None).unwrap();
    let input = json!({"models":[{"slug":"vendor/custom-model","supported_reasoning_levels":[],"default_reasoning_level":null,"use_responses_lite":true}]});
    let rendered:serde_json::Value=serde_json::from_str(&decorate_managed_model_catalog_for_provider(dir.path(), &account, &input.to_string()).unwrap()).unwrap();
    assert_eq!(rendered["models"][0]["supported_reasoning_levels"].as_array().unwrap().iter().map(|x|x["effort"].as_str().unwrap()).collect::<Vec<_>>(), vec!["none", "minimal", "high"]);
    assert_eq!(rendered["models"][0]["default_reasoning_level"], "none");
    assert_eq!(rendered["models"][0]["cockpit_reasoning_known"], true);
    assert_eq!(rendered["models"][0]["use_responses_lite"], true);
    let mut outside = declaration(Some(vec!["high"]), None); outside.model_id = "not-granted".into();
    assert!(save_provider_model_quick_config(dir.path(), &account, true, vec![outside], None).is_err());
    save_provider_model_quick_config(dir.path(), &account, false, vec![declaration(Some(vec![]), None)], None).unwrap();
    assert!(provider_reasoning_override(dir.path(), &account, "vendor/custom-model").unwrap().is_none());
    assert_eq!(decorate_managed_model_catalog_for_provider(dir.path(), &account, &input.to_string()).unwrap(), input.to_string());
}
