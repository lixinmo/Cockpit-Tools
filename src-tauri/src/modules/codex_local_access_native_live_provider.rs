//! Opt-in real StepFun acceptance listener. The selected saved account is read
//! without migration and held in memory; production Native routing, capability
//! checks, conversion, transport and Credits admission remain in use.
//! Compile on CI, then run only this ignored test in a dedicated local process.
use super::*;
use futures_util::future::BoxFuture;
use native_contract::{
    Caller, CapabilityFallbackDecision, ExecutionPolicy, Host, MultiAgentPolicy, RequestReport,
    Route, RouteRequest, ServiceError, UpstreamAdmissionGuard, WireApi,
};
use serde::Deserialize;
use std::io::{BufRead, Write};

const MODELS: &[&str] = &["step-5-preview", "step-3.7-flash"];

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    account_id: String,
    allowed_models: Vec<String>,
    key: String,
    output_dir: PathBuf,
    max_requests: usize,
    #[serde(default)]
    manual_low: bool,
}

fn safe_error(mut error: ServiceError) -> ServiceError {
    // Codes are local &'static str values. Messages/details can contain upstream
    // text or configuration paths and must not reach this driver's output.
    error.message =
        "Provider acceptance request failed; inspect the status and failure stage".into();
    error.response_error = None;
    error
}

fn destination_allowed(base: &str) -> bool {
    reqwest::Url::parse(base).is_ok_and(|url| {
        url.scheme() == "https"
            && url.host_str() == Some("api.stepfun.com")
            && url.port_or_known_default() == Some(443)
            && url.path().trim_end_matches('/') == "/step_plan/v1"
            && url.username().is_empty()
            && url.password().is_none()
            && url.query().is_none()
            && url.fragment().is_none()
    })
}

fn eligible_stepfun_account(account: &CodexAccount) -> bool {
    account.is_api_key_auth()
        && !native_is_chatgpt_plan_account(account)
        && native_eligible(account)
        && is_provider_gateway_eligible_account(account)
        && account.upstream_grok_account_id.is_none()
        && provider_gateway_wire_api_for_account(account) == "chat_completions"
        && destination_allowed(account.api_base_url.as_deref().unwrap_or(""))
    // An auxiliary OAuth binding does not change this account's API-key
    // authentication. Scope and the final route still forbid that identity.
}

fn capability(value: &str) -> &str {
    match value {
        "function"
        | "custom"
        | "namespace"
        | "tool_search_client"
        | "tool_search"
        | "web_search"
        | "image_generation"
        | "file_search"
        | "code_interpreter"
        | "computer_use"
        | "mcp"
        | "programmatic_tool_calling"
        | "image_input"
        | "file_input"
        | "audio_input"
        | "video_input"
        | "compact" => value,
        _ => "unknown",
    }
}

fn failure_stage(value: Option<&str>) -> Option<&str> {
    value.map(|value| match value {
        "routing"
        | "preflight"
        | "capability_preflight"
        | "capability_fallback"
        | "prepare"
        | "mapping"
        | "upstream"
        | "transport"
        | "stream"
        | "client"
        | "authorization"
        | "admission" => value,
        _ => "other",
    })
}

fn tool_shape(value: &Value) -> Value {
    let kind = value.get("type").and_then(Value::as_str).unwrap_or("");
    let kind = match kind {
        "function"
        | "custom"
        | "namespace"
        | "tool_search"
        | "web_search"
        | "web_search_preview"
        | "web_search_preview_2025_03_11"
        | "image_generation"
        | "file_search"
        | "code_interpreter"
        | "computer"
        | "computer_use"
        | "computer_use_preview"
        | "mcp"
        | "programmatic_tool_calling"
        | "function_call"
        | "function_call_output"
        | "custom_tool_call"
        | "custom_tool_call_output"
        | "tool_search_call"
        | "tool_search_output"
        | "additional_tools"
        | "message"
        | "reasoning"
        | "mcp_call"
        | "mcp_list_tools"
        | "mcp_approval_request"
        | "mcp_approval_response" => kind,
        _ => "unknown",
    };
    let execution = match value.get("execution").and_then(Value::as_str) {
        Some("client") => "client",
        Some("server") => "server",
        None => "absent",
        _ => "unknown",
    };
    json!({"type":kind,"execution":execution})
}

fn shapes(value: Option<&Value>) -> Vec<Value> {
    value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .take(256)
        .map(tool_shape)
        .collect()
}

fn effort_shape(body: &Value) -> &'static str {
    match body.pointer("/reasoning/effort").or_else(|| body.get("reasoning_effort")).and_then(Value::as_str) {
        Some("none") => "none", Some("minimal") => "minimal", Some("low") => "low",
        Some("medium") => "medium", Some("high") => "high", Some("xhigh") => "xhigh",
        Some("max") => "max", Some("ultra") => "ultra", Some("auto") => "auto",
        None => "absent", _ => "unknown",
    }
}

#[test]
fn native_provider_live_destination_scope_is_exact() {
    assert!(destination_allowed("https://api.stepfun.com/step_plan/v1"));
    assert!(destination_allowed(
        "https://api.stepfun.com:443/step_plan/v1/"
    ));
    for base in [
        "http://api.stepfun.com/step_plan/v1",
        "https://api.stepfun.com/v1",
        "https://api.stepfun.com:444/step_plan/v1",
        "https://api.stepfun.com.example.invalid/step_plan/v1",
        "https://user:secret@api.stepfun.com/step_plan/v1",
        "https://api.stepfun.com/step_plan/v1?secret=value",
        "https://api.stepfun.com/step_plan/v1#secret",
    ] {
        assert!(!destination_allowed(base));
    }
}

#[test]
fn native_provider_live_evidence_contains_only_allowlisted_shape() {
    let input = json!([{
        "type":"tool_search_call","execution":"client","call_id":"private-call",
        "name":"private-tool","arguments":{"secret":"private-argument"},
        "output":"private-output"
    },{"type":"private-type","execution":"private-execution","text":"private-text"}]);
    assert_eq!(
        shapes(Some(&input)),
        vec![
            json!({"type":"tool_search_call","execution":"client"}),
            json!({"type":"unknown","execution":"unknown"}),
        ]
    );
    assert_eq!(capability("private-capability"), "unknown");
    assert_eq!(failure_stage(Some("private-stage")), Some("other"));
}

struct BudgetGuard {
    inner: Arc<dyn UpstreamAdmissionGuard>,
    remaining: Arc<Semaphore>,
}

impl UpstreamAdmissionGuard for BudgetGuard {
    fn check(&self) -> Result<(), ServiceError> {
        self.inner.check().map_err(safe_error)?;
        self.remaining
            .clone()
            .try_acquire_owned()
            .map_err(|_| {
                native_error(
                    429,
                    "acceptance_send_budget_exhausted",
                    "Acceptance send budget reached",
                )
            })?
            .forget();
        Ok(())
    }
    fn observe_headers(&self, headers: &axum::http::HeaderMap, started: Instant) {
        self.inner.observe_headers(headers, started);
    }
    fn observe_event(&self, event: &Value, started: Instant) {
        self.inner.observe_event(event, started);
    }
    fn observe_rate_limits(
        &self,
        snapshot: native_api::rate_limits::QuotaSnapshot,
        started: Instant,
    ) {
        self.inner.observe_rate_limits(snapshot, started);
    }
}

struct ProviderHost {
    inner: NativeHostBridge,
    account_id: String,
    allowed_models: Vec<String>,
    remaining_routes: Arc<Semaphore>,
    remaining_sends: Arc<Semaphore>,
    reports: Mutex<std::fs::File>,
}

impl ProviderHost {
    fn evidence(&self, value: Value) {
        if let Ok(mut file) = self.reports.lock() {
            if writeln!(file, "{value}")
                .and_then(|_| file.flush())
                .is_err()
            {
                eprintln!("PROVIDER_ACCEPTANCE_REPORT_WRITE_FAILED");
            }
        }
    }

    fn canonical_model(&self, requested_model: &str) -> Result<String, ServiceError> {
        let collection = self.inner.collection_override.as_ref().ok_or_else(|| {
            native_error(
                403,
                "acceptance_scope_missing",
                "Acceptance collection is unavailable",
            )
        })?;
        let key = collection.api_keys.first().ok_or_else(|| {
            native_error(
                403,
                "acceptance_scope_missing",
                "Acceptance model scope is unavailable",
            )
        })?;
        let canonical = native_model_for_key(collection, Some(key), requested_model)?;
        if MODELS.contains(&requested_model) && canonical != requested_model {
            return Err(native_error(
                403,
                "acceptance_model_redirect_not_allowed",
                "Acceptance requires the selected model's original identity",
            ));
        }
        if !self.allowed_models.contains(&canonical) {
            return Err(native_error(
                403,
                "acceptance_model_outside_scope",
                "Model is outside the selected provider scope",
            ));
        }
        Ok(canonical)
    }

    fn candidate(&self, request: &RouteRequest) -> Result<(), ServiceError> {
        self.canonical_model(&request.model)?;
        if request.image_generation_requested
            || request.auxiliary.is_some()
            || request
                .target_account_id
                .as_deref()
                .is_some_and(|id| id != self.account_id)
        {
            return Err(native_error(
                403,
                "acceptance_request_outside_scope",
                "Request is outside the selected provider scope",
            ));
        }
        self.remaining_routes
            .clone()
            .try_acquire_owned()
            .map_err(|_| {
                native_error(
                    429,
                    "acceptance_request_budget_exhausted",
                    "Acceptance request budget reached",
                )
            })?
            .forget();
        Ok(())
    }

    fn check_route(&self, route: &Route, requested_model: &str) -> Result<(), ServiceError> {
        let canonical = self.canonical_model(requested_model)?;
        if route.account_id != self.account_id
            || route.model != canonical
            || !self.allowed_models.contains(&route.model)
            || route.chatgpt
            || route.chatgpt_plan
            || route.official_registration_id.is_some()
            || route.official_fallback_registration_id.is_some()
            || route.wire_api != WireApi::Chat
            || !destination_allowed(&route.base_url)
            || route.send_guard.is_none()
        {
            return Err(native_error(
                403,
                "acceptance_route_outside_scope",
                "Route is outside the selected provider scope",
            ));
        }
        Ok(())
    }

    fn bounded_route(
        &self,
        mut route: Route,
        requested_model: &str,
    ) -> Result<Route, ServiceError> {
        self.check_route(&route, requested_model)?;
        let guard = route.send_guard.take().ok_or_else(|| {
            native_error(
                403,
                "acceptance_admission_guard_missing",
                "Production admission guard is required",
            )
        })?;
        route.send_guard = Some(Arc::new(BudgetGuard {
            inner: guard,
            remaining: self.remaining_sends.clone(),
        }));
        Ok(route)
    }
}

impl Host for ProviderHost {
    fn authorize<'a>(&'a self, bearer: &'a str) -> BoxFuture<'a, Result<Caller, ServiceError>> {
        Box::pin(async move { self.inner.authorize(bearer).await.map_err(safe_error) })
    }
    fn models<'a>(&'a self, caller: &'a Caller) -> BoxFuture<'a, Result<Value, ServiceError>> {
        Box::pin(async move { self.inner.models(caller).await.map_err(safe_error) })
    }
    fn route<'a>(
        &'a self,
        caller: &'a Caller,
        request: RouteRequest,
    ) -> BoxFuture<'a, Result<Route, ServiceError>> {
        Box::pin(async move {
            self.candidate(&request)?;
            let model = request.model.clone();
            let result = self
                .inner
                .route(caller, request)
                .await
                .and_then(|route| self.bounded_route(route, &model));
            self.evidence(json!({"stage":"routing","success":result.is_ok(),"status":result.as_ref().err().map(|e| e.status)}));
            result.map_err(safe_error)
        })
    }
    fn refresh_route<'a>(
        &'a self,
        caller: &'a Caller,
        previous: &'a Route,
        request: RouteRequest,
    ) -> BoxFuture<'a, Result<Route, ServiceError>> {
        Box::pin(async move {
            self.candidate(&request)?;
            self.check_route(previous, &request.model)?;
            let model = request.model.clone();
            self.inner
                .refresh_route(caller, previous, request)
                .await
                .and_then(|route| self.bounded_route(route, &model))
                .map_err(safe_error)
        })
    }
    fn capability_fallback_decision<'a>(
        &'a self,
        caller: &'a Caller,
        route: &'a Route,
        request: &'a RouteRequest,
    ) -> BoxFuture<'a, Result<CapabilityFallbackDecision, ServiceError>> {
        Box::pin(async move {
            self.check_route(route, &request.model)?;
            let result = self
                .inner
                .capability_fallback_decision(caller, route, request)
                .await;
            self.evidence(json!({"stage":"capability_preflight","success":result.is_ok(),
                "required":request.required_capabilities.iter().map(|v| capability(v)).collect::<Vec<_>>(),
                "enabled":result.as_ref().ok().map(|v| v.enabled),
                "unsupported":result.as_ref().ok().map(|v| v.unsupported.iter().map(|v| capability(v)).collect::<Vec<_>>()),
                "status":result.as_ref().err().map(|e| e.status)}));
            result.map_err(safe_error)
        })
    }
    fn prepare_body<'a>(
        &'a self,
        caller: &'a Caller,
        route: &'a Route,
        headers: &'a axum::http::HeaderMap,
        body: Value,
        image: bool,
        compact: bool,
    ) -> BoxFuture<'a, Result<Value, ServiceError>> {
        self.prepare_body_for_model(caller, route, &route.model, headers, body, image, compact)
    }
    fn prepare_body_for_model<'a>(
        &'a self,
        caller: &'a Caller,
        route: &'a Route,
        requested_model: &'a str,
        headers: &'a axum::http::HeaderMap,
        body: Value,
        image: bool,
        compact: bool,
    ) -> BoxFuture<'a, Result<Value, ServiceError>> {
        Box::pin(async move {
            self.check_route(route, &route.model)?;
            self.evidence(json!({"stage":"prepare","tools":shapes(body.get("tools")),"history":shapes(body.get("input"))}));
            let requested_effort = effort_shape(&body);
            let result = self.inner
                .prepare_body_for_model(caller, route, requested_model, headers, body, image, compact)
                .await
                .map_err(safe_error)?;
            self.evidence(json!({"stage":"reasoning","requested":requested_effort,"effective":effort_shape(&result)}));
            Ok(result)
        })
    }
    fn multi_agent_policy<'a>(
        &'a self,
        caller: &'a Caller,
        route: Option<&'a Route>,
        model: &'a str,
    ) -> BoxFuture<'a, Result<MultiAgentPolicy, ServiceError>> {
        self.inner.multi_agent_policy(caller, route, model)
    }
    fn token_count_model<'a>(
        &'a self,
        caller: &'a Caller,
        model: &'a str,
    ) -> BoxFuture<'a, Result<String, ServiceError>> {
        self.inner.token_count_model(caller, model)
    }
    fn execution_policy(&self) -> BoxFuture<'_, Result<ExecutionPolicy, ServiceError>> {
        self.inner.execution_policy()
    }
    fn websockets_enabled(&self) -> BoxFuture<'_, Result<bool, ServiceError>> {
        self.inner.websockets_enabled()
    }
    fn responses_websocket_passthrough<'a>(
        &'a self,
        caller: &'a Caller,
        route: &'a Route,
    ) -> BoxFuture<'a, Result<bool, ServiceError>> {
        self.inner.responses_websocket_passthrough(caller, route)
    }
    fn invalidate_transport(&self, identity: &str) -> bool {
        self.inner.invalidate_transport(identity)
    }
    fn set_cooling_disabled(&self, disabled: bool) {
        self.inner.set_cooling_disabled(disabled);
    }
    fn set_session_affinity_enabled(&self, enabled: bool) {
        self.inner.set_session_affinity_enabled(enabled);
    }
    fn quota_request_started(&self, account: &str) {
        self.inner.quota_request_started(account);
    }
    fn check_quota_before_send(&self, account: &str) -> Result<(), ServiceError> {
        if account != self.account_id {
            return Err(native_error(
                403,
                "acceptance_account_outside_scope",
                "Account is outside the selected provider scope",
            ));
        }
        self.inner
            .check_quota_before_send(account)
            .map_err(safe_error)
    }
    fn observe_rate_limits(
        &self,
        account: &str,
        snapshot: native_api::rate_limits::QuotaSnapshot,
        started: Instant,
    ) {
        self.inner.observe_rate_limits(account, snapshot, started);
    }
    fn route_failed_attempt<'a>(
        &'a self,
        route: &'a Route,
        model: &'a str,
        status: u16,
        wait: Option<Duration>,
        credential_scope: bool,
        message: Option<&'a str>,
    ) -> BoxFuture<'a, ()> {
        self.inner
            .route_failed_attempt(route, model, status, wait, credential_scope, message)
    }
    fn route_failed_with_context<'a>(
        &'a self,
        account: &'a str,
        model: &'a str,
        status: u16,
        wait: Option<Duration>,
        credential_scope: bool,
        message: Option<&'a str>,
    ) -> BoxFuture<'a, ()> {
        self.inner.route_failed_with_context(
            account,
            model,
            status,
            wait,
            credential_scope,
            message,
        )
    }
    fn retry_cooldown_wait<'a>(
        &'a self,
        caller: &'a Caller,
        request: RouteRequest,
        max_wait: Duration,
    ) -> BoxFuture<'a, Result<Option<Duration>, ServiceError>> {
        self.inner.retry_cooldown_wait(caller, request, max_wait)
    }
    fn record(&self, report: RequestReport) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            self.inner.record_failure_cooldown(&report);
            self.inner.record_soft_binding_result(&report).await;
            // Do not delegate production record/diagnostic: they persist usage,
            // account identities and potentially upstream text to user storage.
            self.evidence(json!({"stage":"completed","status":report.status,"success":report.success,
                "failure_stage":failure_stage(report.diagnostics.as_ref().and_then(|d| d.failure_phase.as_deref())),
                "attempts":report.diagnostics.as_ref().map(|d| d.attempts.iter().map(|a| json!({
                    "status":a.status,"success":a.success,"failure_stage":failure_stage(a.failure_phase.as_deref())
                })).collect::<Vec<_>>())}));
        })
    }
}

fn private_new_file(path: &Path) -> Result<std::fs::File, &'static str> {
    let mut options = std::fs::OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
        .open(path)
        .map_err(|_| "Unable to create private acceptance output")
}

fn read_config() -> Result<Config, &'static str> {
    let path = PathBuf::from(
        std::env::var_os("COCKPIT_NATIVE_PROVIDER_LIVE_CONFIG")
            .ok_or("Missing private provider acceptance configuration")?,
    );
    let metadata =
        std::fs::symlink_metadata(&path).map_err(|_| "Cannot inspect acceptance configuration")?;
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > 16_384 {
        return Err("Invalid acceptance configuration file");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err("Acceptance configuration must be private");
        }
    }
    let config: Config = serde_json::from_slice(
        &std::fs::read(path).map_err(|_| "Cannot read acceptance configuration")?,
    )
    .map_err(|_| "Invalid acceptance configuration")?;
    if config.key.len() < 32
        || config.key.len() > 256
        || !(1..=32).contains(&config.max_requests)
        || config.allowed_models.is_empty()
        || config.allowed_models.len() > MODELS.len()
        || config
            .allowed_models
            .iter()
            .any(|m| !MODELS.contains(&m.as_str()))
        || config.account_id.is_empty()
        || (config.manual_low && !config.allowed_models.iter().any(|model| model == "step-3.7-flash"))
        || !config
            .account_id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-'))
    {
        return Err("Invalid provider scope, local key or request budget");
    }
    Ok(config)
}

/// Use the production client account/catalog producers, never a hand-written
/// `wire_api` override in the accepting Codex process. Full switch/update/sync
/// publication is independently exercised by the synthetic profile exporter.
/// This bounded driver cannot call that entrypoint: doing so could refresh the
/// real auxiliary OAuth identity and start another, unbounded listener.
fn write_acceptance_client_profile(
    profile: &Path,
    collection: &CodexLocalAccessCollection,
    account: &CodexAccount,
    local_key: &str,
    port: u16,
    model: &str,
) -> Result<(), &'static str> {
    let mut published = collection.clone();
    published.port = port;
    let client = build_runtime_account(
        build_collection_base_url(&published),
        local_key.to_owned(),
        // Only the ephemeral local credential belongs in this isolated client.
        // Original binding metadata remains on the unchanged routing snapshot.
        None,
        profile_api_key_supports_websockets(&published, local_key),
    );
    codex_account::write_account_bundle_to_dir(profile, &client)
        .map_err(|_| "Cannot publish the production client account profile")?;
    write_native_provider_profile_model_catalog(profile, &published, local_key, account)
        .map_err(|_| "Cannot publish the production provider model catalog")?;
    write_local_access_profile_model_override(profile, model)
        .map_err(|_| "Cannot select the acceptance model")?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "Explicit COCKPIT_NATIVE_PROVIDER_LIVE_CONFIG; sends bounded real StepFun API-key requests"]
async fn native_provider_process_driver() -> Result<(), &'static str> {
    let config = read_config()?;
    // Test binaries normally use a fixture data directory. Requiring this
    // explicit existing directory prevents accidentally creating/migrating one.
    let data = PathBuf::from(
        std::env::var_os("COCKPIT_TOOLS_TEST_DATA_DIR")
            .ok_or("Explicit existing account data directory required")?,
    )
    .canonicalize()
    .map_err(|_| "Account data directory does not exist")?;
    if !data.is_dir()
        || account::get_data_dir()
            .map_err(|_| "Account data directory unavailable")?
            .canonicalize()
            .map_err(|_| "Account data directory unavailable")?
            != data
    {
        return Err("Account data directory mismatch");
    }
    if codex_account::account_storage_status(&config.account_id)
        .map_err(|_| "Cannot validate account storage status")?
        != codex_account::CodexAccountStorageStatus::Present
    {
        return Err("Selected account is unavailable or revoked");
    }
    let path = data
        .join("codex_accounts")
        .join(format!("{}.json", config.account_id));
    let metadata =
        std::fs::symlink_metadata(&path).map_err(|_| "Cannot inspect selected account")?;
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > 4 << 20 {
        return Err("Invalid selected account file");
    }
    let content = std::fs::read_to_string(&path).map_err(|_| "Cannot read selected account")?;
    // Both decoders are read-only; unlike load_account they never schedule
    // plaintext/envelope migration or auto-restore a damaged account file.
    let account: CodexAccount = match serde_json::from_str(&content) {
        Ok(account) => account,
        Err(_) => crate::modules::secure_account_storage::deserialize_encrypted_snapshot(&content)
            .map_err(|_| "Cannot decode selected account without migration")?,
    };
    if account.id != config.account_id || !eligible_stepfun_account(&account) {
        return Err("Selected account is not an eligible StepFun Chat API-key account");
    }
    let mut gateway = provider_gateway_for_account(&account)
        .map_err(|_| "Cannot prepare saved provider configuration")?;
    gateway.model_bindings = Some(Vec::new());
    if config
        .allowed_models
        .iter()
        .any(|model| !gateway.upstream_models.contains(model))
    {
        return Err("Acceptance model is absent from the saved account catalog");
    }
    let template: CodexLocalAccessCollection = serde_json::from_slice(
        &std::fs::read(local_access_file_path().map_err(|_| "Saved template unavailable")?)
            .map_err(|_| "Cannot read saved template")?,
    )
    .map_err(|_| "Invalid saved template")?;
    if template.gateway_mode != CodexLocalAccessGatewayMode::Native
        || template.credits_policy
            != crate::models::codex_local_access::CodexLocalAccessCreditsPolicy::PreferRegular
        || template
            .official_account_bindings
            .contains_key(&config.account_id)
    {
        return Err(
            "Acceptance requires the saved Native/prefer_regular policy and no official binding",
        );
    }
    // Same provider collection construction as the profile gateway, without
    // touching a user profile or calling sanitize_collection's migrating reads.
    let mut collection =
        new_empty_local_access_collection().map_err(|_| "Cannot create acceptance collection")?;
    apply_provider_gateway_template_settings(&mut collection, &template);
    // Preserve the saved auxiliary binding exactly as the production provider
    // builder does, without admitting it to this listener's account scope.
    collection.bound_oauth_account_id =
        provider_gateway_bound_oauth_account_id_for_account(&account);
    collection.enabled = true;
    collection.port = 0;
    collection.api_key = config.key.clone();
    collection.debug_logs = false;
    collection.request_payload_logging = false;
    collection.responses_websockets_enabled = template.responses_websockets_enabled;
    // Real acceptance uses raw IDs. Synthetic regressions verify legacy alias
    // migration without writing to the user's persistent profile state.
    let client_models = config.allowed_models.clone();
    collection.api_keys.push(CodexLocalAccessApiKey {
        provider_model_bindings: None,
        id: "provider-live-acceptance".into(),
        label: "Provider acceptance".into(),
        key: config.key,
        provider_gateway: Some(gateway),
        model_routing: None,
        inherit_account_pool: Some(false),
        account_ids: vec![config.account_id.clone()],
        priority_account_ids: Vec::new(),
        preferred_account_id: None,
        model_prefix: None,
        allowed_models: client_models,
        excluded_models: Vec::new(),
        token_limit: None,
        token_used: 0,
        enabled: true,
        created_at: now_ms(),
        updated_at: now_ms(),
        last_used_at: None,
    });
    credits_compat::validate_mode(&collection, CodexLocalAccessGatewayMode::Native)
        .map_err(|_| "Native Credits validation failed")?;
    if credits_compat::validate_mode(&collection, CodexLocalAccessGatewayMode::Sidecar).is_ok() {
        return Err("Credits engine-switch protection is unexpectedly disabled");
    }
    native_publish_credits_settings(&collection);
    let parent = config
        .output_dir
        .parent()
        .ok_or("Private output directory must have a parent")?
        .canonicalize()
        .map_err(|_| "Output parent must exist")?;
    let temp = std::env::temp_dir()
        .canonicalize()
        .map_err(|_| "Temporary directory unavailable")?;
    let private_tmp = Path::new("/private/tmp").canonicalize().ok();
    if !config.output_dir.is_absolute()
        || (!parent.starts_with(&temp)
            && !private_tmp
                .as_ref()
                .is_some_and(|tmp| parent.starts_with(tmp)))
    {
        return Err("Output directory must be new and inside the system temporary directory");
    }
    let mut builder = std::fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder
        .create(&config.output_dir)
        .map_err(|_| "Output directory must be new")?;
    let client_profile = config.output_dir.join("codex-home");
    let profile_collection = collection.clone();
    let profile_account = account.clone();
    let profile_key = collection.api_key.clone();
    let profile_model = config.allowed_models[0].clone();
    let manual_low = config.manual_low;
    let mut inner = NativeHostBridge::for_collection(collection);
    inner.profile_dir = Some(client_profile.clone());
    inner.account_overrides.insert(account.id.clone(), account);
    let host = Arc::new(ProviderHost {
        inner,
        account_id: config.account_id,
        allowed_models: config.allowed_models,
        remaining_routes: Arc::new(Semaphore::new(config.max_requests)),
        remaining_sends: Arc::new(Semaphore::new(config.max_requests)),
        reports: Mutex::new(private_new_file(&config.output_dir.join("reports.jsonl"))?),
    });
    host.evidence(
        json!({"stage":"ready","production_host":true,"saved_policy":"prefer_regular",
        "saved_engine":"native","sidecar_blocked":true,"account_snapshot":"read_only"}),
    );
    let mut handle = native_api::start(
        "127.0.0.1:0"
            .parse()
            .map_err(|_| "Invalid listener address")?,
        host,
        config.output_dir.join("history.sqlite"),
        native_api::Options::default(),
    )
    .await
    .map_err(|_| "Cannot start provider acceptance listener")?;
    let port = handle.addr.port();
    let profile_result = tokio::task::spawn_blocking(move || {
        write_acceptance_client_profile(
            &client_profile,
            &profile_collection,
            &profile_account,
            &profile_key,
            port,
            &profile_model,
        )?;
        if manual_low {
            codex_account::save_provider_model_quick_config(
                &client_profile, &profile_account, true,
                vec![crate::models::codex::CodexExperimentalModelDefinition {
                    model_id: "step-3.7-flash".into(), display_name: "Step 3.7 Flash".into(),
                    reasoning_efforts: Some(vec!["low".into()]), default_reasoning_effort: Some("low".into()),
                    context_window: None, auto_compact_token_limit: None,
                }], None,
            ).map_err(|_| "Cannot save the temporary provider reasoning policy")?;
            let mut published = profile_collection.clone();
            published.port = port;
            write_native_provider_profile_model_catalog(
                &client_profile, &published, &profile_key, &profile_account,
            ).map_err(|_| "Cannot publish the temporary provider reasoning policy")?;
        }
        Ok::<(), &'static str>(())
    })
    .await
    .map_err(|_| "Production acceptance profile worker failed")?;
    if let Err(error) = profile_result {
        let _ = handle.shutdown(Duration::from_secs(10)).await;
        crate::modules::codex_proxy_engine::shutdown_all();
        return Err(error);
    }
    let mut ready = private_new_file(&config.output_dir.join("ready.json"))?;
    write!(
        ready,
        "{}",
        json!({"port":port,"client_profile":"codex-home","production_client_projection":true})
    )
    .and_then(|_| ready.flush())
    .map_err(|_| "Cannot publish provider acceptance port")?;
    let (send, mut receive) = tokio::sync::mpsc::channel(1);
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines() {
            match line {
                Ok(line) if line.trim() != "stop" => continue,
                _ => break,
            }
        }
        let _ = send.blocking_send(());
    });
    let _ = receive.recv().await;
    let stopped = handle.shutdown(Duration::from_secs(10)).await;
    crate::modules::codex_proxy_engine::shutdown_all();
    stopped.map_err(|_| "Provider acceptance shutdown failed")?;
    Ok(())
}

#[test]
fn native_provider_live_profile_uses_production_client_projection() {
    let profile = tempfile::tempdir().unwrap();
    let mut account = CodexAccount::new_api_key(
        "stepfun-fixture".into(),
        "fixture@example.invalid".into(),
        "synthetic-upstream-secret".into(),
        crate::models::codex::CodexApiProviderMode::Custom,
        Some("https://api.stepfun.com/step_plan/v1".into()),
        None,
        None,
        vec!["step-5-preview".into()],
    );
    account.api_wire_api = Some("chat_completions".into());
    account.bound_oauth_account_id = Some("auxiliary-oauth-fixture".into());
    let mut collection: CodexLocalAccessCollection = serde_json::from_value(json!({
        "enabled":true,"port":0,"apiKey":"synthetic-local-key","accountIds":[],
        "gatewayMode":"native","creditsPolicy":"prefer_regular","createdAt":1,"updatedAt":1,
        "apiKeys":[{"id":"provider-live-acceptance","label":"fixture","key":"synthetic-local-key",
            "inheritAccountPool":false,"accountIds":["stepfun-fixture"],"allowedModels":["step-5-preview"],
            "providerGateway":{"baseUrl":"https://api.stepfun.com/step_plan/v1","apiKey":"synthetic-upstream-secret",
                "upstreamModel":"step-5-preview","upstreamModels":["step-5-preview"],"wireApi":"chat_completions"}}]
    })).unwrap();
    collection.bound_oauth_account_id = account.bound_oauth_account_id.clone();
    write_acceptance_client_profile(
        profile.path(),
        &collection,
        &account,
        "synthetic-local-key",
        61234,
        "step-5-preview",
    )
    .unwrap();
    let content = std::fs::read_to_string(profile.path().join("config.toml")).unwrap();
    let doc =
        crate::modules::codex_config_format::read_codex_config_doc_from_str(&content).unwrap();
    let provider = &doc["model_providers"][CODEX_LOCAL_ACCESS_RUNTIME_PROVIDER_ID];
    assert_eq!(provider["wire_api"].as_str(), Some("responses"));
    let expected_base = build_base_url_with_host(61234, collection.client_base_url_host);
    assert_eq!(provider["base_url"].as_str(), Some(expected_base.as_str()));
    assert_eq!(doc["model"].as_str(), Some("step-5-preview"));
    let auth = std::fs::read_to_string(profile.path().join("auth.json")).unwrap();
    assert!(auth.contains("synthetic-local-key"));
    assert!(!auth.contains("synthetic-upstream-secret"));
    assert!(!content.contains("synthetic-upstream-secret"));
    assert_eq!(account.api_wire_api.as_deref(), Some("chat_completions"));
    assert_eq!(
        collection.bound_oauth_account_id,
        account.bound_oauth_account_id
    );
    assert!(profile
        .path()
        .join(CODEX_MANAGED_MODEL_CATALOG_FILE)
        .is_file());
}

#[tokio::test]
async fn native_provider_live_bound_metadata_keeps_original_api_key_route() {
    let mut account = CodexAccount::new_api_key(
        "stepfun-fixture".into(),
        "fixture@example.invalid".into(),
        "synthetic-stepfun-key".into(),
        crate::models::codex::CodexApiProviderMode::Custom,
        Some("https://api.stepfun.com/step_plan/v1".into()),
        None,
        None,
        vec!["step-5-preview".into()],
    );
    account.api_wire_api = Some("chat_completions".into());
    account.egress_proxy_disabled = true;
    account.bound_oauth_account_id = Some("auxiliary-oauth-fixture".into());
    account.tokens.access_token = "synthetic-unused-oauth-token".into();
    assert!(eligible_stepfun_account(&account));
    assert!(!native_credits_applies(&account));

    let mut collection: CodexLocalAccessCollection = serde_json::from_value(json!({
        "enabled":true,"port":0,"apiKey":"synthetic-local-key","accountIds":[],
        "gatewayMode":"native","creditsPolicy":"prefer_regular","createdAt":1,"updatedAt":1,
        "apiKeys":[{"id":"provider-live-acceptance","label":"fixture","key":"synthetic-local-key",
            "inheritAccountPool":false,"accountIds":["stepfun-fixture"],"allowedModels":["step-5-preview"],
            "providerGateway":{"baseUrl":"https://api.stepfun.com/step_plan/v1","apiKey":"synthetic-stepfun-key",
                "upstreamModel":"step-5-preview","upstreamModels":["step-5-preview"],"wireApi":"chat_completions"}}]
    })).unwrap();
    collection.bound_oauth_account_id =
        provider_gateway_bound_oauth_account_id_for_account(&account);
    assert_eq!(
        collection.bound_oauth_account_id,
        account.bound_oauth_account_id
    );
    assert!(
        credits_compat::validate_mode(&collection, CodexLocalAccessGatewayMode::Sidecar).is_err()
    );
    let mut inner = NativeHostBridge::for_collection(collection);
    inner
        .account_overrides
        .insert(account.id.clone(), account.clone());
    let host = ProviderHost {
        inner,
        account_id: account.id.clone(),
        allowed_models: vec!["step-5-preview".into()],
        remaining_routes: Arc::new(Semaphore::new(4)),
        remaining_sends: Arc::new(Semaphore::new(4)),
        reports: Mutex::new(tempfile::tempfile().unwrap()),
    };
    let caller = host.authorize("synthetic-local-key").await.unwrap();
    let collection = host.inner.collection_override.as_ref().unwrap();
    assert_eq!(
        native_scoped_account_ids(collection, collection.api_keys.first()),
        vec![account.id.clone()]
    );
    // Production route preparation builds a client but sends no HTTP request.
    let mut route = host
        .route(
            &caller,
            RouteRequest {
                model: "step-5-preview".into(),
                session_id: None,
                thread_id: None,
                parent_thread_id: None,
                preferred_account_id: None,
                soft_preferred_account_id: None,
                target_account_id: None,
                force_refresh: false,
                image_generation_requested: false,
                image_input: false,
                excluded_account_ids: vec![],
                requires_permit: false,
                auxiliary: None,
                required_capabilities: vec!["function".into()],
                capability_fallback_from: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(route.account_id, account.id);
    assert_eq!(route.headers[AUTHORIZATION], "Bearer synthetic-stepfun-key");
    assert!(!route.chatgpt && !route.chatgpt_plan);
    assert_eq!(route.wire_api, WireApi::Chat);
    assert!(route.send_guard.as_ref().unwrap().check().is_ok());
    assert_eq!(
        host.inner.account_overrides[&account.id].bound_oauth_account_id,
        account.bound_oauth_account_id
    );
    host.check_route(&route, "step-5-preview").unwrap();
    route.account_id = account.bound_oauth_account_id.unwrap();
    assert_eq!(
        host.check_route(&route, "step-5-preview").unwrap_err().code,
        "acceptance_route_outside_scope"
    );
}
