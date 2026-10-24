// Manual effort changes update the owned provider catalog without restarting a
// listener, changing credentials, or replacing its scoped model set.
fn native_manual_catalog_targets(
    profile: &Path,
    runtimes: &HashMap<String, ProviderGatewayRuntime>,
) -> Vec<(String, CodexLocalAccessCollection)> {
    let prefix = format!("{}\n", normalize_profile_dir_key(profile));
    runtimes.iter().filter_map(|(key, runtime)| {
        let account_id = key.strip_prefix(&prefix)?;
        if account_id.is_empty() || account_id == MIXED_MODEL_ROUTING_RUNTIME_ID
            || runtime.native_handle.is_none() {
            return None;
        }
        let collection = runtime.collection.as_ref()?;
        if !inspect_local_access_profile_attachment(profile, Some(collection)).attached {
            return None;
        }
        Some((account_id.to_string(), collection.clone()))
    }).collect()
}

/// Called under the profile writer lease before editing local policy. Keep an
/// active provider's catalog intact until its authoritative writer republishes.
pub(crate) fn native_provider_catalog_is_running(profile: &Path) -> Result<bool, String> {
    let runtimes = provider_gateway_runtime_store().try_lock()
        .map_err(|_| "NATIVE_PROVIDER_CATALOG_BUSY".to_string())?;
    Ok(!native_manual_catalog_targets(profile, &runtimes).is_empty())
}

pub(crate) async fn refresh_native_provider_catalog_for_profile(profile: &Path) -> Result<(), String> {
    refresh_native_provider_catalog_for_account(profile, None).await
}

pub(crate) async fn refresh_native_provider_catalog_for_account(profile: &Path, account_id: Option<&str>) -> Result<(), String> {
    let _lifecycle = provider_gateway_lifecycle_lock().lock().await;
    let targets = {
        let runtimes = provider_gateway_runtime_store().lock().await;
        native_manual_catalog_targets(profile, &runtimes).into_iter()
            .filter(|(id, _)| account_id.is_none_or(|account_id| account_id == id))
            .collect::<Vec<_>>()
    };
    if targets.is_empty() {
        return Ok(());
    }
    let profile = profile.to_path_buf();
    tokio::task::spawn_blocking(move || {
        let _lease = codex_account::try_acquire_profile_mutation_lease(&profile, "provider-model-settings")?;
        for (account_id, collection) in targets {
            // A profile may have changed while the worker was queued. Do not
            // restore an old provider over a different active credential.
            if !inspect_local_access_profile_attachment(&profile, Some(&collection)).attached {
                continue;
            }
            let account = codex_account::load_account(&account_id)
                .ok_or_else(|| "NATIVE_PROFILE_ACCOUNT_NOT_FOUND".to_string())?;
            if !account_requires_provider_gateway(&account) {
                continue;
            }
            write_native_provider_profile_model_catalog(&profile, &collection, &collection.api_key, &account)?;
            if let Some(default) = codex_account::provider_default_model_override(&profile, &account)?
                .or_else(|| codex_account::profile_default_model_override(&profile)) {
                let catalog: Value = serde_json::from_slice(&fs::read(profile.join(CODEX_MANAGED_MODEL_CATALOG_FILE))
                    .map_err(|_| "NATIVE_PROVIDER_CATALOG_READ_FAILED".to_string())?)
                    .map_err(|_| "NATIVE_PROVIDER_CATALOG_JSON_INVALID".to_string())?;
                if let Some(model) = catalog["models"].as_array().into_iter().flatten()
                    .find_map(|model| model["slug"].as_str().filter(|id| id.eq_ignore_ascii_case(&default))) {
                    write_local_access_profile_model_override(&profile, model)?;
                }
            }
            remember_takeover_ownership(&profile)?;
        }
        Ok::<(), String>(())
    }).await.map_err(|_| "NATIVE_PROVIDER_CATALOG_WORKER_FAILED".to_string())?
}
