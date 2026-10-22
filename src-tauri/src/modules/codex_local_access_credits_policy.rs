//! Local admission rules; this is not an upstream billing/spending lock.
//! Keep independent observations per regular-quota window. Display rounding,
//! scheduler exclusions and errors are never evidence of quota exhaustion.
use crate::models::codex_local_access::CodexLocalAccessCreditsPolicy as Policy;

pub const MAX_AGE_MS: i64 = 300_000;

#[derive(Clone, Debug, PartialEq)]
pub struct Window {
    pub present: bool,
    pub remaining: Option<f64>,
    pub reset_at_ms: Option<i64>,
    pub observed_at_ms: i64,
    pub source: &'static str,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Snapshot {
    pub primary: Option<Window>,
    pub secondary: Option<Window>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Available,
    Exhausted,
    Stale,
    Unknown,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Available => "available",
            Self::Exhausted => "exhausted",
            Self::Stale => "stale",
            Self::Unknown => "unknown",
        }
    }
}

fn valid_remaining(value: Option<f64>) -> Option<f64> {
    value.filter(|v| v.is_finite() && (0.0..=100.0).contains(v))
}

fn merge_window(old: &mut Option<Window>, next: Option<Window>) {
    let Some(next) = next else {
        return;
    };
    if let Some(previous) = old {
        if previous.observed_at_ms > next.observed_at_ms
            || (previous.observed_at_ms == next.observed_at_ms
                && previous.present
                && valid_remaining(previous.remaining) == Some(0.0))
        {
            return;
        }
    }
    *old = Some(next);
}

impl Snapshot {
    pub fn merge(&mut self, next: Self) {
        merge_window(&mut self.primary, next.primary);
        merge_window(&mut self.secondary, next.secondary);
    }

    pub fn status(&self, now: i64) -> Status {
        let (mut present, mut unknown, mut stale) = (false, false, false);
        for window in [&self.primary, &self.secondary] {
            let Some(window) = window else {
                unknown = true;
                continue;
            };
            if window.observed_at_ms <= 0 || window.observed_at_ms > now.saturating_add(5_000) {
                unknown = true;
                continue;
            }
            if now.saturating_sub(window.observed_at_ms) > MAX_AGE_MS {
                stale = true;
                continue;
            }
            if !window.present {
                continue;
            }
            present = true;
            if window
                .reset_at_ms
                .is_some_and(|reset| reset > 0 && reset <= now)
            {
                stale = true;
                continue;
            }
            match valid_remaining(window.remaining) {
                Some(0.0) => return Status::Exhausted,
                Some(_) => {}
                None => unknown = true,
            }
        }
        if stale {
            Status::Stale
        } else if unknown || !present {
            Status::Unknown
        } else {
            Status::Available
        }
    }

    pub fn detail(&self, now: i64) -> String {
        [("primary", &self.primary), ("secondary", &self.secondary)]
            .into_iter()
            .map(|(name, window)| match window {
                None => format!("{name}=unknown"),
                Some(w) => format!(
                    "{name}:present={} remaining={} source={} age_ms={}",
                    w.present,
                    w.remaining
                        .map_or_else(|| "unknown".into(), |v| v.to_string()),
                    w.source,
                    now.saturating_sub(w.observed_at_ms)
                ),
            })
            .collect::<Vec<_>>()
            .join("; ")
    }
}

pub fn restricted(policy: Policy, allowed: bool) -> bool {
    policy != Policy::Default || !allowed
}

/// Credits needs the raw quota signals that the general UI/routing snapshot
/// intentionally omits: explicit exhaustion and absent-window placeholders.
pub fn from_headers(
    previous: &Snapshot,
    now: i64,
    mut get: impl FnMut(&str) -> Option<String>,
) -> Snapshot {
    if get("x-codex-limit-reached").is_some_and(|value| value.eq_ignore_ascii_case("true")) {
        return Snapshot {
            primary: Some(Window {
                present: true,
                remaining: Some(0.0),
                reset_at_ms: None,
                observed_at_ms: now,
                source: "response",
            }),
            secondary: None,
        };
    }
    let mut parse = |name: &str, prior: &Option<Window>| {
        let prefix = format!("x-codex-{name}-");
        let used = get(&format!("{prefix}used-percent"))?
            .parse::<f64>()
            .ok()
            .filter(|v| v.is_finite() && (0.0..=100.0).contains(v))?;
        let minutes = get(&format!("{prefix}window-minutes"))
            .and_then(|v| v.parse::<i64>().ok())
            .unwrap_or(0);
        if used == 0.0 && minutes <= 0 && prior.as_ref().is_some_and(|p| !p.present) {
            return None;
        }
        let reset = get(&format!("{prefix}reset-at"))
            .and_then(|v| v.parse::<i64>().ok())
            .filter(|v| *v > 0)
            .map(|v| v.saturating_mul(1000))
            .or_else(|| {
                get(&format!("{prefix}reset-after-seconds"))
                    .and_then(|v| v.parse::<i64>().ok())
                    .filter(|v| *v > 0 && *v <= 366 * 24 * 60 * 60)
                    .map(|v| now.saturating_add(v.saturating_mul(1000)))
            });
        Some(Window {
            present: true,
            remaining: Some(100.0 - used),
            reset_at_ms: reset.or_else(|| prior.as_ref().and_then(|w| w.reset_at_ms)),
            observed_at_ms: now,
            source: "response",
        })
    };
    Snapshot {
        primary: parse("primary", &previous.primary),
        secondary: parse("secondary", &previous.secondary),
    }
}

pub fn from_event(previous: &Snapshot, event: &serde_json::Value, now: i64) -> Snapshot {
    use serde_json::Value;
    if event["type"] != "codex.rate_limits" {
        return Snapshot::default();
    }
    if event
        .get("metered_limit_name")
        .filter(|v| !v.is_null())
        .or_else(|| event.get("limit_name").filter(|v| !v.is_null()))
        .is_some_and(|v| {
            !v.as_str()
                .is_some_and(|v| v.trim().eq_ignore_ascii_case("codex"))
        })
    {
        return Snapshot::default();
    }
    from_headers(previous, now, |key| {
        if key == "x-codex-limit-reached" {
            return event
                .get("rate_limits")
                .and_then(|rates| rates.get("limit_reached"))
                .or_else(|| event.get("limit_reached"))
                .and_then(Value::as_bool)
                .map(|v| v.to_string());
        }
        let rates = event.get("rate_limits")?;
        let rest = key.strip_prefix("x-codex-")?;
        let (name, field) = rest.split_once('-')?;
        rates
            .get(name)?
            .get(field.replace('-', "_"))
            .filter(|v| v.is_number() || v.is_string())
            .map(|v| {
                v.as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| v.to_string())
            })
    })
}

/// `all_exhausted` is computed over the complete configured subscription pool,
/// including paused, excluded, cooling and temporarily unreadable members.
pub fn block_code(
    policy: Policy,
    allowed: bool,
    status: Status,
    all_exhausted: bool,
    official: bool,
) -> Option<&'static str> {
    if !restricted(policy, allowed) {
        return None;
    }
    // Official grants have a different quota authority. Until they expose an
    // authoritative regular/credits split, local OAuth quota cannot admit them.
    if official {
        return Some("credits_quota_unconfirmed");
    }
    if allowed && policy == Policy::PreferRegular && all_exhausted {
        return None;
    }
    match status {
        Status::Available => None,
        Status::Exhausted => Some("credits_policy_blocked"),
        Status::Stale | Status::Unknown => Some("credits_quota_unconfirmed"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn sample(remaining: f64, at: i64) -> Snapshot {
        Snapshot {
            primary: Some(Window {
                present: true,
                remaining: Some(remaining),
                reset_at_ms: None,
                observed_at_ms: at,
                source: "test",
            }),
            secondary: Some(Window {
                present: false,
                remaining: None,
                reset_at_ms: None,
                observed_at_ms: at,
                source: "test",
            }),
        }
    }
    #[test]
    fn policy_matrix_and_account_denial() {
        for policy in [
            Policy::Default,
            Policy::PreferRegular,
            Policy::StrictRegular,
        ] {
            for allowed in [false, true] {
                for all in [false, true] {
                    assert_eq!(
                        block_code(policy, allowed, Status::Available, all, false),
                        None
                    );
                    let permitted = allowed
                        && (policy == Policy::Default || (policy == Policy::PreferRegular && all));
                    assert_eq!(
                        block_code(policy, allowed, Status::Exhausted, all, false).is_none(),
                        permitted
                    );
                    assert_eq!(
                        block_code(policy, allowed, Status::Unknown, all, false).is_none(),
                        permitted
                    );
                }
            }
        }
    }
    #[test]
    fn partial_observation_preserves_other_window_and_age() {
        let mut original = sample(2.0, 1_000);
        original.secondary = sample(6.0, 1_000).primary;
        original.merge(from_headers(&original, 200_000, |name| {
            (name == "x-codex-primary-used-percent").then(|| "97".into())
        }));
        assert_eq!(original.secondary.as_ref().unwrap().observed_at_ms, 1_000);
        assert_eq!(original.status(302_000), Status::Stale);
    }
    #[test]
    fn fractional_weekly_only_absent_windows_and_reset() {
        let mut quota = Snapshot {
            primary: Some(Window {
                present: false,
                remaining: None,
                reset_at_ms: None,
                observed_at_ms: 10_000,
                source: "usage",
            }),
            secondary: sample(0.01, 10_000).primary,
        };
        assert_eq!(quota.status(10_001), Status::Available);
        quota.secondary.as_mut().unwrap().reset_at_ms = Some(11_000);
        assert_eq!(quota.status(11_001), Status::Stale);
        assert_eq!(Snapshot::default().status(10_000), Status::Unknown);
    }
    #[test]
    fn exhaustion_wins_equal_time_and_newer_positive_recovers() {
        let mut quota = sample(0.0, 10_000);
        quota.merge(sample(60.0, 9_999));
        quota.merge(sample(60.0, 10_000));
        assert_eq!(quota.status(10_001), Status::Exhausted);
        quota.merge(sample(0.1, 10_002));
        assert_eq!(quota.status(10_003), Status::Available);
        quota.merge(sample(0.0, 10_002));
        assert_eq!(quota.status(10_003), Status::Exhausted);
    }
    #[test]
    fn unrelated_observation_never_resurrects_exhaustion() {
        let mut quota = sample(0.0, 10_000);
        quota.merge(Snapshot {
            primary: None,
            secondary: sample(50.0, 20_000).primary,
        });
        assert_eq!(quota.status(20_001), Status::Exhausted);
        assert_eq!(quota.primary.as_ref().unwrap().observed_at_ms, 10_000);
    }
    #[test]
    fn official_never_borrows_oauth_evidence_or_pool_exhaustion() {
        for policy in [Policy::PreferRegular, Policy::StrictRegular] {
            for state in [Status::Available, Status::Exhausted, Status::Unknown] {
                assert_eq!(
                    block_code(policy, true, state, true, true),
                    Some("credits_quota_unconfirmed")
                );
            }
        }
        assert_eq!(
            block_code(Policy::Default, true, Status::Unknown, false, true),
            None
        );
        assert!(block_code(Policy::Default, false, Status::Available, true, true).is_some());
    }

    #[test]
    fn raw_headers_limit_reached_is_authoritative_without_percent() {
        let mut quota = sample(50.0, 1_000);
        quota.merge(from_headers(&quota, 2_000, |name| {
            (name == "x-codex-limit-reached").then(|| "true".into())
        }));
        assert_eq!(quota.status(2_001), Status::Exhausted);
        quota.merge(from_headers(&quota, 3_000, |name| {
            (name == "retry-after").then(|| "1".into())
        }));
        assert_eq!(quota.status(3_001), Status::Exhausted);
    }

    #[test]
    fn raw_headers_preserve_pro_absent_placeholder_and_future_reset_but_accept_real_exhaustion() {
        for remaining in [2.0, 6.0] {
            let mut quota = sample(remaining, 1_000);
            quota.primary.as_mut().unwrap().reset_at_ms = Some(100_000);
            quota.secondary = Some(Window {
                present: false,
                remaining: None,
                reset_at_ms: None,
                observed_at_ms: 1_000,
                source: "host",
            });
            let headers = std::collections::HashMap::from([
                (
                    "x-codex-primary-used-percent",
                    (100.0 - remaining).to_string(),
                ),
                ("x-codex-primary-reset-after-seconds", "0".into()),
                ("x-codex-secondary-used-percent", "0".into()),
                ("x-codex-secondary-window-minutes", "0".into()),
                ("x-codex-secondary-reset-after-seconds", "0".into()),
            ]);
            quota.merge(from_headers(&quota, 2_000, |name| {
                headers.get(name).cloned()
            }));
            assert_eq!(quota.status(2_001), Status::Available);
            assert!(!quota.secondary.as_ref().unwrap().present);
            assert_eq!(quota.secondary.as_ref().unwrap().observed_at_ms, 1_000);
            assert_eq!(quota.primary.as_ref().unwrap().reset_at_ms, Some(100_000));
            quota.merge(from_headers(&quota, 3_000, |name| {
                (name == "x-codex-secondary-used-percent").then(|| "100".into())
            }));
            assert_eq!(quota.status(3_001), Status::Exhausted);
        }
    }

    #[test]
    fn raw_stream_events_preserve_partial_evidence_and_ignore_model_specific_limits() {
        let mut quota = sample(0.25, 1_000);
        quota.secondary = Some(Window {
            present: false,
            remaining: None,
            reset_at_ms: None,
            observed_at_ms: 1_000,
            source: "host",
        });
        quota.merge(from_event(
            &quota,
            &serde_json::json!({"type":"codex.rate_limits", "rate_limits": {
            "primary":{"used_percent":99.9,"reset_after_seconds":60},
            "secondary":{"used_percent":0,"window_minutes":0,"reset_after_seconds":0}}}),
            2_000,
        ));
        assert_eq!(quota.status(2_001), Status::Available);
        assert!(!quota.secondary.as_ref().unwrap().present);
        quota.merge(from_event(&quota, &serde_json::json!({"type":"codex.rate_limits","metered_limit_name":"gpt-reserve", "rate_limits":{"limit_reached":true}}), 3_000));
        assert_eq!(quota.status(3_001), Status::Available);
        quota.merge(from_event(
            &quota,
            &serde_json::json!({"type":"codex.rate_limits", "rate_limits":{"limit_reached":true}}),
            4_000,
        ));
        assert_eq!(quota.status(4_001), Status::Exhausted);
    }

    #[test]
    fn first_partial_positive_observation_remains_unknown_until_other_window_is_confirmed() {
        let mut quota = Snapshot::default();
        quota.merge(from_headers(&quota, 1_000, |name| {
            (name == "x-codex-primary-used-percent").then(|| "20".into())
        }));
        assert_eq!(quota.status(1_001), Status::Unknown);
        quota.merge(from_headers(&quota, 2_000, |name| {
            (name == "x-codex-limit-reached").then(|| "true".into())
        }));
        assert_eq!(quota.status(2_001), Status::Exhausted);
    }

    #[test]
    fn complete_refresh_without_reset_does_not_inherit_expired_partial_reset() {
        let mut quota = sample(0.0, 1_000);
        quota.primary.as_mut().unwrap().reset_at_ms = Some(2_000);
        quota.merge(sample(20.0, 3_000));
        assert_eq!(quota.status(3_001), Status::Available);
        assert_eq!(quota.primary.as_ref().unwrap().reset_at_ms, None);
    }
}
