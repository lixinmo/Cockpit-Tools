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
fn stock_old_modes_and_fresh_defaults_retain_native_behavior() {
    for mode in [None, Some("legacy"), Some("sidecar"), Some("native")] {
        let mut raw = fixture();
        if let Some(mode) = mode {
            raw["gatewayMode"] = json!(mode);
        }
        assert_eq!(
            compat::migration_target(&raw, &parsed(&raw)),
            Some(CodexLocalAccessGatewayMode::Native)
        );
    }
    assert_eq!(
        CodexLocalAccessGatewayMode::default(),
        CodexLocalAccessGatewayMode::Native
    );
    let mut raw = fixture();
    raw["nativeGatewayDefaultMigrated"] = json!(true);
    raw["nativeCreditsPolicyMigrated"] = json!(true);
    raw["creditsPolicy"] = json!("default");
    raw["accountCreditsAllowed"] = json!({});
    assert_eq!(compat::migration_target(&raw, &parsed(&raw)), None);
}

#[test]
fn existing_local_configs_migrate_to_native_without_losing_policy() {
    for mode in [None, Some("legacy"), Some("sidecar"), Some("native")] {
        for (field, value) in [
            ("creditsPolicy", json!("default")),
            ("accountCreditsAllowed", json!({"a": true})),
        ] {
            let mut raw = fixture();
            if let Some(mode) = mode {
                raw["gatewayMode"] = json!(mode);
            }
            raw[field] = value;
            let target = compat::migration_target(&raw, &parsed(&raw)).unwrap();
            assert_eq!(target, CodexLocalAccessGatewayMode::Native);
            let changed: Value = serde_json::from_str(
                &compat::patch_gateway_mode(&raw.to_string(), target, 9).unwrap(),
            )
            .unwrap();
            for key in ["apiKey", "accountIds", "enabled", "futureField", field] {
                assert_eq!(changed[key], raw[key]);
            }
            assert_eq!(changed["gatewayMode"], "native");
            assert_eq!(changed["nativeGatewayDefaultMigrated"], true);
            assert_eq!(compat::migration_target(&changed, &parsed(&changed)), None);
        }
    }
}

#[test]
fn restricted_go_configs_recover_to_native_even_after_prior_migration() {
    for policy in ["prefer_regular", "strict_regular", "default"] {
        let mut raw = fixture();
        raw["gatewayMode"] = json!("sidecar");
        raw["nativeGatewayDefaultMigrated"] = json!(true);
        raw["creditsPolicy"] = json!(policy);
        if policy == "default" {
            raw["accountCreditsAllowed"] = json!({"a": false});
        }
        assert_eq!(
            compat::migration_target(&raw, &parsed(&raw)),
            Some(CodexLocalAccessGatewayMode::Native)
        );
        assert!(
            compat::validate_mode(&parsed(&raw), CodexLocalAccessGatewayMode::Sidecar)
                .unwrap_err()
                .contains("CREDITS_REQUIRES_NATIVE")
        );
        let changed: Value = serde_json::from_str(
            &compat::patch_gateway_mode(&raw.to_string(), CodexLocalAccessGatewayMode::Native, 9)
                .unwrap(),
        )
        .unwrap();
        assert_eq!(changed["creditsPolicy"], raw["creditsPolicy"]);
        assert_eq!(
            changed["accountCreditsAllowed"],
            raw["accountCreditsAllowed"]
        );
    }
}

#[test]
fn explicit_go_choice_cannot_discard_a_policy_or_account_denial() {
    for (field, value) in [
        ("creditsPolicy", json!("prefer_regular")),
        ("creditsPolicy", json!("strict_regular")),
        ("accountCreditsAllowed", json!({"a": false})),
    ] {
        let mut raw = fixture();
        raw[field] = value;
        let original = raw.to_string();
        let error = compat::patch_gateway_mode(&original, CodexLocalAccessGatewayMode::Sidecar, 9)
            .unwrap_err();
        assert!(error.contains("CREDITS_REQUIRES_NATIVE"));
        assert_eq!(original, raw.to_string());
    }
}

#[test]
fn unrestricted_explicit_go_choice_survives_later_restart() {
    let mut raw = fixture();
    raw["creditsPolicy"] = json!("default");
    raw["accountCreditsAllowed"] = json!({"a": true});
    let changed: Value = serde_json::from_str(
        &compat::patch_gateway_mode(&raw.to_string(), CodexLocalAccessGatewayMode::Sidecar, 9)
            .unwrap(),
    )
    .unwrap();
    assert_eq!(compat::migration_target(&changed, &parsed(&changed)), None);
    assert_eq!(
        changed["accountCreditsAllowed"],
        raw["accountCreditsAllowed"]
    );
}

#[test]
fn candidate_restriction_requires_native_without_changing_saved_settings() {
    let saved = parsed(&fixture());
    assert_eq!(saved.gateway_mode, CodexLocalAccessGatewayMode::Native);
    let mut candidate = saved.clone();
    candidate.credits_policy = CodexLocalAccessCreditsPolicy::StrictRegular;
    assert!(compat::validate_mode(&candidate, CodexLocalAccessGatewayMode::Sidecar).is_err());
    assert!(compat::validate_mode(&saved, CodexLocalAccessGatewayMode::Sidecar).is_ok());
    assert_eq!(saved.credits_policy, CodexLocalAccessCreditsPolicy::Default);
}

#[test]
fn minimal_engine_patch_preserves_unrelated_partial_schema_and_unknown_fields() {
    // The upstream native_mode_patch fixture deliberately is not a complete
    // collection: its API key is partial and createdAt/apiKey are missing.
    let original = json!({
        "gatewayMode":"sidecar", "updatedAt":1, "port":18455,
        "enabled":false, "requestPayloadLogging":true,
        "apiKeys":[{"id":"key-latest", "tokenUsed":456}],
        "accountIds":["latest-account"], "futureField":{"preserve":[1,2,3]}
    });
    for mode in [
        CodexLocalAccessGatewayMode::Native,
        CodexLocalAccessGatewayMode::Sidecar,
    ] {
        let patched: Value = serde_json::from_str(
            &compat::patch_gateway_mode(&original.to_string(), mode, 9).unwrap(),
        )
        .unwrap();
        assert_eq!(patched["updatedAt"], 9);
        for key in [
            "port",
            "enabled",
            "requestPayloadLogging",
            "apiKeys",
            "accountIds",
            "futureField",
        ] {
            assert_eq!(patched[key], original[key], "{key}");
        }
    }
    for invalid in ["", "{", "null", "[]", "42", "\"string\""] {
        assert!(
            compat::patch_gateway_mode(invalid, CodexLocalAccessGatewayMode::Native, 9).is_err()
        );
    }
}

#[test]
fn minimal_go_patch_rejects_invalid_or_restricted_credit_fields() {
    for fields in [
        json!({"creditsPolicy":"future_restriction"}),
        json!({"creditsPolicy":null}),
        json!({"accountCreditsAllowed":{"a":"false"}}),
        json!({"accountCreditsAllowed":null}),
        json!({"creditsPolicy":"strict_regular"}),
        json!({"accountCreditsAllowed":{"a":false}}),
    ] {
        assert!(compat::patch_gateway_mode(
            &fields.to_string(),
            CodexLocalAccessGatewayMode::Sidecar,
            9
        )
        .is_err());
        let patched: Value = serde_json::from_str(
            &compat::patch_gateway_mode(
                &fields.to_string(),
                CodexLocalAccessGatewayMode::Native,
                9,
            )
            .unwrap(),
        )
        .unwrap();
        for (key, value) in fields.as_object().unwrap() {
            assert_eq!(&patched[key], value);
        }
    }
}

#[test]
fn upstream_key_budget_limits_require_native_without_changing_credits_or_engine() {
    for field in ["limitMicrousd", "dailyLimitMicrousd", "maxConcurrent", "dailyImageLimit"] {
        for enabled in [false, true] {
            let mut raw = fixture();
            raw["gatewayMode"] = json!("sidecar");
            raw["nativeGatewayDefaultMigrated"] = json!(true);
            raw["nativeCreditsPolicyMigrated"] = json!(true);
            raw["apiKeys"] = json!([{"id":"budget-key","label":"fixture","key":"test-only",
                "enabled":enabled,"budget":{"period":"daily"}}]);
            raw["apiKeys"][0]["budget"][field] = json!(1);
            let collection = parsed(&raw);
            assert!(!compat::has_restrictions(&collection), "caller budget is not subscription Credits");
            assert_eq!(compat::migration_target(&raw, &collection), None, "budget must not switch engines");
            assert!(compat::validate_mode(&collection, CodexLocalAccessGatewayMode::Sidecar)
                .unwrap_err().contains("KEY_BUDGET_REQUIRES_NATIVE"));
            assert!(compat::validate_mode(&collection, CodexLocalAccessGatewayMode::Native).is_ok());
            let original = raw.to_string();
            assert!(compat::patch_gateway_mode(&original, CodexLocalAccessGatewayMode::Sidecar, 9)
                .unwrap_err().contains("KEY_BUDGET_REQUIRES_NATIVE"));
            assert_eq!(raw.to_string(), original);
            let native: Value = serde_json::from_str(&compat::patch_gateway_mode(
                &original, CodexLocalAccessGatewayMode::Native, 9).unwrap()).unwrap();
            assert_eq!(native["apiKeys"], raw["apiKeys"]);
        }
    }
}

#[test]
fn unlimited_key_budget_and_period_do_not_disable_go() {
    for budget in [json!({}), json!({"period":"monthly","limitMicrousd":null,
        "dailyLimitMicrousd":null,"maxConcurrent":null,"dailyImageLimit":null})] {
        let raw = json!({"apiKeys":[{"id":"partial","budget":budget}],"unrelated":"keep"});
        let changed: Value = serde_json::from_str(&compat::patch_gateway_mode(
            &raw.to_string(), CodexLocalAccessGatewayMode::Sidecar, 9).unwrap()).unwrap();
        assert_eq!(changed["gatewayMode"], "sidecar");
        assert_eq!(changed["apiKeys"], raw["apiKeys"]);
        assert_eq!(changed["unrelated"], raw["unrelated"]);
    }
}
