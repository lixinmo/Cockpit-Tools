# Isolated local-project recovery tests

Run from the repository root with the existing Rust toolchain:

```sh
CARGO_TARGET_DIR=target/project-recovery-harness cargo test --locked --release \
  --manifest-path tests/codex-local-project-recovery/Cargo.toml --lib -- --test-threads=1
```

This independent workspace compiles the actual project recovery module and its
existing tests. It also imports the actual launch-budget implementation and
atomic-write implementation and SSH identity helpers, including SQLite interruption, rollback,
compare-and-swap and concurrency protections. `build.rs` selects parent-module
path/SQLite helpers and four existing deadline tests directly from their source
syntax trees; missing source items fail the build. No helper implementation is
re-created here. The only shim is a warning-log sink.

The custom `cockpit_project_recovery_harness` cfg excludes tests of the
enclosing desktop/session orchestration and their helpers. They still run
in the full App suite, including with `--all-features`; the App only declares
that cfg for linting and does not enable it. This harness does **not** validate
desktop stop detection, credential-owner resolution, UI entry points or the
combined history-index repair. Core project/storage/account-state tests are
not excluded.

## Optional read-only preview

The ignored preview calls the real `scan_for_repair(home, None, true)` under the
real 500 ms launch budget. Supply the intended Codex data directory explicitly:

```sh
COCKPIT_PROJECT_RECOVERY_READONLY_HOME=/absolute/codex/data \
CARGO_TARGET_DIR=target/project-recovery-harness cargo test --locked --release \
  --manifest-path tests/codex-local-project-recovery/Cargo.toml \
  --lib readonly_preview_current_project_metadata -- --ignored --exact --nocapture
```

It prints only elapsed time, whether a plan would change metadata, and change
counts. It does not call `apply`, `rollback`, `recover_pending` or account-state
`prepare`, does not switch accounts or stop an application, and does not print
project names, paths, account identities or conversation contents. A preview
failure reports a coarse category without printing the underlying data error.
The preview is optional and is not a background repair service.
