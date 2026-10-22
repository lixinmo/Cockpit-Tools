//! Headless scheduling for opted-in accounts. The adapter owns account/config
//! access; this driver needs no window, WebView, AppHandle, or account credentials.

use std::collections::{BTreeMap, HashSet};
use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use futures_util::future::BoxFuture;
use futures_util::stream::{FuturesUnordered, StreamExt};
use futures_util::FutureExt;
use tokio::sync::Notify;

const SCAN_INTERVAL: Duration = Duration::from_secs(5);
const REFRESH_INTERVAL_MS: i64 = 60_000;
const MAX_IN_FLIGHT: usize = 2;

pub trait Operations: Send + Sync + 'static {
    /// Return only explicitly enabled, existing, supported accounts.
    fn enabled_account_ids(&self) -> Vec<String>;

    /// Recheck immediately before the queued refresh starts. Adapters may avoid
    /// rebuilding the whole list when checking one account.
    fn is_enabled(&self, account_id: &str) -> bool {
        self.enabled_account_ids().iter().any(|id| id == account_id)
    }

    fn refresh(&self, account_id: String) -> impl Future<Output = Result<(), String>> + Send;

    fn refresh_finished(&self, _account_id: &str, _result: &Result<(), String>) {}
}

#[derive(Default)]
struct Slot {
    enabled: bool,
    running: bool,
    due_at: i64,
}

#[derive(Default)]
struct MonitorSchedule {
    accounts: BTreeMap<String, Slot>,
    last_scan_at: Option<i64>,
}

impl MonitorSchedule {
    fn synchronize(&mut self, now: i64, enabled_ids: Vec<String>) {
        // SystemTime advances across machine sleep. A backwards clock correction
        // must not postpone a one-minute refresh for hours.
        if self.last_scan_at.is_some_and(|previous| now < previous) {
            for slot in self.accounts.values_mut() {
                slot.due_at = slot.due_at.min(now.saturating_add(REFRESH_INTERVAL_MS));
            }
        }
        self.last_scan_at = Some(now);
        let enabled: HashSet<_> = enabled_ids
            .into_iter()
            .filter(|id| !id.is_empty() && id.trim() == id)
            .collect();
        for (id, slot) in &mut self.accounts {
            if !enabled.contains(id) {
                slot.enabled = false;
            }
        }
        for id in enabled {
            let slot = self.accounts.entry(id).or_default();
            if !slot.enabled {
                slot.enabled = true;
                slot.due_at = now;
            }
        }
        // Keep disabled in-flight entries until completion so re-enabling an
        // account cannot create a second refresh for it.
        self.accounts.retain(|_, slot| slot.enabled || slot.running);
    }

    fn due_accounts(&self, now: i64) -> Vec<String> {
        let mut due: Vec<_> = self
            .accounts
            .iter()
            .filter(|(_, slot)| slot.enabled && !slot.running && slot.due_at <= now)
            .map(|(id, slot)| (slot.due_at, id.clone()))
            .collect();
        due.sort();
        due.into_iter().map(|(_, id)| id).collect()
    }

    fn started(&mut self, id: &str) {
        if let Some(slot) = self.accounts.get_mut(id) {
            slot.running = true;
        }
    }

    fn finished(&mut self, id: &str, now: i64) {
        if let Some(slot) = self.accounts.get_mut(id) {
            slot.running = false;
            // Both success and failure wait one full interval. Long suspension
            // causes one overdue check, never a replay of every missed interval.
            slot.due_at = now.saturating_add(REFRESH_INTERVAL_MS);
        }
        self.accounts.retain(|_, slot| slot.enabled || slot.running);
    }
}

type Completion = (String, Result<(), String>);

struct Driver<O: Operations> {
    operations: Arc<O>,
    schedule: MonitorSchedule,
    pending: FuturesUnordered<BoxFuture<'static, Completion>>,
}

impl<O: Operations> Driver<O> {
    fn new(operations: O) -> Self {
        Self {
            operations: Arc::new(operations),
            schedule: MonitorSchedule::default(),
            pending: FuturesUnordered::new(),
        }
    }

    fn scan(&mut self, now: i64) {
        self.schedule
            .synchronize(now, self.operations.enabled_account_ids());
        for account_id in self.schedule.due_accounts(now) {
            if self.pending.len() >= MAX_IN_FLIGHT {
                break;
            }
            if !self.operations.is_enabled(&account_id) {
                continue;
            }
            self.schedule.started(&account_id);
            let operations = Arc::clone(&self.operations);
            self.pending.push(Box::pin(async move {
                // Include future construction in the unwind boundary as well
                // as polling it. A bad refresh cannot stop the monitor forever.
                let result = AssertUnwindSafe(async {
                    if operations.is_enabled(&account_id) {
                        operations.refresh(account_id.clone()).await
                    } else {
                        Ok(())
                    }
                })
                .catch_unwind()
                .await
                .unwrap_or_else(|_| Err("Codex 自动 Reset 额度刷新任务异常退出".to_string()));
                (account_id, result)
            }));
        }
    }

    fn finished(&mut self, completion: Completion, now: i64) {
        let (account_id, result) = completion;
        self.schedule.finished(&account_id, now);
        // Diagnostics must not take down the scheduler either.
        let _ = std::panic::catch_unwind(AssertUnwindSafe(|| {
            self.operations.refresh_finished(&account_id, &result);
        }));
    }
}

fn wall_clock_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(i64::MAX as u128) as i64)
        .unwrap_or(0)
}

/// Start once from the backend lifecycle. `notify` wakes scans after settings
/// changes; wakeups coalesce and do not reset an existing account's deadline.
pub async fn run(operations: impl Operations, notify: Arc<Notify>) -> ! {
    let mut driver = Driver::new(operations);
    loop {
        driver.scan(wall_clock_ms());
        tokio::select! {
            _ = tokio::time::sleep(SCAN_INTERVAL) => {}
            _ = notify.notified() => {}
            Some(completion) = driver.pending.next(), if !driver.pending.is_empty() => {
                driver.finished(completion, wall_clock_ms());
            }
        }
    }
}

#[cfg(test)]
#[path = "codex_auto_reset_monitor_tests.rs"]
mod tests;
