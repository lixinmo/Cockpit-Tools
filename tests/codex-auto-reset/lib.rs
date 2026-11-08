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
#[path = "../../src-tauri/src/modules/codex_reset_automation.rs"]
pub mod codex_reset_automation;

pub mod modules {
    pub use crate::atomic_write;
    pub use crate::codex_auto_reset;

    // Pure serialization fields referenced by the production collection model.
    // Importing diagnostics would also link the HTTP server; these fixtures
    // intentionally have no Native runtime, request logging or network access.
    pub mod codex_native_api {
        pub mod diagnostics {
            #[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
            #[serde(rename_all = "camelCase")]
            pub struct RequestSize {
                pub total_bytes: u64,
                pub input_bytes: u64,
                pub tools_bytes: u64,
                pub instructions_bytes: u64,
            }
            #[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
            #[serde(rename_all = "camelCase")]
            pub struct OverloadRetry {
                pub retry: u32,
                pub wait_ms: u64,
                pub recovery_ms: u64,
            }
        }
    }

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

// Import the production upstream automation and journal without Tauri, account
// storage or HTTP. Runtime calls deliberately fail in this offline fixture.
extern crate self as tauri;
extern crate self as tempfile;
#[derive(Clone)]
pub struct AppHandle;
pub trait Emitter {
    fn emit<T: serde::Serialize>(&self, _: &str, _: T) -> Result<(), String> { Ok(()) }
}
impl Emitter for AppHandle {}
pub mod async_runtime {
    pub use tokio::task::spawn_blocking;
}
pub struct TempDir(std::path::PathBuf);
impl TempDir { pub fn path(&self) -> &std::path::Path { &self.0 } }
impl Drop for TempDir { fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); } }
pub fn tempdir() -> std::io::Result<TempDir> {
    let path = std::env::temp_dir().join(format!("cockpit-reset-migration-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&path)?;
    Ok(TempDir(path))
}
pub use modules::logger;
pub mod config {
    #[derive(Default)]
    pub struct Config { pub language: String, pub codex_auto_reset: crate::codex_auto_reset::Preferences }
    pub fn get_user_config() -> Config { Config::default() }
    pub fn get_data_dir() -> Result<std::path::PathBuf, String> {
        static DIR: std::sync::OnceLock<crate::TempDir> = std::sync::OnceLock::new();
        Ok(DIR.get_or_init(|| crate::tempdir().unwrap()).path().to_path_buf())
    }
}
pub mod i18n {
    pub fn translate(_: &str, key: &str, _: &[(&str, &str)]) -> String { key.to_string() }
}
pub mod background_maintenance { pub fn try_resource() -> Option<()> { Some(()) } }
pub mod app_lifecycle { pub fn is_shutdown_started() -> bool { true } }
pub mod tray_layout { pub fn is_platform_enabled(_: &str) -> bool { false } }
pub mod codex_account {
    use crate::models::codex::CodexAccount;
    pub fn load_account(_: &str) -> Option<CodexAccount> { None }
    pub struct Index { pub accounts: Vec<CodexAccount> }
    pub fn load_account_index() -> Index { Index { accounts: vec![] } }
    pub fn with_reset_automation_account_ids<T>(f: impl FnOnce(&[String]) -> Result<T, String>) -> Result<T, String> { f(&[]) }
}
pub mod codex_quota {
    use crate::models::codex::{CodexQuota, CodexResetCredit};
    pub struct CodexResetCreditsSnapshot {
        pub available_count: Option<i64>, pub credits: Vec<CodexResetCredit>, pub next_expires_at: Option<i64>,
    }
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum ResetCreditConsumeOutcome { Reset, NothingToReset, NoCredit, AlreadyRedeemed }
    impl ResetCreditConsumeOutcome {
        pub fn into_result(self) -> Result<(), String> {
            match self { Self::Reset | Self::AlreadyRedeemed => Ok(()), _ => Err("no reset".into()) }
        }
    }
    pub async fn consume_reset_credit_with_request_id(_: &str, _: &str, _: &str, _: Option<&str>) -> Result<ResetCreditConsumeOutcome, String> {
        panic!("offline tests must not consume Reset")
    }
    pub async fn fetch_account_reset_credits(_: &str) -> Result<CodexResetCreditsSnapshot, String> {
        panic!("offline tests must not query live accounts")
    }
    pub async fn refresh_account_quota_for_reset(_: &str) -> Result<Option<CodexQuota>, String> {
        panic!("offline tests must not query live accounts")
    }
}
