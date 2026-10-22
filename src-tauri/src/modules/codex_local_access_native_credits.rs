// Native Credits admission wraps the upstream route/affinity/concurrency paths.
// OAuth reserve remains upstream-owned and independent of this billing policy.
use credits_policy::{
    Snapshot as CreditsSnapshot, Status as CreditsStatus, Window as CreditsWindow,
};

static NATIVE_CREDITS_REFRESH: std::sync::LazyLock<Arc<credits_refresh::Refreshes>> =
    std::sync::LazyLock::new(|| Arc::new(credits_refresh::Refreshes::default()));
static NATIVE_CREDITS_SETTINGS: std::sync::LazyLock<Mutex<Option<NativeCreditsSettings>>> =
    std::sync::LazyLock::new(|| Mutex::new(None));
static NATIVE_CREDITS_HOST_OBSERVATIONS: std::sync::LazyLock<
    Mutex<HashMap<String, NativeCreditsHostObservation>>,
> = std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));

struct NativeCreditsHostObservation {
    published_at: i64,
    started_at: i64,
    value: CreditsSnapshot,
}

pub(crate) fn confirm_native_credits_quota(account: &CodexAccount, started: Instant) {
    if !native_credits_applies(account) || native_is_chatgpt_plan_account(account) {
        return;
    }
    let (Some(quota), Some(published)) = (&account.quota, account.usage_updated_at) else {
        return;
    };
    let identity = native_credits_identity(account);
    let started_at =
        now_ms().saturating_sub(started.elapsed().as_millis().min(i64::MAX as u128) as i64);
    if let Ok(mut observed) = NATIVE_CREDITS_HOST_OBSERVATIONS.lock() {
        if observed.contains_key(&identity) || observed.len() < 4096 {
            let newer = observed
                .get(&identity)
                .is_none_or(|old| old.started_at <= started_at);
            if newer {
                observed.insert(
                    identity.clone(),
                    NativeCreditsHostObservation {
                        published_at: published.saturating_mul(1000),
                        started_at,
                        value: native_credits_snapshot(quota, 0, "host"),
                    },
                );
            }
        }
    }
    NATIVE_CREDITS_REFRESH.merge(
        &identity,
        native_credits_snapshot(quota, started_at, "host_refresh"),
    );
}

fn native_credits_host_snapshot(account: &CodexAccount) -> CreditsSnapshot {
    let (Some(quota), Some(published)) = (&account.quota, account.usage_updated_at) else {
        return CreditsSnapshot::default();
    };
    let published = published.saturating_mul(1000);
    let at = NATIVE_CREDITS_HOST_OBSERVATIONS
        .lock()
        .ok()
        .and_then(|observed| {
            observed
                .get(&native_credits_identity(account))
                .map(|entry| {
                    if entry.published_at >= published {
                        // Re-reading the persisted result must not turn an older GET
                        // into a newer observation than a passive exhaustion signal.
                        if entry.value == native_credits_snapshot(quota, 0, "host") {
                            Some(entry.started_at)
                        } else {
                            None
                        }
                    } else {
                        Some(published)
                    }
                })
        })
        .unwrap_or(Some(published));
    at.map(|at| native_credits_snapshot(quota, at, "host"))
        .unwrap_or_default()
}

#[derive(Clone)]
struct NativeCreditsSettings {
    policy: crate::models::codex_local_access::CodexLocalAccessCreditsPolicy,
    allowed: HashMap<String, bool>,
}

impl NativeCreditsSettings {
    fn from_collection(collection: &CodexLocalAccessCollection) -> Self {
        Self {
            policy: collection.credits_policy,
            allowed: collection.account_credits_allowed.clone(),
        }
    }
    fn allows(&self, id: &str) -> bool {
        self.allowed.get(id).copied().unwrap_or(true)
    }
}

fn native_publish_credits_settings(collection: &CodexLocalAccessCollection) {
    if let Ok(mut current) = NATIVE_CREDITS_SETTINGS.lock() {
        *current = Some(NativeCreditsSettings::from_collection(collection));
    }
}

fn native_credits_applies(account: &CodexAccount) -> bool {
    native_is_chatgpt_plan_account(account)
        || (!account.is_api_key_auth() && !codex_account::is_grok_upstream_provider(account))
}

fn native_credits_recovered_host_quota(account: &CodexAccount, host_at: i64) -> bool {
    let snapshot = NATIVE_CREDITS_REFRESH.snapshot(&native_credits_identity(account));
    if snapshot.status(now_ms()) != CreditsStatus::Available {
        return false;
    }
    let Some(quota) = &account.quota else {
        return false;
    };
    let host = native_credits_snapshot(quota, host_at, "host");
    let recovered = [&host.primary, &host.secondary]
        .into_iter()
        .zip([&snapshot.primary, &snapshot.secondary])
        .all(|(old, next)| {
            !old.as_ref()
                .is_some_and(|w| w.present && w.remaining == Some(0.0))
                || next.as_ref().is_some_and(|w| {
                    w.observed_at_ms >= host_at && w.present && w.remaining.is_some_and(|v| v > 0.0)
                })
        });
    recovered
}

fn native_credits_identity(account: &CodexAccount) -> String {
    // Never store tokens/emails. Credential refresh for the same principal keeps
    // valid quota; changing workspace/user cannot inherit previous evidence.
    serde_json::to_string(&(
        &account.id,
        &account.account_id,
        &account.user_id,
        &account.organization_id,
        &account.auth_mode,
        &account.token_source_mode,
        account
            .agent_identity
            .as_ref()
            .map(|identity| (&identity.account_id, &identity.chatgpt_user_id)),
    ))
    .unwrap_or_else(|_| account.id.clone())
}

fn native_credits_snapshot(quota: &CodexQuota, at: i64, source: &'static str) -> CreditsSnapshot {
    let window = |primary: bool| {
        let (remaining, present, minutes, reset, name) = if primary {
            (
                quota.hourly_percentage,
                quota.hourly_window_present,
                quota.hourly_window_minutes,
                quota.hourly_reset_time,
                "primary_window",
            )
        } else {
            (
                quota.weekly_percentage,
                quota.weekly_window_present,
                quota.weekly_window_minutes,
                quota.weekly_reset_time,
                "secondary_window",
            )
        };
        let raw = quota
            .raw_data
            .as_ref()
            .and_then(|v| v.get("rate_limit"))
            .and_then(|v| v.get(name));
        let present = present.or_else(|| raw.map(|v| !v.is_null())).or_else(|| {
            (minutes.is_some_and(|v| v > 0) || reset.is_some_and(|v| v > 0) || remaining > 0)
                .then_some(true)
        })?;
        let precise = json_finite_f64(raw.and_then(|v| v.get("used_percent")))
            .filter(|v| v.is_finite() && (0.0..=100.0).contains(v))
            .map(|v| 100.0 - v)
            .or_else(|| {
                (0..=100)
                    .contains(&remaining)
                    .then_some(f64::from(remaining))
            });
        Some(CreditsWindow {
            present,
            remaining: precise,
            reset_at_ms: reset.filter(|v| *v > 0).map(|v| v.saturating_mul(1000)),
            observed_at_ms: at,
            source,
        })
    };
    let mut snapshot = CreditsSnapshot {
        primary: window(true),
        secondary: window(false),
    };
    if quota
        .raw_data
        .as_ref()
        .and_then(|v| v.get("rate_limit"))
        .and_then(|v| v.get("limit_reached"))
        .and_then(Value::as_bool)
        == Some(true)
    {
        snapshot.primary = Some(CreditsWindow {
            present: true,
            remaining: Some(0.0),
            reset_at_ms: None,
            observed_at_ms: at,
            source,
        });
    }
    snapshot
}

fn native_credits_scope(
    collection: &CodexLocalAccessCollection,
    key: Option<&CodexLocalAccessApiKey>,
) -> Vec<String> {
    // Unlike native_scoped_account_ids, do not discard paused accounts. Their
    // exclusion is a scheduling choice, never proof of exhausted regular quota.
    match key {
        Some(key)
            if !key
                .inherit_account_pool
                .unwrap_or(key.account_ids.is_empty()) =>
        {
            key.account_ids.clone()
        }
        _ => collection.account_ids.clone(),
    }
}

#[derive(Clone)]
struct NativeCreditsMember {
    id: String,
    identity: String,
    applies: bool,
    official: bool,
    paused: bool,
    refresh: String,
}

#[derive(Clone)]
struct NativeCreditsPool {
    settings: NativeCreditsSettings,
    members: Vec<NativeCreditsMember>,
    use_current_settings: bool,
    scheduler: HashMap<String, &'static str>,
}

impl NativeCreditsPool {
    fn mark_removed(&mut self, before: &[String], after: &[CodexAccount], reason: &'static str) {
        for id in before {
            if !after.iter().any(|account| &account.id == id) {
                self.scheduler.insert(id.clone(), reason);
            }
        }
    }
    fn has_policy_blocks(&self) -> bool {
        self.members
            .iter()
            .any(|m| self.code(&m.id, m.official).is_some())
    }
    fn restricted(&self) -> bool {
        let settings = self.settings();
        self.members.iter().any(|m| {
            m.applies && credits_policy::restricted(settings.policy, settings.allows(&m.id))
        })
    }
    fn mark_host_cooldowns(&mut self, before: &[CodexAccount]) {
        for account in before {
            if self.scheduler.get(&account.id) == Some(&"model_or_account_policy")
                && account_quota_cooldown(account, now_ms()).is_some_and(|q| {
                    q.active(now_ms())
                        && !native_credits_recovered_host_quota(account, q.updated_at_ms)
                })
            {
                self.scheduler.insert(account.id.clone(), "quota_cooldown");
            }
        }
    }
    fn settings(&self) -> NativeCreditsSettings {
        if self.use_current_settings {
            if let Some(current) = NATIVE_CREDITS_SETTINGS.lock().ok().and_then(|s| s.clone()) {
                return current;
            }
        }
        self.settings.clone()
    }
    fn all_exhausted(&self) -> bool {
        let members = self
            .members
            .iter()
            .filter(|m| m.applies)
            .collect::<Vec<_>>();
        !members.is_empty()
            && members.iter().all(|m| {
                !m.official
                    && NATIVE_CREDITS_REFRESH
                        .snapshot(&m.identity)
                        .status(now_ms())
                        == CreditsStatus::Exhausted
            })
    }
    fn code(&self, account_id: &str, official: bool) -> Option<&'static str> {
        let settings = self.settings();
        let member = self.members.iter().find(|m| m.id == account_id);
        if member.is_some_and(|m| !m.applies) {
            return None;
        }
        let status = member.map_or(CreditsStatus::Unknown, |m| {
            NATIVE_CREDITS_REFRESH
                .snapshot(&m.identity)
                .status(now_ms())
        });
        credits_policy::block_code(
            settings.policy,
            settings.allows(account_id),
            status,
            self.all_exhausted(),
            official,
        )
    }
    fn statuses(&self) -> Vec<SidecarAccountStatus> {
        self.members
            .iter()
            .map(|member| {
                let snapshot = NATIVE_CREDITS_REFRESH.snapshot(&member.identity);
                let code = self.code(&member.id, member.official);
                let scheduler = self.scheduler.get(&member.id).copied();
                let stage = if member.paused {
                    "paused"
                } else if member.official && code.is_some() {
                    "official_quota_unconfirmed"
                } else if code.is_some() {
                    "credits"
                } else if let Some(reason) = scheduler {
                    reason
                } else {
                    "credits_admitted"
                };
                SidecarAccountStatus {
                    account_id: member.id.clone(),
                    available: code.is_none() && !member.paused && scheduler.is_none(),
                    reason_code: if member.paused {
                        "paused".into()
                    } else {
                        code.or(scheduler).unwrap_or("credits_admitted").into()
                    },
                    reason_message: format!(
                        "stage={stage} regular={}; {}; refresh={}",
                        if member.official {
                            "unknown_official"
                        } else {
                            snapshot.status(now_ms()).as_str()
                        },
                        if member.official {
                            "source=official_unconfirmed".into()
                        } else {
                            snapshot.detail(now_ms())
                        },
                        member.refresh
                    ),
                    ..Default::default()
                }
            })
            .collect()
    }
    fn error(&self, code: &'static str) -> native_contract::ServiceError {
        let details = self
            .statuses()
            .into_iter()
            .map(|s| format!("{}: {}", s.account_id, s.reason_message))
            .collect::<Vec<_>>()
            .join(" | ");
        native_error(
            503,
            code,
            &format!("Local Credits policy rejected admission; {details}"),
        )
    }
    async fn report(&self, caller: &native_contract::Caller, model: &str, code: &'static str) {
        let statuses = self.statuses();
        let available = statuses.iter().filter(|s| s.available).count();
        let event = SidecarAuthResultEvent {
            api_key_id: caller.id.clone(),
            provider: "codex".into(),
            model: model.into(),
            request_kind: "credits_admission".into(),
            started_at_ms: now_ms(),
            error_code: Some(code.into()),
            error_message: Some("Native Credits admission; see per-account evidence".into()),
            http_status: Some(503),
            candidate_auths: statuses.len(),
            scoped_auths: statuses.len(),
            available_auths: available,
            unavailable_auths: statuses.len().saturating_sub(available),
            account_statuses: statuses,
            ..Default::default()
        };
        if update_sidecar_account_pool_health(&event, true).await {
            emit_local_access_state_updated();
        }
    }
    async fn filter(
        &self,
        caller: &native_contract::Caller,
        model: &str,
        accounts: &mut Vec<CodexAccount>,
    ) -> Result<(), native_contract::ServiceError> {
        let mut blocked = None;
        accounts.retain(|account| {
            if let Some(code) = self.code(&account.id, native_is_chatgpt_plan_account(account)) {
                if blocked != Some("credits_quota_unconfirmed") {
                    blocked = Some(code);
                }
                false
            } else {
                true
            }
        });
        if let Some(code) = blocked {
            if accounts.is_empty() {
                self.report(caller, model, code).await;
                return Err(self.error(code));
            }
        }
        Ok(())
    }
}

async fn native_credits_pool(
    host: &NativeHostBridge,
    collection: &CodexLocalAccessCollection,
    ids: Vec<String>,
    candidates: &[CodexAccount],
) -> Result<NativeCreditsPool, native_contract::ServiceError> {
    let raw = host.load_accounts(ids.clone()).await?;
    let settings = NativeCreditsSettings::from_collection(collection);
    let mut members = Vec::new();
    let mut work = Vec::new();
    for id in ids {
        if members.iter().any(|m: &NativeCreditsMember| m.id == id) {
            continue;
        }
        let account = raw.iter().find(|a| a.id == id);
        let applies = account.is_none_or(native_credits_applies);
        let official = candidates
            .iter()
            .find(|a| a.id == id)
            .map(native_is_chatgpt_plan_account)
            .unwrap_or_else(|| {
                collection.official_account_bindings.contains_key(&id)
                    || id.starts_with(codex_account::CHATGPT_PLAN_ACCOUNT_PREFIX)
            });
        let identity = account
            .map(native_credits_identity)
            .unwrap_or_else(|| format!("missing:{id}"));
        let mut snapshot = CreditsSnapshot::default();
        if let Some(account) = account.filter(|_| applies && !official) {
            snapshot = native_credits_host_snapshot(account);
        }
        snapshot = NATIVE_CREDITS_REFRESH.merge(&identity, snapshot);
        if applies
            && !official
            && credits_policy::restricted(settings.policy, settings.allows(&id))
            && matches!(
                snapshot.status(now_ms()),
                CreditsStatus::Unknown | CreditsStatus::Stale
            )
        {
            if let Some(account) = account.cloned() {
                let identity = identity.clone();
                let id = id.clone();
                work.push(async move {
                    let expected = identity.clone();
                    let result = NATIVE_CREDITS_REFRESH
                        .refresh(identity, move || async move {
                            let account = native_prepare_account(account, false)
                                .await
                                .map_err(|_| "refresh_credentials_unavailable")?;
                            if native_credits_identity(&account) != expected {
                                return Err("refresh_identity_changed");
                            }
                            let started = now_ms();
                            // Exactly the existing read-only usage GET and account transport;
                            // no inference, quota reset, retry loop or alternate credential.
                            let result = codex_quota::fetch_quota_for_credits(&account)
                                .await
                                .map_err(|_| "refresh_failed")?;
                            let snapshot =
                                native_credits_snapshot(&result.quota, started, "refresh");
                            if matches!(
                                snapshot.status(now_ms()),
                                CreditsStatus::Unknown | CreditsStatus::Stale
                            ) {
                                return Err("refresh_unconfirmed");
                            }
                            Ok(snapshot)
                        })
                        .await;
                    (
                        id,
                        result
                            .map(|_| "refresh_succeeded")
                            .unwrap_or_else(|e| e)
                            .to_string(),
                    )
                });
            }
        }
        members.push(NativeCreditsMember {
            paused: service_account_paused(collection, &id),
            id,
            identity,
            applies,
            official,
            refresh: if official {
                "official_authority_unavailable"
            } else {
                "not_needed"
            }
            .into(),
        });
    }
    match tokio::time::timeout(Duration::from_secs(9), futures_util::future::join_all(work)).await {
        Ok(results) => {
            for (id, result) in results {
                if let Some(member) = members.iter_mut().find(|m| m.id == id) {
                    member.refresh = result;
                }
            }
        }
        Err(_) => {
            for member in &mut members {
                if member.applies && !member.official {
                    member.refresh = "refresh_wait_timeout".into();
                }
            }
        }
    }
    Ok(NativeCreditsPool {
        settings,
        members,
        use_current_settings: host.collection_override.is_none() || host.profile_dir.is_some(),
        scheduler: HashMap::new(),
    })
}

struct NativeCreditsGuard {
    pool: NativeCreditsPool,
    account_id: String,
    official: bool,
    identity: Option<String>,
    identity_changed: bool,
}

impl native_contract::UpstreamAdmissionGuard for NativeCreditsGuard {
    fn check(&self) -> Result<(), native_contract::ServiceError> {
        let settings = self.pool.settings();
        if self.identity_changed
            && credits_policy::restricted(settings.policy, settings.allows(&self.account_id))
        {
            return Err(self.pool.error("credits_quota_unconfirmed"));
        }
        if let Some(code) = self.pool.code(&self.account_id, self.official) {
            return Err(self.pool.error(code));
        }
        Ok(())
    }
    fn observe_headers(&self, headers: &axum::http::HeaderMap, _started: Instant) {
        if self.official {
            return;
        }
        let Some(identity) = &self.identity else {
            return;
        };
        let previous = NATIVE_CREDITS_REFRESH.snapshot(identity);
        let snapshot = credits_policy::from_headers(&previous, now_ms(), |name| {
            headers
                .get(name)
                .and_then(|value| value.to_str().ok())
                .map(str::to_owned)
        });
        NATIVE_CREDITS_REFRESH.merge(identity, snapshot);
    }
    fn observe_event(&self, event: &Value, _started: Instant) {
        if self.official {
            return;
        }
        let Some(identity) = &self.identity else {
            return;
        };
        let previous = NATIVE_CREDITS_REFRESH.snapshot(identity);
        NATIVE_CREDITS_REFRESH.merge(
            identity,
            credits_policy::from_event(&previous, event, now_ms()),
        );
    }
}

fn native_credits_guard(
    pool: &NativeCreditsPool,
    route: &native_contract::Route,
) -> NativeCreditsGuard {
    let mut pool = pool.clone();
    let member = pool
        .members
        .iter_mut()
        .find(|member| member.id == route.account_id);
    let applicable = member.as_ref().is_none_or(|member| member.applies);
    let identity_changed = applicable
        && route.chatgpt
        && member
            .as_ref()
            .is_none_or(|member| route.quota_identity.as_deref() != Some(member.identity.as_str()));
    if identity_changed {
        if let Some(member) = member {
            member.identity = route
                .quota_identity
                .clone()
                .unwrap_or_else(|| format!("unverified:{}", route.account_id));
            member.applies = true;
            member.refresh = "credential_identity_changed".into();
        }
    }
    let identity = (applicable && route.chatgpt && !route.chatgpt_plan)
        .then(|| route.quota_identity.clone())
        .flatten();
    NativeCreditsGuard {
        pool,
        account_id: route.account_id.clone(),
        official: route.chatgpt_plan,
        identity,
        identity_changed,
    }
}

fn native_attach_credits_guard(
    pool: &NativeCreditsPool,
    route: &mut native_contract::Route,
) -> Result<(), native_contract::ServiceError> {
    let guard = native_credits_guard(pool, route);
    native_contract::UpstreamAdmissionGuard::check(&guard)?;
    route.send_guard = Some(Arc::new(guard));
    Ok(())
}
