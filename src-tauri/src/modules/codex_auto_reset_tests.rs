use super::*;

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

#[test]
fn weekly_only_primary_matches_actual_exhausted_account_layout() {
    let exhausted = primary_weekly_quota(0);
    assert_eq!(exhausted.hourly_window_minutes, Some(10080));
    assert_eq!(exhausted.weekly_window_present, Some(false));
    assert_eq!(exhausted.weekly_percentage, 100);
    assert_eq!(weekly_remaining(&exhausted), Some(0));
    assert_eq!(weekly_remaining(&primary_weekly_quota(91)), Some(91));
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

#[test]
fn migrated_barrier_survives_restart_until_fresh_weekly_recovery() {
    let dir = TempDir::new();
    let guard = dir.guard();
    guard.record(&Attempt {
        credit_id: Some("old-card".into()), redeem_request_id: "old-request".into(),
        threshold: 8, awaiting_refresh: true,
    }).unwrap();
    drop(guard);
    let guard = dir.guard();
    assert_eq!(guard.pending().unwrap().unwrap().redeem_request_id, "old-request");
    // Lowering the configured threshold never clears a previously written latch.
    assert!(guard.observe_recovery(&quota(5, 100), 2).unwrap());
    let pending = guard.pending().unwrap().unwrap();
    assert!(!pending.awaiting_refresh);
    assert_eq!(pending.credit_id.as_deref(), Some("old-card"));
    assert!(guard.observe_recovery(&quota(9, 100), 2).unwrap());
    assert!(guard.pending().unwrap().is_none());
}

#[test]
fn migration_never_discards_corrupt_or_unwritable_legacy_barrier() {
    let dir = TempDir::new();
    let guard = dir.guard();
    fs::write(&guard.state_path, "{broken").unwrap();
    assert!(guard.pending().is_err());
    assert!(guard.observe_recovery(&quota(100, 100), 2).is_err());
    assert_eq!(fs::read_to_string(&guard.state_path).unwrap(), "{broken");
}

#[test]
fn old_and_new_reset_paths_share_the_same_account_file_lock() {
    let dir = TempDir::new();
    let legacy = dir.guard_for("same-account");
    assert!(ResetGuard::acquire(&dir.0, "same-account").unwrap().is_none());
    assert!(ResetGuard::acquire(&dir.0, "different-account").unwrap().is_some());
    drop(legacy);
    assert!(ResetGuard::acquire(&dir.0, "same-account").unwrap().is_some());
}
