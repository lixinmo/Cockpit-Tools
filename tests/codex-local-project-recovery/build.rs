use quote::ToTokens;
use std::{env, fs, path::Path};

// Include selected existing parent-module items without reproducing their
// implementation in this harness or bringing in the desktop process runtime.
// Missing/renamed items fail the harness build instead of silently using a shim.
fn source_items(root: &Path, relative: &str, names: &[&str]) -> String {
    let path = root.join(relative);
    println!("cargo:rerun-if-changed={}", path.display());
    let source = fs::read_to_string(&path).expect("read production source");
    let parsed = syn::parse_file(&source).expect("parse production source");
    let mut output = String::new();
    for name in names {
        let matches = parsed.items.iter().filter(|item| {
            let ident = match item {
                syn::Item::Fn(item) => Some(&item.sig.ident),
                syn::Item::Const(item) => Some(&item.ident),
                syn::Item::Struct(item) => Some(&item.ident),
                _ => None,
            };
            ident.is_some_and(|ident| ident.to_string() == *name)
        }).collect::<Vec<_>>();
        assert_eq!(matches.len(), 1, "expected one {name} in {relative}");
        output.push_str(&matches[0].to_token_stream().to_string());
        output.push('\n');
    }
    output
}

fn main() {
    println!("cargo:rustc-check-cfg=cfg(cockpit_project_recovery_harness)");
    println!("cargo:rustc-cfg=cockpit_project_recovery_harness");
    let manifest = env::var_os("CARGO_MANIFEST_DIR").expect("manifest directory");
    let root = Path::new(&manifest).join("../..");
    let out = env::var_os("OUT_DIR").expect("build output directory");
    let out = Path::new(&out);
    let mut helpers = source_items(&root,
        "src-tauri/src/modules/codex_session_visibility_repair_api.rs",
        &["STATE_DB_FILE", "SQLITE_DIR_NAME", "PREFERRED_SQLITE_DB_FILE"]);
    helpers.push_str(&source_items(&root,
        "src-tauri/src/modules/codex_session_visibility_projects.rs",
        &["QUICK_PROJECT_REPAIR_BUDGET"]));
    helpers.push_str(&source_items(&root,
        "src-tauri/src/modules/codex_session_visibility_catalog.rs",
        &["to_desktop_workspace_path", "is_windows_workspace_path", "trim_safe_windows_trailing_separators"]));
    helpers.push_str(&source_items(&root,
        "src-tauri/src/modules/codex_session_visibility_sqlite_repair.rs",
        &["table_columns"]));
    helpers.push_str(&source_items(&root,
        "src-tauri/src/modules/codex_session_visibility_backup.rs",
        &["sqlite_candidate_paths", "sqlite_dir_session_dbs", "is_sqlite_candidate", "has_codex_session_table"]));
    fs::write(out.join("visibility_helpers.rs"), helpers).expect("write source-derived helpers");

    let windows = source_items(&root,
        "src-tauri/src/modules/windows_operation.rs",
        &["WINDOWS_OPERATION_ERROR_PREFIX", "WindowsOperationErrorPayload", "is_access_denied_message", "format_error", "format_permission_io_error"]);
    fs::write(out.join("windows_errors.rs"), windows).expect("write source-derived error formatting");

    let budget_tests = source_items(&root,
        "src-tauri/src/modules/codex_session_visibility_launch_budget_tests.rs",
        &["launch_budget_interrupts_sql_and_rolls_back_its_transaction",
          "expired_launch_budget_is_paused_for_existing_and_fresh_rollback_connections",
          "launch_budget_does_not_detach_a_slow_read_or_leave_deadlines_on_next_worker",
          "only_pre_mutation_process_snapshot_failures_can_be_deferred"]);
    fs::write(out.join("launch_budget_tests.rs"), budget_tests).expect("write source-derived deadline tests");
}
