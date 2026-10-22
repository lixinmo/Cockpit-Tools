#![allow(dead_code)]

#[path = "../../src-tauri/src/models/codex_local_access.rs"]
pub mod model;

pub mod models {
    pub use crate::model as codex_local_access;
}

// Data-only contract required by the production request-diagnostics model.
// This harness deliberately imports no identity storage or account runtime.
pub mod modules {
    pub mod codex_api_identity {
        #[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
        #[serde(rename_all = "camelCase")]
        pub struct InstallationProfile {
            pub version: u8,
            pub scope: String,
            pub installation_id: String,
            pub revision: u64,
        }
    }
}

#[path = "../../src-tauri/src/modules/codex_local_access_credits_compat.rs"]
mod credits_compat;

#[path = "../../src-tauri/src/modules/codex_local_access_credits_policy.rs"]
mod credits_policy;
#[path = "../../src-tauri/src/modules/codex_local_access_credits_refresh.rs"]
mod credits_refresh;

#[cfg(test)]
#[path = "gateway_compat_tests.rs"]
mod gateway_compat_tests;

#[cfg(test)]
mod tests {
    use super::model::{CodexLocalAccessCollection, CodexLocalAccessCreditsPolicy};
    use serde_json::json;

    fn old_collection() -> serde_json::Value {
        json!({"enabled": false, "port": 58393, "apiKey": "test-only",
            "accountIds": ["a"], "createdAt": 1, "updatedAt": 1})
    }

    #[test]
    fn old_configuration_keeps_default_policy_and_allows_credits() {
        let collection: CodexLocalAccessCollection =
            serde_json::from_value(old_collection()).unwrap();
        assert_eq!(
            collection.credits_policy,
            CodexLocalAccessCreditsPolicy::Default
        );
        assert_eq!(
            collection
                .account_credits_allowed
                .get("a")
                .copied()
                .unwrap_or(true),
            true
        );
    }

    #[test]
    fn every_policy_and_account_override_roundtrips_without_losing_other_settings() {
        for policy in ["default", "prefer_regular", "strict_regular"] {
            let mut value = old_collection();
            value["creditsPolicy"] = json!(policy);
            value["accountCreditsAllowed"] = json!({"a": false, "b": true});
            value["sessionAffinity"] = json!(false);
            let collection: CodexLocalAccessCollection = serde_json::from_value(value).unwrap();
            let stored = serde_json::to_value(&collection).unwrap();
            assert_eq!(stored["creditsPolicy"], policy);
            assert_eq!(
                stored["accountCreditsAllowed"],
                json!({"a": false, "b": true})
            );
            assert_eq!(stored["sessionAffinity"], false);
            assert_eq!(stored["accountIds"], json!(["a"]));
        }
    }

    #[test]
    fn invalid_policy_and_non_boolean_override_are_rejected() {
        let mut value = old_collection();
        value["creditsPolicy"] = json!("unexpected");
        assert!(serde_json::from_value::<CodexLocalAccessCollection>(value).is_err());
        let mut value = old_collection();
        value["accountCreditsAllowed"] = json!({"a": "false"});
        assert!(serde_json::from_value::<CodexLocalAccessCollection>(value).is_err());
    }

    #[test]
    fn v1366_settings_and_credit_permissions_survive_the_same_roundtrip() {
        let mut value = old_collection();
        value["creditsPolicy"] = json!("strict_regular");
        value["accountCreditsAllowed"] = json!({"a": false, "b": true});
        value["requestPayloadLogging"] = json!(true);
        value["imageGenerationMainModel"] = json!("test-image-main-model");
        let collection: CodexLocalAccessCollection = serde_json::from_value(value).unwrap();
        let stored = serde_json::to_value(collection).unwrap();
        assert_eq!(stored["creditsPolicy"], "strict_regular");
        assert_eq!(
            stored["accountCreditsAllowed"],
            json!({"a": false, "b": true})
        );
        assert_eq!(stored["requestPayloadLogging"], true);
        assert_eq!(stored["imageGenerationMainModel"], "test-image-main-model");
    }

    #[test]
    fn v1368_image_catalog_and_explicit_concurrency_survive_credits_updates() {
        let mut value = old_collection();
        value["imageGenerationModels"] = json!(["gpt-image-2.5", "test-image-model"]);
        value["imageGenerationDeletedModels"] = json!(["gpt-image-2"]);
        value["imageGenerationModelCatalogRevision"] = json!(7);
        value["imageGenerationAccountIds"] = json!(["images"]);
        value["imageGenerationAccountModels"] = json!({"images": "test-image-model"});
        value["maxAccountConcurrency"] = json!(0);
        value["accountConcurrencyDefaultMigrated"] = json!(true);

        let mut collection: CodexLocalAccessCollection =
            serde_json::from_value(value.clone()).unwrap();
        collection.credits_policy = CodexLocalAccessCreditsPolicy::PreferRegular;
        collection.account_credits_allowed.insert("a".into(), false);
        let stored = serde_json::to_value(collection).unwrap();
        for field in [
            "imageGenerationModels",
            "imageGenerationDeletedModels",
            "imageGenerationModelCatalogRevision",
            "imageGenerationAccountIds",
            "imageGenerationAccountModels",
            "maxAccountConcurrency",
            "accountConcurrencyDefaultMigrated",
        ] {
            assert_eq!(
                stored[field], value[field],
                "lost {field} during credits update"
            );
        }
        assert_eq!(stored["creditsPolicy"], "prefer_regular");
        assert_eq!(stored["accountCreditsAllowed"], json!({"a": false}));
    }

    #[test]
    fn legacy_credits_settings_receive_upstream_concurrency_and_catalog_defaults() {
        let mut value = old_collection();
        value["creditsPolicy"] = json!("strict_regular");
        value["accountCreditsAllowed"] = json!({"a": false});
        let collection: CodexLocalAccessCollection = serde_json::from_value(value).unwrap();
        assert_eq!(collection.max_account_concurrency, 2);
        assert!(!collection.account_concurrency_default_migrated);
        assert_eq!(
            collection.image_generation_models,
            ["gpt-image-2.5", "gpt-image-2"]
        );
        assert_eq!(
            collection.credits_policy,
            CodexLocalAccessCreditsPolicy::StrictRegular
        );
        assert_eq!(collection.account_credits_allowed.get("a"), Some(&false));
    }
}
