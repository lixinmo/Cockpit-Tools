//! Client-owned tool discovery over the ordinary Chat function bridge.
//! Hosted tools are never reinterpreted as client execution or silently dropped.
use super::super::{chatgpt_plan_tools, contract::ServiceError};
use serde_json::Value;
use std::collections::HashSet;

fn unsupported() -> ServiceError {
    ServiceError::new(
        400,
        "unsupported_chat_tool",
        "This tool requires an upstream execution capability unavailable on the Chat bridge",
    )
}

fn invalid() -> ServiceError {
    ServiceError::new(
        400,
        "invalid_client_tool_discovery",
        "Client tool discovery requires explicit client execution and matching call/result history",
    )
}

fn validate_tools(value: Option<&Value>, depth: usize) -> Result<(), ServiceError> {
    let Some(value) = value.filter(|value| !value.is_null()) else {
        return Ok(());
    };
    if depth > 16 {
        return Err(invalid());
    }
    let tools = value.as_array().ok_or_else(invalid)?;
    for tool in tools {
        match tool["type"].as_str() {
            Some("function" | "custom") => {
                if tool["allowed_callers"]
                    .as_array()
                    .is_some_and(|callers| callers.iter().any(|caller| caller == "programmatic"))
                {
                    return Err(unsupported());
                }
            }
            Some("namespace") if tool["name"].as_str().is_some_and(|name| !name.is_empty()) => {
                if !tool["tools"].is_array() {
                    return Err(invalid());
                }
                validate_tools(tool.get("tools"), depth + 1)?;
            }
            Some("tool_search") if depth == 0 && chatgpt_plan_tools::client_declaration(tool) => {}
            _ => return Err(unsupported()),
        }
    }
    Ok(())
}

fn validate_choice(
    value: Option<&Value>,
    client_declared: bool,
    depth: usize,
) -> Result<(), ServiceError> {
    let Some(value) = value.filter(|value| value.is_object()) else {
        return Ok(());
    };
    if depth > 16 {
        return Err(invalid());
    }
    match value["type"].as_str() {
        Some("function" | "custom") => Ok(()),
        Some("tool_search")
            if client_declared
                && value
                    .get("execution")
                    .is_none_or(|execution| execution == "client") =>
        {
            Ok(())
        }
        Some("allowed_tools") => {
            let choices = value["tools"].as_array().ok_or_else(invalid)?;
            for choice in choices {
                validate_choice(Some(choice), client_declared, depth + 1)?;
            }
            Ok(())
        }
        _ => Err(unsupported()),
    }
}

fn validate_history(body: &Value, client_declared: bool) -> Result<(), ServiceError> {
    let mut seen = HashSet::new();
    let mut pending = HashSet::new();
    for item in body["input"].as_array().into_iter().flatten() {
        match item["type"].as_str() {
            Some("additional_tools") => validate_tools(item.get("tools"), 0)?,
            Some("tool_search_call") => {
                if item["execution"] != "client"
                    && !(item.get("execution").is_none() && client_declared)
                {
                    return Err(unsupported());
                }
                let id = item["call_id"]
                    .as_str()
                    .filter(|id| !id.is_empty())
                    .ok_or_else(invalid)?;
                if !seen.insert(id) || !pending.insert(id) {
                    return Err(invalid());
                }
            }
            Some("tool_search_output") => {
                if item
                    .get("execution")
                    .is_some_and(|execution| execution != "client")
                    || item
                        .get("status")
                        .is_some_and(|status| status != "completed")
                {
                    return Err(unsupported());
                }
                let id = item["call_id"]
                    .as_str()
                    .filter(|id| !id.is_empty())
                    .ok_or_else(invalid)?;
                if !pending.remove(id) {
                    return Err(invalid());
                }
                // Validate discovered schemas even when tools=[] intentionally
                // prevents their promotion; do not reinterpret hosted content.
                validate_tools(item.get("tools"), 0)?;
                if let Some(output) = item.get("output") {
                    let decoded = output
                        .as_str()
                        .and_then(|text| serde_json::from_str::<Value>(text).ok());
                    let output = decoded.as_ref().unwrap_or(output);
                    validate_tools(
                        if output.is_array() {
                            Some(output)
                        } else {
                            output.get("tools")
                        },
                        0,
                    )?;
                }
            }
            _ => {}
        }
    }
    if !pending.is_empty() {
        return Err(invalid());
    }
    Ok(())
}

pub(super) fn prepare(body: &Value) -> Result<(Value, chatgpt_plan_tools::Context), ServiceError> {
    let client_declared = chatgpt_plan_tools::has_client_declaration(body);
    validate_tools(body.get("tools"), 0)?;
    validate_choice(body.get("tool_choice"), client_declared, 0)?;
    validate_history(body, client_declared)?;
    let mut lowered = body.clone();
    // Reuse the strict plan adapter: request-local collision-free names,
    // discovery schema precedence, history lowering and reversible SSE state.
    let context = chatgpt_plan_tools::prepare(&mut lowered)?;
    validate_tools(lowered.get("tools"), 0)?;
    Ok((lowered, context))
}
