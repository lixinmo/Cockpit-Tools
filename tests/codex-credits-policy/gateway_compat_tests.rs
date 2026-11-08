use super::{credits_compat as compat, model::*};
use serde_json::{json, Value};

fn fixture() -> Value {
    json!({"enabled": false, "port": 58393, "apiKey": "test-only",
        "accountIds": ["a"], "createdAt": 1, "updatedAt": 1,
        "futureField": {"opaque": [1, 2, 3]}})
}

fn parsed(raw: &Value) -> CodexLocalAccessCollection {
    serde_json::from_value(raw.clone()).unwrap()
}

#[test]
fn every_retired_mode_migrates_to_native_even_after_older_migration_markers() {
    for mode in [None, Some("legacy"), Some("sidecar"), Some("native")] {
        for marked in [false, true] {
            let mut raw = fixture();
            if let Some(mode) = mode { raw["gatewayMode"] = json!(mode); }
            raw["nativeGatewayDefaultMigrated"] = json!(marked);
            raw["nativeCreditsPolicyMigrated"] = json!(marked);
            let expected = !marked || matches!(mode, Some("legacy" | "sidecar"));
            assert_eq!(compat::needs_migration(&parsed(&raw)), expected);
            let patched: Value = serde_json::from_str(
                &compat::patch_native_mode(&raw.to_string(), 9).unwrap()).unwrap();
            assert_eq!(patched["gatewayMode"], "native");
            assert!(!compat::needs_migration(&parsed(&patched)));
        }
    }
    assert_eq!(CodexLocalAccessGatewayMode::default(), CodexLocalAccessGatewayMode::Native);
}

#[test]
fn native_migration_preserves_every_credit_policy_and_per_account_permission() {
    for policy in ["default", "prefer_regular", "strict_regular"] {
        for mode in ["legacy", "sidecar", "native"] {
            let mut raw = fixture();
            raw["gatewayMode"] = json!(mode);
            raw["creditsPolicy"] = json!(policy);
            raw["accountCreditsAllowed"] = json!({"a":false,"b":true});
            raw["apiKeys"] = json!([{"id":"budget-key","label":"fixture","key":"test-only",
                "budget":{"period":"daily","limitMicrousd":1000000,"maxConcurrent":2}}]);
            let patched: Value = serde_json::from_str(
                &compat::patch_native_mode(&raw.to_string(), 9).unwrap()).unwrap();
            for key in ["apiKey", "accountIds", "enabled", "futureField", "creditsPolicy", "accountCreditsAllowed", "apiKeys"] {
                assert_eq!(patched[key], raw[key], "lost {key} during migration");
            }
            assert_eq!(patched["nativeGatewayDefaultMigrated"], true);
            assert_eq!(patched["nativeCreditsPolicyMigrated"], true);
            assert!(!compat::needs_migration(&parsed(&patched)));
        }
    }
}

#[test]
fn native_credit_marker_migrates_without_changing_an_existing_native_policy() {
    let mut raw = fixture();
    raw["gatewayMode"] = json!("native");
    raw["nativeGatewayDefaultMigrated"] = json!(true);
    raw["creditsPolicy"] = json!("strict_regular");
    assert!(compat::needs_migration(&parsed(&raw)));
    let patched: Value = serde_json::from_str(
        &compat::patch_native_mode(&raw.to_string(), 9).unwrap()).unwrap();
    assert_eq!(patched["creditsPolicy"], "strict_regular");
    assert_eq!(patched["updatedAt"], 9);
    assert!(!compat::needs_migration(&parsed(&patched)));
}

#[test]
fn subscription_credit_restrictions_remain_distinct_from_upstream_caller_budgets() {
    let mut raw = fixture();
    raw["apiKeys"] = json!([{"id":"budget-key","label":"fixture","key":"test-only",
        "budget":{"period":"daily","limitMicrousd":1,"dailyImageLimit":1}}]);
    assert!(!compat::has_restrictions(&parsed(&raw)));
    for policy in ["prefer_regular", "strict_regular"] {
        raw["creditsPolicy"] = json!(policy);
        assert!(compat::has_restrictions(&parsed(&raw)));
    }
    raw["creditsPolicy"] = json!("default");
    raw["accountCreditsAllowed"] = json!({"a":false});
    assert!(compat::has_restrictions(&parsed(&raw)));
}

#[test]
fn field_only_migration_preserves_partial_schema_and_rejects_nonobjects() {
    let original = json!({"gatewayMode":"sidecar","port":18455,"enabled":false,
        "apiKeys":[{"id":"partial","tokenUsed":456}],"futureField":{"preserve":[1,2,3]}});
    let patched: Value = serde_json::from_str(
        &compat::patch_native_mode(&original.to_string(), 9).unwrap()).unwrap();
    for key in ["port", "enabled", "apiKeys", "futureField"] {
        assert_eq!(patched[key], original[key]);
    }
    assert_eq!(patched["gatewayMode"], "native");
    for invalid in ["", "{", "null", "[]", "42", "\"string\""] {
        assert!(compat::patch_native_mode(invalid, 9).is_err());
    }
}
