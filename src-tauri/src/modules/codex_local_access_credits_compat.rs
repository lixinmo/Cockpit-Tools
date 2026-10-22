//! One-time migration of the local Go Credits settings to the Native policy.
//! Pure JSON helpers are shared by production and offline configuration tests.
use crate::models::codex_local_access::{
    CodexLocalAccessCollection, CodexLocalAccessCreditsPolicy, CodexLocalAccessGatewayMode,
};
use serde_json::Value;

pub(crate) const NATIVE_CREDITS_ERROR: &str =
    "CREDITS_REQUIRES_NATIVE: Credits 限制仅支持 Rust Native 引擎。请将 API 服务及相关实例切换到 Rust 后重试；已保存策略保持不变。";

pub(crate) fn has_restrictions(collection: &CodexLocalAccessCollection) -> bool {
    collection.credits_policy != CodexLocalAccessCreditsPolicy::Default
        || collection
            .account_credits_allowed
            .values()
            .any(|allowed| !allowed)
}

pub(crate) fn validate_mode(
    collection: &CodexLocalAccessCollection,
    mode: CodexLocalAccessGatewayMode,
) -> Result<(), String> {
    if mode != CodexLocalAccessGatewayMode::Native && has_restrictions(collection) {
        Err(NATIVE_CREDITS_ERROR.into())
    } else {
        Ok(())
    }
}

pub(crate) fn needs_migration(collection: &CodexLocalAccessCollection) -> bool {
    !collection.native_gateway_default_migrated
        || !collection.native_credits_policy_migrated
        || collection.gateway_mode == CodexLocalAccessGatewayMode::Legacy
        || (collection.gateway_mode != CodexLocalAccessGatewayMode::Native
            && has_restrictions(collection))
}

pub(crate) fn migration_target(
    _raw: &Value,
    collection: &CodexLocalAccessCollection,
) -> Option<CodexLocalAccessGatewayMode> {
    needs_migration(collection).then_some(CodexLocalAccessGatewayMode::Native)
}

pub(crate) fn patch_gateway_mode(
    current: &str,
    mode: CodexLocalAccessGatewayMode,
    updated_at: i64,
) -> Result<String, String> {
    let mut value: Value = serde_json::from_str(current).map_err(|error| error.to_string())?;
    if !value.is_object() {
        return Err("Invalid API service configuration".into());
    }
    // This is a field transaction, not a full collection-schema migration.
    // Older/partial snapshots and future unrelated fields must remain valid
    // inputs to the upstream engine switch. Only a non-Native destination
    // needs the Credits fields to establish that no restriction is bypassed.
    if mode != CodexLocalAccessGatewayMode::Native {
        let policy = value
            .get("creditsPolicy")
            .cloned()
            .map(serde_json::from_value::<CodexLocalAccessCreditsPolicy>)
            .transpose()
            .map_err(|error| error.to_string())?
            .unwrap_or_default();
        let overrides = value
            .get("accountCreditsAllowed")
            .cloned()
            .map(serde_json::from_value::<std::collections::HashMap<String, bool>>)
            .transpose()
            .map_err(|error| error.to_string())?
            .unwrap_or_default();
        if policy != CodexLocalAccessCreditsPolicy::Default
            || overrides.values().any(|allowed| !allowed)
        {
            return Err(NATIVE_CREDITS_ERROR.into());
        }
    }
    value["gatewayMode"] = serde_json::to_value(mode).map_err(|error| error.to_string())?;
    value["nativeGatewayDefaultMigrated"] = Value::Bool(true);
    value["nativeCreditsPolicyMigrated"] = Value::Bool(true);
    value["updatedAt"] = Value::from(updated_at);
    serde_json::to_string_pretty(&value).map_err(|error| error.to_string())
}
