//! Offline harness: compile the actual reset policy, account data types, and
//! atomic persistence without linking account storage, authentication, or HTTP.
#![allow(dead_code)]

// The production account display name uses reqwest's url::Url re-export.
// Exercise that same parser without adding an HTTP client to this harness.
extern crate self as reqwest;
pub use url::Url;

#[path = "../../src-tauri/src/models/codex_local_access.rs"]
pub mod codex_local_access;
#[path = "../../src-tauri/src/models/codex.rs"]
pub mod codex_model;
pub mod models {
    pub use crate::codex_local_access;
    pub use crate::codex_model as codex;
}

#[path = "../../src-tauri/src/modules/atomic_write.rs"]
pub mod atomic_write;
#[path = "../../src-tauri/src/modules/codex_auto_reset.rs"]
pub mod codex_auto_reset;
#[path = "../../src-tauri/src/modules/codex_auto_reset_monitor.rs"]
pub mod codex_auto_reset_monitor;

pub mod modules {
    pub use crate::atomic_write;
    pub use crate::codex_auto_reset;

    // The reset fixtures do not exercise the identity database. Keep only its
    // serializable profile type so importing the production collection model
    // does not pull account storage or network access into this offline harness.
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

    pub mod logger {
        pub fn log_info(_: &str) {}
        pub fn log_warn(_: &str) {}
    }

    // These platform/UI helpers are unrelated to reset policy. All filesystem
    // operations still go through production atomic_write, in temporary dirs.
    pub mod windows_operation {
        pub fn format_permission_io_error(
            _: &str,
            _: &str,
            _: &str,
            _: &std::io::Error,
        ) -> Option<String> {
            None
        }
    }

    pub mod codex_account_proxy {
        pub fn serialize_summary<S: serde::Serializer>(
            value: &Option<String>,
            serializer: S,
        ) -> Result<S::Ok, S::Error> {
            assert!(
                value.is_none(),
                "offline reset fixtures never configure a proxy"
            );
            serializer.serialize_none()
        }
    }
}
