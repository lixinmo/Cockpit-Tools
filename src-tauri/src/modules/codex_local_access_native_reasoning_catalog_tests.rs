use super::*;

fn fixture() -> (
    CodexLocalAccessCollection,
    CodexLocalAccessApiKey,
    Vec<CodexAccount>,
) {
    let mut collection: CodexLocalAccessCollection = serde_json::from_value(json!({
        "enabled":true,"gatewayMode":"native","port":19001,"apiKey":"synthetic-key",
        "accountIds":["a","b","outside"],"createdAt":0,"updatedAt":0
    }))
    .unwrap();
    let key: CodexLocalAccessApiKey = serde_json::from_value(json!({
        "id":"provider","label":"fixture","key":"synthetic-key","enabled":true,
        "inheritAccountPool":false,"accountIds":["a","b"],"modelPrefix":"team",
        "allowedModels":["shared","legacy"],
        "providerGateway":{"baseUrl":"https://gateway.example.invalid/v1","apiKey":"synthetic-key",
            "upstreamModel":"shared","upstreamModels":["shared","private"],"wireApi":"chat_completions",
            "modelBindings":[{"clientModel":"legacy","upstreamModel":"shared"}]}
    })).unwrap();
    collection.api_keys = vec![key.clone()];
    let accounts = ["a", "b", "outside"]
        .into_iter()
        .map(|id| {
            let mut account = CodexAccount::new_api_key(
                id.into(),
                "fixture@example.invalid".into(),
                format!("synthetic-{id}"),
                CodexApiProviderMode::Custom,
                Some(format!("https://{id}.example.invalid/v1")),
                None,
                None,
                vec!["shared".into(), "private".into()],
            );
            account.api_wire_api = Some("chat_completions".into());
            account
        })
        .collect();
    (collection, key, accounts)
}

fn save_policy(
    profile: &Path,
    account: &CodexAccount,
    levels: &[&str],
    default: &str,
    context: i64,
) {
    codex_account::save_provider_model_quick_config(
        profile,
        account,
        true,
        vec![crate::models::codex::CodexExperimentalModelDefinition {
            model_id: "shared".into(),
            display_name: "Shared".into(),
            reasoning_efforts: Some(levels.iter().map(|value| (*value).into()).collect()),
            default_reasoning_effort: Some(default.into()),
            context_window: Some(context),
            auto_compact_token_limit: None,
        }],
        None,
    )
    .unwrap();
}

fn entry<'a>(catalog: &'a Value, slug: &str) -> &'a Value {
    catalog["models"]
        .as_array()
        .unwrap()
        .iter()
        .find(|model| model["slug"] == slug)
        .unwrap()
}

fn efforts(model: &Value) -> Vec<&str> {
    model["supported_reasoning_levels"]
        .as_array()
        .unwrap()
        .iter()
        .map(|level| level["effort"].as_str().unwrap())
        .collect()
}

#[test]
fn native_provider_reasoning_catalog_intersects_authorized_accounts_and_aliases_without_scope_growth(
) {
    let profile = tempfile::tempdir().unwrap();
    let (collection, mut key, accounts) = fixture();
    save_policy(
        profile.path(),
        &accounts[0],
        &["low", "high"],
        "low",
        120_000,
    );
    save_policy(
        profile.path(),
        &accounts[1],
        &["medium", "high"],
        "high",
        100_000,
    );
    save_policy(profile.path(), &accounts[2], &["ultra"], "ultra", 1_000);
    let mut catalog = native_catalog(&collection, Some(&key), &accounts).unwrap();
    let ids = catalog["models"]
        .as_array()
        .unwrap()
        .iter()
        .map(|model| model["slug"].clone())
        .collect::<Vec<_>>();
    assert_eq!(ids.len(), 2);
    native_decorate_provider_models(
        profile.path(),
        &collection,
        Some(&key),
        &accounts,
        &mut catalog,
    )
    .unwrap();
    assert_eq!(
        catalog["models"]
            .as_array()
            .unwrap()
            .iter()
            .map(|model| model["slug"].clone())
            .collect::<Vec<_>>(),
        ids
    );
    for slug in ["team/shared", "team/legacy"] {
        let model = entry(&catalog, slug);
        assert_eq!(efforts(model), ["high"]);
        assert_eq!(model["default_reasoning_level"], "high");
        assert_eq!(model["context_window"], 100_000);
        assert_eq!(model["auto_compact_token_limit"], 90_000);
        assert_eq!(model["use_responses_lite"], true);
        let public = catalog["data"]
            .as_array()
            .unwrap()
            .iter()
            .find(|model| model["id"] == slug)
            .unwrap();
        assert_eq!(public["model_info"]["upstream_id"], "shared");
        assert_eq!(
            public["model_info"]["supported_reasoning_efforts"],
            json!(["high"])
        );
        assert_eq!(public["context_length"], 100_000);
    }
    // The outside account's restrictive policy and private model grant cannot
    // influence this key. Narrowing the key to A reveals A's own declared levels.
    key.account_ids = vec!["a".into()];
    let mut narrowed = native_catalog(&collection, Some(&key), &accounts).unwrap();
    native_decorate_provider_models(
        profile.path(),
        &collection,
        Some(&key),
        &accounts,
        &mut narrowed,
    )
    .unwrap();
    assert_eq!(efforts(entry(&narrowed, "team/legacy")), ["low", "high"]);
    assert_eq!(entry(&narrowed, "team/legacy")["context_window"], 120_000);
}

#[tokio::test]
async fn native_provider_reasoning_catalog_writer_and_reconciliation_keep_scoped_policy() {
    let profile = tempfile::tempdir().unwrap();
    let (mut collection, mut key, accounts) = fixture();
    key.account_ids = vec!["a".into()];
    collection.api_keys = vec![key.clone()];
    save_policy(
        profile.path(),
        &accounts[0],
        &["low", "high"],
        "high",
        180_000,
    );
    write_native_provider_profile_model_catalog(
        profile.path(),
        &collection,
        &key.key,
        &accounts[0],
    )
    .unwrap();
    let path = profile.path().join(CODEX_MANAGED_MODEL_CATALOG_FILE);
    let first: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(efforts(entry(&first, "team/legacy")), ["low", "high"]);
    assert_eq!(entry(&first, "team/legacy")["context_window"], 180_000);
    native_sync_profile_catalog_tool_capabilities(
        profile.path(),
        &collection,
        &key.key,
        &accounts[..1],
    )
    .await
    .unwrap();
    let refreshed: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(refreshed, first);
    codex_account::save_provider_model_quick_config(
        profile.path(),
        &accounts[0],
        false,
        vec![],
        None,
    )
    .unwrap();
    native_sync_profile_catalog_tool_capabilities(
        profile.path(),
        &collection,
        &key.key,
        &accounts[..1],
    )
    .await
    .unwrap();
    let inherited: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(
        efforts(entry(&inherited, "team/legacy")),
        ["low", "medium", "high", "xhigh", "max", "ultra"]
    );
    assert_eq!(
        entry(&inherited, "team/legacy")["cockpit_reasoning_known"],
        false
    );
}

#[test]
fn native_provider_reasoning_catalog_unknown_pool_member_does_not_claim_actual_thinking_support() {
    let profile = tempfile::tempdir().unwrap();
    let (collection, key, accounts) = fixture();
    save_policy(
        profile.path(),
        &accounts[0],
        &["low", "high"],
        "high",
        180_000,
    );
    let mut catalog = native_catalog(&collection, Some(&key), &accounts).unwrap();
    native_decorate_provider_models(
        profile.path(),
        &collection,
        Some(&key),
        &accounts,
        &mut catalog,
    )
    .unwrap();
    assert_eq!(efforts(entry(&catalog, "team/shared")), ["low", "high"]);
    assert_eq!(
        entry(&catalog, "team/shared")["cockpit_reasoning_known"],
        false
    );
    let public = catalog["data"]
        .as_array()
        .unwrap()
        .iter()
        .find(|model| model["id"] == "team/shared")
        .unwrap();
    assert!(public["model_info"]
        .get("supported_reasoning_efforts")
        .is_none());
    assert!(public["model_info"]
        .get("default_reasoning_effort")
        .is_none());
    assert_eq!(
        crate::modules::codex_provider_metadata::parse_metadata(public)
            .unwrap()
            .reasoning_efforts,
        None
    );
    assert!(public.get("context_length").is_none());
    assert!(!public["capabilities"]
        .as_array()
        .unwrap()
        .contains(&json!("thinking")));
}

#[test]
fn native_provider_reasoning_catalog_global_alias_then_provider_fields_have_stable_precedence() {
    let profile = tempfile::tempdir().unwrap();
    let (collection, mut key, accounts) = fixture();
    key.account_ids = vec!["a".into()];
    fs::write(
        profile
            .path()
            .join(".cockpit-experimental-model-catalog-enabled"),
        "enabled\n",
    )
    .unwrap();
    fs::write(profile.path().join(".cockpit-experimental-model-catalog-config.json"), json!({
        "version":4,"models":[
            {"model_id":"shared","display_name":"Shared","reasoning_efforts":["low","high"],"default_reasoning_effort":"low"},
            {"model_id":"team/legacy","display_name":"Alias","reasoning_efforts":["medium","high"],"default_reasoning_effort":"medium"}
        ]
    }).to_string()).unwrap();
    let mut catalog = native_catalog(&collection, Some(&key), &accounts).unwrap();
    native_decorate_provider_models(
        profile.path(),
        &collection,
        Some(&key),
        &accounts,
        &mut catalog,
    )
    .unwrap();
    assert_eq!(efforts(entry(&catalog, "team/shared")), ["low", "high"]);
    assert_eq!(efforts(entry(&catalog, "team/legacy")), ["medium", "high"]);
    // A scoped default-only rule changes the default, not inherited explicit
    // levels. The selected provider's policy has priority over the global alias.
    codex_account::save_provider_model_quick_config(
        profile.path(),
        &accounts[0],
        true,
        vec![crate::models::codex::CodexExperimentalModelDefinition {
            model_id: "shared".into(),
            display_name: "Shared".into(),
            reasoning_efforts: None,
            default_reasoning_effort: Some("high".into()),
            context_window: None,
            auto_compact_token_limit: None,
        }],
        None,
    )
    .unwrap();
    native_decorate_provider_models(
        profile.path(),
        &collection,
        Some(&key),
        &accounts,
        &mut catalog,
    )
    .unwrap();
    assert_eq!(efforts(entry(&catalog, "team/legacy")), ["medium", "high"]);
    assert_eq!(
        entry(&catalog, "team/legacy")["default_reasoning_level"],
        "high"
    );
    save_policy(profile.path(), &accounts[0], &["high"], "high", 150_000);
    native_decorate_provider_models(
        profile.path(),
        &collection,
        Some(&key),
        &accounts,
        &mut catalog,
    )
    .unwrap();
    assert_eq!(efforts(entry(&catalog, "team/legacy")), ["high"]);
}
