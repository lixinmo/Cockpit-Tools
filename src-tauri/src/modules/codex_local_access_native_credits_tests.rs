// Production pool/guard tests: synthetic accounts, no live usage or inference.
use super::*;
use crate::models::codex_local_access::CodexLocalAccessCreditsPolicy as Policy;

fn account(remaining: f64) -> CodexAccount {
    use base64::Engine as _;
    let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(json!({"exp":chrono::Utc::now().timestamp()+86400}).to_string());
    let mut account = CodexAccount::new(
        uuid::Uuid::new_v4().to_string(),
        "credits@example.invalid".into(),
        crate::models::codex::CodexTokens {
            access_token: format!("e30.{payload}.synthetic"),
            id_token: "synthetic".into(),
            refresh_token: None,
        },
    );
    account.account_id = Some(uuid::Uuid::new_v4().to_string());
    account.user_id = Some("synthetic-user".into());
    account.egress_proxy_disabled = true;
    account.quota = Some(serde_json::from_value(json!({
        "hourly_percentage": remaining.ceil() as i32, "hourly_window_present": true,
        "hourly_reset_time": chrono::Utc::now().timestamp() + 3600,
        "weekly_percentage": 0, "weekly_window_present": false,
        "raw_data": {"rate_limit": {"primary_window": {"used_percent": 100.0 - remaining}, "secondary_window": null}}
    })).unwrap());
    account.usage_updated_at = Some(chrono::Utc::now().timestamp());
    account
}

fn host(policy: Policy, accounts: &[CodexAccount]) -> NativeHostBridge {
    let mut collection: CodexLocalAccessCollection = serde_json::from_value(json!({
        "enabled": true, "port": 1455, "apiKey": "synthetic", "createdAt": 1, "updatedAt": 1,
        "accountIds": accounts.iter().map(|a| &a.id).collect::<Vec<_>>()
    }))
    .unwrap();
    collection.credits_policy = policy;
    let mut host = NativeHostBridge::for_collection(collection);
    host.account_overrides = accounts
        .iter()
        .cloned()
        .map(|a| (a.id.clone(), a))
        .collect();
    host
}

#[tokio::test]
async fn native_credits_complete_pool_preserves_paused_and_excluded_evidence() {
    let exhausted = account(0.0);
    let positive = account(6.0);
    let mut host = host(
        Policy::PreferRegular,
        &[exhausted.clone(), positive.clone()],
    );
    host.collection_override
        .as_mut()
        .unwrap()
        .paused_account_ids = vec![positive.id.clone()];
    let collection = host.collection().await.unwrap();
    let ids = native_credits_scope(&collection, None);
    assert!(ids.contains(&positive.id));
    // The scheduler's candidate slice has already lost the positive member.
    let pool = native_credits_pool(&host, &collection, ids, &[exhausted.clone()])
        .await
        .unwrap();
    assert_eq!(
        pool.code(&exhausted.id, false),
        Some("credits_policy_blocked")
    );
    assert!(!pool.all_exhausted());
    assert!(pool
        .statuses()
        .iter()
        .any(|m| m.account_id == positive.id && m.reason_code == "paused"));
}

#[tokio::test]
async fn native_credits_image_pool_does_not_expand_to_conversation_catalog() {
    let image = account(0.0);
    let text = account(50.0);
    let host = host(Policy::PreferRegular, &[image.clone(), text.clone()]);
    let collection = host.collection().await.unwrap();
    let pool = native_credits_pool(
        &host,
        &collection,
        vec![image.id.clone()],
        &[image.clone(), text],
    )
    .await
    .unwrap();
    assert_eq!(pool.members.len(), 1);
    assert!(pool.all_exhausted());
    assert_eq!(pool.code(&image.id, false), None);
}

#[tokio::test]
async fn native_credits_per_account_denial_survives_complete_pool_exhaustion() {
    let source = account(0.0);
    let mut host = host(Policy::PreferRegular, &[source.clone()]);
    host.collection_override
        .as_mut()
        .unwrap()
        .account_credits_allowed
        .insert(source.id.clone(), false);
    let collection = host.collection().await.unwrap();
    let pool = native_credits_pool(
        &host,
        &collection,
        collection.account_ids.clone(),
        &[source.clone()],
    )
    .await
    .unwrap();
    assert!(pool.all_exhausted());
    assert_eq!(pool.code(&source.id, false), Some("credits_policy_blocked"));
}

#[tokio::test]
async fn native_credits_official_projection_cannot_borrow_source_quota() {
    let source = account(50.0);
    let host = host(Policy::StrictRegular, &[source.clone()]);
    let collection = host.collection().await.unwrap();
    let mut official = source.clone();
    official.api_provider_id = Some("cockpit_chatgpt_plan".into());
    let pool = native_credits_pool(
        &host,
        &collection,
        collection.account_ids.clone(),
        &[official],
    )
    .await
    .unwrap();
    assert_eq!(
        pool.code(&source.id, true),
        Some("credits_quota_unconfirmed")
    );
    assert!(pool.statuses()[0]
        .reason_message
        .contains("source=official_unconfirmed"));
    let local = native_credits_pool(
        &host,
        &collection,
        collection.account_ids.clone(),
        &[source.clone()],
    )
    .await
    .unwrap();
    assert_eq!(local.code(&source.id, false), None);
    assert_eq!(
        pool.code(&source.id, true),
        Some("credits_quota_unconfirmed")
    );
}

#[tokio::test]
async fn native_credits_send_guard_observes_exhaustion_and_preserves_other_window() {
    let source = account(2.0);
    let host = host(Policy::StrictRegular, &[source.clone()]);
    let collection = host.collection().await.unwrap();
    let pool = native_credits_pool(
        &host,
        &collection,
        collection.account_ids.clone(),
        &[source.clone()],
    )
    .await
    .unwrap();
    let guard = NativeCreditsGuard {
        pool,
        account_id: source.id.clone(),
        official: false,
        identity: Some(native_credits_identity(&source)),
        identity_changed: false,
    };
    use native_contract::UpstreamAdmissionGuard;
    guard.check().unwrap();
    let mut headers = axum::http::HeaderMap::new();
    headers.insert("x-codex-limit-reached", "true".parse().unwrap());
    guard.observe_headers(&headers, Instant::now());
    assert_eq!(guard.check().unwrap_err().code, "credits_policy_blocked");
    assert_eq!(
        NATIVE_CREDITS_REFRESH
            .snapshot(&native_credits_identity(&source))
            .secondary
            .unwrap()
            .present,
        false
    );
}

#[tokio::test]
async fn native_credits_snapshot_isolates_workspace_and_recovers_only_host_cooldown() {
    let source = account(0.0);
    let host = host(Policy::StrictRegular, &[source.clone()]);
    let collection = host.collection().await.unwrap();
    let _pool = native_credits_pool(
        &host,
        &collection,
        collection.account_ids.clone(),
        &[source.clone()],
    )
    .await
    .unwrap();
    let mut fresh =
        native_credits_snapshot(source.quota.as_ref().unwrap(), now_ms() + 1, "refresh");
    fresh.primary.as_mut().unwrap().remaining = Some(0.25);
    NATIVE_CREDITS_REFRESH.merge(&native_credits_identity(&source), fresh);
    assert!(native_quota_allows(&collection, &source));
    let mut different = source.clone();
    different.account_id = Some("different-workspace".into());
    assert!(!native_quota_allows(&collection, &different));
    // The helper never touches host.cooldowns (runtime 429/model evidence).
    assert!(host.cooldowns.lock().unwrap().is_empty());
}

#[test]
fn native_credits_subscriber_authority_is_not_dropped_for_web_session_accounts() {
    let mut source = account(10.0);
    assert!(native_credits_applies(&source));
    source.token_source_mode = "chatgpt_web_session".into();
    assert!(native_credits_applies(&source));
}

#[test]
fn native_credits_host_reread_cannot_renew_a_get_that_preceded_exhaustion() {
    let source = account(50.0);
    let started = Instant::now() - Duration::from_secs(1);
    confirm_native_credits_quota(&source, started);
    let identity = native_credits_identity(&source);
    let mut exhausted =
        native_credits_snapshot(source.quota.as_ref().unwrap(), now_ms(), "response");
    exhausted.primary.as_mut().unwrap().remaining = Some(0.0);
    NATIVE_CREDITS_REFRESH.merge(&identity, exhausted);
    let reread = native_credits_host_snapshot(&source);
    NATIVE_CREDITS_REFRESH.merge(&identity, reread);
    assert_eq!(
        NATIVE_CREDITS_REFRESH.snapshot(&identity).status(now_ms()),
        CreditsStatus::Exhausted
    );
}

fn request() -> native_contract::RouteRequest {
    native_contract::RouteRequest {
        model: "gpt-6.1-sol".into(),
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
        required_capabilities: vec![],
        capability_fallback_from: None,
    }
}

fn scoped_caller(host: &mut NativeHostBridge) -> native_contract::Caller {
    let id = uuid::Uuid::new_v4().to_string();
    host.collection_override.as_mut().unwrap().api_keys = vec![serde_json::from_value(json!({
        "id": id, "label": "synthetic", "key": "synthetic-key", "inheritAccountPool": true,
        "enabled":true, "createdAt":1, "updatedAt":1
    }))
    .unwrap()];
    native_contract::Caller {
        id,
        label: "synthetic".into(),
    }
}

#[tokio::test]
async fn native_credits_actual_route_keeps_full_four_account_diagnostics_after_runtime_cooldown() {
    let mut a = account(50.0);
    a.quota = None;
    a.usage_updated_at = None;
    let mut b = account(50.0);
    b.quota = None;
    b.usage_updated_at = None;
    let c = account(50.0);
    let d = account(50.0);
    // Seed a failed read/backoff: the production refresh path is exercised but
    // its transport cannot run or read a real credential during this fixture.
    for unknown in [&a, &b] {
        NATIVE_CREDITS_REFRESH
            .refresh(native_credits_identity(unknown), || async {
                Err("fixture_refresh_failed")
            })
            .await
            .unwrap_err();
    }
    let mut host = host(
        Policy::StrictRegular,
        &[a.clone(), b.clone(), c.clone(), d.clone()],
    );
    let caller = scoped_caller(&mut host);
    for cooled in [&c, &d] {
        native_upsert_cooldown(
            &mut host.cooldowns.lock().unwrap(),
            &cooled.id,
            "gpt-6.1-sol",
            429,
            Some(Duration::from_secs(60)),
            false,
            Instant::now(),
        );
    }
    let error = match native_select_route_once(&host, &caller, request()).await {
        Err(error) => error,
        Ok(_) => panic!("expected unavailable pool"),
    };
    assert_eq!(error.code, "no_available_account"); // preserve the upstream final classification
    let mut runtime = gateway_runtime().lock().await;
    let health = runtime
        .account_pool_health
        .values()
        .find(|h| h.api_key_id == caller.id)
        .unwrap();
    assert_eq!(health.account_statuses.len(), 4);
    for id in [&a.id, &b.id] {
        assert_eq!(
            health
                .account_statuses
                .iter()
                .find(|m| &m.account_id == id)
                .unwrap()
                .reason_code,
            "credits_quota_unconfirmed"
        );
    }
    for id in [&c.id, &d.id] {
        assert_eq!(
            health
                .account_statuses
                .iter()
                .find(|m| &m.account_id == id)
                .unwrap()
                .reason_code,
            "quota_cooldown"
        );
    }
    assert_eq!(health.available_auths, 0);
    runtime
        .account_pool_health
        .retain(|_, h| h.api_key_id != caller.id);
}

#[tokio::test]
async fn native_credits_actual_route_mixed_unknown_exhausted_prioritizes_unconfirmed() {
    let mut unknown = account(50.0);
    unknown.quota = None;
    unknown.usage_updated_at = None;
    let exhausted = account(0.0);
    NATIVE_CREDITS_REFRESH
        .refresh(native_credits_identity(&unknown), || async {
            Err("fixture_refresh_failed")
        })
        .await
        .unwrap_err();
    let mut host = host(Policy::StrictRegular, &[unknown, exhausted]);
    let caller = scoped_caller(&mut host);
    let error = match native_select_route_once(&host, &caller, request()).await {
        Err(error) => error,
        Ok(_) => panic!("expected policy rejection"),
    };
    assert_eq!(
        (error.status, error.code),
        (503, "credits_quota_unconfirmed")
    );
    gateway_runtime()
        .lock()
        .await
        .account_pool_health
        .retain(|_, h| h.api_key_id != caller.id);
}

#[test]
fn native_credits_complete_usage_limit_reached_overrides_positive_or_missing_percent() {
    let mut source = account(50.0);
    let quota = source.quota.as_mut().unwrap();
    quota.raw_data.as_mut().unwrap()["rate_limit"]["limit_reached"] = json!(true);
    assert_eq!(
        native_credits_snapshot(quota, now_ms(), "refresh").status(now_ms()),
        CreditsStatus::Exhausted
    );
    quota.raw_data.as_mut().unwrap()["rate_limit"]["primary_window"] = json!({});
    assert_eq!(
        native_credits_snapshot(quota, now_ms(), "refresh").status(now_ms()),
        CreditsStatus::Exhausted
    );
}

#[tokio::test]
async fn native_credits_actual_route_concurrency_never_reintroduces_blocked_accounts_and_keeps_429()
{
    let blocked = account(0.0);
    let allowed = account(50.0);
    let mut host = host(Policy::StrictRegular, &[blocked, allowed.clone()]);
    let collection = host.collection_override.as_mut().unwrap();
    collection.max_account_concurrency = 1;
    collection.account_concurrency_wait_ms = 0;
    let caller = scoped_caller(&mut host);
    let collection = host.collection().await.unwrap();
    let (_, held) = native_acquire_account_permit(&host, &collection, &[allowed], true)
        .await
        .unwrap();
    let mut request = request();
    request.requires_permit = true;
    let error = match native_select_route_once(&host, &caller, request).await {
        Err(error) => error,
        Ok(_) => panic!("blocked account must not provide its free slot"),
    };
    assert_eq!((error.status, error.code), (429, "account_busy"));
    drop(held);
    gateway_runtime()
        .lock()
        .await
        .account_pool_health
        .retain(|_, h| h.api_key_id != caller.id);
}

#[tokio::test]
async fn native_credits_actual_route_rejects_revoked_key_before_quota_refresh() {
    let mut unknown = account(50.0);
    unknown.quota = None;
    unknown.usage_updated_at = None;
    let identity = native_credits_identity(&unknown);
    let mut host = host(Policy::StrictRegular, &[unknown]);
    let mut caller = scoped_caller(&mut host);
    caller.id = "revoked-key".into();
    let error = match native_select_route_once(&host, &caller, request()).await {
        Err(error) => error,
        Ok(_) => panic!("revoked key"),
    };
    assert_eq!((error.status, error.code), (401, "invalid_api_key"));
    assert_eq!(
        NATIVE_CREDITS_REFRESH.snapshot(&identity),
        CreditsSnapshot::default()
    );
}

#[tokio::test]
async fn native_credits_actual_route_refreshed_capacity_keeps_affinity_until_exhaustion() {
    let mut first = account(0.0);
    first.usage_updated_at = Some(chrono::Utc::now().timestamp() - 600);
    let second = account(20.0);
    let mut host = host(Policy::StrictRegular, &[first.clone(), second.clone()]);
    let caller = scoped_caller(&mut host);
    host.collection_override
        .as_mut()
        .unwrap()
        .custom_routing_rules =
        vec![serde_json::from_value(json!({"accountId": first.id, "isPreferred": true})).unwrap()];
    let mut positive = native_credits_snapshot(first.quota.as_ref().unwrap(), now_ms(), "refresh");
    positive.primary.as_mut().unwrap().remaining = Some(2.0);
    positive.secondary.as_mut().unwrap().observed_at_ms = now_ms();
    NATIVE_CREDITS_REFRESH
        .refresh(native_credits_identity(&first), move || async move {
            Ok(positive)
        })
        .await
        .unwrap();
    let mut request = request();
    request.session_id = Some(uuid::Uuid::new_v4().to_string());
    let route = native_select_route_once(&host, &caller, request.clone())
        .await
        .unwrap();
    assert_eq!(route.account_id, first.id);
    host.collection_override
        .as_mut()
        .unwrap()
        .custom_routing_rules
        .clear();
    let again = native_select_route_once(&host, &caller, request.clone())
        .await
        .unwrap();
    assert_eq!(again.account_id, first.id);
    let mut headers = axum::http::HeaderMap::new();
    headers.insert("x-codex-limit-reached", "true".parse().unwrap());
    again
        .send_guard
        .as_ref()
        .unwrap()
        .observe_headers(&headers, Instant::now());
    assert_eq!(
        again.send_guard.as_ref().unwrap().check().unwrap_err().code,
        "credits_policy_blocked"
    );
    let replacement = native_select_route_once(&host, &caller, request)
        .await
        .unwrap();
    assert_eq!(replacement.account_id, second.id);
    {
        let runtime = gateway_runtime().lock().await;
        let health = runtime
            .account_pool_health
            .values()
            .find(|h| h.api_key_id == caller.id)
            .unwrap();
        assert_eq!(
            health
                .account_statuses
                .iter()
                .find(|m| m.account_id == first.id)
                .unwrap()
                .reason_code,
            "credits_policy_blocked"
        );
        assert!(
            health
                .account_statuses
                .iter()
                .find(|m| m.account_id == second.id)
                .unwrap()
                .available
        );
    }
    gateway_runtime()
        .lock()
        .await
        .account_pool_health
        .retain(|_, h| h.api_key_id != caller.id);
}

#[tokio::test]
async fn native_credits_cached_exhaustion_survives_missing_host_quota_and_recovery() {
    let source = account(0.0);
    let mut host = host(Policy::StrictRegular, &[source.clone()]);
    let collection = host.collection().await.unwrap();
    native_credits_pool(
        &host,
        &collection,
        collection.account_ids.clone(),
        &[source.clone()],
    )
    .await
    .unwrap();
    let mut missing = source.clone();
    missing.quota = None;
    missing.usage_updated_at = None;
    host.account_overrides
        .insert(source.id.clone(), missing.clone());
    let pool = native_credits_pool(
        &host,
        &collection,
        collection.account_ids.clone(),
        &[missing],
    )
    .await
    .unwrap();
    assert_eq!(pool.code(&source.id, false), Some("credits_policy_blocked"));
}

#[tokio::test]
async fn native_credits_same_observation_fraction_overrides_integer_zero() {
    let mut source = account(0.25);
    source.quota.as_mut().unwrap().hourly_percentage = 0;
    let host = host(Policy::StrictRegular, &[source.clone()]);
    let collection = host.collection().await.unwrap();
    let pool = native_credits_pool(
        &host,
        &collection,
        collection.account_ids.clone(),
        &[source.clone()],
    )
    .await
    .unwrap();
    assert_eq!(pool.code(&source.id, false), None);
    assert!(native_quota_allows(&collection, &source));
}

fn route(account: &CodexAccount) -> native_contract::Route {
    native_contract::Route {
        account_id: account.id.clone(),
        continuity_account_id: None,
        owner_id: "deliberately-not-parsed".into(),
        installation_profile: None,
        transport_identity: None,
        official_authorization_revision: None,
        official_registration_id: None,
        official_fallback_registration_id: None,
        official_fallback_authorization_revision: None,
        authorization_scope_fingerprint: None,
        send_guard: None,
        quota_identity: Some(native_credits_identity(account)),
        official_fallback_failure: None,
        base_url: "https://example.invalid".into(),
        model: "gpt-6.1-sol".into(),
        headers: Default::default(),
        client: reqwest::Client::new(),
        proxy_url: None,
        proxy_route: None,
        chatgpt: true,
        chatgpt_plan: false,
        wire_api: native_contract::WireApi::Responses,
        compact_endpoint: None,
        websocket_endpoint: None,
        default_service_tier: None,
        permit: None,
    }
}

#[tokio::test]
async fn native_credits_final_identity_checks_workspace_organization_and_agent_principal() {
    use native_contract::UpstreamAdmissionGuard;
    let mut source = account(50.0);
    source.agent_identity = Some(
        serde_json::from_value(json!({
            "agent_runtime_id":"synthetic-runtime", "agent_private_key":"synthetic-private-key",
            "account_id":"agent-workspace-a", "chatgpt_user_id":"agent-user-a"
        }))
        .unwrap(),
    );
    let host = host(Policy::StrictRegular, &[source.clone()]);
    let collection = host.collection().await.unwrap();
    let pool = native_credits_pool(
        &host,
        &collection,
        collection.account_ids.clone(),
        &[source.clone()],
    )
    .await
    .unwrap();
    let mut rotated = source.clone();
    rotated.tokens.access_token = "rotated-synthetic-token".into();
    assert!(!native_credits_guard(&pool, &route(&rotated)).identity_changed);
    native_credits_guard(&pool, &route(&rotated))
        .check()
        .unwrap();
    for change in 0..4 {
        let mut changed = source.clone();
        match change {
            0 => changed.account_id = Some("different-workspace".into()),
            1 => changed.organization_id = Some("different-organization".into()),
            2 => {
                changed.agent_identity.as_mut().unwrap().account_id =
                    "different-agent-workspace".into()
            }
            _ => {
                changed.agent_identity.as_mut().unwrap().chatgpt_user_id =
                    "different-agent-user".into()
            }
        }
        let mut route = route(&changed);
        assert_eq!(
            native_attach_credits_guard(&pool, &mut route)
                .unwrap_err()
                .code,
            "credits_quota_unconfirmed"
        );
    }
}

#[tokio::test]
async fn native_credits_changed_default_route_cannot_poison_old_identity_or_bypass_later_strict() {
    use native_contract::UpstreamAdmissionGuard;
    let source = account(50.0);
    let host = host(Policy::Default, &[source.clone()]);
    let collection = host.collection().await.unwrap();
    let pool = native_credits_pool(
        &host,
        &collection,
        collection.account_ids.clone(),
        &[source.clone()],
    )
    .await
    .unwrap();
    let mut changed = source.clone();
    changed.organization_id = Some("different-organization".into());
    let mut guard = native_credits_guard(&pool, &route(&changed));
    guard.check().unwrap();
    let mut headers = axum::http::HeaderMap::new();
    headers.insert("x-codex-limit-reached", "true".parse().unwrap());
    guard.observe_headers(&headers, Instant::now());
    assert_eq!(
        NATIVE_CREDITS_REFRESH
            .snapshot(&native_credits_identity(&source))
            .status(now_ms()),
        CreditsStatus::Available
    );
    assert_eq!(
        NATIVE_CREDITS_REFRESH
            .snapshot(&native_credits_identity(&changed))
            .status(now_ms()),
        CreditsStatus::Exhausted
    );
    guard.pool.settings.policy = Policy::StrictRegular;
    assert_eq!(guard.check().unwrap_err().code, "credits_quota_unconfirmed");
}

#[tokio::test]
async fn native_credits_grok_final_identity_remains_exempt_and_never_supplies_codex_evidence() {
    let mut source = account(0.0);
    source.auth_mode = CodexAuthMode::Apikey;
    source.upstream_grok_account_id = Some("synthetic-grok-source".into());
    assert!(codex_account::is_grok_upstream_provider(&source));
    let host = host(Policy::StrictRegular, &[source.clone()]);
    let collection = host.collection().await.unwrap();
    let pool = native_credits_pool(
        &host,
        &collection,
        collection.account_ids.clone(),
        &[source.clone()],
    )
    .await
    .unwrap();
    // The linked Grok OAuth source uses the provider/API-key account shape.
    // bind_owner refreshes its principal/workspace. Even a shared chatgpt wire
    // flag must not turn that known provider into a Codex quota authority.
    let mut final_account = source.clone();
    final_account.account_id = Some("synthetic-grok-workspace".into());
    final_account.user_id = Some("synthetic-grok-user".into());
    let final_identity = native_credits_identity(&final_account);
    let mut final_route = route(&final_account);
    assert!(final_route.chatgpt);
    native_attach_credits_guard(&pool, &mut final_route).unwrap();
    let guard = final_route.send_guard.unwrap();
    guard.check().unwrap();
    let mut headers = axum::http::HeaderMap::new();
    headers.insert("x-codex-limit-reached", "true".parse().unwrap());
    guard.observe_headers(&headers, Instant::now());
    guard.observe_event(
        &json!({"type":"codex.rate_limits","rate_limits":{"limit_reached":true}}),
        Instant::now(),
    );
    assert_eq!(
        NATIVE_CREDITS_REFRESH.snapshot(&final_identity),
        CreditsSnapshot::default()
    );
    guard.check().unwrap();
}

#[tokio::test]
async fn native_credits_actual_image_route_ignores_conversation_catalog_when_classifying_rejection()
{
    let image = account(0.0);
    let text = account(50.0);
    let mut host = host(Policy::StrictRegular, &[image.clone(), text.clone()]);
    let caller = scoped_caller(&mut host);
    let collection = host.collection_override.as_mut().unwrap();
    collection.image_generation_account_ids = vec![image.id.clone()];
    collection.account_ids = vec![text.id.clone()];
    let request = native_contract::ImageRouteRequest {
        model: Some("gpt-image-2.5".into()),
        responses_model: None,
        edit: false,
        target_account_id: None,
        force_refresh: false,
        preferred_account_id: None,
        excluded_account_ids: vec![],
    };
    let error = match native_image_route_once(&host, &caller, request).await {
        Err(error) => error,
        Ok(_) => panic!("exhausted image pool must reject admission"),
    };
    assert_eq!((error.status, error.code), (503, "credits_policy_blocked"));
    let mut runtime = gateway_runtime().lock().await;
    let health = runtime
        .account_pool_health
        .values()
        .find(|h| h.api_key_id == caller.id)
        .unwrap();
    assert_eq!(health.account_statuses.len(), 1);
    assert_eq!(health.account_statuses[0].account_id, image.id);
    runtime
        .account_pool_health
        .retain(|_, h| h.api_key_id != caller.id);
}
