//! Preserve local Credits settings when retired engine configurations move to Native.
//! No engine-selection or Go budget policy lives here: upstream dispatch is Native-only.
use crate::models::codex_local_access::{
    CodexLocalAccessCollection, CodexLocalAccessCreditsPolicy, CodexLocalAccessGatewayMode,
};
use serde_json::Value;

pub(crate) fn has_restrictions(collection: &CodexLocalAccessCollection) -> bool {
    collection.credits_policy != CodexLocalAccessCreditsPolicy::Default
        || collection.account_credits_allowed.values().any(|allowed| !allowed)
}

pub(crate) fn needs_migration(collection: &CodexLocalAccessCollection) -> bool {
    !collection.native_gateway_default_migrated
        || !collection.native_credits_policy_migrated
        || collection.gateway_mode != CodexLocalAccessGatewayMode::Native
}

pub(crate) fn patch_native_mode(current: &str, updated_at: i64) -> Result<String, String> {
    let mut value: Value = serde_json::from_str(current).map_err(|error| error.to_string())?;
    if !value.is_object() {
        return Err("Invalid API service configuration".into());
    }
    // Field-only migration preserves Credits policy, per-account permissions,
    // caller budgets and unrelated old/future fields without reinterpreting them.
    value["gatewayMode"] = Value::String("native".into());
    value["nativeGatewayDefaultMigrated"] = Value::Bool(true);
    value["nativeCreditsPolicyMigrated"] = Value::Bool(true);
    value["updatedAt"] = Value::from(updated_at);
    serde_json::to_string_pretty(&value).map_err(|error| error.to_string())
}
