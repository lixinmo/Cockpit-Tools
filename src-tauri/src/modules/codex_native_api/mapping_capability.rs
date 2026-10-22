//! Capability replacement always rebuilds from the untouched logical request.
use super::*;
use crate::modules::codex_native_api::{capabilities, diagnostics::CapabilityFallbackTrace};

// Keep evidence of a rejected route even when no replacement can be selected.
// This deliberately inspects only protocol tags: no tool names, schemas,
// arguments, conversation text, credentials or account identities are retained.
fn failure_diagnostics(body: &Value, required: &[String], unsupported: &[String], reason: &str) -> Value {
    fn capability_ids(values: &[String]) -> Vec<&str> {
        let mut result: Vec<_> = values.iter().map(|value| {
            if capabilities::IDS.contains(&value.as_str()) {
                value.as_str()
            } else {
                "unknown_tool"
            }
        }).collect();
        result.sort_unstable();
        result.dedup();
        result
    }
    fn shape(value: &Value, source: &str) -> Value {
        let kind = value.get("type").and_then(Value::as_str).unwrap_or("missing");
        let kind = match kind {
            "function" | "custom" | "namespace" | "tool_search" | "allowed_tools"
            | "web_search" | "web_search_preview" | "web_search_preview_2025_03_11"
            | "image_generation" | "file_search" | "code_interpreter" | "computer"
            | "computer_use" | "computer_use_preview" | "mcp" | "programmatic_tool_calling"
            | "function_call" | "function_call_output" | "custom_tool_call" | "custom_tool_call_output"
            | "tool_search_call" | "tool_search_output" | "web_search_call" | "image_generation_call"
            | "file_search_call" | "code_interpreter_call" | "computer_call" | "mcp_call"
            | "mcp_list_tools" | "mcp_approval_request" | "mcp_approval_response"
            | "additional_tools" | "message" | "reasoning" | "input_image" | "image_url"
            | "input_file" | "input_audio" | "audio" | "audio_url" | "input_video"
            | "video" | "video_url" | "input_text" | "output_text" | "text" | "missing" => kind,
            _ => "unknown",
        };
        let execution = match value.get("execution") {
            None => "missing",
            Some(Value::String(value)) if value == "client" => "client",
            Some(Value::String(value)) if value == "server" => "server",
            _ => "unknown",
        };
        serde_json::json!({"source":source,"type":kind,"execution":execution})
    }
    fn visit(value: &Value, source: &str, depth: usize, features: &mut Vec<Value>, truncated: &mut bool) {
        if depth > 16 || features.len() >= 128 {
            *truncated = true;
            return;
        }
        if let Some(values) = value.as_array() {
            for value in values {
                visit(value, source, depth + 1, features, truncated);
                if *truncated { break; }
            }
        } else if value.is_object() {
            let feature = shape(value, source);
            if !features.contains(&feature) { features.push(feature); }
            // These containers carry declarations, never parameter schemas.
            if matches!(value["type"].as_str(), Some("namespace" | "allowed_tools" | "additional_tools" | "tool_search_output")) {
                if let Some(tools) = value.get("tools") {
                    visit(tools, source, depth + 1, features, truncated);
                }
            }
        }
    }
    let mut features = Vec::new();
    let mut truncated = false;
    for source in ["tools", "tool_choice", "input"] {
        if let Some(value) = body.get(source) {
            visit(value, source, 0, &mut features, &mut truncated);
        }
    }
    serde_json::json!({
        "trigger_stage": match reason { "preflight" => "preflight", "upstream_unsupported" => "upstream_unsupported", _ => "unknown" },
        "required": capability_ids(required),
        "unsupported": capability_ids(unsupported),
        "features": features,
        "truncated": truncated,
    })
}

fn with_failure_diagnostics(mut error: ServiceError, diagnostics: &Value, stage: &str) -> ServiceError {
    let mut diagnostics = diagnostics.clone();
    diagnostics["failure_stage"] = serde_json::json!(stage);
    let mut details = error.response_error.take().filter(Value::is_object)
        .unwrap_or_else(|| serde_json::json!({}));
    details["capability_diagnostics"] = diagnostics;
    error.with_error_details(details)
}

pub(super) struct Selection {
    pub context: RequestIdentity,
    pub route: Route,
    pub request: RouteRequest,
    pub headers: HeaderMap,
    pub body: Value,
    pub plan: Option<super::super::identity::rebuild::Plan>,
    pub trace: Option<CapabilityFallbackTrace>,
    pub enabled: bool,
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn select(
    state: &AppState,
    caller: &Caller,
    headers: &HeaderMap,
    body: Value,
    mut context: RequestIdentity,
    mut selected: Route,
    mut request: RouteRequest,
    pinned: bool,
    force: bool,
    reason: &str,
) -> Result<Selection, ServiceError> {
    let decision = super::super::execution::within(
        state,
        state
            .host
            .capability_fallback_decision(caller, &selected, &request),
    )
    .await?;
    if !decision.enabled || (!force && decision.unsupported.is_empty()) {
        return Ok(Selection {
            context,
            route: selected,
            request,
            headers: headers.clone(),
            body,
            plan: None,
            trace: None,
            enabled: decision.enabled,
        });
    }
    let diagnostics = failure_diagnostics(&body, &request.required_capabilities, &decision.unsupported, reason);
    if pinned || request.target_account_id.is_some() {
        return Err(capabilities::failure("capability_fallback_pinned", "The required capability is unavailable on this pinned account; the original conversation was preserved"));
    }
    if request
        .required_capabilities
        .iter()
        .any(|id| id == "unknown_tool")
    {
        return Err(capabilities::failure(
            "capability_fallback_unknown_tool",
            "An unknown tool capability cannot be routed to a backup account",
        ));
    }
    if request.excluded_account_ids.len() > capabilities::MAX_REPLACEMENTS {
        return Err(capabilities::failure(
            "capability_fallback_exhausted",
            "The capability fallback attempt limit was reached",
        ));
    }
    let identity = state.identity.clone();
    let caller_id = caller.id.clone();
    let mut logical = body.clone();
    let incoming = headers.clone();
    let (mut context, plan) = super::super::blocking::run_prepared(state, move || {
        identity.normalize_capability_input(&mut context, &incoming, &mut logical)?;
        let plan = identity.configure_rebuild(&mut context, &incoming, &logical, true)?;
        let plan = match plan {
            Some(plan) => Some(plan),
            None => identity.preview_rebuild_with_checkpoint_parameters(&context, &incoming, &logical)?,
        }.ok_or_else(|| capabilities::failure("capability_fallback_history_unsafe", "Capability fallback requires complete verified history; the original conversation was preserved"))?;
        capabilities::validate_replay(&plan.headers, &plan.body)?;
        // The old incremental identity still owns its parent_response anchor.
        // Re-inspect the proven complete replay, just as prepare_rebuilt does;
        // never detach an owner merely by deleting that reference from context.
        let mut replay = plan.body.clone();
        let mut rebuilt = identity.inspect_for_rebuild(&caller_id, &plan.headers, &replay)?;
        identity.normalize_capability_input(&mut rebuilt, &plan.headers, &mut replay)?;
        identity.configure_rebuild(&mut rebuilt, &plan.headers, &replay, true)?
            .ok_or_else(|| capabilities::failure("capability_fallback_history_unsafe", "The complete capability replay could not be verified; existing history was preserved"))?;
        // Keep the original snapshot's CAS revision despite the second read.
        rebuilt.start_rebuild_generation(&plan);
        Ok::<_, ServiceError>((rebuilt, plan))
    }).await?;
    if plan.body.to_string().len() > state.options.max_body_bytes {
        return Err(ServiceError::new(
            413,
            "capability_fallback_history_too_large",
            "Capability fallback history exceeds the request size limit",
        ));
    }
    request.required_capabilities = capabilities::collect(
        &plan.body,
        request.required_capabilities.iter().any(|v| v == "compact"),
    );
    request
        .capability_fallback_from
        .get_or_insert_with(|| selected.account_id.clone());
    if !request.excluded_account_ids.contains(&selected.account_id) {
        request
            .excluded_account_ids
            .push(selected.account_id.clone());
    }
    request.preferred_account_id = None;
    request.soft_preferred_account_id = None;
    request.force_refresh = false;
    // Release capacity before selecting the next account. No attempt was sent.
    selected.permit = None;
    let remaining = capabilities::MAX_REPLACEMENTS
        .saturating_add(1)
        .saturating_sub(request.excluded_account_ids.len());
    let mut accepted = None;
    let mut last_incompatible = None;
    for _ in 0..remaining {
        let mut target = match route(state, caller, request.clone()).await {
            Ok(target) => target,
            Err(error) if error.status == 503 && last_incompatible.is_some() => break,
            Err(error) => return Err(with_failure_diagnostics(error, &diagnostics, "candidate_selection")),
        };
        if request.excluded_account_ids.contains(&target.account_id) {
            return Err(capabilities::failure(
                "capability_fallback_exhausted",
                "No untried capability fallback account is available",
            ));
        }
        // The original selected source remains the compatibility authority for
        // every candidate. A rejected candidate never becomes a history owner.
        let incompatible = if target.wire_api != super::super::contract::WireApi::Responses {
            Some(capabilities::failure(
                "capability_fallback_protocol_unsafe",
                "Capability history fallback requires a Responses account",
            ))
        } else if !capabilities::encrypted_compatible(&selected, &target, &plan.body) {
            Some(capabilities::failure("capability_fallback_encryption_unsafe", "Encrypted history is incompatible with the fallback account; existing history was preserved"))
        } else {
            let confirmed = super::super::execution::within(
                state,
                state
                    .host
                    .capability_fallback_decision(caller, &target, &request),
            )
            .await?;
            if !confirmed.enabled {
                return Err(capabilities::failure(
                    "capability_fallback_unavailable",
                    "Capability fallback is no longer enabled",
                ));
            }
            (!confirmed.unsupported.is_empty()).then(|| {
                capabilities::failure(
                    "capability_fallback_unavailable",
                    "The selected backup does not support all required capabilities",
                )
            })
        };
        if let Some(error) = incompatible {
            request.excluded_account_ids.push(target.account_id.clone());
            target.permit = None;
            last_incompatible = Some(error);
            continue;
        }
        accepted = Some(target);
        break;
    }
    let target = accepted.ok_or_else(|| {
        last_incompatible.unwrap_or_else(|| {
            capabilities::failure(
                "capability_fallback_exhausted",
                "No compatible capability fallback account is available",
            )
        })
    }).map_err(|error| with_failure_diagnostics(error, &diagnostics, "candidate_compatibility"))?;
    // Retain the full, validated evidence while changing only the upstream owner.
    context.start_rebuild_generation(&plan);
    let trace = CapabilityFallbackTrace {
        reason: reason.to_owned(),
        capabilities: request.required_capabilities.clone(),
        from_account_id: selected.account_id.clone(),
        from_model: selected.model.clone(),
        to_account_id: target.account_id.clone(),
        to_model: target.model.clone(),
    };
    Ok(Selection {
        context,
        route: target,
        request,
        headers: plan.headers.clone(),
        body: plan.body.clone(),
        plan: Some(plan),
        trace: Some(trace),
        enabled: true,
    })
}

pub(crate) async fn retry(state: &AppState, prepared: &mut Prepared) -> Result<bool, ServiceError> {
    if !prepared.capability_fallback_enabled {
        return Ok(false);
    }
    let body = prepared.replay_body().clone();
    let headers = prepared.replay_headers().clone();
    let context = inspect_mode(state, &prepared.caller, &headers, &body, true).await?;
    prepared.route.permit = None;
    let selection = select(
        state,
        &prepared.caller,
        &headers,
        body,
        context,
        prepared.route.clone(),
        prepared.request.clone(),
        prepared.transport_pinned,
        true,
        "upstream_unsupported",
    )
    .await?;
    if selection.trace.is_none() {
        return Ok(false);
    }
    let mut next = finalize(
        state,
        &prepared.caller,
        &selection.headers,
        selection.body,
        selection.context,
        selection.route,
        selection.request,
        None,
        prepared.compact,
        false,
        prepared.multi_agent_policy.options.tools_prepared,
    )
    .await?;
    next.source_body = prepared.source_body.clone();
    next.source_headers = prepared.source_headers.clone();
    next.rebuild_plan = selection.plan;
    next.rebuild_applied = true;
    next.capability_fallback = selection.trace;
    *prepared = next;
    Ok(true)
}

#[cfg(test)]
#[path = "mapping_capability_diagnostics_tests.rs"]
mod diagnostics_tests;
