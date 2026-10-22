use super::*;
use std::collections::HashMap;
use std::sync::Mutex;
use tokio::sync::Semaphore;

#[derive(Clone, Default)]
struct MockOperations(Arc<MockState>);

#[derive(Default)]
struct MockState {
    enabled: Mutex<Vec<String>>,
    calls: Mutex<Vec<String>>,
    completed: Mutex<Vec<Completion>>,
    gates: Mutex<HashMap<String, Arc<Semaphore>>>,
    failures: Mutex<HashSet<String>>,
    panics: Mutex<HashSet<String>>,
}

impl MockOperations {
    fn enabled(ids: &[&str]) -> Self {
        let operations = Self::default();
        operations.set_enabled(ids);
        operations
    }

    fn set_enabled(&self, ids: &[&str]) {
        *self.0.enabled.lock().unwrap() = ids.iter().map(|id| id.to_string()).collect();
    }

    fn calls(&self) -> Vec<String> {
        self.0.calls.lock().unwrap().clone()
    }

    fn gate(&self, id: &str) -> Arc<Semaphore> {
        let gate = Arc::new(Semaphore::new(0));
        self.0
            .gates
            .lock()
            .unwrap()
            .insert(id.to_string(), gate.clone());
        gate
    }
}

impl Operations for MockOperations {
    fn enabled_account_ids(&self) -> Vec<String> {
        self.0.enabled.lock().unwrap().clone()
    }

    async fn refresh(&self, account_id: String) -> Result<(), String> {
        self.0.calls.lock().unwrap().push(account_id.clone());
        let gate = self.0.gates.lock().unwrap().get(&account_id).cloned();
        if let Some(gate) = gate {
            let _permit = gate.acquire().await.unwrap();
        }
        if self.0.panics.lock().unwrap().contains(&account_id) {
            panic!("isolated mock refresh panic");
        }
        if self.0.failures.lock().unwrap().contains(&account_id) {
            return Err("isolated mock query failure".into());
        }
        Ok(())
    }

    fn refresh_finished(&self, id: &str, result: &Result<(), String>) {
        self.0
            .completed
            .lock()
            .unwrap()
            .push((id.into(), result.clone()));
    }
}

async fn finish_one(driver: &mut Driver<MockOperations>, now: i64) {
    let completion = driver.pending.next().await.expect("queued refresh");
    driver.finished(completion, now);
}

async fn poll_pending(driver: &mut Driver<MockOperations>) {
    assert!(
        tokio::time::timeout(Duration::from_millis(5), driver.pending.next())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn empty_defaults_do_nothing_and_new_enabled_accounts_run_immediately() {
    let operations = MockOperations::default();
    let mut driver = Driver::new(operations.clone());
    driver.scan(0);
    assert!(driver.pending.is_empty());
    assert!(operations.calls().is_empty());
    operations.set_enabled(&["a", "a", "", " bad "]);
    driver.scan(5_000);
    assert_eq!(driver.pending.len(), 1);
    finish_one(&mut driver, 5_000).await;
    assert_eq!(operations.calls(), ["a"]);
    driver.scan(64_999);
    assert!(driver.pending.is_empty());
    driver.scan(65_000);
    finish_one(&mut driver, 65_000).await;
    assert_eq!(operations.calls(), ["a", "a"]);
}

#[tokio::test]
async fn disabled_accounts_are_removed_and_queued_disable_is_rechecked() {
    let operations = MockOperations::enabled(&["a", "b"]);
    let mut driver = Driver::new(operations.clone());
    driver.scan(0);
    operations.set_enabled(&[]); // Disable between scan and future execution.
    finish_one(&mut driver, 0).await;
    finish_one(&mut driver, 0).await;
    driver.scan(1);
    assert!(driver.schedule.accounts.is_empty());
    assert!(operations.calls().is_empty());
    operations.set_enabled(&["b"]);
    driver.scan(2);
    finish_one(&mut driver, 2).await;
    assert_eq!(operations.calls(), ["b"]);
}

#[tokio::test]
async fn bounded_single_flight_is_fair_to_waiting_accounts() {
    let operations = MockOperations::enabled(&["d", "c", "b", "a"]);
    let gate_a = operations.gate("a");
    let gate_b = operations.gate("b");
    let mut driver = Driver::new(operations.clone());
    driver.scan(0);
    poll_pending(&mut driver).await;
    assert_eq!(operations.calls(), ["a", "b"]);
    for now in [5_000, 65_000, 300_000] {
        driver.scan(now);
        assert_eq!(driver.pending.len(), 2);
    }
    gate_a.add_permits(1);
    finish_one(&mut driver, 300_000).await;
    driver.scan(300_000);
    finish_one(&mut driver, 300_000).await;
    driver.scan(300_000);
    finish_one(&mut driver, 300_000).await;
    assert_eq!(operations.calls(), ["a", "b", "c", "d"]);
    gate_b.add_permits(1);
    finish_one(&mut driver, 300_000).await;
    assert!(driver.pending.is_empty());
}

#[tokio::test]
async fn disable_then_enable_during_refresh_never_duplicates_in_flight_account() {
    let operations = MockOperations::enabled(&["a"]);
    let gate = operations.gate("a");
    let mut driver = Driver::new(operations.clone());
    driver.scan(0);
    poll_pending(&mut driver).await;
    operations.set_enabled(&[]);
    driver.scan(5_000);
    operations.set_enabled(&["a"]);
    driver.scan(10_000);
    assert_eq!(driver.pending.len(), 1);
    assert_eq!(operations.calls(), ["a"]);
    gate.add_permits(1);
    finish_one(&mut driver, 10_000).await;
    driver.scan(10_000);
    assert!(driver.pending.is_empty());
}

#[tokio::test]
async fn failures_and_panics_release_capacity_and_retry_next_interval() {
    let operations = MockOperations::enabled(&["a", "b", "c"]);
    operations.0.panics.lock().unwrap().insert("a".into());
    operations.0.failures.lock().unwrap().insert("b".into());
    let mut driver = Driver::new(operations.clone());
    driver.scan(0);
    finish_one(&mut driver, 0).await;
    finish_one(&mut driver, 0).await;
    driver.scan(0);
    finish_one(&mut driver, 0).await;
    assert_eq!(operations.calls(), ["a", "b", "c"]);
    let completed = operations.0.completed.lock().unwrap().clone();
    assert!(completed[0].1.is_err());
    assert!(completed[1].1.is_err());
    assert!(completed[2].1.is_ok());
    driver.scan(59_999);
    assert!(driver.pending.is_empty());
    operations.set_enabled(&["a", "b"]);
    driver.scan(60_000);
    finish_one(&mut driver, 60_000).await;
    finish_one(&mut driver, 60_000).await;
    assert_eq!(operations.calls(), ["a", "b", "c", "a", "b"]);
}

#[tokio::test]
async fn forward_clock_jump_and_repeated_wakes_coalesce_without_catch_up_burst() {
    let operations = MockOperations::enabled(&["a"]);
    let mut driver = Driver::new(operations.clone());
    driver.scan(0);
    finish_one(&mut driver, 0).await;
    let after_sleep = 8 * 60 * 60 * 1_000;
    driver.scan(after_sleep);
    for _ in 0..20 {
        driver.scan(after_sleep);
    }
    assert_eq!(driver.pending.len(), 1);
    finish_one(&mut driver, after_sleep).await;
    for _ in 0..20 {
        driver.scan(after_sleep);
    }
    assert!(driver.pending.is_empty());
    assert_eq!(operations.calls(), ["a", "a"]);
}

#[tokio::test]
async fn backward_clock_correction_does_not_stall_until_old_wall_time() {
    let operations = MockOperations::enabled(&["a"]);
    let mut driver = Driver::new(operations.clone());
    driver.scan(1_000_000);
    finish_one(&mut driver, 1_000_000).await;
    driver.scan(0);
    assert!(driver.pending.is_empty());
    driver.scan(59_999);
    assert!(driver.pending.is_empty());
    driver.scan(60_000);
    finish_one(&mut driver, 60_000).await;
    assert_eq!(operations.calls(), ["a", "a"]);
}

#[tokio::test]
async fn real_run_loop_needs_no_window_and_settings_wakeup_discovers_new_account() {
    let operations = MockOperations::default();
    let notify = Arc::new(Notify::new());
    let task = tokio::spawn(run(operations.clone(), notify.clone()));
    tokio::task::yield_now().await;
    assert!(operations.calls().is_empty());
    operations.set_enabled(&["a"]);
    notify.notify_one();
    tokio::time::timeout(Duration::from_secs(1), async {
        while operations.calls().is_empty() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("settings wake should not wait for the 5-second scan");
    assert_eq!(operations.calls(), ["a"]);
    for _ in 0..10 {
        notify.notify_one();
    }
    tokio::task::yield_now().await;
    assert_eq!(operations.calls(), ["a"]);
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
}

#[tokio::test]
async fn headless_monitor_refreshes_weekly_primary_and_consumes_one_earliest_reset() {
    use crate::models::codex::{CodexQuota, CodexResetCredit};
    use crate::modules::codex_auto_reset::{self as policy, Preferences, ResetGuard, Settings};
    use std::path::PathBuf;

    struct TemporaryData(PathBuf);
    impl Drop for TemporaryData {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn primary_weekly(remaining: i32) -> CodexQuota {
        serde_json::from_value(serde_json::json!({
            "hourly_percentage": remaining,
            "hourly_window_minutes": 10080,
            "hourly_window_present": true,
            "weekly_percentage": 100,
            "weekly_window_present": false,
            "reset_credits_available": 2,
            "raw_data": { "rate_limit": {
                "primary_window": { "limit_window_seconds": 604800, "used_percent": 100 - remaining },
                "secondary_window": null
            }}
        })).unwrap()
    }

    struct EndToEndState {
        preferences: Preferences,
        data_dir: PathBuf,
        events: Mutex<Vec<(String, &'static str)>>,
        consumed: Mutex<Vec<(String, String, String)>>,
        latest_weekly: Mutex<Option<i32>>,
        completions: Mutex<Vec<Completion>>,
        quota_started: Notify,
        quota_release: Semaphore,
        finished: Notify,
    }

    struct ResetOperations<'a> {
        state: Arc<EndToEndState>,
        account_id: String,
        guard: &'a ResetGuard,
    }

    impl policy::Operations for ResetOperations<'_> {
        fn settings(&self) -> Settings {
            self.state.preferences.for_account(&self.account_id)
        }

        async fn fetch_credits(&mut self) -> Result<Vec<CodexResetCredit>, String> {
            self.state
                .events
                .lock()
                .unwrap()
                .push((self.account_id.clone(), "credits"));
            let now = chrono::Utc::now().timestamp();
            Ok(serde_json::from_value(serde_json::json!([
                { "id": "later-official", "status": "available", "reset_type": "codex_rate_limits", "expires_at": now + 600 },
                { "id": "earliest-legacy", "status": "available", "reset_type": "rate_limit_reset", "expires_at": now + 300 }
            ])).unwrap())
        }

        async fn consume(&mut self, credit_id: &str, request_id: &str) -> Result<(), String> {
            let pending = self.guard.pending()?.expect("persisted before consumption");
            assert_eq!(pending.credit_id.as_deref(), Some(credit_id));
            assert_eq!(pending.redeem_request_id, request_id);
            assert!(pending.awaiting_refresh);
            assert!(uuid::Uuid::parse_str(request_id).is_ok());
            self.state
                .events
                .lock()
                .unwrap()
                .push((self.account_id.clone(), "consume"));
            self.state.consumed.lock().unwrap().push((
                self.account_id.clone(),
                credit_id.into(),
                request_id.into(),
            ));
            Ok(())
        }

        async fn refresh(&mut self) -> Result<Option<CodexQuota>, String> {
            self.state
                .events
                .lock()
                .unwrap()
                .push((self.account_id.clone(), "refresh-after-reset"));
            Ok(Some(primary_weekly(91)))
        }
    }

    #[derive(Clone)]
    struct HeadlessOperations(Arc<EndToEndState>);

    impl Operations for HeadlessOperations {
        fn enabled_account_ids(&self) -> Vec<String> {
            self.0.preferences.enabled_account_ids()
        }

        async fn refresh(&self, account_id: String) -> Result<(), String> {
            let guard = ResetGuard::acquire(&self.0.data_dir, &account_id)?
                .expect("exclusive account guard");
            self.0
                .events
                .lock()
                .unwrap()
                .push((account_id.clone(), "quota"));
            self.0.quota_started.notify_one();
            self.0.quota_release.acquire().await.unwrap().forget();
            let mut operations = ResetOperations {
                state: self.0.clone(),
                account_id,
                guard: &guard,
            };
            let latest = policy::after_refresh(&guard, primary_weekly(0), &mut operations).await?;
            *self.0.latest_weekly.lock().unwrap() = policy::weekly_remaining(&latest);
            assert!(
                guard.pending()?.is_none(),
                "fresh primary recovery clears consumption barrier"
            );
            Ok(())
        }

        fn refresh_finished(&self, account_id: &str, result: &Result<(), String>) {
            self.0
                .completions
                .lock()
                .unwrap()
                .push((account_id.into(), result.clone()));
            self.0.finished.notify_one();
        }
    }

    let temporary = TemporaryData(std::env::temp_dir().join(format!(
        "cockpit-headless-reset-e2e-{}",
        uuid::Uuid::new_v4()
    )));
    std::fs::create_dir(&temporary.0).unwrap();
    let mut preferences = Preferences::default();
    preferences
        .patch_account("account-a", Some(true), Some(1))
        .unwrap();
    preferences
        .patch_account("account-b", Some(false), Some(1))
        .unwrap();
    let state = Arc::new(EndToEndState {
        preferences,
        data_dir: temporary.0.clone(),
        events: Mutex::new(Vec::new()),
        consumed: Mutex::new(Vec::new()),
        latest_weekly: Mutex::new(None),
        completions: Mutex::new(Vec::new()),
        quota_started: Notify::new(),
        quota_release: Semaphore::new(0),
        finished: Notify::new(),
    });
    // Run the actual backend loop. This harness has no WebView, AppHandle,
    // frontend timer, account store, authentication, or HTTP client.
    let monitor = tokio::spawn(run(
        HeadlessOperations(state.clone()),
        Arc::new(Notify::new()),
    ));
    tokio::time::timeout(Duration::from_secs(1), state.quota_started.notified())
        .await
        .expect("backend monitor starts refresh");
    assert_eq!(
        *state.events.lock().unwrap(),
        [("account-a".into(), "quota")]
    );
    assert!(state.consumed.lock().unwrap().is_empty());
    state.quota_release.add_permits(1);
    tokio::time::timeout(Duration::from_secs(1), state.finished.notified())
        .await
        .expect("headless reset completes");
    assert_eq!(
        *state.completions.lock().unwrap(),
        [("account-a".into(), Ok(()))]
    );
    let consumed = state.consumed.lock().unwrap().clone();
    assert_eq!(consumed.len(), 1);
    assert_eq!(
        (&*consumed[0].0, &*consumed[0].1),
        ("account-a", "earliest-legacy")
    );
    assert_eq!(*state.latest_weekly.lock().unwrap(), Some(91));
    assert_eq!(
        *state.events.lock().unwrap(),
        [
            ("account-a".into(), "quota"),
            ("account-a".into(), "credits"),
            ("account-a".into(), "consume"),
            ("account-a".into(), "refresh-after-reset"),
        ]
    );
    let disabled_guard = ResetGuard::acquire(&temporary.0, "account-b")
        .unwrap()
        .unwrap();
    assert!(disabled_guard.pending().unwrap().is_none());
    monitor.abort();
    assert!(monitor.await.unwrap_err().is_cancelled());
}
