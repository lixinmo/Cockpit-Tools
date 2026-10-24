// Decorate already-authorized catalog entries with profile/provider settings.
// Policy files cannot create models, authorize an account, or change a route.
const NATIVE_PROVIDER_MODEL_FIELDS: &[&str] = &[
    "supported_reasoning_levels",
    "default_reasoning_level",
    "cockpit_reasoning_known",
    "context_window",
    "max_context_window",
    "auto_compact_token_limit",
    "cockpit_context_known",
];

fn native_copy_provider_model_fields(target: &mut Value, source: &Value) {
    for field in NATIVE_PROVIDER_MODEL_FIELDS {
        if let Some(value) = source.get(*field) {
            target[*field] = value.clone();
        }
    }
}

fn native_provider_catalog_candidate(account: &CodexAccount, actual: &str, model: &Value) -> Value {
    let documented = native_documented_provider_model_metadata(account, actual).or_else(|| {
        (native_wire_api(account).ok() == Some(native_contract::WireApi::Responses))
            .then(|| codex_protocol::known_codex_model_template(actual))
            .flatten()
    });
    let discovered = crate::modules::codex_provider_metadata::lookup(account, actual);
    let template = native_declared_provider_model_metadata(account, actual)
        .or_else(|| documented.clone())
        .unwrap_or_else(|| codex_protocol::build_provider_client_model(actual, 0));
    let mut candidate = model.clone();
    native_copy_provider_model_fields(&mut candidate, &template);
    candidate["slug"] = json!(actual);
    candidate["cockpit_reasoning_known"] = json!(template["cockpit_reasoning_known"] != false);
    candidate["cockpit_context_known"] = json!(
        documented
            .as_ref()
            .and_then(|value| value["context_window"].as_i64())
            .is_some_and(|v| v > 0)
            || discovered
                .as_ref()
                .and_then(|value| value.context_window)
                .is_some()
    );
    if let Some(window) = account
        .api_model_context_windows
        .iter()
        .find(|(model, window)| model.eq_ignore_ascii_case(actual) && **window > 0)
        .map(|(_, window)| *window)
    {
        candidate["context_window"] = json!(window);
        candidate["max_context_window"] = json!(window);
        candidate["auto_compact_token_limit"] =
            json!(codex_protocol::derived_auto_compact_token_limit(window));
        candidate["cockpit_context_known"] = json!(true);
    }
    candidate
}

fn native_catalog_policy_batch(
    profile: &Path,
    account: Option<&CodexAccount>,
    models: Vec<Value>,
) -> Result<Vec<Value>, String> {
    let expected = models.len();
    let json = json!({"models":models}).to_string();
    let decorated = if let Some(account) = account {
        codex_account::decorate_managed_model_catalog_for_provider(profile, account, &json)?
    } else {
        codex_account::decorate_managed_model_catalog_for_profile(profile, &json)?
    };
    let mut catalog: Value = serde_json::from_str(&decorated)
        .map_err(|_| "NATIVE_PROVIDER_POLICY_CATALOG_INVALID".to_string())?;
    let models = catalog["models"]
        .as_array_mut()
        .ok_or_else(|| "NATIVE_PROVIDER_POLICY_CATALOG_INVALID".to_string())?;
    if models.len() != expected {
        return Err("NATIVE_PROVIDER_POLICY_MODEL_SET_CHANGED".into());
    }
    Ok(std::mem::take(models))
}

fn native_intersect_provider_candidates(candidates: &[Value]) -> Value {
    let mut result = candidates[0].clone();
    let mut common = result["supported_reasoning_levels"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    common.retain(|level| {
        candidates.iter().skip(1).all(|candidate| {
            candidate["supported_reasoning_levels"]
                .as_array()
                .is_some_and(|levels| {
                    levels
                        .iter()
                        .any(|other| other["effort"] == level["effort"])
                })
        })
    });
    let same_default = candidates
        .iter()
        .all(|candidate| candidate["default_reasoning_level"] == result["default_reasoning_level"]);
    let default = same_default
        .then(|| result["default_reasoning_level"].clone())
        .filter(|default| common.iter().any(|level| level["effort"] == *default))
        .or_else(|| {
            common
                .iter()
                .find(|level| level["effort"] == "medium")
                .map(|level| level["effort"].clone())
        })
        .or_else(|| common.first().map(|level| level["effort"].clone()))
        .unwrap_or(Value::Null);
    result["supported_reasoning_levels"] = json!(common);
    result["default_reasoning_level"] = default;
    result["cockpit_reasoning_known"] = json!(candidates
        .iter()
        .all(|candidate| candidate["cockpit_reasoning_known"] != false));
    result["cockpit_context_known"] = json!(candidates
        .iter()
        .all(|candidate| candidate["cockpit_context_known"] == true));
    for field in [
        "context_window",
        "max_context_window",
        "auto_compact_token_limit",
    ] {
        if let Some(minimum) = candidates
            .iter()
            .filter_map(|candidate| candidate[field].as_i64().filter(|value| *value > 0))
            .min()
        {
            result[field] = json!(minimum);
        }
    }
    result
}

fn native_sync_public_provider_model_fields(catalog: &mut Value, resolved_ids: &HashSet<String>) {
    let models = catalog["models"].as_array().cloned().unwrap_or_default();
    for entry in catalog["data"].as_array_mut().into_iter().flatten() {
        if !entry["id"]
            .as_str()
            .is_some_and(|id| resolved_ids.contains(id))
        {
            continue;
        }
        let Some(model) = models.iter().find(|model| model["slug"] == entry["id"]) else {
            continue;
        };
        let known = model["cockpit_reasoning_known"] != false
            && model["supported_reasoning_levels"].is_array();
        let levels = if known {
            model["supported_reasoning_levels"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|level| level["effort"].as_str())
                .map(str::to_owned)
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        if known {
            entry["model_info"]["supported_reasoning_efforts"] = json!(levels);
            entry["model_info"]["default_reasoning_effort"] =
                model["default_reasoning_level"].clone();
        } else if let Some(info) = entry["model_info"].as_object_mut() {
            info.remove("supported_reasoning_efforts");
            info.remove("default_reasoning_effort");
        }
        if let Some(capabilities) = entry["capabilities"].as_array_mut() {
            capabilities.retain(|capability| capability != "thinking");
            if levels.iter().any(|level| level != "none") {
                capabilities.push(json!("thinking"));
            }
        }
        if model["cockpit_context_known"] == true {
            if let Some(window) = model["context_window"]
                .as_i64()
                .filter(|window| *window > 0)
            {
                entry["context_length"] = json!(window);
                entry["model_info"]["context_length"] = json!(window);
            }
        } else {
            if let Some(entry) = entry.as_object_mut() {
                entry.remove("context_length");
            }
            if let Some(info) = entry["model_info"].as_object_mut() {
                info.remove("context_length");
            }
        }
    }
}

fn native_decorate_provider_models(
    profile: &Path,
    collection: &CodexLocalAccessCollection,
    key: Option<&CodexLocalAccessApiKey>,
    accounts: &[CodexAccount],
    catalog: &mut Value,
) -> Result<(), String> {
    let decorated =
        codex_account::decorate_managed_model_catalog_for_profile(profile, &catalog.to_string())?;
    let mut working: Value = serde_json::from_str(&decorated)
        .map_err(|_| "NATIVE_PROVIDER_POLICY_CATALOG_INVALID".to_string())?;
    let source_models = working["models"].as_array().cloned().unwrap_or_default();
    let plans = source_models
        .iter()
        .map(|model| {
            let exposed = model["slug"].as_str()?;
            let canonical = native_model_for_key(collection, key, exposed).ok()?;
            native_model_route_plan(collection, key, &canonical).ok()
        })
        .collect::<Vec<_>>();
    let mut resolved = vec![Vec::<Value>::new(); source_models.len()];
    // Batch the policy reads per account. Large catalogs must not reparse the
    // entire saved policy once for every model and alias.
    for account in accounts {
        let targets = source_models
            .iter()
            .enumerate()
            .filter_map(|(index, model)| {
                let plan = plans[index].as_ref()?;
                if !native_model_visible(collection, key, plan, account) {
                    return None;
                }
                Some((
                    index,
                    native_plan_account_model(plan, account)?,
                    model["slug"].as_str()?.to_owned(),
                ))
            })
            .collect::<Vec<_>>();
        if targets.is_empty() {
            continue;
        }
        let models = targets
            .iter()
            .map(|(index, actual, _)| {
                native_provider_catalog_candidate(account, actual, &source_models[*index])
            })
            .collect();
        let mut models = native_catalog_policy_batch(profile, None, models)?;
        for (model, (_, _, exposed)) in models.iter_mut().zip(&targets) {
            model["slug"] = json!(exposed);
        }
        models = native_catalog_policy_batch(profile, None, models)?;
        if account.is_api_key_auth() {
            for (model, (_, actual, _)) in models.iter_mut().zip(&targets) {
                model["slug"] = json!(actual);
            }
            models = native_catalog_policy_batch(profile, Some(account), models)?;
            for (model, (_, _, exposed)) in models.iter_mut().zip(&targets) {
                model["slug"] = json!(exposed);
            }
            models = native_catalog_policy_batch(profile, Some(account), models)?;
        }
        for (model, (index, _, _)) in models.into_iter().zip(targets) {
            resolved[index].push(model);
        }
    }
    let mut resolved_ids = HashSet::new();
    if let Some(models) = working["models"].as_array_mut() {
        for (model, candidates) in models.iter_mut().zip(resolved) {
            if !candidates.is_empty() {
                native_copy_provider_model_fields(
                    model,
                    &native_intersect_provider_candidates(&candidates),
                );
                if let Some(id) = model["slug"].as_str() {
                    resolved_ids.insert(id.to_owned());
                }
            }
        }
    }
    native_sync_public_provider_model_fields(&mut working, &resolved_ids);
    *catalog = working;
    Ok(())
}

#[cfg(test)]
#[path = "codex_local_access_native_reasoning_catalog_tests.rs"]
mod native_provider_reasoning_catalog_tests;
