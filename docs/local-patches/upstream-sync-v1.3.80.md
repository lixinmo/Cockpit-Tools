# Upstream integration: v1.3.80

## Source and update channel

Integration target: `02498ace` (`upstream/main`, 2026-11-07), including release
`v1.3.80` at `0436a3d4`. The previously installed build reports `1.3.73`.
There are seven intervening official releases (`1.3.74` through `1.3.80`).
The integration preserves both public `main` and the subsequent maintained
`codex/stepfun-native-capability` history; no upstream source snapshot replaces
the repository history.

The fork's Tauri updater intentionally uses `lixinmo/Cockpit-Tools`. Its rolling
`cloud-macos-x86_64` release is a prerelease, not GitHub's latest stable release,
and does not provide a signed updater manifest. Consequently, the stable
`releases/latest/download/latest*.json` endpoints return 404 until a formal fork
release is published. Automatic checks can remain enabled without reporting a
new upstream version. This integration keeps the installation channel on the
fork; it does not install upstream binaries over the maintained policies.

## Resolution decisions

| Area | Upstream changes and retained behavior |
| --- | --- |
| Local projects after account switches | Adopt upstream's account-state/project recovery and safe handling of malformed global state. No second local project-recovery implementation is added. |
| Automatic Reset cards | Use upstream's rule UI, worker and redemption journal. Migrate existing account opt-ins and thresholds without enabling an expiry trigger; honor pending legacy consumption barriers. See `auto-consume-reset.md`. |
| Chat provider tools | Use upstream tool discovery and protocol restoration. Preserve only the additional provider capability declarations and legacy-history compatibility that upstream does not cover. |
| Provider catalog and reasoning | Combine upstream catalog refresh, protocol/auth-scheme and image metadata with generic configurable reasoning/context metadata and atomic profile publication. Do not restore provider-specific model heuristics. |
| Credits and API-key budgets | Upstream API-key monetary/concurrency budgets do not replace the local OpenAI Credits admission policy. Both apply at their respective boundaries. |
| Setup-only connection retry | Preserve `CT-CONNECT-RETRY-001` v1.0 and its Native dependency hooks. Review upstream transport recovery separately from the setup-only budget. |
| Go sidecar | Upstream retired the Go implementation in v1.3.76. Retain the requested optional sidecar and reference contract, including the final upstream updates before retirement (`19f00699`). Native remains the default. |
| Build and publication | Retain platform-selectable Cloud Build, one frontend typecheck, contract gates, dependency/incremental caches, short-lived artifacts and rolling downloads. Incorporate upstream formal macOS signing/notarization safeguards. All compilation runs on standard GitHub-hosted runners. |

## Validation

Integration is in progress. A passing cloud run and its exact source commit
must be recorded before this merge is handed off as a validated application.
No live account credentials or configuration are used by the verification jobs.
