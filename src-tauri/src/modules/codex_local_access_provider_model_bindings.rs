// Native provider model identity is independent of catalog ordering or GPT slots.
// This file is included in codex_local_access, alongside the legacy Go helpers.
use crate::models::codex_local_access::CodexLocalAccessProviderModelBinding;

const PROVIDER_BINDING_EVIDENCE_LIMIT: u64 = 64 * 1024 * 1024;

fn checked_provider_binding_id(value: &str) -> Result<String, String> {
    let value = value.trim();
    if value.is_empty() || value.len() > 1024 || value.chars().any(char::is_control) {
        return Err("PROVIDER_MODEL_BINDING_INVALID".into());
    }
    Ok(value.to_owned())
}

pub(crate) fn normalize_provider_model_bindings(
    bindings: &[CodexLocalAccessProviderModelBinding],
) -> Result<Vec<CodexLocalAccessProviderModelBinding>, String> {
    let mut result: Vec<CodexLocalAccessProviderModelBinding> = Vec::new();
    for binding in bindings {
        let client_model = checked_provider_binding_id(&binding.client_model)?;
        let upstream_model = binding
            .upstream_model
            .as_deref()
            .map(checked_provider_binding_id)
            .transpose()?;
        if let Some(previous) = result
            .iter()
            .find(|previous| previous.client_model.eq_ignore_ascii_case(&client_model))
        {
            let same = match (&previous.upstream_model, &upstream_model) {
                (Some(a), Some(b)) => a.eq_ignore_ascii_case(b),
                (None, None) => true,
                _ => false,
            };
            if !same {
                return Err("PROVIDER_MODEL_BINDING_CONFLICT".into());
            }
            continue;
        }
        result.push(CodexLocalAccessProviderModelBinding {
            client_model,
            upstream_model,
        });
    }
    Ok(result)
}

/// Current model membership controls availability, never the meaning of an old
/// alias. Removed targets stay in the persisted binding vector as tombstones.
pub(crate) fn stable_provider_model_slots(
    models: &[String],
    bindings: &[CodexLocalAccessProviderModelBinding],
) -> Result<Vec<ProviderGatewayModelSlot>, String> {
    let bindings = normalize_provider_model_bindings(bindings)?;
    let models = normalize_provider_gateway_models(models.iter().map(String::as_str).collect());
    let mut slots = Vec::new();
    for model in &models {
        checked_provider_binding_id(model)?;
        if bindings.iter().any(|binding| {
            binding.client_model.eq_ignore_ascii_case(model)
                && binding
                    .upstream_model
                    .as_deref()
                    .is_none_or(|target| !target.eq_ignore_ascii_case(model))
        }) {
            // A newly listed real GPT ID cannot take over an older public alias.
            return Err(
                "PROVIDER_MODEL_IDENTITY_CONFLICT: choose an unambiguous raw model ID".into(),
            );
        }
        slots.push(ProviderGatewayModelSlot {
            client_model: model.clone(),
            upstream_model: model.clone(),
        });
    }
    for binding in bindings {
        let Some(target) = binding.upstream_model else {
            continue;
        };
        let Some(current) = models
            .iter()
            .find(|model| model.eq_ignore_ascii_case(&target))
        else {
            continue;
        };
        if !slots.iter().any(|slot| {
            slot.client_model
                .eq_ignore_ascii_case(&binding.client_model)
        }) {
            slots.push(ProviderGatewayModelSlot {
                client_model: binding.client_model,
                upstream_model: current.clone(),
            });
        }
    }
    Ok(slots)
}

pub(crate) fn provider_gateway_model_slots_for_collection(
    collection: &CodexLocalAccessCollection,
    gateway: &CodexLocalAccessProviderGateway,
) -> Result<Vec<ProviderGatewayModelSlot>, String> {
    if collection.gateway_mode != CodexLocalAccessGatewayMode::Native {
        return Ok(provider_gateway_model_slots(&gateway.upstream_models));
    }
    let models = normalize_provider_gateway_models(
        gateway
            .upstream_models
            .iter()
            .map(String::as_str)
            .chain(std::iter::once(gateway.upstream_model.as_str()))
            .collect(),
    );
    // A legacy/manual descriptor without a binding snapshot exposes raw IDs;
    // absence never permits re-deriving aliases from its current list order.
    stable_provider_model_slots(&models, gateway.model_bindings.as_deref().unwrap_or(&[]))
}

fn provider_binding_evidence_text(path: &Path) -> Result<Option<String>, String> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err("PROVIDER_MODEL_BINDING_EVIDENCE_UNREADABLE".into()),
    };
    if !metadata.is_file()
        || metadata.file_type().is_symlink()
        || metadata.len() > PROVIDER_BINDING_EVIDENCE_LIMIT
    {
        return Err("PROVIDER_MODEL_BINDING_EVIDENCE_INVALID".into());
    }
    std::fs::read_to_string(path)
        .map(Some)
        .map_err(|_| "PROVIDER_MODEL_BINDING_EVIDENCE_UNREADABLE".into())
}

fn provider_binding_manifest_evidence(
    manifest: &Value,
    state: &ProviderGatewayProfileState,
    account_id: &str,
    grok: bool,
) -> Result<Option<Vec<CodexLocalAccessProviderModelBinding>>, String> {
    if state.api_key.trim().is_empty() {
        return Ok(None);
    }
    let Some(key) = manifest["apiKeys"].as_array().and_then(|keys| {
        keys.iter().find(|key| {
            key["key"]
                .as_str()
                .is_some_and(|key| key.trim() == state.api_key.trim())
                && key["id"] == provider_gateway_api_key_id(account_id)
                && key["enabled"] != false
                && key["accountIds"]
                    .as_array()
                    .is_some_and(|ids| ids.len() == 1 && ids[0].as_str() == Some(account_id))
        })
    }) else {
        return Ok(None);
    };
    let models = if key["providerGateway"].is_object() {
        let gateway: CodexLocalAccessProviderGateway =
            serde_json::from_value(key["providerGateway"].clone())
                .map_err(|_| "PROVIDER_MODEL_BINDING_EVIDENCE_INVALID")?;
        normalize_provider_gateway_models(
            gateway
                .upstream_models
                .iter()
                .map(String::as_str)
                .chain(std::iter::once(gateway.upstream_model.as_str()))
                .collect(),
        )
    } else if grok {
        let Some(snapshot) = manifest["accounts"].as_array().and_then(|accounts| {
            accounts.iter().find(|account| {
                account["id"] == account_id
                    && account["provider"] == codex_account::GROK_PROVIDER_ID
            })
        }) else {
            return Ok(None);
        };
        let models = snapshot["modelIds"]
            .as_array()
            .ok_or("PROVIDER_MODEL_BINDING_EVIDENCE_INVALID")?;
        normalize_provider_gateway_models(models.iter().filter_map(Value::as_str).collect())
    } else {
        return Ok(None);
    };
    if let Some(bindings) = key
        .get("providerModelBindings")
        .filter(|value| !value.is_null())
        .or_else(|| {
            key["providerGateway"]
                .get("modelBindings")
                .filter(|value| !value.is_null())
        })
    {
        let bindings =
            serde_json::from_value::<Vec<CodexLocalAccessProviderModelBinding>>(bindings.clone())
                .map_err(|_| "PROVIDER_MODEL_BINDING_EVIDENCE_INVALID")?;
        let bindings = bindings
            .into_iter()
            .filter(|binding| legacy_provider_alias_id(&binding.client_model))
            .collect::<Vec<_>>();
        return normalize_provider_model_bindings(&bindings).map(Some);
    }
    let aliases: Vec<CodexLocalAccessModelAlias> = serde_json::from_value(
        manifest
            .get("modelAliases")
            .cloned()
            .unwrap_or_else(|| json!([])),
    )
    .map_err(|_| "PROVIDER_MODEL_BINDING_EVIDENCE_INVALID")?;
    let occupied_aliases = aliases
        .iter()
        .filter(|alias| legacy_provider_alias_id(&alias.alias))
        .map(|alias| alias.alias.to_ascii_lowercase())
        .collect::<HashSet<_>>();
    // The manifest's own prior model list establishes scope. Never substitute
    // today's list when a model has since been removed or reordered.
    let mut bindings = aliases
        .into_iter()
        .filter(|alias| {
            legacy_provider_alias_id(&alias.alias)
                && models
                    .iter()
                    .any(|model| model.eq_ignore_ascii_case(&alias.source_model))
        })
        .map(|alias| CodexLocalAccessProviderModelBinding {
            client_model: alias.alias,
            upstream_model: Some(alias.source_model),
        })
        .collect::<Vec<_>>();
    for model in models
        .iter()
        .filter(|model| legacy_provider_alias_id(model))
    {
        if !occupied_aliases.contains(&model.to_ascii_lowercase())
            && !bindings
                .iter()
                .any(|binding| binding.client_model.eq_ignore_ascii_case(model))
        {
            // The prior scoped snapshot proves this was already a real raw ID,
            // rather than a name newly occupying a retired compatibility slot.
            bindings.push(CodexLocalAccessProviderModelBinding {
                client_model: model.clone(),
                upstream_model: Some(model.clone()),
            });
        }
    }
    Ok(Some(bindings))
}

fn provider_binding_runtime_evidence(
    profile: &Path,
    account: &CodexAccount,
    state: &ProviderGatewayProfileState,
) -> Result<Option<Vec<CodexLocalAccessProviderModelBinding>>, String> {
    let account_id = account.id.as_str();
    let Ok(runtimes) = provider_gateway_runtime_store().try_lock() else {
        return Ok(None);
    };
    let Some(collection) = runtimes
        .get(&provider_gateway_runtime_key(profile, account_id))
        .and_then(|runtime| runtime.collection.as_ref())
        .filter(|collection| {
            collection.api_key.trim() == state.api_key.trim() && state.port == Some(collection.port)
        })
    else {
        return Ok(None);
    };
    provider_binding_runtime_collection_evidence(collection, account, state)
}

fn provider_binding_runtime_collection_evidence(
    collection: &CodexLocalAccessCollection,
    account: &CodexAccount,
    state: &ProviderGatewayProfileState,
) -> Result<Option<Vec<CodexLocalAccessProviderModelBinding>>, String> {
    let account_id = account.id.as_str();
    if collection.api_key.trim() != state.api_key.trim() || state.port != Some(collection.port) {
        return Ok(None);
    }
    if codex_account::is_grok_upstream_provider(account) {
        let Some(key) = collection.api_keys.iter().find(|key| {
            key.enabled
                && key.id == provider_gateway_api_key_id(account_id)
                && key.key.trim() == state.api_key.trim()
                && key.account_ids.len() == 1
                && key.account_ids[0] == account_id
                && key.provider_gateway.is_none()
        }) else {
            return Ok(None);
        };
        // Legacy Grok runtimes did not retain their old model catalog. Never
        // pretend today's account catalog is that missing historical snapshot:
        // a newly added raw GPT ID could otherwise steal an older alias.
        return key
            .provider_model_bindings
            .as_ref()
            .map(|bindings| {
                let bindings = bindings
                    .iter()
                    .filter(|binding| legacy_provider_alias_id(&binding.client_model))
                    .cloned()
                    .collect::<Vec<_>>();
                normalize_provider_model_bindings(&bindings)
            })
            .transpose();
    }
    let manifest = json!({
        "apiKeys": collection.api_keys.iter().map(|key| json!({
            "id":key.id,"key":key.key,"enabled":key.enabled,
            "accountIds":key.account_ids,"providerGateway":key.provider_gateway,
            "providerModelBindings":key.provider_model_bindings,
        })).collect::<Vec<_>>(),
        "modelAliases": collection.model_aliases,
    });
    provider_binding_manifest_evidence(&manifest, state, account_id, false)
}

fn provider_binding_profile_owned(doc: &Document, state: &ProviderGatewayProfileState) -> bool {
    if state.api_key.trim().is_empty()
        || doc.get("model_provider").and_then(|item| item.as_str())
            != Some(CODEX_LOCAL_ACCESS_RUNTIME_PROVIDER_ID)
    {
        return false;
    }
    let Some(provider) = doc
        .get("model_providers")
        .and_then(|item| item.get(CODEX_LOCAL_ACCESS_RUNTIME_PROVIDER_ID))
    else {
        return false;
    };
    if provider
        .get("experimental_bearer_token")
        .and_then(|item| item.as_str())
        .is_none_or(|key| key.trim() != state.api_key.trim())
    {
        return false;
    }
    provider
        .get("base_url")
        .and_then(|item| item.as_str())
        .and_then(|base| Url::parse(base).ok())
        .is_some_and(|url| {
            url.scheme() == "http"
                && matches!(
                    url.host_str(),
                    Some("localhost" | "127.0.0.1" | "[::1]" | "::1")
                )
                && url.port_or_known_default() == state.port
                && url.path().trim_end_matches('/') == "/v1"
                && url.username().is_empty()
                && url.password().is_none()
                && url.query().is_none()
                && url.fragment().is_none()
        })
}

fn legacy_provider_alias_id(id: &str) -> bool {
    CODEX_PROVIDER_MODEL_SHELL_POOL
        .iter()
        .any(|shell| shell.eq_ignore_ascii_case(id))
}

fn provider_binding_catalog_evidence(
    profile: &Path,
    state: &ProviderGatewayProfileState,
    allowed_models: &[String],
) -> Result<Vec<CodexLocalAccessProviderModelBinding>, String> {
    let Some(config) = provider_binding_evidence_text(&profile_config_path(profile))? else {
        return Ok(Vec::new());
    };
    let Ok(doc) = crate::modules::codex_config_format::read_codex_config_doc_from_str(&config)
    else {
        return Ok(Vec::new());
    };
    if !provider_binding_profile_owned(&doc, state)
        || recorded_takeover_ownership_matches(profile, &doc)? == Some(false)
    {
        return Ok(Vec::new());
    }
    let Some(name) = doc.get("model_catalog_json").and_then(|item| item.as_str()) else {
        return Ok(Vec::new());
    };
    // Only the active, generated catalog is evidence. Never inspect backups,
    // arbitrary configured paths, or a different account's profile catalog.
    if !is_cockpit_managed_model_catalog_name(name)
        || Path::new(name).file_name().and_then(|name| name.to_str()) != Some(name)
    {
        return Ok(Vec::new());
    }
    let catalog_path = profile.join(name);
    let Some(content) = provider_binding_evidence_text(&catalog_path)? else {
        return Ok(Vec::new());
    };
    let catalog: Value =
        serde_json::from_str(&content).map_err(|_| "PROVIDER_MODEL_BINDING_EVIDENCE_INVALID")?;
    // Older provider writers changed the catalog after the takeover writer had
    // stamped its hash. A stale meta file is therefore not evidence of a user
    // edit. This one-time legacy path instead requires the saved endpoint/key,
    // current ownership marker/reference, a known compatibility ID, the exact
    // generated identity pair, and membership in the account's current scope.
    // Deleted targets require a real runtime/manifest snapshot, never inference
    // from display fields or today's model-list ordering.
    let mut bindings = Vec::new();
    for entry in catalog["models"].as_array().into_iter().flatten() {
        let Some(client) = entry["slug"]
            .as_str()
            .filter(|id| legacy_provider_alias_id(id))
        else {
            continue;
        };
        let target = entry["display_name"].as_str().filter(|target| {
            entry["description"].as_str() == Some(*target)
                && checked_provider_binding_id(target).is_ok()
                && !target.chars().any(char::is_whitespace)
                && allowed_models
                    .iter()
                    .any(|model| model.eq_ignore_ascii_case(target))
        });
        bindings.push(CodexLocalAccessProviderModelBinding {
            client_model: client.to_owned(),
            upstream_model: target.map(str::to_owned),
        });
    }
    Ok(bindings)
}

fn merge_provider_binding_evidence(
    evidence: Vec<CodexLocalAccessProviderModelBinding>,
) -> Result<Vec<CodexLocalAccessProviderModelBinding>, String> {
    let mut entries: std::collections::BTreeMap<String, (String, Vec<String>)> =
        std::collections::BTreeMap::new();
    for binding in evidence {
        let client = checked_provider_binding_id(&binding.client_model)?;
        let entry = entries
            .entry(client.to_ascii_lowercase())
            .or_insert((client, Vec::new()));
        if let Some(target) = binding.upstream_model {
            let target = checked_provider_binding_id(&target)?;
            if !entry
                .1
                .iter()
                .any(|existing| existing.eq_ignore_ascii_case(&target))
            {
                entry.1.push(target);
            }
        }
    }
    Ok(entries
        .into_values()
        .map(|(client_model, targets)| {
            CodexLocalAccessProviderModelBinding {
                client_model,
                // Conflicting snapshots cannot authorize a guessed target.
                upstream_model: (targets.len() == 1).then(|| targets[0].clone()),
            }
        })
        .collect())
}

fn resolve_native_provider_model_bindings_for_profile(
    profile: &Path,
    account: &CodexAccount,
    previous_state: Option<&ProviderGatewayProfileState>,
) -> Result<Vec<CodexLocalAccessProviderModelBinding>, String> {
    let current = load_provider_gateway_profile_state(profile, &account.id)?
        .ok_or("PROVIDER_MODEL_BINDING_STATE_MISSING")?;
    if previous_state.is_some_and(|previous| previous.api_key != current.api_key) {
        return Err("PROVIDER_MODEL_BINDING_STATE_CHANGED".into());
    }
    if let Some(bindings) = &current.model_bindings {
        return normalize_provider_model_bindings(bindings);
    }
    // User-editable account mappings are a separate configuration authority.
    // Do not freeze them or manufacture new profile aliases from them.
    let mut evidence = Vec::new();
    if let Some(legacy) = previous_state {
        // Reserve old generated names even when no trustworthy target survived.
        // Their absence from today's list cannot turn them into a new grant.
        evidence.extend(CODEX_PROVIDER_MODEL_SHELL_POOL.iter().map(|id| {
            CodexLocalAccessProviderModelBinding {
                client_model: (*id).into(),
                upstream_model: None,
            }
        }));
        if let Some(runtime) = provider_binding_runtime_evidence(profile, account, legacy)? {
            evidence.extend(runtime);
        } else {
            evidence.extend(provider_binding_catalog_evidence(
                profile,
                legacy,
                &provider_gateway_models_for_account(account),
            )?);
            let manifest_path =
                sidecar_manifest_path(&provider_gateway_sidecar_dir(profile, &account.id)?);
            if let Some(content) = provider_binding_evidence_text(&manifest_path)? {
                let manifest: Value = serde_json::from_str(&content)
                    .map_err(|_| "PROVIDER_MODEL_BINDING_EVIDENCE_INVALID")?;
                if let Some(bindings) = provider_binding_manifest_evidence(
                    &manifest,
                    legacy,
                    &account.id,
                    codex_account::is_grok_upstream_provider(account),
                )? {
                    evidence.extend(bindings);
                }
            }
        }
    }
    let resolved = normalize_provider_model_bindings(&merge_provider_binding_evidence(evidence)?)?;
    let state_path = provider_gateway_state_path(profile, &account.id)?;
    let mut published = None;
    update_string_atomic(&state_path, |text| {
        let mut state: ProviderGatewayProfileState =
            serde_json::from_str(text.ok_or("PROVIDER_MODEL_BINDING_STATE_CHANGED")?)
                .map_err(|_| "PROVIDER_MODEL_BINDING_STATE_INVALID")?;
        if state.api_key != current.api_key || state.port != current.port {
            return Err("PROVIDER_MODEL_BINDING_STATE_CHANGED".into());
        }
        if let Some(bindings) = &state.model_bindings {
            published = Some(normalize_provider_model_bindings(bindings)?);
            return Ok(text.expect("state text checked").to_owned());
        }
        state.model_bindings = Some(resolved.clone());
        state.updated_at = now_ms();
        published = Some(resolved.clone());
        serde_json::to_string_pretty(&state)
            .map_err(|_| "PROVIDER_MODEL_BINDING_STATE_INVALID".into())
    })?;
    published.ok_or_else(|| "PROVIDER_MODEL_BINDING_STATE_CHANGED".into())
}

fn freeze_existing_native_provider_model_bindings(
    profile: &Path,
    account: &CodexAccount,
) -> Result<(), String> {
    if load_collection_from_disk()?
        .is_some_and(|collection| collection.gateway_mode != CodexLocalAccessGatewayMode::Native)
    {
        return Ok(());
    }
    if let Some(state) = load_provider_gateway_profile_state(profile, &account.id)? {
        if state.model_bindings.is_none() {
            // Do this before a cold-start port collision clears the saved port,
            // while the previous endpoint still proves catalog ownership.
            resolve_native_provider_model_bindings_for_profile(profile, account, Some(&state))?;
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "codex_local_access_provider_model_bindings_tests.rs"]
mod provider_model_bindings_tests;
