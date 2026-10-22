# Auto Consume Reset

Local addition on upstream `4434c32e` (Cockpit Tools v1.3.64), 2026-10-01.

The v1.3.70 integration on 2026-10-16 retains this policy and background worker.
Its offline harness now supplies the newly referenced installation-profile data
type without linking account storage or identity/network operations. All 64
offline reset/model/monitor tests and the full frontend suite passed. The actual
account settings, durable reset barriers and conversation data were not changed
by this source integration.

## v1.3.73 quota-sync integration, 2026-10-22

The upstream activity-driven quota synchronizer and the independent one-minute
Reset monitor share the existing per-account refresh scheduler. Both use the
same production Reset wrapper, which acquires the durable guard before starting
the quota GET. The upstream `quota_only` option is passed through the fetch and
the post-consumption refresh: it skips optional subscription enrichment, not
Reset eligibility or duplicate-consumption protection.

Upstream usage-identity checks, transactional quota writes, quota-update events,
and gateway-reserve confirmation remain in the underlying refresh operation.
Every successful return explicitly distinguishes freshly committed quota from
ownership-deferred cached quota. Cached or rejected results cannot consume a
Reset or release an existing barrier. The per-account switch and threshold,
independent monitor, selected-credit/idempotency behavior, and durable low-quota
episode barrier remain unchanged. Auto Reset continues to run in the Rust host
with either API-service engine or without a running API service.

Account eligibility is shared by the settings command, monitor and Reset API
boundary. API-key accounts (including upstream's official ChatGPT Plan
projections) and read-only Web Session imports are rejected; existing Codex OAuth
and Agent Identity support is retained. No saved switches or thresholds are
rewritten by this check.

The isolated native harness passed 71 tests, including five new regressions
against the actual production refresh wrapper and account predicate: propagation
of both `quota_only` modes with a lock held before GET, cached-quota barrier
preservation, concurrent activity/monitor checks with one consumption per low
episode, failed-query/storage behavior, and unsupported account exclusion.
The harness imports production policy, account models and atomic persistence,
and substitutes quota/Reset operations; it performs no real account, credential,
network or consumption operations. `git diff --check` passed for these changes.

## Controls and persistence

Each Codex account's Reset Credits dialog exposes its own **Auto Consume Reset**
switch and remaining-weekly-quota threshold. Both are stored independently by
account ID. All accounts default to **off** and **2%**; the accepted threshold
range is an integer from 0 through 99. Changing A never enables B or changes B's
threshold. Editing a threshold alone does not enable that account.

The earlier global implementation was corrected on the user's request. Its
`codex_auto_consume_reset_enabled` / `codex_auto_consume_reset_threshold` fields
are now ignored, even if the old global switch was enabled. They are removed on
the next normal configuration save. Users must explicitly enable the intended
accounts in the new version; no global setting is used as an account fallback.
The unscoped quick-settings toggle has been removed.

The scoped `get_codex_auto_reset_settings` / `set_codex_auto_reset_settings`
commands require an existing supported account ID. They persist to
`config.json` under `codex_auto_consume_reset_accounts`. The setter changes only
specified fields for that account inside the existing `patch_user_config`
transaction (runtime lock, cross-process lock, latest-file reload, atomic write).
Clients cannot overwrite the whole map through the general-config patch API.
The UI discards old async responses when the selected account changes.

## Trigger and reuse

An independent Rust background worker checks enabled accounts approximately once
per minute, using the existing quota-refresh implementation. Its configuration
scan runs approximately every five seconds. Enabled accounts are scheduled for a
first check on startup; newly enabled accounts become eligible at the next scan.
This worker does not depend on a frontend timer or the general Codex auto-refresh
setting. Existing manual and other background quota-refresh paths can still run
the same consumption check. The original global/current-account/group refresh
schedules are not modified. Accounts without Auto Consume Reset keep their
existing refresh behavior; with no opted-in accounts the new monitor performs
no account queries. Disabling an account removes it from new monitor work. An
already-started request may finish, but consumption rechecks the enabled setting.

All paths check consumption only after successfully fetching and saving fresh
quota. Token-ownership deferral can return cached quota in this project; that
branch is explicitly marked as cached and cannot trigger consumption or release
a pending barrier. The worker reuses the same per-account lock and durable
consumption barrier as manual refresh and manual Reset actions.

Closing or unfocusing the main window, or turning off the display, does not stop
the worker while Cockpit Tools remains running and the computer remains awake.
Quitting the app or system sleep stops checks; a screen-off state alone is not
system sleep. After wake, overdue checks become eligible at the next scheduler
scan. After connectivity returns, an enabled account is retried on its normal
check schedule, normally within about a minute; there is no promise of an
immediate network-change callback. Network failures, existing request deferrals,
and running requests can delay successful quota retrieval. No quota or Reset is
consumed merely to keep the worker alive.

If that account is enabled and its weekly remaining is at or below its own
threshold, the existing Reset details query is called. A seven-day window is
identified by its duration in either primary or secondary, including accounts
with only a primary weekly window. A legacy secondary window with no duration
is still recognized as weekly. A 5-hour or daily window cannot trigger a reset;
invalid weekly percentages fail closed. When both windows are weekly, the lower
remaining value is used.

Only available, unredeemed, unexpired Reset records with an ID are eligible.
Known reset types `codex_rate_limits` and the existing legacy `rate_limit_reset`
are accepted; other explicit types are excluded. An explicit `reset_type` takes
precedence over the generic `type` field during parsing. Dated records are
sorted by `expires_at` ascending; records without an expiry are last, with ID as
a deterministic tie-break. An empty eligible set results in no POST and no
consumption marker. The selected ID is sent as `credit_id` through the existing
consumption/token-refresh/proxy code, alongside a single `redeem_request_id`.
Authentication retries reuse both values. The HTTP-success response is also
checked for the official `reset` / `already_redeemed` outcome codes.

The selection contract is confirmed in the public official
[Codex backend client](https://github.com/openai/codex/blob/main/codex-rs/backend-client/src/client/rate_limit_resets.rs)
and its
[request tests](https://github.com/openai/codex/blob/main/codex-rs/backend-client/src/client/rate_limit_resets_tests.rs).
Full Banked Reset refreshes eligible weekly and 5-hour windows, according to
[OpenAI's help article](https://help.openai.com/en/articles/20001498).
The weekly-only restriction here controls when to trigger it, not what the
server resets.

## Duplicate-consumption protection

- A per-account file lock is shared by automatic and existing manual Reset
  actions. Automatic refresh obtains it before the initial quota query and keeps
  it through selection, POST, and the post-consumption quota refresh. A competing
  refresh may refresh its display, but cannot run the consumption check.
- Before POST, an atomic durable record stores the chosen credit, request ID,
  triggering threshold, and a flag indicating that a fresh refresh is required.
  Records are in `codex_auto_reset/<SHA256-of-account-id>.json` under the existing
  Cockpit data directory. Lock files are separate from atomically replaced JSON.
- Only one credit is submitted per check. After POST, the existing quota-query
  implementation is called directly, avoiding a recursive scheduler join or a
  second automatic check.
- An error, timeout, cancelled task, app restart, cached response, or refresh
  failure cannot discard the durable barrier. Corrupt/unwritable state fails
  closed before spending anything. Uncertain POST outcomes are not automatically
  retried with a fresh request ID.
- A fresh query completes the waiting-for-refresh phase. Automatic consumption
  remains suppressed throughout the same low-quota episode, until a fresh weekly
  percentage is above both the triggering and current threshold. This also
  prevents delayed or unchanged server quota from draining multiple cards.
  A later drop can trigger one new card. Explicit manual Reset actions remain
  available after the fresh-query phase.
- Turning the switch off/on or lowering its threshold does not clear an existing
  pending marker. Settings and barriers survive application restart.

## Files changed for this feature

- Backend policy: `src-tauri/src/modules/codex_auto_reset.rs` (new),
  `src-tauri/src/modules/codex_auto_reset_monitor.rs` (new),
  `src-tauri/src/modules/codex_quota.rs`, `src-tauri/src/modules/mod.rs`.
- Persistent settings and commands: `src-tauri/src/modules/config.rs`,
  `src-tauri/src/commands/codex_account_commands.rs`, `src-tauri/src/lib.rs`.
  The former changes to `system_config_types.rs` and `system_network_general.rs`
  have been removed with the global settings interface.
- UI: `src/components/codex/CodexAutoConsumeResetSettings.tsx` and `.css` (new),
  `src/pages/CodexAccountsOverviewPanel.tsx`, `src/pages/useCodexAccountsOverviewController.tsx`.
  The global addition to `QuickSettingsPopover.tsx` has been removed.
- Translations (three keys in each): `src/locales/{ar,cs,de,en,en-US,es,fr,id,it,ja,ko,pl,pt-br,ru,tr,vi,zh-CN,zh-tw}.json`.
- Tests: `src-tauri/src/modules/codex_auto_reset_tests.rs` (new),
  `src-tauri/src/modules/codex_auto_reset_monitor_tests.rs` (new),
  `src-tauri/src/modules/config.rs`, `src-tauri/src/commands/system_tests.rs`,
  `src/components/codex/CodexAutoConsumeResetSettings.test.ts` (new), and
  `tests/codex-auto-reset/{Cargo.toml,Cargo.lock,lib.rs}` (new offline harness).
- Documentation: this file and the v1.3.64 integration entry in
  `docs/local-patches/same-account-connect-retry.md`.

## Verification and limits

All consumption tests use synthetic data and mock operations. The isolated Rust
harness imports the real policy, real atomic persistence, and real account types;
it does not link HTTP, authentication, account storage, or the application runtime
configuration loader.
Temporary directories isolate every persistent test. The frontend test intercepts
every Tauri invocation. No real quota consumption, Banked Reset, or Credits use
was tested, and the built application was not launched against an account.

The historical results below predate the independent Rust background worker;
they do not establish that the current worker revision has been built or tested.

Initial global-version verification (before the per-account correction):
37 isolated Rust tests (25 policy, 6 atomic persistence, 6 account
model), 4 component mock tests, 823 existing TypeScript regression tests, and
2 desktop configuration tests for default-off upgrades, persistence and patch
validation. The native desktop test command was:

```sh
COCKPIT_NATIVE_APP_BUILD=1 COCKPIT_SKIP_CLIPROXY_BUILD=1 \
  cargo test --locked --release --target x86_64-apple-darwin \
  -p cockpit-tools --lib codex_auto_consume_reset -- --test-threads=1
```

All 18 locale files pass the project translation checker. The native
connection-retry verifier also passed after the upstream integration.
The single TypeScript check and Vite production build also passed.

`npm run build:app:local` completed successfully with Node 24.19.0, Go 1.26.8
and Rust 1.98.0. Exactly one bundle was generated:
`target/x86_64-apple-darwin/release/bundle/macos/Cockpit Tools.app`.
That initial bundle was 145.8 MiB, version 1.3.64. Both executables are exclusively
x86_64, and the bundled sidecar SHA-256 matches the newly built native sidecar.
No DMG, updater archive, Universal/arm64 sidecar or other-platform artifact was
generated. The application was not installed or launched. The build emitted
non-blocking deprecated/unused-code and Vite chunk-size warnings.

Bundled sidecar SHA-256: `328aab8e4bbb3571b841fe4837e82558c51e0d9f380571258193e3c6cb666520`.

Mock tests cannot establish live account eligibility, actual server-side
redemption/idempotency, or how quickly the live quota response reflects a reset.
Those are the remaining live-service integration uncertainties. They were not
tested on a real account; the implementation retains its barrier while quota
recovery is unconfirmed.

## Historical per-account correction verification

The correction uses isolated account settings and consumption mocks; no real
account, credentials, quota, Banked Reset or Credits consumption is tested.
Passed: 46 isolated native Rust tests (34 reset-policy/settings tests, 6 atomic
persistence tests, 6 account-model tests), 9 component mock tests, and the
18-language translation checker. New cases cover per-account default-off,
legacy-global migration, atomic settings round trips, partial-update isolation,
independent thresholds, two low-quota accounts with only one enabled, independent
pending barriers, stale A/B reads and saves, and A→B→A failure recovery.

The two desktop configuration integration tests were updated for account scope;
this correction uses the focused offline harness and the native production build
rather than rerunning the full desktop test executable. The full TypeScript suite
also passed all 832 tests (including the 9 component tests).

The per-account correction's `npm run build:app:local` completed successfully
before the independent background worker was added (one TypeScript check and
one native app bundle). That v1.3.64 bundle was 146.4 MiB; both
main and sidecar executables are x86_64 only. The sidecar hash matches the newly
built native sidecar. No DMG, updater archive or other-architecture artifact was
generated, and the app was not installed or launched.

Corrected app executable SHA-256: `3b580ad5ef375ef27b5b2013e9d8a758e866e2da43c45ac6e99f5852eb8731ed`.

## Independent background-check correction

The UI text now states that checks run in the background approximately once a
minute while the app is running and the computer is awake. All 18 translations
retain the account-specific scope, default-off setting, weekly-only threshold,
earliest-expiry selection, and quota-recovery barrier. The worker does not change
any account's saved switch or threshold.

For this wording revision, all 9 account-settings component mock tests and the
18-language translation checker pass; `git diff --check` also passes. The
frontend component behavior and saved settings were unchanged.

The native offline harness passes 62 tests: 41 policy/settings tests, 9 monitor
tests, and 12 real model/atomic-write tests. The original incident regression
(primary 604800 seconds, secondary absent, zero weekly remaining) now passes.
An end-to-end test runs the actual monitor without a WebView, feeds it mock
quota, executes the real policy and persistent guard, selects the earliest
legacy/official credit, mocks one consume and fresh weekly recovery, and verifies
that a disabled second account never participates. Clock jumps, bounded
concurrency, no catch-up burst, failure retry, panic recovery, queued disable,
and settings wakeup are also covered. These tests import no live authentication,
account storage, or HTTP implementation.

The production monitor starts once during Rust setup, wakes after an account
setting save, and uses the existing background quota scheduler. Production
checks settings again at the automatic POST boundary after token preparation
and on authentication retries. No account switches or thresholds are changed.
New diagnostic log entries include account ID, check start, identified weekly
window, threshold, skip reason, selection expiry, and post-consumption refresh
outcome. Tokens and account email addresses are not included in these entries.

The current `npm run build:app:local` passed: one TypeScript check, Vite production
build, and one native `.app` bundle. The final version is 1.3.64, 146.1 MiB.
Both main and sidecar executables are exclusively x86_64; the bundled sidecar
hash matches the newly built native sidecar. The backend monitor marker is
present in the built main executable. No DMG, updater archive, Universal or
foreign-architecture artifact was generated. No live account calls, app launch,
installation, saved account-setting changes or real Reset consumption were
performed during this correction.

Headless-fix main executable SHA-256: `7f864e9c14f1e0ad9a2fa0f6f3f863aa8b7751f97f7890bace7fb8052067bb56`.
Native/bundled sidecar SHA-256: `328aab8e4bbb3571b841fe4837e82558c51e0d9f380571258193e3c6cb666520`.
