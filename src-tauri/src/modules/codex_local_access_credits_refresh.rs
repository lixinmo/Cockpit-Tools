//! Shared, bounded read-only refresh. A caller dropping its wait does not cancel
//! another caller's refresh; completion (including failure) starts the backoff.
use super::credits_policy::Snapshot;
use std::{
    collections::HashMap,
    future::Future,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::sync::{watch, Semaphore};

type ResultSnapshot = Result<Snapshot, &'static str>;
#[derive(Default)]
struct Entry {
    snapshot: Snapshot,
    completed: Option<Instant>,
    last_result: Option<ResultSnapshot>,
    flight: Option<watch::Receiver<Option<ResultSnapshot>>>,
}

pub struct Refreshes {
    entries: Mutex<HashMap<String, Entry>>,
    slots: Arc<Semaphore>,
    deadline: Duration,
    backoff: Duration,
}

impl Default for Refreshes {
    fn default() -> Self {
        Self::new(Duration::from_secs(8), Duration::from_secs(30), 4)
    }
}

impl Refreshes {
    pub fn new(deadline: Duration, backoff: Duration, slots: usize) -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
            slots: Arc::new(Semaphore::new(slots)),
            deadline,
            backoff,
        }
    }
    pub fn merge(&self, identity: &str, snapshot: Snapshot) -> Snapshot {
        let Ok(mut entries) = self.entries.lock() else {
            return Snapshot::default();
        };
        if !entries.contains_key(identity) && entries.len() >= 4096 {
            return Snapshot::default();
        }
        let entry = entries.entry(identity.into()).or_default();
        entry.snapshot.merge(snapshot);
        entry.snapshot.clone()
    }
    pub fn snapshot(&self, identity: &str) -> Snapshot {
        self.entries
            .lock()
            .ok()
            .and_then(|entries| entries.get(identity).map(|e| e.snapshot.clone()))
            .unwrap_or_default()
    }
    pub async fn refresh<F, Fut>(self: &Arc<Self>, identity: String, fetch: F) -> ResultSnapshot
    where
        F: FnOnce() -> Fut + Send + 'static,
        Fut: Future<Output = ResultSnapshot> + Send + 'static,
    {
        let mut receiver = {
            let mut entries = self
                .entries
                .lock()
                .map_err(|_| "refresh_state_unavailable")?;
            if !entries.contains_key(&identity) && entries.len() >= 4096 {
                return Err("refresh_capacity_reached");
            }
            let entry = entries.entry(identity.clone()).or_default();
            if let Some(receiver) = &entry.flight {
                receiver.clone()
            } else if entry
                .completed
                .is_some_and(|at| at.elapsed() < self.backoff)
            {
                return entry.last_result.clone().unwrap_or(Err("refresh_backoff"));
            } else {
                let (sender, receiver) = watch::channel(None);
                entry.flight = Some(receiver.clone());
                let controller = self.clone();
                tokio::spawn(async move {
                    let result = match tokio::time::timeout(controller.deadline, async {
                        let _slot = controller
                            .slots
                            .clone()
                            .acquire_owned()
                            .await
                            .map_err(|_| "refresh_unavailable")?;
                        fetch().await
                    })
                    .await
                    {
                        Ok(result) => result,
                        Err(_) => Err("refresh_timeout"),
                    };
                    if let Ok(mut entries) = controller.entries.lock() {
                        if let Some(entry) = entries.get_mut(&identity) {
                            if let Ok(snapshot) = &result {
                                entry.snapshot.merge(snapshot.clone());
                            }
                            entry.completed = Some(Instant::now());
                            entry.last_result = Some(result.clone());
                            entry.flight = None;
                        }
                    }
                    sender.send_replace(Some(result));
                });
                receiver
            }
        };
        loop {
            if let Some(result) = receiver.borrow().clone() {
                return result;
            }
            receiver
                .changed()
                .await
                .map_err(|_| "refresh_unavailable")?;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    #[tokio::test]
    async fn refresh_is_shared_survives_caller_cancellation_and_backs_off() {
        let state = Arc::new(Refreshes::new(
            Duration::from_secs(1),
            Duration::from_secs(30),
            4,
        ));
        let calls = Arc::new(AtomicUsize::new(0));
        let first = {
            let state = state.clone();
            let calls = calls.clone();
            tokio::spawn(async move {
                state
                    .refresh("a".into(), move || async move {
                        calls.fetch_add(1, Ordering::SeqCst);
                        tokio::time::sleep(Duration::from_millis(40)).await;
                        Ok(Snapshot::default())
                    })
                    .await
            })
        };
        tokio::time::sleep(Duration::from_millis(10)).await;
        first.abort();
        assert!(state
            .refresh("a".into(), || async { panic!("must share work") })
            .await
            .is_ok());
        assert!(state
            .refresh("a".into(), || async { panic!("must back off") })
            .await
            .is_ok());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
    #[tokio::test]
    async fn timeout_and_failed_completion_are_bounded_and_back_off() {
        let state = Arc::new(Refreshes::new(
            Duration::from_millis(15),
            Duration::from_secs(30),
            1,
        ));
        assert_eq!(
            state
                .refresh("a".into(), || async {
                    std::future::pending::<ResultSnapshot>().await
                })
                .await,
            Err("refresh_timeout")
        );
        assert_eq!(
            state
                .refresh("a".into(), || async { panic!("timeout must back off") })
                .await,
            Err("refresh_timeout")
        );
        assert_eq!(
            state
                .refresh("b".into(), || async { Err("refresh_failed") })
                .await,
            Err("refresh_failed")
        );
        assert_eq!(
            state
                .refresh("b".into(), || async { panic!("failure must back off") })
                .await,
            Err("refresh_failed")
        );
    }
    #[tokio::test]
    async fn account_refreshes_never_exceed_four_concurrent_reads() {
        let state = Arc::new(Refreshes::new(Duration::from_secs(1), Duration::ZERO, 4));
        let current = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let mut tasks = Vec::new();
        for i in 0..12 {
            let state = state.clone();
            let current = current.clone();
            let peak = peak.clone();
            tasks.push(tokio::spawn(async move {
                state
                    .refresh(i.to_string(), move || async move {
                        let count = current.fetch_add(1, Ordering::SeqCst) + 1;
                        peak.fetch_max(count, Ordering::SeqCst);
                        tokio::time::sleep(Duration::from_millis(10)).await;
                        current.fetch_sub(1, Ordering::SeqCst);
                        Ok(Snapshot::default())
                    })
                    .await
            }));
        }
        for task in tasks {
            task.await.unwrap().unwrap();
        }
        assert_eq!(peak.load(Ordering::SeqCst), 4);
    }

    #[tokio::test]
    async fn failed_refresh_recovers_after_backoff_without_renewing_old_evidence() {
        let state = Arc::new(Refreshes::new(
            Duration::from_secs(1),
            Duration::from_millis(5),
            1,
        ));
        let original = Snapshot {
            primary: Some(super::super::credits_policy::Window {
                present: true,
                remaining: Some(0.0),
                reset_at_ms: None,
                observed_at_ms: 1000,
                source: "response",
            }),
            secondary: None,
        };
        state.merge("a", original.clone());
        assert_eq!(
            state
                .refresh("a".into(), || async { Err("refresh_failed") })
                .await,
            Err("refresh_failed")
        );
        assert_eq!(state.snapshot("a"), original);
        tokio::time::sleep(Duration::from_millis(10)).await;
        let mut positive = original.clone();
        positive.primary.as_mut().unwrap().remaining = Some(2.0);
        positive.primary.as_mut().unwrap().observed_at_ms = 2000;
        let expected = positive.clone();
        state
            .refresh("a".into(), move || async move { Ok(positive) })
            .await
            .unwrap();
        assert_eq!(state.snapshot("a"), expected);
    }
}
