use super::*;
use std::sync::Arc;
use tokio::sync::Notify;

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let path =
            std::env::temp_dir().join(format!("cockpit-auto-reset-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&path).expect("create isolated fixture directory");
        Self(path)
    }

    fn guard(&self) -> ResetGuard {
        self.guard_for("mock-account")
    }

    fn guard_for(&self, account_id: &str) -> ResetGuard {
        ResetGuard::acquire(&self.0, account_id)
            .expect("acquire fixture lock")
            .expect("fixture is not already locked")
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn quota(weekly: i32, hourly: i32) -> CodexQuota {
    CodexQuota {
        hourly_percentage: hourly,
        hourly_reset_time: Some(2_000_000_000),
        hourly_window_minutes: Some(300),
        hourly_window_present: Some(true),
        weekly_percentage: weekly,
        weekly_reset_time: Some(2_000_000_000),
        weekly_window_minutes: Some(7 * 24 * 60),
        weekly_window_present: Some(true),
        reset_credits_available: Some(3),
        reset_credits: Vec::new(),
        reset_credits_next_expires_at: None,
        raw_data: None,
    }
}

fn primary_weekly_quota(remaining: i32) -> CodexQuota {
    // Match the observed weekly-only API layout and its existing parser output:
    // primary is 604800 seconds, secondary is null, and the legacy weekly_*
    // model fields therefore contain the absent-secondary defaults.
    serde_json::from_value(serde_json::json!({
        "hourly_percentage": remaining,
        "hourly_window_minutes": 10080,
        "hourly_window_present": true,
        "weekly_percentage": 100,
        "weekly_window_present": false,
        "reset_credits_available": 3,
        "raw_data": {
            "rate_limit": {
                "primary_window": {
                    "limit_window_seconds": 604800,
                    "used_percent": 100 - remaining
                },
                "secondary_window": null
            }
        }
    }))
    .expect("deserialize weekly-only primary quota fixture")
}

fn credit(id: &str, expires: Option<i64>) -> CodexResetCredit {
    CodexResetCredit {
        id: Some(id.into()),
        status: Some("available".into()),
        reset_type: Some("codex_rate_limits".into()),
        granted_at: Some(1),
        expires_at: expires,
        redeemed_at: None,
        raw_status: None,
    }
}

#[derive(Default)]
struct Gate {
    entered: Notify,
    release: Notify,
}

struct MockOperations {
    settings: Settings,
    settings_after_fetch: Option<Settings>,
    credits: Result<Vec<CodexResetCredit>, String>,
    consume_result: Result<(), String>,
    refreshed: Result<Option<CodexQuota>, String>,
    fetch_calls: usize,
    refresh_calls: usize,
    consumed: Vec<(String, String)>,
    events: Vec<&'static str>,
    state_path: PathBuf,
    sabotage_state_write: bool,
    consume_gate: Option<Arc<Gate>>,
    refresh_gate: Option<Arc<Gate>>,
}

impl MockOperations {
    fn new(guard: &ResetGuard) -> Self {
        Self {
            settings: Settings {
                enabled: true,
                threshold: 2,
            },
            settings_after_fetch: None,
            credits: Ok(vec![credit("only-credit", None)]),
            consume_result: Ok(()),
            refreshed: Ok(Some(quota(100, 100))),
            fetch_calls: 0,
            refresh_calls: 0,
            consumed: Vec::new(),
            events: Vec::new(),
            state_path: guard.state_path.clone(),
            sabotage_state_write: false,
            consume_gate: None,
            refresh_gate: None,
        }
    }
}

impl Operations for MockOperations {
    fn settings(&self) -> Settings {
        self.settings
    }

    async fn fetch_credits(&mut self) -> Result<Vec<CodexResetCredit>, String> {
        self.fetch_calls += 1;
        self.events.push("fetch");
        if let Some(settings) = self.settings_after_fetch.take() {
            self.settings = settings;
        }
        if self.sabotage_state_write {
            fs::create_dir(&self.state_path)
                .expect("make pending destination unwritable as a file");
        }
        self.credits.clone()
    }

    async fn consume(&mut self, credit_id: &str, request_id: &str) -> Result<(), String> {
        // Every attempted POST must already have a durable record, containing
        // the exact selected credit and idempotency key passed to the backend.
        let persisted: Attempt = serde_json::from_str(
            &fs::read_to_string(&self.state_path).expect("persist BEFORE consume"),
        )
        .expect("valid durable attempt");
        assert_eq!(persisted.credit_id.as_deref(), Some(credit_id));
        assert_eq!(persisted.redeem_request_id, request_id);
        assert_eq!(persisted.threshold, self.settings.threshold);
        assert!(persisted.awaiting_refresh);
        assert!(uuid::Uuid::parse_str(request_id).is_ok());
        assert_eq!(
            consume_body(request_id, Some(credit_id)),
            serde_json::json!({
                "redeem_request_id": request_id,
                "credit_id": credit_id,
            })
        );
        self.events.push("consume");
        self.consumed.push((credit_id.into(), request_id.into()));
        if let Some(gate) = &self.consume_gate {
            gate.entered.notify_one();
            gate.release.notified().await;
        }
        self.consume_result.clone()
    }

    async fn refresh(&mut self) -> Result<Option<CodexQuota>, String> {
        self.refresh_calls += 1;
        self.events.push("refresh");
        if let Some(gate) = &self.refresh_gate {
            gate.entered.notify_one();
            gate.release.notified().await;
        }
        self.refreshed.clone()
    }
}

#[tokio::test]
async fn refresh_wrapper_preserves_quota_only_and_locks_before_either_query_mode() {
    for quota_only in [false, true] {
        let dir = TempDir::new();
        let guard = dir.guard();
        let mut operations = MockOperations::new(&guard);
        drop(guard);
        let latest = refresh_quota(
            Ok(dir.0.clone()),
            quota_only,
            |actual_quota_only| {
                assert_eq!(actual_quota_only, quota_only);
                assert!(ResetGuard::acquire(&dir.0, "mock-account").unwrap().is_none());
                async { Ok(RefreshedQuota { quota: quota(1, 100), fresh: true }) }
            },
            &mut operations,
        ).await.unwrap();
        assert_eq!(latest.weekly_percentage, 100);
        assert_eq!(operations.events, ["fetch", "consume", "refresh"]);
        assert_eq!(operations.consumed.len(), 1);
        assert!(dir.guard().pending().unwrap().is_none());
    }
}

#[tokio::test]
async fn refresh_wrapper_cached_quota_neither_spends_nor_releases_a_pending_barrier() {
    for quota_only in [false, true] {
        for remaining in [0, 100] {
            let dir = TempDir::new();
            let guard = dir.guard();
            let mut operations = MockOperations::new(&guard);
            guard.record(&Attempt {
                credit_id: Some("in-flight-credit".into()),
                redeem_request_id: "original-request".into(),
                threshold: 2,
                awaiting_refresh: true,
            }).unwrap();
            drop(guard);
            refresh_quota(
                Ok(dir.0.clone()), quota_only,
                |_| async { Ok(RefreshedQuota { quota: quota(remaining, 100), fresh: false }) },
                &mut operations,
            ).await.unwrap();
            assert!(operations.events.is_empty());
            let pending = dir.guard().pending().unwrap().unwrap();
            assert!(pending.awaiting_refresh);
            assert_eq!(pending.redeem_request_id, "original-request");
        }
    }
}

#[tokio::test]
async fn activity_sync_and_monitor_cannot_double_consume_during_one_low_episode() {
    let dir = TempDir::new();
    let guard = dir.guard();
    let mut activity = MockOperations::new(&guard);
    activity.refreshed = Ok(Some(quota(0, 100)));
    let mut monitor = MockOperations::new(&guard);
    drop(guard);
    let gate = Gate::default();
    let active = refresh_quota(
        Ok(dir.0.clone()), true,
        |quota_only| {
            assert!(quota_only);
            async {
                gate.entered.notify_one();
                gate.release.notified().await;
                Ok(RefreshedQuota { quota: quota(0, 100), fresh: true })
            }
        },
        &mut activity,
    );
    let competing = async {
        gate.entered.notified().await;
        refresh_quota(
            Ok(dir.0.clone()), false,
            |quota_only| {
                assert!(!quota_only);
                async { Ok(RefreshedQuota { quota: quota(0, 100), fresh: true }) }
            },
            &mut monitor,
        ).await.unwrap();
        assert!(monitor.events.is_empty(), "a concurrent GET cannot consume without the guard");
        gate.release.notify_one();
    };
    let (result, ()) = tokio::join!(active, competing);
    result.unwrap();
    assert_eq!(activity.consumed.len(), 1);
    assert!(dir.guard().pending().unwrap().is_some());
    // The next monitor pass sees the activity worker's durable low-episode
    // barrier even after both previous futures and locks have been dropped.
    refresh_quota(
        Ok(dir.0.clone()), false,
        |_| async { Ok(RefreshedQuota { quota: quota(0, 100), fresh: true }) },
        &mut monitor,
    ).await.unwrap();
    assert!(monitor.events.is_empty());
}

#[tokio::test]
async fn refresh_wrapper_failed_query_keeps_pending_state_and_guard_failure_never_spends() {
    let dir = TempDir::new();
    let guard = dir.guard();
    let mut operations = MockOperations::new(&guard);
    guard.record(&Attempt {
        credit_id: Some("pending-credit".into()),
        redeem_request_id: "pending-request".into(),
        threshold: 2,
        awaiting_refresh: true,
    }).unwrap();
    drop(guard);
    let error = refresh_quota(
        Ok(dir.0.clone()), true,
        |_| async { Err("CODEX_STALE_QUOTA_REQUEST".into()) },
        &mut operations,
    ).await.unwrap_err();
    assert_eq!(error, "CODEX_STALE_QUOTA_REQUEST");
    assert!(dir.guard().pending().unwrap().unwrap().awaiting_refresh);
    let fresh = refresh_quota(
        Err("fixture storage unavailable".into()), false,
        |_| async { Ok(RefreshedQuota { quota: quota(0, 100), fresh: true }) },
        &mut operations,
    ).await.unwrap();
    assert_eq!(fresh.weekly_percentage, 0);
    assert!(operations.events.is_empty());
}

#[test]
fn account_eligibility_excludes_official_plan_projections_and_read_only_sessions() {
    use crate::models::codex::{CodexAccount, CodexApiProviderMode, CodexAuthMode};
    let mut account = CodexAccount::new_api_key(
        "chatgpt_plan_fixture".into(), "fixture@example.invalid".into(),
        "chatgpt-plan-managed".into(), CodexApiProviderMode::Custom,
        None, Some("cockpit_chatgpt_plan".into()), None, Vec::new(),
    );
    // Matches the upstream official-plan projection constructor: these records
    // are API-key references, not an OAuth account eligible to redeem Resets.
    assert!(!supports_account(&account));
    account.api_provider_id = None;
    assert!(!supports_account(&account));
    account.auth_mode = CodexAuthMode::OAuth;
    assert!(supports_account(&account));
    account.token_source_mode = "chatgpt_web_session".into();
    assert!(!supports_account(&account));
}

#[tokio::test]
async fn disabled_never_queries_or_consumes_even_at_zero() {
    let dir = TempDir::new();
    let guard = dir.guard();
    let mut operations = MockOperations::new(&guard);
    operations.settings.enabled = false;
    after_refresh(&guard, quota(0, 0), &mut operations)
        .await
        .unwrap();
    assert!(operations.events.is_empty());
    assert!(guard.pending().unwrap().is_none());
}

#[tokio::test]
async fn threshold_inclusive_and_only_weekly_percentage_matters() {
    for (weekly, hourly, expected) in [
        (0, 100, 1),
        (1, 100, 1),
        (2, 100, 1),
        (3, 0, 0),
        (100, 0, 0),
    ] {
        let dir = TempDir::new();
        let guard = dir.guard();
        let mut operations = MockOperations::new(&guard);
        after_refresh(&guard, quota(weekly, hourly), &mut operations)
            .await
            .unwrap();
        assert_eq!(
            operations.consumed.len(),
            expected,
            "weekly={weekly}, hourly={hourly}"
        );
        assert_eq!(operations.refresh_calls, expected);
    }
}

#[test]
fn weekly_only_primary_matches_actual_exhausted_account_layout() {
    let exhausted = primary_weekly_quota(0);
    assert_eq!(exhausted.hourly_window_minutes, Some(10080));
    assert_eq!(exhausted.weekly_window_present, Some(false));
    assert_eq!(exhausted.weekly_percentage, 100);
    assert_eq!(weekly_remaining(&exhausted), Some(0));
    assert_eq!(weekly_remaining(&primary_weekly_quota(91)), Some(91));
}

#[tokio::test]
async fn primary_weekly_threshold_consumes_one_card_and_fresh_recovery_releases_latch() {
    for (remaining, expected_consumptions) in [(0, 1), (1, 1), (2, 1), (3, 0), (100, 0)] {
        let dir = TempDir::new();
        let guard = dir.guard();
        let mut operations = MockOperations::new(&guard);
        let now = chrono::Utc::now().timestamp();
        operations.credits = Ok(vec![
            credit("later", Some(now + 600)),
            credit("nearest-expiry", Some(now + 300)),
        ]);
        operations.refreshed = Ok(Some(primary_weekly_quota(91)));
        let latest = after_refresh(&guard, primary_weekly_quota(remaining), &mut operations)
            .await
            .unwrap();
        assert_eq!(
            operations.consumed.len(),
            expected_consumptions,
            "remaining={remaining}"
        );
        assert!(guard.pending().unwrap().is_none());
        if expected_consumptions == 1 {
            assert_eq!(operations.consumed[0].0, "nearest-expiry");
            assert_eq!(operations.events, ["fetch", "consume", "refresh"]);
            assert_eq!(weekly_remaining(&latest), Some(91));
        } else {
            assert!(operations.events.is_empty());
            assert_eq!(weekly_remaining(&latest), Some(remaining));
        }
    }
}

#[tokio::test]
async fn primary_weekly_unchanged_result_stays_latched_until_primary_recovers() {
    let dir = TempDir::new();
    let guard = dir.guard();
    let mut operations = MockOperations::new(&guard);
    operations.refreshed = Ok(Some(primary_weekly_quota(0)));
    after_refresh(&guard, primary_weekly_quota(0), &mut operations)
        .await
        .unwrap();
    after_refresh(&guard, primary_weekly_quota(0), &mut operations)
        .await
        .unwrap();
    assert_eq!(operations.consumed.len(), 1);
    assert!(guard.pending().unwrap().is_some());
    after_refresh(&guard, primary_weekly_quota(91), &mut operations)
        .await
        .unwrap();
    assert_eq!(operations.consumed.len(), 1);
    assert!(guard.pending().unwrap().is_none());
    after_refresh(&guard, primary_weekly_quota(1), &mut operations)
        .await
        .unwrap();
    assert_eq!(operations.consumed.len(), 2);
}

#[tokio::test]
async fn five_hour_only_exhaustion_never_consumes_a_weekly_reset() {
    let dir = TempDir::new();
    let guard = dir.guard();
    let mut operations = MockOperations::new(&guard);
    let mut hourly_only = quota(100, 0);
    hourly_only.weekly_window_present = Some(false);
    hourly_only.weekly_window_minutes = None;
    assert_eq!(weekly_remaining(&hourly_only), None);
    after_refresh(&guard, hourly_only, &mut operations)
        .await
        .unwrap();
    assert!(operations.events.is_empty());
    // A present healthy secondary week also prevents a 5h exhaustion trigger.
    after_refresh(&guard, quota(91, 0), &mut operations)
        .await
        .unwrap();
    assert!(operations.events.is_empty());
}

#[test]
fn weekly_window_duration_and_presence_boundaries_fail_closed() {
    for minutes in [
        None,
        Some(-1),
        Some(0),
        Some(300),
        Some(1440),
        Some(10078),
        Some(10081),
        Some(20160),
    ] {
        let mut primary = primary_weekly_quota(0);
        primary.hourly_window_minutes = minutes;
        assert_eq!(
            weekly_remaining(&primary),
            None,
            "primary duration={minutes:?}"
        );
    }
    for minutes in [Some(10079), Some(10080)] {
        let mut primary = primary_weekly_quota(1);
        primary.hourly_window_minutes = minutes;
        assert_eq!(weekly_remaining(&primary), Some(1));
        let mut secondary = quota(2, 0);
        secondary.weekly_window_minutes = minutes;
        assert_eq!(weekly_remaining(&secondary), Some(2));
    }
    for present in [None, Some(false)] {
        let mut primary = primary_weekly_quota(0);
        primary.hourly_window_present = present;
        assert_eq!(weekly_remaining(&primary), None);
    }
    let mut legacy_secondary = quota(2, 0);
    legacy_secondary.weekly_window_minutes = None;
    assert_eq!(weekly_remaining(&legacy_secondary), Some(2));
    legacy_secondary.weekly_window_present = None;
    assert_eq!(weekly_remaining(&legacy_secondary), None);
}

#[test]
fn multiple_weekly_windows_use_minimum_and_reject_any_invalid_weekly_percentage() {
    let mut both = quota(7, 1);
    both.hourly_window_minutes = Some(10080);
    assert_eq!(weekly_remaining(&both), Some(1));
    both.hourly_percentage = 9;
    assert_eq!(weekly_remaining(&both), Some(7));
    for (primary, secondary) in [(-1, 0), (101, 0), (0, -1), (0, 101)] {
        both.hourly_percentage = primary;
        both.weekly_percentage = secondary;
        assert_eq!(
            weekly_remaining(&both),
            None,
            "primary={primary}, secondary={secondary}"
        );
    }
    for remaining in [-1, 101] {
        assert_eq!(weekly_remaining(&primary_weekly_quota(remaining)), None);
    }
}

#[tokio::test]
async fn custom_threshold_is_used_and_invalid_thresholds_fail_closed() {
    for (threshold, weekly, expected) in [
        (20, 20, 1),
        (20, 21, 0),
        (0, 0, 1),
        (99, 99, 1),
        (-1, 0, 0),
        (100, 100, 0),
    ] {
        let dir = TempDir::new();
        let guard = dir.guard();
        let mut operations = MockOperations::new(&guard);
        operations.settings.threshold = threshold;
        after_refresh(&guard, quota(weekly, 100), &mut operations)
            .await
            .unwrap();
        assert_eq!(operations.consumed.len(), expected, "threshold={threshold}");
    }
}

#[tokio::test]
async fn absent_unknown_daily_or_invalid_secondary_window_never_triggers() {
    let mut samples = Vec::new();
    for present in [None, Some(false)] {
        let mut value = quota(0, 0);
        value.weekly_window_present = present;
        samples.push(value);
    }
    for minutes in [0, 300, 1440, -1] {
        let mut value = quota(0, 0);
        value.weekly_window_minutes = Some(minutes);
        samples.push(value);
    }
    samples.extend([quota(-1, 0), quota(101, 0)]);
    for sample in samples {
        let dir = TempDir::new();
        let guard = dir.guard();
        let mut operations = MockOperations::new(&guard);
        after_refresh(&guard, sample, &mut operations)
            .await
            .unwrap();
        assert!(operations.events.is_empty());
    }
}

#[test]
fn earliest_expiry_is_selected_independently_of_server_order() {
    let samples = [
        credit("late", Some(300)),
        credit("soon", Some(200)),
        credit("no-expiry", None),
    ];
    for order in [
        [0, 1, 2],
        [0, 2, 1],
        [1, 0, 2],
        [1, 2, 0],
        [2, 0, 1],
        [2, 1, 0],
    ] {
        let credits: Vec<_> = order.into_iter().map(|i| samples[i].clone()).collect();
        assert_eq!(
            earliest_credit(&credits, 100).unwrap().id.as_deref(),
            Some("soon")
        );
    }
    assert_eq!(
        earliest_credit(&[credit("no-expiry", None)], 100)
            .unwrap()
            .id
            .as_deref(),
        Some("no-expiry")
    );
}

#[test]
fn invalid_expired_redeeming_and_redeemed_credits_are_excluded() {
    let mut invalid = vec![credit("expired", Some(99)), credit("boundary", Some(100))];
    for status in [
        "redeemed",
        "used",
        "consumed",
        "expired",
        "redeeming",
        "revoked",
        "unknown",
        "",
    ] {
        let mut item = credit(status, Some(200));
        item.status = Some(status.into());
        invalid.push(item);
    }
    let mut redeemed = credit("redeemed-with-available-status", Some(200));
    redeemed.redeemed_at = Some(90);
    invalid.push(redeemed);
    for id in [None, Some("".into()), Some("  ".into())] {
        let mut item = credit("invalid-id", Some(200));
        item.id = id;
        invalid.push(item);
    }
    let mut missing_status = credit("missing-status", None);
    missing_status.status = None;
    invalid.push(missing_status);
    assert!(earliest_credit(&invalid, 100).is_none());
    let mut valid = credit("valid", Some(500));
    valid.status = Some(" AVAILABLE ".into());
    invalid.push(valid);
    assert_eq!(
        earliest_credit(&invalid, 100).unwrap().id.as_deref(),
        Some("valid")
    );
}

#[test]
fn raw_status_fallback_and_equal_expiry_tie_are_deterministic() {
    let mut first = credit("a", Some(200));
    first.status = None;
    first.raw_status = Some("available".into());
    let credits = [credit("b", Some(200)), first];
    assert_eq!(
        earliest_credit(&credits, 100).unwrap().id.as_deref(),
        Some("a")
    );
}

#[test]
fn only_full_reset_or_legacy_unspecified_types_are_eligible() {
    for reset_type in ["hourly", "codex_5h", "future_reset_type", ""] {
        let mut item = credit("wrong-type", Some(200));
        item.reset_type = Some(reset_type.into());
        assert!(earliest_credit(&[item], 100).is_none(), "{reset_type}");
    }
    for reset_type in [
        None,
        Some(" CODEX_RATE_LIMITS ".into()),
        Some("rate_limit_reset".into()),
    ] {
        let mut item = credit("full-or-legacy", Some(200));
        item.reset_type = reset_type;
        assert!(earliest_credit(&[item], 100).is_some());
    }
}

#[test]
fn known_legacy_reset_type_keeps_earliest_expiry_priority_over_official_type() {
    let mut legacy = credit("legacy-earliest", Some(200));
    legacy.reset_type = Some(" RATE_LIMIT_RESET ".into());
    let official = credit("official-later", Some(300));
    for credits in [
        vec![official.clone(), legacy.clone()],
        vec![legacy, official],
    ] {
        assert_eq!(
            earliest_credit(&credits, 100).unwrap().id.as_deref(),
            Some("legacy-earliest")
        );
    }
}

#[tokio::test]
async fn no_available_resets_is_a_noop() {
    for credits in [vec![], vec![credit("expired", Some(1))]] {
        let dir = TempDir::new();
        let guard = dir.guard();
        let mut operations = MockOperations::new(&guard);
        operations.credits = Ok(credits);
        after_refresh(&guard, quota(0, 0), &mut operations)
            .await
            .unwrap();
        assert_eq!(operations.events, ["fetch"]);
        assert!(guard.pending().unwrap().is_none());
    }
}

#[tokio::test]
async fn consumes_only_earliest_one_then_returns_latest_quota() {
    let dir = TempDir::new();
    let guard = dir.guard();
    let mut operations = MockOperations::new(&guard);
    let now = chrono::Utc::now().timestamp();
    operations.credits = Ok(vec![
        credit("late", Some(now + 600)),
        credit("soon", Some(now + 300)),
        credit("never", None),
    ]);
    let latest = after_refresh(&guard, quota(2, 100), &mut operations)
        .await
        .unwrap();
    assert_eq!(operations.consumed.len(), 1);
    assert_eq!(operations.consumed[0].0, "soon");
    assert_eq!(operations.events, ["fetch", "consume", "refresh"]);
    assert_eq!(latest.weekly_percentage, 100);
    assert!(guard.pending().unwrap().is_none());
}

#[tokio::test]
async fn disable_or_lower_threshold_during_query_cancels_consumption() {
    for settings in [
        Settings {
            enabled: false,
            threshold: 2,
        },
        Settings {
            enabled: true,
            threshold: 1,
        },
    ] {
        let dir = TempDir::new();
        let guard = dir.guard();
        let mut operations = MockOperations::new(&guard);
        operations.settings_after_fetch = Some(settings);
        after_refresh(&guard, quota(2, 100), &mut operations)
            .await
            .unwrap();
        assert_eq!(operations.events, ["fetch"]);
        assert!(guard.pending().unwrap().is_none());
    }
}

#[tokio::test]
async fn lock_is_held_across_post_and_post_reset_refresh() {
    let dir = TempDir::new();
    let guard = dir.guard();
    let mut operations = MockOperations::new(&guard);
    let consume_gate = Arc::new(Gate::default());
    let refresh_gate = Arc::new(Gate::default());
    operations.consume_gate = Some(consume_gate.clone());
    operations.refresh_gate = Some(refresh_gate.clone());
    let task = after_refresh(&guard, quota(2, 100), &mut operations);
    let check_competitors = async {
        consume_gate.entered.notified().await;
        assert!(ResetGuard::acquire(&dir.0, "mock-account")
            .unwrap()
            .is_none());
        assert!(guard.pending().unwrap().is_some());
        consume_gate.release.notify_one();
        refresh_gate.entered.notified().await;
        assert!(ResetGuard::acquire(&dir.0, "mock-account")
            .unwrap()
            .is_none());
        assert!(guard.pending().unwrap().is_some());
        refresh_gate.release.notify_one();
    };
    let (result, ()) = tokio::join!(task, check_competitors);
    result.unwrap();
    assert_eq!(operations.consumed.len(), 1);
    drop(guard);
    assert!(ResetGuard::acquire(&dir.0, "mock-account")
        .unwrap()
        .is_some());
}

#[test]
fn guard_excludes_same_account_but_allows_other_accounts() {
    let dir = TempDir::new();
    let guard = dir.guard();
    assert!(ResetGuard::acquire(&dir.0, "mock-account")
        .unwrap()
        .is_none());
    assert!(ResetGuard::acquire(&dir.0, "different-account")
        .unwrap()
        .is_some());
    drop(guard);
    assert!(ResetGuard::acquire(&dir.0, "mock-account")
        .unwrap()
        .is_some());
}

#[tokio::test]
async fn failed_cached_or_unchanged_refresh_keeps_durable_latch_after_restart() {
    for refreshed in [
        Err("offline".into()),
        Ok(None),
        Ok(Some(quota(2, 100))),
        Ok(Some(quota(0, 100))),
    ] {
        let dir = TempDir::new();
        let guard = dir.guard();
        let mut operations = MockOperations::new(&guard);
        let awaiting_refresh = !matches!(&refreshed, Ok(Some(_)));
        operations.refreshed = refreshed;
        after_refresh(&guard, quota(2, 0), &mut operations)
            .await
            .unwrap();
        let pending = guard.pending().unwrap().unwrap();
        assert_eq!(pending.awaiting_refresh, awaiting_refresh);
        let request = pending.redeem_request_id;
        drop(guard);
        let restarted = dir.guard();
        let mut operations = MockOperations::new(&restarted);
        after_refresh(&restarted, quota(2, 100), &mut operations)
            .await
            .unwrap();
        assert!(operations.events.is_empty());
        let pending = restarted.pending().unwrap().unwrap();
        assert_eq!(pending.redeem_request_id, request);
        assert!(
            !pending.awaiting_refresh,
            "fresh low observation clears only manual refresh barrier"
        );
    }
}

#[tokio::test]
async fn ambiguous_post_failure_refreshes_but_never_retries_another_credit() {
    let dir = TempDir::new();
    let guard = dir.guard();
    let mut operations = MockOperations::new(&guard);
    operations.consume_result = Err("connection lost after request was sent".into());
    operations.refreshed = Err("offline".into());
    after_refresh(&guard, quota(1, 100), &mut operations)
        .await
        .unwrap();
    assert_eq!(operations.events, ["fetch", "consume", "refresh"]);
    let request = operations.consumed[0].1.clone();
    after_refresh(&guard, quota(1, 100), &mut operations)
        .await
        .unwrap();
    assert_eq!(operations.consumed.len(), 1);
    assert_eq!(guard.pending().unwrap().unwrap().redeem_request_id, request);
}

#[tokio::test]
async fn cancellation_after_post_start_leaves_persisted_barrier() {
    let dir = TempDir::new();
    let guard = dir.guard();
    let mut operations = MockOperations::new(&guard);
    operations.consume_gate = Some(Arc::new(Gate::default()));
    assert!(tokio::time::timeout(
        std::time::Duration::from_millis(20),
        after_refresh(&guard, quota(0, 100), &mut operations)
    )
    .await
    .is_err());
    assert_eq!(operations.consumed.len(), 1);
    assert_eq!(operations.refresh_calls, 0);
    assert!(guard.pending().unwrap().unwrap().awaiting_refresh);
    drop(guard);
    let restarted = dir.guard();
    let mut operations = MockOperations::new(&restarted);
    after_refresh(&restarted, quota(0, 100), &mut operations)
        .await
        .unwrap();
    assert!(operations.events.is_empty());
}

#[tokio::test]
async fn fresh_recovery_rearms_only_for_a_later_low_observation() {
    let dir = TempDir::new();
    let guard = dir.guard();
    let mut operations = MockOperations::new(&guard);
    operations.refreshed = Ok(Some(quota(2, 100)));
    after_refresh(&guard, quota(2, 100), &mut operations)
        .await
        .unwrap();
    let first_request = operations.consumed[0].1.clone();
    after_refresh(&guard, quota(3, 100), &mut operations)
        .await
        .unwrap();
    assert!(guard.pending().unwrap().is_none());
    assert_eq!(operations.consumed.len(), 1);
    after_refresh(&guard, quota(2, 100), &mut operations)
        .await
        .unwrap();
    assert_eq!(operations.consumed.len(), 2);
    assert_ne!(operations.consumed[1].1, first_request);
}

#[tokio::test]
async fn toggling_or_lowering_threshold_cannot_bypass_existing_attempt() {
    let dir = TempDir::new();
    let guard = dir.guard();
    let mut operations = MockOperations::new(&guard);
    operations.refreshed = Ok(None);
    after_refresh(&guard, quota(2, 100), &mut operations)
        .await
        .unwrap();
    operations.settings.enabled = false;
    operations.settings.threshold = 0;
    after_refresh(&guard, quota(1, 100), &mut operations)
        .await
        .unwrap();
    operations.settings.enabled = true;
    after_refresh(&guard, quota(0, 100), &mut operations)
        .await
        .unwrap();
    assert_eq!(operations.consumed.len(), 1);
    assert!(guard.pending().unwrap().is_some());
}

#[tokio::test]
async fn missing_weekly_after_reset_does_not_clear_attempt() {
    for present in [None, Some(false)] {
        let dir = TempDir::new();
        let guard = dir.guard();
        let mut operations = MockOperations::new(&guard);
        let mut latest = quota(100, 100);
        latest.weekly_window_present = present;
        operations.refreshed = Ok(Some(latest));
        after_refresh(&guard, quota(0, 0), &mut operations)
            .await
            .unwrap();
        let pending = guard.pending().unwrap().unwrap();
        assert!(!pending.awaiting_refresh);
        after_refresh(&guard, quota(0, 100), &mut operations)
            .await
            .unwrap();
        assert_eq!(operations.consumed.len(), 1);
        assert!(guard.pending().unwrap().is_some());
    }
}

#[test]
fn legacy_attempt_without_refresh_marker_remains_blocked() {
    let dir = TempDir::new();
    let guard = dir.guard();
    fs::write(
        &guard.state_path,
        r#"{"credit_id":"credit-1","redeem_request_id":"request-1","threshold":2}"#,
    )
    .unwrap();
    assert!(guard.pending().unwrap().unwrap().awaiting_refresh);
}

#[tokio::test]
async fn corrupted_durable_state_fails_closed_before_any_operation() {
    let dir = TempDir::new();
    let guard = dir.guard();
    fs::write(&guard.state_path, "{invalid-json").unwrap();
    let mut operations = MockOperations::new(&guard);
    assert!(after_refresh(&guard, quota(0, 0), &mut operations)
        .await
        .is_err());
    assert!(operations.events.is_empty());
}

#[tokio::test]
async fn state_write_failure_prevents_post() {
    let dir = TempDir::new();
    let guard = dir.guard();
    let mut operations = MockOperations::new(&guard);
    // Initial read succeeds. The mocked GET then obstructs the state-file
    // destination, proving the durable write itself must succeed before POST.
    operations.sabotage_state_write = true;
    assert!(after_refresh(&guard, quota(0, 100), &mut operations)
        .await
        .is_err());
    assert_eq!(operations.events, ["fetch"]);
    assert!(operations.consumed.is_empty());
    assert_eq!(operations.refresh_calls, 0);
}

#[tokio::test]
async fn query_failure_never_creates_consumption_state() {
    let dir = TempDir::new();
    let guard = dir.guard();
    let mut operations = MockOperations::new(&guard);
    operations.credits = Err("mock query failed".into());
    assert!(after_refresh(&guard, quota(0, 0), &mut operations)
        .await
        .is_err());
    assert_eq!(operations.events, ["fetch"]);
    assert!(guard.pending().unwrap().is_none());
}

#[test]
fn request_shape_matches_official_optional_credit_id_contract() {
    assert_eq!(
        consume_body("redeem-1", None),
        serde_json::json!({ "redeem_request_id": "redeem-1" })
    );
    assert_eq!(
        consume_body("redeem-2", Some("credit-2")),
        serde_json::json!({ "redeem_request_id": "redeem-2", "credit_id": "credit-2" })
    );
}

#[test]
fn consume_response_requires_known_success_code_not_just_http_success() {
    for body in [
        r#"{"code":"reset","windows_reset":2}"#,
        r#"{"code":"already_redeemed","windows_reset":0}"#,
    ] {
        assert!(validate_consume_response(body).is_ok());
    }
    for body in [
        "",
        "not json",
        "null",
        "{}",
        r#"{"code":"no_credit"}"#,
        r#"{"code":"nothing_to_reset"}"#,
        r#"{"code":"unknown"}"#,
        r#"{"code":true}"#,
    ] {
        assert!(validate_consume_response(body).is_err(), "body={body}");
    }
}

#[test]
fn account_settings_default_disabled_with_two_percent_threshold() {
    assert_eq!(
        Settings::default(),
        Settings {
            enabled: false,
            threshold: 2
        }
    );
    let empty: Settings = serde_json::from_str("{}").unwrap();
    assert_eq!(empty, Settings::default());
    assert_eq!(
        serde_json::from_str::<Settings>(r#"{"enabled":true}"#).unwrap(),
        Settings {
            enabled: true,
            threshold: 2
        }
    );
    assert_eq!(
        serde_json::from_str::<Settings>(r#"{"threshold":7}"#).unwrap(),
        Settings {
            enabled: false,
            threshold: 7
        }
    );
    let preferences: Preferences = serde_json::from_str("{}").unwrap();
    assert_eq!(preferences.for_account("account-a"), Settings::default());
    assert_eq!(preferences.for_account("account-b"), Settings::default());
}

#[test]
fn legacy_global_enabled_and_threshold_never_enable_any_account() {
    for legacy in [
        serde_json::json!({ "codex_auto_consume_reset_enabled": true, "codex_auto_consume_reset_threshold": 99 }),
        serde_json::json!({ "codex_auto_consume_reset_enabled": true, "codex_auto_consume_reset_threshold": 0, "codex_auto_consume_reset_accounts": {} }),
    ] {
        let preferences: Preferences = serde_json::from_value(legacy).unwrap();
        for account_id in ["account-a", "account-b", "newly-imported-account"] {
            assert_eq!(preferences.for_account(account_id), Settings::default());
        }
        let saved = serde_json::to_value(preferences).unwrap();
        assert_eq!(
            saved,
            serde_json::json!({ "codex_auto_consume_reset_accounts": {} })
        );
    }
}

#[test]
fn persisted_account_map_wins_over_ignored_old_global_configuration() {
    let preferences: Preferences = serde_json::from_value(serde_json::json!({
        "codex_auto_consume_reset_enabled": true,
        "codex_auto_consume_reset_threshold": 99,
        "codex_auto_consume_reset_accounts": {
            "account-a": { "enabled": true, "threshold": 7 },
            "account-b": { "enabled": false, "threshold": 3 },
            "account-c": {}
        }
    }))
    .unwrap();
    assert_eq!(
        preferences.for_account("account-a"),
        Settings {
            enabled: true,
            threshold: 7
        }
    );
    assert_eq!(
        preferences.for_account("account-b"),
        Settings {
            enabled: false,
            threshold: 3
        }
    );
    assert_eq!(preferences.for_account("account-c"), Settings::default());
    assert_eq!(preferences.for_account("new-account"), Settings::default());
}

#[test]
fn account_preference_edits_preserve_other_accounts_and_unspecified_fields() {
    let mut preferences = Preferences::default();
    preferences
        .patch_account("account-b", Some(true), Some(13))
        .unwrap();
    let account_b = preferences.for_account("account-b");

    assert_eq!(
        preferences
            .patch_account("account-a", Some(true), None)
            .unwrap(),
        Settings {
            enabled: true,
            threshold: 2
        }
    );
    assert_eq!(preferences.for_account("account-b"), account_b);
    assert_eq!(
        preferences
            .patch_account("account-a", None, Some(7))
            .unwrap(),
        Settings {
            enabled: true,
            threshold: 7
        }
    );
    assert_eq!(preferences.for_account("account-b"), account_b);
    assert_eq!(
        preferences
            .patch_account("account-a", Some(false), None)
            .unwrap(),
        Settings {
            enabled: false,
            threshold: 7
        }
    );
    assert_eq!(preferences.for_account("account-b"), account_b);
    assert_eq!(
        preferences
            .patch_account("account-a", None, Some(9))
            .unwrap(),
        Settings {
            enabled: false,
            threshold: 9
        }
    );
    assert_eq!(preferences.for_account("account-b"), account_b);
    assert_eq!(
        preferences.patch_account("account-a", None, None).unwrap(),
        Settings {
            enabled: false,
            threshold: 9
        }
    );
    assert_eq!(
        preferences.for_account("never-configured"),
        Settings::default()
    );
}

#[test]
fn invalid_account_preference_patch_never_partially_changes_or_adds_entry() {
    let mut preferences = Preferences::default();
    preferences
        .patch_account("account-a", Some(true), Some(7))
        .unwrap();
    preferences
        .patch_account("account-b", Some(false), Some(11))
        .unwrap();
    let before = serde_json::to_value(&preferences).unwrap();
    for account_id in ["account-a", "new-account"] {
        for threshold in [-1, 100, i32::MIN, i32::MAX] {
            assert!(preferences
                .patch_account(account_id, Some(false), Some(threshold))
                .is_err());
            assert_eq!(serde_json::to_value(&preferences).unwrap(), before);
        }
    }
    for account_id in ["", " ", " account-a", "account-a ", "\taccount-a"] {
        assert!(preferences
            .patch_account(account_id, Some(false), Some(2))
            .is_err());
        assert_eq!(serde_json::to_value(&preferences).unwrap(), before);
    }
    preferences
        .patch_account("account-a", None, Some(0))
        .unwrap();
    preferences
        .patch_account("account-b", None, Some(99))
        .unwrap();
    assert_eq!(
        preferences.for_account("account-a"),
        Settings {
            enabled: true,
            threshold: 0
        }
    );
    assert_eq!(
        preferences.for_account("account-b"),
        Settings {
            enabled: false,
            threshold: 99
        }
    );
}

#[test]
fn production_preferences_survive_atomic_persistence_and_restart() {
    let dir = TempDir::new();
    let path = dir.0.join("settings.json");
    let mut preferences = Preferences::default();
    preferences
        .patch_account("account-a", Some(true), Some(7))
        .unwrap();
    preferences
        .patch_account("account-b", Some(false), Some(11))
        .unwrap();
    atomic_write::write_string_atomic(&path, &serde_json::to_string(&preferences).unwrap())
        .unwrap();
    drop(preferences);

    // Deserialize the real production Preferences type from bytes written by
    // production atomic_write, never touching the user's real config path.
    let mut restarted: Preferences =
        serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(
        restarted.for_account("account-a"),
        Settings {
            enabled: true,
            threshold: 7
        }
    );
    assert_eq!(
        restarted.for_account("account-b"),
        Settings {
            enabled: false,
            threshold: 11
        }
    );
    assert_eq!(restarted.for_account("unconfigured"), Settings::default());
    restarted
        .patch_account("account-b", Some(true), None)
        .unwrap();
    atomic_write::write_string_atomic(&path, &serde_json::to_string(&restarted).unwrap()).unwrap();
    drop(restarted);
    let restarted_again: Preferences =
        serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(
        restarted_again.for_account("account-a"),
        Settings {
            enabled: true,
            threshold: 7
        }
    );
    assert_eq!(
        restarted_again.for_account("account-b"),
        Settings {
            enabled: true,
            threshold: 11
        }
    );
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&fs::read_to_string(&path).unwrap()).unwrap(),
        serde_json::json!({
            "codex_auto_consume_reset_accounts": {
                "account-a": { "enabled": true, "threshold": 7 },
                "account-b": { "enabled": true, "threshold": 11 }
            }
        })
    );
}

#[tokio::test]
async fn two_low_quota_accounts_consume_only_for_the_explicitly_enabled_account() {
    let dir = TempDir::new();
    let guard_a = dir.guard_for("account-a");
    let guard_b = dir.guard_for("account-b");
    let mut preferences: Preferences = serde_json::from_value(serde_json::json!({
        "codex_auto_consume_reset_enabled": true,
        "codex_auto_consume_reset_threshold": 99
    }))
    .unwrap();
    preferences
        .patch_account("account-a", Some(true), None)
        .unwrap();
    let mut operations_a = MockOperations::new(&guard_a);
    operations_a.settings = preferences.for_account("account-a");
    operations_a.credits = Ok(vec![credit("account-a-credit", None)]);
    let mut operations_b = MockOperations::new(&guard_b);
    operations_b.settings = preferences.for_account("account-b");
    operations_b.credits = Ok(vec![credit("account-b-credit", None)]);
    let (result_a, result_b) = tokio::join!(
        after_refresh(&guard_a, quota(2, 100), &mut operations_a),
        after_refresh(&guard_b, quota(0, 0), &mut operations_b),
    );
    result_a.unwrap();
    result_b.unwrap();
    assert_eq!(operations_a.consumed.len(), 1);
    assert_eq!(operations_a.consumed[0].0, "account-a-credit");
    assert!(operations_b.events.is_empty());
    assert!(guard_b.pending().unwrap().is_none());
}

#[tokio::test]
async fn per_account_thresholds_and_disabling_one_account_are_independent() {
    let dir = TempDir::new();
    let guard_a = dir.guard_for("account-a");
    let guard_b = dir.guard_for("account-b");
    let mut preferences = Preferences::default();
    preferences
        .patch_account("account-a", Some(true), Some(2))
        .unwrap();
    preferences
        .patch_account("account-b", Some(true), Some(7))
        .unwrap();
    let mut operations_a = MockOperations::new(&guard_a);
    let mut operations_b = MockOperations::new(&guard_b);
    operations_a.settings = preferences.for_account("account-a");
    operations_b.settings = preferences.for_account("account-b");
    after_refresh(&guard_a, quota(5, 0), &mut operations_a)
        .await
        .unwrap();
    after_refresh(&guard_b, quota(5, 100), &mut operations_b)
        .await
        .unwrap();
    assert!(operations_a.events.is_empty());
    assert_eq!(operations_b.consumed.len(), 1);

    preferences
        .patch_account("account-a", Some(false), Some(9))
        .unwrap();
    operations_a.settings = preferences.for_account("account-a");
    operations_b.settings = preferences.for_account("account-b");
    after_refresh(&guard_a, quota(0, 0), &mut operations_a)
        .await
        .unwrap();
    after_refresh(&guard_b, quota(5, 100), &mut operations_b)
        .await
        .unwrap();
    assert!(operations_a.events.is_empty());
    assert_eq!(
        operations_b.settings,
        Settings {
            enabled: true,
            threshold: 7
        }
    );
    assert_eq!(operations_b.consumed.len(), 2);
}

#[tokio::test]
async fn pending_attempt_and_recovery_are_isolated_between_accounts_after_restart() {
    let dir = TempDir::new();
    let guard_a = dir.guard_for("account-a");
    let guard_b = dir.guard_for("account-b");
    assert_ne!(guard_a.state_path, guard_b.state_path);
    let mut preferences = Preferences::default();
    preferences
        .patch_account("account-a", Some(true), Some(2))
        .unwrap();
    preferences
        .patch_account("account-b", Some(true), Some(7))
        .unwrap();
    let mut operations_a = MockOperations::new(&guard_a);
    operations_a.settings = preferences.for_account("account-a");
    operations_a.refreshed = Err("account-a refresh unavailable".into());
    let mut operations_b = MockOperations::new(&guard_b);
    operations_b.settings = preferences.for_account("account-b");
    operations_b.refreshed = Ok(Some(quota(7, 100)));
    let (result_a, result_b) = tokio::join!(
        after_refresh(&guard_a, quota(2, 100), &mut operations_a),
        after_refresh(&guard_b, quota(7, 100), &mut operations_b),
    );
    result_a.unwrap();
    result_b.unwrap();
    let pending_a = guard_a.pending().unwrap().unwrap();
    let pending_b = guard_b.pending().unwrap().unwrap();
    assert_ne!(pending_a.redeem_request_id, pending_b.redeem_request_id);
    assert!(pending_a.awaiting_refresh);
    assert!(!pending_b.awaiting_refresh);
    assert_eq!(pending_a.threshold, 2);
    assert_eq!(pending_b.threshold, 7);
    drop(guard_a);
    drop(guard_b);

    let restarted_a = dir.guard_for("account-a");
    let restarted_b = dir.guard_for("account-b");
    let mut operations_a = MockOperations::new(&restarted_a);
    operations_a.settings = preferences.for_account("account-a");
    let mut operations_b = MockOperations::new(&restarted_b);
    operations_b.settings = preferences.for_account("account-b");
    // A fresh recovery for B must not release A's durable barrier.
    after_refresh(&restarted_b, quota(100, 100), &mut operations_b)
        .await
        .unwrap();
    assert!(restarted_b.pending().unwrap().is_none());
    let still_pending_a = restarted_a.pending().unwrap().unwrap();
    assert_eq!(
        still_pending_a.redeem_request_id,
        pending_a.redeem_request_id
    );
    assert!(still_pending_a.awaiting_refresh);
    after_refresh(&restarted_a, quota(2, 100), &mut operations_a)
        .await
        .unwrap();
    after_refresh(&restarted_b, quota(7, 100), &mut operations_b)
        .await
        .unwrap();
    assert!(operations_a.events.is_empty());
    assert_eq!(operations_b.consumed.len(), 1);
    assert_eq!(
        restarted_a.pending().unwrap().unwrap().redeem_request_id,
        pending_a.redeem_request_id
    );
}
