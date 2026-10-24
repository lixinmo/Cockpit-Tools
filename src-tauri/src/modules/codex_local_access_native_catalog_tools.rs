// One resolved metadata source for /models and managed on-disk catalogs.
fn native_provider_effort_levels(template: &Value) -> Option<Vec<String>> {
    if template["cockpit_reasoning_known"] == false { return None; }
    template.get("supported_reasoning_levels")?.as_array().map(|levels| {
        levels.iter().filter_map(|level|
            level.as_str().or_else(|| level["effort"].as_str())
        ).map(str::to_owned).collect()
    })
}

pub(crate) fn provider_model_reasoning_levels(account: &CodexAccount, model: &str) -> Vec<String> {
    native_declared_provider_model_metadata(account, model)
        .as_ref().and_then(native_provider_effort_levels)
        .unwrap_or_else(|| ["low", "medium", "high", "xhigh", "max", "ultra"]
            .iter().map(|effort| (*effort).to_owned()).collect())
}

fn native_declared_provider_model_metadata(account: &CodexAccount, model: &str) -> Option<Value> {
    let mut declared = native_documented_provider_model_metadata(account, model);
    if let Some(metadata) = crate::modules::codex_provider_metadata::lookup(account, model) {
        let template = declared.get_or_insert_with(|| {
            let mut fallback = codex_protocol::build_provider_client_model(model, 0);
            fallback["cockpit_reasoning_known"] = json!(false);
            fallback["cockpit_context_known"] = json!(false);
            fallback
        });
        if let Some(window) = metadata.context_window {
            template["cockpit_context_known"] = json!(true);
            template["context_window"] = json!(window);
            template["max_context_window"] = json!(window);
            template["auto_compact_token_limit"] =
                json!(codex_protocol::derived_auto_compact_token_limit(window));
        }
        if let Some(levels) = metadata.reasoning_efforts {
            // `auto` is a provider control, not a Codex selectable enum value.
            let levels = levels.into_iter().filter(|level| level != "auto").collect::<Vec<_>>();
            let default = metadata.default_reasoning_effort
                .filter(|default| levels.contains(default))
                .or_else(|| levels.iter().find(|level| *level == "medium").cloned())
                .or_else(|| levels.first().cloned());
            template["supported_reasoning_levels"] = json!(levels.iter().map(|level|
                json!({"effort":level,"description":level})
            ).collect::<Vec<_>>());
            template["default_reasoning_level"] = json!(default);
            template["cockpit_reasoning_known"] = json!(true);
        } else if let Some(default) = metadata.default_reasoning_effort {
            if template["supported_reasoning_levels"].as_array().is_some_and(|levels|
                levels.iter().any(|level| level["effort"].as_str() == Some(default.as_str()))) {
                template["default_reasoning_level"] = json!(default);
            }
        }
    }
    declared
}

fn native_documented_provider_model_metadata(account: &CodexAccount, model: &str) -> Option<Value> {
    if let Some(template) = account_model_template(account, model) {
        return Some(template);
    }
    // Model limits are provider data, never inferred from a display alias.
    // Source: https://platform.stepfun.com/docs/zh/guides/developer/integrations/codex
    // Only reuse documented model limits/efforts. Execution capabilities below
    // come from this gateway's adapter, not the provider's Responses example.
    let url = reqwest::Url::parse(account.api_base_url.as_deref()?).ok()?;
    if !account.is_api_key_auth()
        || url.scheme() != "https"
        || url.port_or_known_default() != Some(443)
        || !matches!(url.host_str(), Some("api.stepfun.com" | "api.stepfun.ai"))
        || !matches!(url.path().trim_end_matches('/'), "/v1" | "/step_plan/v1")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    let window = match model {
        "step-5-preview" => 1_000_000,
        "step-3.7-flash" => 256_000,
        "step-3.5-flash-2603" => 256_000,
        _ => return None,
    };
    let mut template = codex_protocol::build_provider_client_model(model, 0);
    template["context_window"] = json!(window);
    template["max_context_window"] = json!(window);
    template["auto_compact_token_limit"] =
        json!(codex_protocol::derived_auto_compact_token_limit(window));
    template["default_reasoning_level"] = json!("medium");
    template["cockpit_reasoning_known"] = json!(true);
    template["supported_reasoning_levels"] = json!([
        {"effort":"low","description":"Low"},
        {"effort":"medium","description":"Medium"},
        {"effort":"high","description":"High"}
    ]);
    // StepFun's Chat API reference explicitly gives the 2603 release two levels.
    if model == "step-3.5-flash-2603" {
        template["default_reasoning_level"] = json!("high");
        template["supported_reasoning_levels"] = json!([
            {"effort":"low","description":"Low"},
            {"effort":"high","description":"High"}
        ]);
    }
    Some(template)
}

fn native_provider_catalog_base(
    exposed: &str,
    plan: &NativeModelRoutePlan,
    eligible: &[&CodexAccount],
    index: usize,
) -> Option<Value> {
    if eligible.is_empty() {
        return None;
    }
    let account = eligible[0];
    let first_upstream = native_plan_account_model(plan, account)?;
    let upstream = if eligible.iter().all(|account| {
        native_plan_account_model(plan, account).as_deref() == Some(first_upstream.as_str())
    }) {
        first_upstream
    } else {
        exposed.to_owned()
    };
    // A display alias is not a model capability source. Provider metadata is
    // looked up using the actual upstream identity, after scoped resolution.
    let declared_for = |account: &CodexAccount| {
        let actual = native_plan_account_model(plan, account)?;
        native_declared_provider_model_metadata(account, &actual).or_else(|| {
            (native_wire_api(account).ok() == Some(native_contract::WireApi::Responses))
                .then(|| codex_protocol::known_codex_model_template(&actual))
                .flatten()
        })
    };
    let declared = declared_for(account).filter(|first| {
        eligible
            .iter()
            .all(|account| declared_for(account).as_ref() == Some(first))
    });
    let mut model =
        declared.unwrap_or_else(|| codex_protocol::build_provider_client_model(&upstream, index));
    model["slug"] = json!(exposed);
    model["priority"] = json!(1000 + index);
    if exposed != upstream {
        model["display_name"] = json!(upstream);
        model["description"] = json!(upstream);
    }
    Some(model)
}

fn native_apply_catalog_tool_capabilities(
    model: &mut Value,
    collection: &CodexLocalAccessCollection,
    key: Option<&CodexLocalAccessApiKey>,
    plan: &NativeModelRoutePlan,
    accounts: &[CodexAccount],
) {
    // Use every eligible account, not the display name or its position in a list.
    let eligible = accounts
        .iter()
        .filter(|account| native_model_visible(collection, key, plan, account))
        .collect::<Vec<_>>();
    let legacy_alias = model["slug"].as_str().is_some_and(|slug| {
        let prefix = key
            .and_then(|key| key.model_prefix.as_deref())
            .filter(|prefix| !prefix.is_empty())
            .map(|prefix| format!("{}/", prefix.trim_end_matches('/')));
        let id = prefix
            .as_deref()
            .and_then(|prefix| slug.strip_prefix(prefix))
            .unwrap_or(slug);
        key.and_then(|key| {
            key.provider_model_bindings.as_ref().or_else(|| {
                key.provider_gateway
                    .as_ref()
                    .and_then(|gateway| gateway.model_bindings.as_ref())
            })
        })
        .is_some_and(|bindings| {
            bindings.iter().any(|binding| {
                binding.client_model.eq_ignore_ascii_case(id)
                    && binding
                        .upstream_model
                        .as_deref()
                        .is_some_and(|target| !target.eq_ignore_ascii_case(id))
            })
        })
    });
    if let Some(exposed) = model["slug"].as_str().map(str::to_owned) {
        if let Some(base) = native_provider_catalog_base(&exposed, plan, &eligible, 0) {
            // Keep explicit profile display names/context limits, but never
            // inherit execution modes or GPT identity from a compatibility alias.
            for field in [
                "tool_mode",
                "use_responses_lite",
                "model_messages",
                "base_instructions",
                "apply_patch_tool_type",
                "shell_type",
                "web_search_tool_type",
                "supports_search_tool",
                "supports_parallel_tool_calls",
                "support_verbosity",
                "default_verbosity",
                "supported_reasoning_levels",
                "default_reasoning_level",
                "supports_reasoning_summary_parameter",
                "supports_reasoning_summaries",
                "include_skills",
                "include_skills_usage_instructions",
                "include_apps_usage_instructions",
                "include_plugin_usage_instructions",
                "available_in_plans",
                "service_tiers",
                "default_service_tier",
                "additional_speed_tiers",
                "auto_review_model_override",
                "model_specialty",
            ] {
                if let Some(value) = base.get(field) {
                    model[field] = value.clone();
                }
            }
        }
    }
    if legacy_alias {
        model["visibility"] = json!("hide");
    }
    if model["visibility"] != "hide" || legacy_alias {
        model["multi_agent_version"] = json!("v2");
    }
    // Client discovery now has a complete Chat adapter. Explicit per-model
    // refusals still win; this flag never declares hosted web search or MCP.
    if !eligible.is_empty() {
        let discovery = eligible
            .iter()
            .map(|account| {
                native_capability_support(
                    collection,
                    account,
                    &native_plan_account_model(plan, account).unwrap_or_default(),
                    "tool_search_client",
                )
                .or_else(|| {
                    (native_is_chatgpt_plan_account(account)
                        || codex_account::is_grok_upstream_provider(account))
                    .then_some(CodexLocalAccessCapabilitySupport::Supported)
                })
            })
            .collect::<Vec<_>>();
        if discovery
            .iter()
            .any(|value| *value == Some(CodexLocalAccessCapabilitySupport::Unsupported))
            || eligible
                .iter()
                .any(|account| native_wire_api(account).is_err())
        {
            model["supports_search_tool"] = json!(false);
        } else if discovery
            .iter()
            .all(|value| *value == Some(CodexLocalAccessCapabilitySupport::Supported))
        {
            model["supports_search_tool"] = json!(true);
        }
    }
    if !eligible.is_empty()
        && eligible
            .iter()
            .all(|account| matches!(native_wire_api(account), Ok(native_contract::WireApi::Chat)))
    {
        // Chat uses the client-tool bridge, including declarations in
        // input.additional_tools. Negotiate that protocol for raw provider IDs
        // as well as official model shells; the generic unknown-model template
        // otherwise asks the client to use hosted Responses tools. This is only
        // a catalog declaration: explicit hosted tools still face preflight.
        // Mixed pools retain hosted declarations for eligible Responses routes.
        model["use_responses_lite"] = json!(true);
    }
}

fn native_apply_profile_catalog_tool_capabilities(
    catalog: &mut Value,
    collection: &CodexLocalAccessCollection,
    key: Option<&CodexLocalAccessApiKey>,
    accounts: &[CodexAccount],
) -> Result<(), String> {
    let authoritative = native_catalog(collection, key, accounts).map_err(|error| error.message)?;
    if let Some(models) = catalog.get_mut("models").and_then(Value::as_array_mut) {
        for model in models {
            let Some(slug) = model["slug"].as_str() else {
                continue;
            };
            let Some(resolved) = authoritative["models"].as_array().and_then(|models| {
                models
                    .iter()
                    .find(|entry| entry["slug"].as_str() == Some(slug))
            }) else {
                continue;
            };
            // Retain only presentation/order chosen by the profile. Context,
            // reasoning and execution all come from the same resolved route as
            // /models. Explicit saved profile overrides are applied last by
            // the writer, never inferred from stale generated catalog values.
            let presentation = [
                "display_name",
                "description",
                "priority",
                "prefer_websockets",
            ]
            .into_iter()
            .filter_map(|field| model.get(field).map(|v| (field, v.clone())))
            .collect::<Vec<_>>();
            *model = resolved.clone();
            for (field, value) in presentation {
                model[field] = value;
            }
        }
    }
    Ok(())
}

fn write_native_provider_profile_model_catalog(
    profile: &Path,
    collection: &CodexLocalAccessCollection,
    api_key: &str,
    account: &CodexAccount,
) -> Result<(), String> {
    let key = collection
        .api_keys
        .iter()
        .find(|key| key.enabled && key.key.trim() == api_key.trim())
        .ok_or_else(|| "NATIVE_PROFILE_KEY_NOT_FOUND".to_string())?;
    let catalog = native_catalog(collection, Some(key), std::slice::from_ref(account))
        .map_err(|error| error.message)?;
    // /models has already enforced the key/account scope. A legacy GPT alias
    // is not permission to append an otherwise unavailable Reserve model.
    write_profile_client_models_with_native_policy(
        profile, None, catalog, false,
        Some((collection, Some(key), std::slice::from_ref(account))),
    )
}

fn native_profile_catalog_account_ids(
    collection: &CodexLocalAccessCollection,
    key: Option<&CodexLocalAccessApiKey>,
) -> Vec<String> {
    let mut ids = native_scoped_account_ids(collection, key);
    if let Some(routing) = key.and_then(|key| key.model_routing.as_ref()) {
        ids.extend(
            routing
                .routes
                .iter()
                .map(|route| route.provider_account_id.clone()),
        );
    }
    ids.sort();
    ids.dedup();
    ids
}

/// Runs after existing catalog/experimental writers. Reading credentials uses
/// the established bounded worker pool; this path does not start a listener or
/// call any upstream service. The actual file update stays on a blocking worker.
async fn native_sync_profile_catalog_tool_capabilities(
    profile: &Path,
    collection: &CodexLocalAccessCollection,
    api_key: &str,
    known_accounts: &[CodexAccount],
) -> Result<(), String> {
    if collection.gateway_mode != CodexLocalAccessGatewayMode::Native {
        return Ok(());
    }
    let key = collection
        .api_keys
        .iter()
        .find(|key| key.enabled && key.key.trim() == api_key.trim());
    let mut ids = native_profile_catalog_account_ids(collection, key);
    ids.retain(|id| !known_accounts.iter().any(|account| &account.id == id));
    let mut accounts = timeout(Duration::from_secs(30), native_load_accounts(ids))
        .await
        .map_err(|_| "NATIVE_CATALOG_ACCOUNTS_TIMEOUT".to_string())?
        .map_err(|error| error.message)?;
    accounts.extend_from_slice(known_accounts);
    let collection = collection.clone();
    let key = key.cloned();
    let path = profile.join(CODEX_MANAGED_MODEL_CATALOG_FILE);
    let profile = profile.to_path_buf();
    tokio::task::spawn_blocking(move || {
        if !path.exists() {
            return Ok(());
        }
        let mut changed = false;
        update_string_atomic(&path, |current| {
            let content = current.ok_or_else(|| "NATIVE_CATALOG_FILE_CHANGED".to_string())?;
            let mut catalog: Value =
                serde_json::from_str(content).map_err(|error| error.to_string())?;
            let original = catalog.clone();
            native_apply_profile_catalog_tool_capabilities(
                &mut catalog,
                &collection,
                key.as_ref(),
                &accounts,
            )?;
            let decorated = codex_account::decorate_managed_model_catalog_for_profile(
                &profile,
                &serde_json::to_string(&catalog).map_err(|error| error.to_string())?,
            )?;
            catalog = serde_json::from_str(&decorated).map_err(|error| error.to_string())?;
            native_decorate_provider_models(&profile, &collection, key.as_ref(), &accounts, &mut catalog)?;
            changed = catalog != original;
            if changed {
                serde_json::to_string_pretty(&catalog).map_err(|error| error.to_string())
            } else {
                Ok(content.to_owned())
            }
        })?;
        if changed {
            crate::modules::codex_managed_model_catalog_version::write_managed_catalog_meta(&path)?;
            invalidate_codex_model_cache(&profile)?;
        }
        Ok(())
    })
    .await
    .map_err(|_| "NATIVE_CATALOG_WRITE_FAILED".to_string())?
}

#[cfg(test)]
#[path = "codex_local_access_native_catalog_tools_tests.rs"]
mod native_catalog_tools_tests;

include!("codex_local_access_native_reasoning_catalog.rs");
