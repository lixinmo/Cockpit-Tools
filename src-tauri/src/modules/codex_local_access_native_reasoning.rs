// Provider effort is resolved after account/model authorization. This changes
// only the requested effort, never model identity, tools, history or admission.
const NATIVE_PROVIDER_EFFORT_ORDER: &[&str] =
    &["minimal", "low", "medium", "high", "xhigh", "max", "ultra"];

fn native_provider_effort_levels(template: &Value) -> Option<Vec<String>> {
    if template["cockpit_reasoning_known"] == false { return None; }
    template.get("supported_reasoning_levels")?.as_array().map(|levels| {
        levels.iter().filter_map(|level|
            level.as_str().or_else(|| level["effort"].as_str())
        ).map(str::to_owned).collect()
    })
}

fn native_map_provider_effort(requested: &str, supported: Option<&[String]>)
    -> Result<Option<String>, native_contract::ServiceError>
{
    let requested = requested.trim().to_ascii_lowercase();
    let Some(supported) = supported else { return Ok(Some(requested)); };
    if supported.is_empty() {
        return if matches!(requested.as_str(), "none" | "auto") {
            Ok(None)
        } else {
            Err(native_error(400, "provider_reasoning_not_supported",
                "This model is configured without adjustable reasoning effort"))
        };
    }
    // Disabling/automatic reasoning is not an ordinal intensity. Preserve it
    // for provider validation; never turn an explicit disable into more compute.
    if matches!(requested.as_str(), "none" | "auto")
        || supported.iter().any(|level| level == &requested)
    {
        return Ok(Some(requested));
    }
    let Some(rank) = NATIVE_PROVIDER_EFFORT_ORDER.iter().position(|v| *v == requested) else {
        return Err(native_error(400, "provider_reasoning_effort_invalid", "Unknown reasoning effort"));
    };
    // Match the existing Go cross-provider high-intent preference before the
    // general nearest-level rule. Unlike its older order, ultra is covered.
    let preference: &[&str] = match requested.as_str() {
        "xhigh" => &["max"],
        "max" => &["xhigh"],
        _ => &[],
    };
    if let Some(level) = preference.iter().find(|level| supported.iter().any(|v| v == **level)) {
        return Ok(Some((*level).to_string()));
    }
    supported.iter().filter_map(|level| {
        NATIVE_PROVIDER_EFFORT_ORDER.iter().position(|v| *v == level)
            .map(|index| ((index.abs_diff(rank), index), level.clone()))
    }).min_by_key(|(distance, _)| *distance).map(|(_, level)| Some(level))
      .ok_or_else(|| native_error(400, "provider_reasoning_effort_invalid",
          "The model has no compatible reasoning intensity"))
}

#[cfg(test)]
fn native_prepare_provider_reasoning_body(
    profile: Option<&Path>, account: &CodexAccount, model: &str, body: Value,
) -> Result<Value, native_contract::ServiceError> {
    native_prepare_provider_reasoning_body_for_model(profile, account, model, model, body)
}

fn native_prepare_provider_reasoning_body_for_model(
    profile: Option<&Path>, account: &CodexAccount, model: &str, requested_model: &str, mut body: Value,
) -> Result<Value, native_contract::ServiceError> {
    if !account.is_api_key_auth() || native_is_chatgpt_plan_account(account)
        || codex_account::is_grok_upstream_provider(account)
        || is_official_deepseek_account(account)
        || account.api_provider_mode == CodexApiProviderMode::OpenaiBuiltin
    {
        return Ok(body);
    }
    let discovered = crate::modules::codex_provider_metadata::lookup(account, model);
    let template = native_documented_provider_model_metadata(account, model);
    let mut supported = discovered.and_then(|metadata| metadata.reasoning_efforts)
        .or_else(|| template.as_ref().and_then(native_provider_effort_levels));
    let mut manual_default = None;
    if let Some(profile) = profile {
        // Fieldwise precedence matches catalog decoration. A default-only
        // provider rule must not erase a lower-priority explicit level list.
        for scoped in [false, true] {
            for id in [model, requested_model] {
                let definition = if scoped {
                    codex_account::provider_reasoning_override(profile, account, id)
                } else {
                    codex_account::profile_reasoning_override(profile, id)
                }.map_err(|_| native_error(400, "provider_reasoning_override_invalid",
                    "The model reasoning override is invalid"))?;
                if let Some(definition) = definition {
                    if let Some(levels) = definition.reasoning_efforts {
                        if levels.is_empty() { manual_default = None; }
                        supported = Some(levels);
                    }
                    if let Some(default) = definition.default_reasoning_effort {
                        manual_default = Some(default);
                    }
                }
            }
        }
    }
    let nested = body.pointer("/reasoning/effort");
    let raw = nested.or_else(|| body.get("reasoning_effort"));
    if raw.is_some_and(|value| !value.is_null() && !value.is_string()) {
        return Err(native_error(400, "provider_reasoning_effort_invalid", "Reasoning effort must be text"));
    }
    let requested = raw.and_then(Value::as_str).or(manual_default.as_deref());
    let Some(requested) = requested else { return Ok(body); };
    let mapped = native_map_provider_effort(requested, supported.as_deref())?;
    let had_legacy = body.get("reasoning_effort").is_some();
    if let Some(mapped) = mapped {
        if body.get("reasoning").is_none_or(Value::is_null) { body["reasoning"] = json!({}); }
        if !body["reasoning"].is_object() {
            return Err(native_error(400, "provider_reasoning_effort_invalid", "Reasoning must be an object"));
        }
        body["reasoning"]["effort"] = json!(mapped);
        if had_legacy { body["reasoning_effort"] = json!(mapped); }
    } else {
        if let Some(reasoning) = body.get_mut("reasoning").and_then(Value::as_object_mut) {
            reasoning.remove("effort");
        }
        let empty = body.get("reasoning").and_then(Value::as_object).is_some_and(|value| value.is_empty());
        if let Some(object) = body.as_object_mut() { object.remove("reasoning_effort"); }
        if empty { body.as_object_mut().unwrap().remove("reasoning"); }
    }
    Ok(body)
}

#[cfg(test)]
#[path = "codex_local_access_native_reasoning_tests.rs"]
mod native_provider_reasoning_tests;
