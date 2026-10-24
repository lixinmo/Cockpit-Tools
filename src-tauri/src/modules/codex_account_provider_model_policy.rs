// Provider policies are independent from the instance's subscription-model
// policy. Editing an inactive provider must not publish into the active profile.
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ProviderModelPolicy {
    version: u32,
    enabled: bool,
    models: Vec<CodexExperimentalModelDefinition>,
    #[serde(default)]
    default_model_id: Option<String>,
}

fn provider_model_policy_path(base_dir: &Path, account: &CodexAccount) -> Result<PathBuf, String> {
    if !account.is_api_key_auth() {
        return Err("PROVIDER_MODEL_POLICY_ACCOUNT_INVALID".into());
    }
    let mut hash = Sha256::new();
    for part in [account.id.as_str(), account.api_base_url.as_deref().unwrap_or("").trim().trim_end_matches('/'), account.openai_api_key.as_deref().unwrap_or("").trim()] {
        hash.update(part.as_bytes());
        hash.update([0]);
    }
    Ok(base_dir.join(".cockpit-provider-model-config").join(format!("{:x}.json", hash.finalize())))
}

fn read_provider_model_policy(base_dir: &Path, account: &CodexAccount) -> Result<Option<ProviderModelPolicy>, String> {
    let path = provider_model_policy_path(base_dir, account)?;
    let raw = match fs::read(&path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err("PROVIDER_MODEL_POLICY_READ_FAILED".into()),
    };
    if raw.len() > 4 * 1024 * 1024 {
        return Err("MODEL_CONFIG_TOO_LARGE".into());
    }
    let policy: ProviderModelPolicy = serde_json::from_slice(&raw)
        .map_err(|_| "PROVIDER_MODEL_POLICY_INVALID".to_string())?;
    if policy.version != 1 {
        return Err("PROVIDER_MODEL_POLICY_VERSION_UNSUPPORTED".into());
    }
    Ok(Some(policy))
}

fn provider_policy_models(account: &CodexAccount) -> Vec<CodexExperimentalModelDefinition> {
    let mut seen = HashSet::new();
    account.api_model_catalog.iter().filter_map(|id| {
        let id = id.trim();
        if !is_valid_model_catalog_id(id) || !seen.insert(id.to_ascii_lowercase()) { return None; }
        Some(CodexExperimentalModelDefinition {
            model_id: id.into(), display_name: provider_model_display_name(id),
            reasoning_efforts: None, default_reasoning_effort: None,
            context_window: None, auto_compact_token_limit: None,
        })
    }).collect()
}

fn normalize_provider_policy_models(account: &CodexAccount, models: Vec<CodexExperimentalModelDefinition>) -> Result<Vec<CodexExperimentalModelDefinition>, String> {
    let allowed = provider_policy_models(account);
    let explicit_defaults = models.iter().map(|model| model.default_reasoning_effort.clone()).collect::<Vec<_>>();
    let mut normalized = normalize_experimental_model_definitions(models)?;
    for (model, original_default) in normalized.iter_mut().zip(explicit_defaults) {
        if original_default.is_none() { model.default_reasoning_effort = None; }
        let actual = allowed.iter().find(|allowed| allowed.model_id.eq_ignore_ascii_case(&model.model_id))
            .ok_or_else(|| "PROVIDER_MODEL_NOT_IN_SCOPE".to_string())?;
        model.model_id = actual.model_id.clone();
        if let Some(default) = model.default_reasoning_effort.as_ref() {
            let supported = model.reasoning_efforts.clone().unwrap_or_else(|| {
                crate::modules::codex_local_access::provider_model_reasoning_levels(account, &model.model_id)
            });
            if !supported.contains(default) {
                return Err("EXPERIMENTAL_MODEL_CATALOG_DEFAULT_REASONING_INVALID".into());
            }
        }
    }
    Ok(normalized)
}

pub(crate) fn read_provider_model_quick_config(base_dir: &Path, account: &CodexAccount) -> Result<CodexQuickConfig, String> {
    // Do not run subscription-catalog migrations while previewing another provider.
    let raw = fs::read_to_string(get_config_toml_path(base_dir)).unwrap_or_default();
    let doc = if raw.trim().is_empty() { Document::new() } else {
        crate::modules::codex_config_format::read_codex_config_doc_from_str(&raw)
            .map_err(|_| "PROVIDER_MODEL_POLICY_PROFILE_INVALID".to_string())?
    };
    let window = read_top_level_int_from_doc(&doc, CODEX_CONFIG_MODEL_CONTEXT_WINDOW_KEY);
    let compact = read_top_level_int_from_doc(&doc, CODEX_CONFIG_MODEL_AUTO_COMPACT_TOKEN_LIMIT_KEY).filter(|value| *value > 0);
    let mut view = CodexQuickConfig {
        context_window_1m: window == Some(CODEX_CONTEXT_WINDOW_1M_VALUE),
        auto_compact_token_limit: compact.unwrap_or(CODEX_AUTO_COMPACT_DEFAULT_LIMIT),
        detected_model_context_window: window, detected_auto_compact_token_limit: compact,
        context_management_experimental_mode: read_context_management_experimental_mode_from_doc(&doc),
        experimental_model_catalog_enabled: false, experimental_model_catalog_available: true,
        experimental_model_catalog_unavailable_reason: None, experimental_model_catalog_conflict: None,
        experimental_model_catalog_models: Vec::new(), experimental_model_catalog_default_model_id: None,
        experimental_model_catalog_reset_models: Vec::new(), experimental_model_catalog_reset_default_model_id: None,
    };
    let defaults = provider_policy_models(account);
    let policy = read_provider_model_policy(base_dir, account)?;
    view.experimental_model_catalog_enabled = policy.as_ref().is_some_and(|policy| policy.enabled);
    view.experimental_model_catalog_available = !defaults.is_empty();
    view.experimental_model_catalog_unavailable_reason = None;
    view.experimental_model_catalog_conflict = None;
    view.experimental_model_catalog_models = defaults.iter().map(|model| {
        policy.as_ref().and_then(|policy| policy.models.iter().find(|saved| saved.model_id.eq_ignore_ascii_case(&model.model_id)))
            .cloned().unwrap_or_else(|| model.clone())
    }).collect();
    view.experimental_model_catalog_default_model_id = policy.as_ref().and_then(|policy| policy.default_model_id.clone())
        .filter(|id| defaults.iter().any(|model| model.model_id.eq_ignore_ascii_case(id)));
    view.experimental_model_catalog_reset_models = defaults;
    view.experimental_model_catalog_reset_default_model_id = None;
    Ok(view)
}

pub(crate) fn save_provider_model_quick_config(
    base_dir: &Path,
    account: &CodexAccount,
    enabled: bool,
    models: Vec<CodexExperimentalModelDefinition>,
    default_model_id: Option<String>,
) -> Result<CodexQuickConfig, String> {
    let _lease = try_acquire_profile_mutation_lease(base_dir, "provider-model-settings-save")?;
    let previous = read_provider_model_policy(base_dir, account)?;
    let models = if enabled { normalize_provider_policy_models(account, models)? }
        else { previous.as_ref().map(|policy| policy.models.clone()).unwrap_or_else(|| provider_policy_models(account)) };
    let default_model_id = if enabled { default_model_id } else { previous.and_then(|policy|policy.default_model_id) };
    let default_model_id = default_model_id.filter(|id| !id.trim().is_empty());
    if default_model_id.as_ref().is_some_and(|id| !models.iter().any(|model| model.model_id.eq_ignore_ascii_case(id))) {
        return Err("PROVIDER_MODEL_NOT_IN_SCOPE".into());
    }
    let path = provider_model_policy_path(base_dir, account)?;
    if let Some(parent) = path.parent() { fs::create_dir_all(parent).map_err(|_| "PROVIDER_MODEL_POLICY_WRITE_FAILED".to_string())?; }
    let policy = ProviderModelPolicy { version:1, enabled, models, default_model_id };
    let content = serde_json::to_string_pretty(&policy).map_err(|_| "PROVIDER_MODEL_POLICY_INVALID".to_string())?;
    write_string_atomic(&path, &content).map_err(|_| "PROVIDER_MODEL_POLICY_WRITE_FAILED".to_string())?;
    read_provider_model_quick_config(base_dir, account)
}

pub(crate) fn provider_reasoning_override(base_dir: &Path, account: &CodexAccount, model: &str) -> Result<Option<CodexExperimentalModelDefinition>, String> {
    // A dormant rule cannot reject a route authorized by a newer gateway
    // inventory. Match the catalog decorator's treatment of removed entries.
    if !provider_policy_models(account).iter().any(|entry| entry.model_id.eq_ignore_ascii_case(model)) {
        return Ok(None);
    }
    let Some(policy) = read_provider_model_policy(base_dir, account)?.filter(|policy| policy.enabled) else { return Ok(None); };
    let Some(definition) = policy.models.into_iter().find(|definition| definition.model_id.eq_ignore_ascii_case(model)
        && (definition.reasoning_efforts.is_some() || definition.default_reasoning_effort.is_some())) else { return Ok(None); };
    // Do not manufacture a default while reading. The inherited provider default
    // must remain the same in runtime requests and the rendered catalog.
    let original_default = definition.default_reasoning_effort.clone();
    let mut normalized = normalize_provider_policy_models(account, vec![definition])?.remove(0);
    if original_default.is_none() { normalized.default_reasoning_effort = None; }
    Ok(Some(normalized))
}

pub(crate) fn provider_default_model_override(base_dir: &Path, account: &CodexAccount) -> Result<Option<String>, String> {
    Ok(read_provider_model_policy(base_dir, account)?.filter(|policy| policy.enabled).and_then(|policy| policy.default_model_id))
}

pub(crate) fn decorate_managed_model_catalog_for_provider(base_dir: &Path, account: &CodexAccount, catalog_json: &str) -> Result<String, String> {
    let Some(policy) = read_provider_model_policy(base_dir, account)?.filter(|policy| policy.enabled) else { return Ok(catalog_json.to_owned()); };
    // Keep dormant rules on disk when the upstream inventory changes, but do
    // not let an absent model block or expand the currently authorized catalog.
    let allowed = provider_policy_models(account);
    let active = policy.models.into_iter().filter(|model| allowed.iter()
        .any(|entry| entry.model_id.eq_ignore_ascii_case(&model.model_id))).collect::<Vec<_>>();
    if active.is_empty() { return Ok(catalog_json.to_owned()); }
    let definitions = normalize_provider_policy_models(account, active)?;
    let mut catalog: serde_json::Value = serde_json::from_str(catalog_json).map_err(|_| "PROVIDER_MODEL_CATALOG_INVALID".to_string())?;
    apply_model_context_config_to_catalog(&mut catalog, &definitions);
    apply_model_reasoning_config_to_catalog(&mut catalog, &definitions)?;
    serde_json::to_string_pretty(&catalog).map_err(|_| "PROVIDER_MODEL_CATALOG_INVALID".into())
}
