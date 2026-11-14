//! Isolated tests of the actual local-project planner, transactional storage,
//! account renderer-state bridge, launch deadline and atomic file writer.
//!
//! The recovery module and its unit tests are imported directly. Tests
//! of the enclosing desktop/session orchestration remain in the full App suite;
//! this crate does not replace them with mocks or claim to validate process-stop
//! detection, credential-owner resolution, UI entry points or history rebuilding.
//! Build-generated helpers are selected from production syntax trees unchanged.
//! The only shim is the external warning-log sink; no credentials or live data
//! directory is configured. Existing fixtures use temporary files and databases.
#![allow(dead_code)]

use rusqlite::{Connection, OpenFlags};
use std::{collections::HashSet, ffi::OsStr, fs, path::{Path, PathBuf}, sync::Mutex, time::Duration};

#[path = "../../../src-tauri/src/modules/atomic_write.rs"]
pub mod atomic_write;

#[path = "../../../src-tauri/src/modules/ssh_server_identity.rs"]
pub mod ssh_server_identity;

pub mod modules {
    pub use crate::atomic_write;
    pub use crate::ssh_server_identity;

    pub mod logger {
        pub fn log_warn(message: &str) {
            eprintln!("{message}");
        }
    }

    pub mod windows_operation {
        #[cfg(target_os = "windows")]
        use serde::Serialize;
        include!(concat!(env!("OUT_DIR"), "/windows_errors.rs"));
    }
}

include!(concat!(env!("OUT_DIR"), "/visibility_helpers.rs"));
include!("../../../src-tauri/src/modules/codex_session_visibility_launch_budget.rs");

#[path = "../../../src-tauri/src/modules/codex_local_project_recovery/mod.rs"]
mod local_projects;

#[cfg(test)]
mod launch_budget_tests {
    use super::*;
    include!(concat!(env!("OUT_DIR"), "/launch_budget_tests.rs"));
}

#[test]
#[ignore = "Explicit read-only metadata preview; requires COCKPIT_PROJECT_RECOVERY_READONLY_HOME"]
fn readonly_preview_current_project_metadata() {
    let home = std::env::var_os("COCKPIT_PROJECT_RECOVERY_READONLY_HOME")
        .map(PathBuf::from)
        .expect("explicit read-only metadata directory is required");
    assert!(home.is_absolute() && home.is_dir(), "expected an existing absolute directory");
    let started = std::time::Instant::now();
    let result = with_launch_visibility_budget(Duration::from_millis(500), || {
        let plan = local_projects::scan_for_repair(&home, None, true)?;
        check_launch_visibility_budget("readonly_preview_done")?;
        Ok(plan)
    });
    // Do not print the plan, paths, account identifiers, thread content or raw
    // errors. This entry point never calls apply, rollback, recover or prepare.
    let (counts, changed) = match result {
        Ok(Some(plan)) => (Some(plan.counts.clone()), plan.changed()),
        Ok(None) => (None, false),
        Err(error) => {
            let category = if error.contains(LAUNCH_VISIBILITY_TIMEOUT) {
                "timeout"
            } else if error.contains("CODEX_PROJECT_QUICK_LIMIT") {
                "quick_limit"
            } else {
                "scan_failed"
            };
            panic!("read-only project scan: {category}; elapsed_ms={}", started.elapsed().as_millis());
        }
    };
    println!("{}", serde_json::json!({
        "elapsed_ms": started.elapsed().as_millis(),
        "changed": changed,
        "counts": counts,
    }));
}
