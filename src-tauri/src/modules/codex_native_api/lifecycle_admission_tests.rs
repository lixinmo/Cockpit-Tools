// Admission follows the exact pending/sent route across retries and fallbacks.
#[derive(Default)]
struct AdmissionProbe {
    denied: AtomicBool,
    checks: AtomicUsize,
    observations: Mutex<Vec<super::super::rate_limits::QuotaSnapshot>>,
}

impl UpstreamAdmissionGuard for AdmissionProbe {
    fn check(&self) -> Result<(), ServiceError> {
        self.checks.fetch_add(1, Ordering::SeqCst);
        if self.denied.load(Ordering::SeqCst) {
            Err(ServiceError::new(503, "credits_policy_blocked", "Synthetic admission"))
        } else {
            Ok(())
        }
    }

    fn observe_rate_limits(
        &self,
        snapshot: super::super::rate_limits::QuotaSnapshot,
        _: Instant,
    ) {
        self.observations.lock().unwrap().push(snapshot);
    }
}

fn guarded_route(account: &str, guard: &Arc<AdmissionProbe>, official: bool) -> Route {
    let mut route = dispatch_route(account);
    route.send_guard = Some(guard.clone());
    route.chatgpt_plan = official;
    route
}

#[tokio::test]
async fn native_credits_admission_checks_pending_official_before_reserve_and_dispatch() {
    let (mut tracker, host, activity, _stop) = unsent_fixture();
    let old = Arc::new(AdmissionProbe::default());
    old.denied.store(true, Ordering::SeqCst);
    let selected = Arc::new(AdmissionProbe::default());
    let caller = Caller { id: "key".into(), label: "test".into() };
    tracker.routed(&caller, &guarded_route("old", &old, false), "requested");
    tracker.routed(&caller, &guarded_route("official", &selected, true), "requested");
    tracker.check_quota_before_send().unwrap();
    assert_eq!(old.checks.load(Ordering::SeqCst), 0);
    assert_eq!(selected.checks.load(Ordering::SeqCst), 1);
    assert!(host.quota_checks.lock().unwrap().is_empty());
    selected.denied.store(true, Ordering::SeqCst);
    let error = tracker.check_quota_before_send().unwrap_err();
    assert_eq!(error.code, "credits_policy_blocked");
    assert!(error.upstream_not_sent);
    assert_eq!(tracker.upstream_attempt_count(), 0);
    tracker.fail(&error).await;
    drop(tracker);
    assert_no_dispatch_records(&host, &activity).await;
}

#[tokio::test]
async fn native_credits_admission_rechecks_retry_and_preserves_upstream_reserve_error() {
    let (mut tracker, host, _activity, _stop) = unsent_fixture();
    let guard = Arc::new(AdmissionProbe::default());
    let caller = Caller { id: "key".into(), label: "test".into() };
    tracker.routed(&caller, &guarded_route("oauth", &guard, false), "requested");
    tracker.check_quota_before_send().unwrap();
    tracker.begin_attempt("http");
    assert!(tracker.discard_unsent_attempt());
    guard.denied.store(true, Ordering::SeqCst);
    assert_eq!(tracker.check_quota_before_send().unwrap_err().code, "credits_policy_blocked");
    assert_eq!(tracker.upstream_attempt_count(), 0);
    guard.denied.store(false, Ordering::SeqCst);
    host.quota_denied.lock().unwrap().push("oauth".into());
    let error = tracker.check_quota_before_send().unwrap_err();
    assert_eq!(error.code, "quota_reserved");
    assert_eq!(error.status, 429);
    assert!(error.upstream_not_sent);
    assert_eq!(guard.checks.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn native_credits_observations_stay_with_sent_identity_during_fallback() {
    let (mut tracker, _host, _activity, _stop) = unsent_fixture();
    let sent = Arc::new(AdmissionProbe::default());
    let pending = Arc::new(AdmissionProbe::default());
    let caller = Caller { id: "key".into(), label: "test".into() };
    tracker.routed(&caller, &guarded_route("same-id", &sent, false), "requested");
    tracker.begin_attempt("http");
    // Reauthorization/workspace changes may retain the business account ID.
    tracker.routed(&caller, &guarded_route("same-id", &pending, false), "requested");
    let mut headers = axum::http::HeaderMap::new();
    headers.insert("x-codex-primary-used-percent", "99.5".parse().unwrap());
    tracker.quota_headers(&headers);
    assert_eq!(sent.observations.lock().unwrap()[0].primary.as_ref().unwrap().remaining_percent, 0.5);
    assert!(pending.observations.lock().unwrap().is_empty());
    tracker.attempt_result(429, false, Some("rate_limited"));
    // The next real attempt takes its own credential/workspace-bound guard.
    tracker.routed(&caller, &guarded_route("same-id", &pending, true), "requested");
    tracker.begin_attempt("http");
    tracker.quota_event(&serde_json::json!({
        "type": "codex.rate_limits",
        "rate_limits": { "primary": { "used_percent": 100.0 } }
    }));
    assert_eq!(pending.observations.lock().unwrap().len(), 1);
    assert_eq!(pending.observations.lock().unwrap()[0].primary.as_ref().unwrap().remaining_percent, 0.0);
    // Headers and events use the same route-scoped observation callback.
    tracker.quota_headers(&headers);
    assert_eq!(pending.observations.lock().unwrap().len(), 2);
    assert_eq!(sent.observations.lock().unwrap().len(), 1);
    tracker.context("POST", "/v1/responses", &axum::http::HeaderMap::new(), false);
    let prior = pending.observations.lock().unwrap().len();
    tracker.quota_headers(&headers);
    assert_eq!(pending.observations.lock().unwrap().len(), prior);
}
