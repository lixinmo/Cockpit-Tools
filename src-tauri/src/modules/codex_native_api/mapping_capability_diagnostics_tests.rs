use super::*;
use serde_json::json;

#[test]
fn failed_capability_selection_retains_only_protocol_evidence() {
    let body = json!({"model":"private-model","api_key":"private-key","account_id":"private-account",
        "tools":[{"type":"tool_search","execution":"client","name":"private-tool","parameters":{"type":"private-schema"}},
            {"type":"namespace","name":"private-namespace","tools":[{"type":"function","name":"private-function","description":"private-description","parameters":{"type":"object","properties":{"private-property":{"type":"string"}}}}]}],
        "tool_choice":{"type":"allowed_tools","tools":[{"type":"tool_search"}]},
        "input":[{"role":"user","content":"private-conversation"},
            {"type":"tool_search_call","execution":"client","call_id":"private-call","arguments":{"query":"private-query"}},
            {"type":"tool_search_output","call_id":"private-call","tools":[{"type":"mcp","server_url":"https://private-host.invalid","authorization":"private-auth"}]}]});
    let original = body.clone();
    let diagnostics = failure_diagnostics(&body,
        &["tool_search_client".into(), "tool_search".into(), "mcp".into()],
        &["tool_search".into(), "mcp".into()], "preflight");
    let error = with_failure_diagnostics(ServiceError::new(503,
        "capability_fallback_no_compatible_account", "No compatible account is available within this API key's model scope")
        .with_retry_after(Some(std::time::Duration::from_secs(3))), &diagnostics, "candidate_selection");
    assert_eq!(body, original);
    assert_eq!(error.status, 503);
    assert_eq!(error.retry_after, Some(std::time::Duration::from_secs(3)));
    let public = super::super::super::output_error::json_error(&error);
    assert_eq!(public["error"]["code"], "capability_fallback_no_compatible_account");
    let details = &public["error"]["capability_diagnostics"];
    assert_eq!(details["trigger_stage"], "preflight");
    assert_eq!(details["failure_stage"], "candidate_selection");
    assert_eq!(details["unsupported"], json!(["mcp", "tool_search"]));
    assert!(details["features"].as_array().unwrap().contains(&json!({"source":"input","type":"tool_search_output","execution":"missing"})));
    assert!(details["features"].as_array().unwrap().contains(&json!({"source":"input","type":"mcp","execution":"missing"})));
    assert!(!public.to_string().contains("private-"));
    for codex in [false, true] {
        let event = super::super::super::output_error::responses_event(&error, codex, 0);
        let path = if codex { "/response/error/capability_diagnostics" } else { "/error/capability_diagnostics" };
        assert_eq!(event.pointer(path), Some(details));
        assert!(!event.to_string().contains("private-"));
    }
}

#[test]
fn diagnostic_tags_are_allowlisted_and_bounded_without_schema_traversal() {
    let mut nested = json!({"type":"function","name":"private-name"});
    for _ in 0..20 { nested = json!({"type":"namespace","tools":[nested]}); }
    let diagnostics = failure_diagnostics(&json!({"tools":[
        {"type":"private-type","execution":"private-execution"},
        {"type":"function","parameters":{"type":"mcp","execution":"server"}},
        nested],"input":[{"type":"message","content":"private-input"}]}),
        &["private-capability".into(), "function".into()], &["private-unsupported".into()], "private-reason");
    assert_eq!(diagnostics["required"], json!(["function", "unknown_tool"]));
    assert_eq!(diagnostics["unsupported"], json!(["unknown_tool"]));
    assert_eq!(diagnostics["trigger_stage"], "unknown");
    assert_eq!(diagnostics["truncated"], true);
    assert!(!diagnostics.to_string().contains("private-"));
    assert!(!diagnostics.to_string().contains("mcp"));
}

#[test]
fn capability_diagnostics_preserve_existing_error_fields_and_upstream_trigger() {
    let diagnostics = failure_diagnostics(&json!({"input":[]}), &[], &[], "upstream_unsupported");
    let error = with_failure_diagnostics(ServiceError::new(409, "capability_fallback_protocol_unsafe", "Incompatible protocol")
        .with_error_details(json!({"param":"tools","retryable":false})), &diagnostics, "candidate_compatibility");
    assert_eq!(error.code, "capability_fallback_protocol_unsafe");
    assert_eq!(error.message, "Incompatible protocol");
    let details = error.response_error.unwrap();
    assert_eq!(details["param"], "tools");
    assert_eq!(details["retryable"], false);
    assert_eq!(details["capability_diagnostics"]["trigger_stage"], "upstream_unsupported");
    assert_eq!(details["capability_diagnostics"]["unsupported"], json!([]));
}
