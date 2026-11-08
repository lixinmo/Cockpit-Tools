//! Compatibility for legacy per-account settings and durable Reset barriers.
//! No scheduler or consumption path: codex_reset_automation owns both.

use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};

use fs2::FileExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::models::codex::{CodexAccount, CodexQuota, CodexResetCredit};
use crate::modules::atomic_write;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub enabled: bool,
    pub threshold: i32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            enabled: false,
            threshold: 2,
        }
    }
}

/// Official ChatGPT plan projections use API-key auth and are not Codex
/// credentials. Web-session imports are read-only; Agent Identity keeps its
/// existing explicit Reset API support.
pub(crate) fn supports_account(account: &CodexAccount) -> bool {
    !account.is_api_key_auth() && !account.is_web_session_auth()
}

pub(crate) struct RefreshedQuota {
    pub quota: CodexQuota,
    pub fresh: bool,
}

/// Flattened into UserConfig. Old global enabled/threshold keys are deliberately
/// not deserialized: enabling one account must never opt in any other account.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Preferences {
    #[serde(default)]
    codex_auto_consume_reset_accounts: HashMap<String, Settings>,
}

impl Preferences {
    pub(crate) fn entries(&self) -> &HashMap<String, Settings> {
        &self.codex_auto_consume_reset_accounts
    }

    pub(crate) fn enabled_account_ids(&self) -> Vec<String> {
        let mut ids: Vec<_> = self
            .codex_auto_consume_reset_accounts
            .iter()
            .filter(|(_, settings)| settings.enabled)
            .map(|(id, _)| id.clone())
            .collect();
        ids.sort();
        ids
    }

    pub fn for_account(&self, account_id: &str) -> Settings {
        self.codex_auto_consume_reset_accounts
            .get(account_id)
            .copied()
            .unwrap_or_default()
    }

    /// Run inside config::patch_user_config so concurrent saves merge into the
    /// latest persisted config, rather than replacing a client's stale map.
    pub fn patch_account(
        &mut self,
        account_id: &str,
        enabled: Option<bool>,
        threshold: Option<i32>,
    ) -> Result<Settings, String> {
        if account_id.trim().is_empty() || account_id.trim() != account_id {
            return Err("无效的 Codex 账号 ID".to_string());
        }
        if threshold.is_some_and(|value| !(0..=99).contains(&value)) {
            return Err("周额度剩余阈值必须为 0 到 99 的整数".to_string());
        }
        let mut next = self.for_account(account_id);
        if let Some(enabled) = enabled {
            next.enabled = enabled;
        }
        if let Some(threshold) = threshold {
            next.threshold = threshold;
        }
        self.codex_auto_consume_reset_accounts
            .insert(account_id.to_string(), next);
        Ok(next)
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct Attempt {
    pub credit_id: Option<String>,
    pub redeem_request_id: String,
    pub threshold: i32,
    #[serde(default = "awaiting_refresh_by_default")]
    pub awaiting_refresh: bool,
}

fn awaiting_refresh_by_default() -> bool {
    true
}

pub(crate) struct ResetGuard {
    // A separate lock file survives atomic replacement of the state file.
    _lock: File,
    state_path: PathBuf,
}

impl ResetGuard {
    pub fn acquire(data_dir: &Path, account_id: &str) -> Result<Option<Self>, String> {
        let dir = data_dir.join("codex_auto_reset");
        fs::create_dir_all(&dir).map_err(|e| format!("创建 Reset 状态目录失败: {e}"))?;
        let key = format!("{:x}", Sha256::digest(account_id.as_bytes()));
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(dir.join(format!("{key}.lock")))
            .map_err(|e| format!("打开 Reset 锁失败: {e}"))?;
        match lock.try_lock_exclusive() {
            Ok(()) => Ok(Some(Self {
                _lock: lock,
                state_path: dir.join(format!("{key}.json")),
            })),
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => Ok(None),
            Err(e) => Err(format!("获取 Reset 锁失败: {e}")),
        }
    }

    pub fn pending(&self) -> Result<Option<Attempt>, String> {
        match fs::read_to_string(&self.state_path) {
            Ok(content) => serde_json::from_str(&content)
                .map_err(|e| format!("读取 Reset 消费状态失败，已停止自动消费: {e}")),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(format!("读取 Reset 消费状态失败: {e}")),
        }
    }

    pub fn record(&self, attempt: &Attempt) -> Result<(), String> {
        let content = serde_json::to_string(attempt).map_err(|e| e.to_string())?;
        atomic_write::write_string_atomic(&self.state_path, &content)
    }

    /// Only a fresh query made while holding this guard may release the latch.
    /// A lower threshold or toggling the feature cannot bypass a pending reset.
    pub fn observe_recovery(&self, quota: &CodexQuota, threshold: i32) -> Result<bool, String> {
        let Some(mut pending) = self.pending()? else {
            return Ok(false);
        };
        if weekly_remaining(quota)
            .is_some_and(|remaining| remaining > pending.threshold.max(threshold))
        {
            atomic_write::write_string_atomic(&self.state_path, "null")?;
        } else if pending.awaiting_refresh {
            // Explicit manual actions may continue after a confirmed fresh
            // query; automation remains latched for this low-quota episode.
            pending.awaiting_refresh = false;
            self.record(&pending)?;
        }
        // Even the refresh which releases a latch never consumes another reset.
        Ok(true)
    }
}

pub(crate) fn weekly_remaining(quota: &CodexQuota) -> Option<i32> {
    // The legacy field names identify primary/secondary positions, not their
    // duration: weekly-only accounts put their seven-day window in primary.
    // Match the UI's one-minute tolerance, without including longer windows.
    const WEEK_MINUTES: i64 = 7 * 24 * 60;
    let is_weekly = |minutes: Option<i64>| {
        minutes.is_some_and(|minutes| (WEEK_MINUTES - 1..=WEEK_MINUTES).contains(&minutes))
    };
    let candidates = [
        (
            quota.hourly_window_present == Some(true) && is_weekly(quota.hourly_window_minutes),
            quota.hourly_percentage,
        ),
        (
            quota.weekly_window_present == Some(true)
                && (quota.weekly_window_minutes.is_none()
                    || is_weekly(quota.weekly_window_minutes)),
            quota.weekly_percentage,
        ),
    ];
    let mut remaining = None;
    for (is_weekly, percentage) in candidates {
        if !is_weekly {
            continue;
        }
        // Do not choose a seemingly valid window while ignoring conflicting
        // invalid data in another window that is also identified as weekly.
        if !(0..=100).contains(&percentage) {
            return None;
        }
        remaining = Some(remaining.map_or(percentage, |value: i32| value.min(percentage)));
    }
    remaining
}

pub(crate) fn earliest_credit(credits: &[CodexResetCredit], now: i64) -> Option<&CodexResetCredit> {
    credits
        .iter()
        .filter(|credit| {
            credit.id.as_deref().is_some_and(|id| !id.trim().is_empty())
                && credit.redeemed_at.is_none()
                && credit.expires_at.is_none_or(|expires| expires > now)
                && credit.reset_type.as_deref().is_none_or(|reset_type| {
                    // Both the official client type and the existing Cockpit
                    // details-parser fixture describe a banked limit reset.
                    matches!(
                        reset_type.trim().to_ascii_lowercase().as_str(),
                        "codex_rate_limits" | "rate_limit_reset"
                    )
                })
                && credit
                    .status
                    .as_deref()
                    .or(credit.raw_status.as_deref())
                    .is_some_and(|status| status.trim().eq_ignore_ascii_case("available"))
        })
        // Known expiry first; stable ID tie-break also makes mocks deterministic.
        .min_by_key(|credit| (credit.expires_at.unwrap_or(i64::MAX), credit.id.as_deref()))
}

/// This is the request shape used by the official Codex backend client.
pub(crate) fn consume_body(request_id: &str, credit_id: Option<&str>) -> serde_json::Value {
    let mut body = serde_json::json!({ "redeem_request_id": request_id });
    if let Some(credit_id) = credit_id {
        body["credit_id"] = serde_json::json!(credit_id);
    }
    body
}

#[cfg(test)]
#[path = "codex_auto_reset_tests.rs"]
mod tests;
