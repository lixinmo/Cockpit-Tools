# Codex API service credits policy

The API service has a persisted `creditsPolicy` (`default`, `prefer_regular`,
`strict_regular`) and `accountCreditsAllowed` map. Missing policy means default;
missing account entries mean true. False overrides both default and preference.
Strict mode preserves the account preferences but does not permit credits.
The API service's Credits panel saves each change immediately. The v1.3.73 local
integration enforces the policy in the Rust Native engine, including a fresh
check immediately before sending a selected request. Existing queued routes
observe policy changes; a previously admitted in-flight request cannot be undone.

## Rust Native migration — v1.3.73, 2026-10-22

The local Go implementation has been retired. The Native implementation reuses
upstream routing, account permissions, concurrency, OAuth reserves, official
fallback and quota querying. Only the missing Credits admission policy and its
per-window evidence are local. The old implementation and regressions remain
recoverable at commit `54012972cdc3b04b5b79b638b9c1150ac265fa25`.

Existing local configurations migrate once to Native, retaining every policy
and account override. `nativeCreditsPolicyMigrated` records this migration so a
later explicit unrestricted Go selection is respected. Restrictive policies
require Native; attempting to save or start them with Go fails explicitly before
changing the service. Existing instance/profile snapshots also receive the
current policy instead of retaining a stale permission.

Official ChatGPT grants and local OAuth have distinct quota authorities. Default
policy with all accounts allowed retains upstream official routing. A restrictive
policy cannot use local OAuth quota as proof that an official request will avoid
Credits. It uses upstream's permitted same-account local fallback when available;
otherwise it rejects the unconfirmed official route. This does not relax official
authorization, model/tool permissions or history-replay checks.

The complete configured Key pool remains the basis of `prefer_regular`, including
paused, disabled, model-ineligible and cooling members. Images use their own
configured pool; loading a conversation model catalog cannot enlarge that pool.
API-key providers and Grok remain outside the subscription Credits policy.

The new runtime files are `codex_local_access_native_credits.rs`,
`codex_local_access_credits_policy.rs` and
`codex_local_access_credits_refresh.rs`. The small Native `Route.send_guard`
extension carries the selected credential/workspace identity through pending,
sent and retry states, and forwards raw quota observations to the local policy.
Upstream reserve parsing and its error/status semantics remain intact.

Default preserves previous selection: regular quota and credits are both usable.
Preference permits credits only when **all Codex subscription accounts in the
request's API-key pool** have confirmed exhausted regular quota. A retry,
disabled account, unavailable model, concurrency limit or network error does not
count as quota exhaustion. Image forwarding uses its configured image pool.
API-key providers and Grok do not participate in Codex subscription-credit policy.

Enforcement runs outside affinity, preferred/backup selection, model cooldown,
and reserve-model exceptions. Unknown or stale quota fails closed whenever
credits are restricted. A fresh snapshot is at most five minutes old; a passed
reset timestamp is not evidence that quota has actually been refreshed. Either
present regular window at zero establishes exhaustion. Absent windows are not
treated as zero. Host quota polling and passive HTTP/WebSocket observations
feed the decision. Manual/automatic error recovery cannot clear this evidence.

## Billing boundary

This is a **local request-admission policy, not an upstream spending lock**.
No verified upstream per-request no-credits parameter is available in this
integration. An already admitted request may cross the regular-quota boundary;
concurrent requests or other clients can consume quota between observations.
Consequently absolute zero-credit billing is **not guaranteed**. Configure an
upstream spending limit for that guarantee. The UI explains this limitation and
explains automatic bounded quota refresh and safe rejection if it fails. Tests use synthetic accounts and do
not consume real quota or credits.

## Verification

- Native policy/configuration/refresh tests:
  `cargo test --offline --manifest-path tests/codex-credits-policy/Cargo.toml
  --target-dir target/local-app-cache/credits-policy-tests`.
- `node --import tsx --test src/components/codex/CodexCreditsSettings.test.ts`.
- Main-library `native_credits`, `admission`, and `native_gateway_credits` tests
  exercise the production host, route lifecycle and localhost transports.
- Native `scripts/verify-cockpit-connect-retry.sh` (includes outer sidecar suite).
- `npm run build:app:local` for this Mac's app and matching sidecar.

The fixed CT-CONNECT-RETRY-001 transport implementation and retry budget are
unchanged. This feature does not add network retries or request replay.

## v1.3.70 engine compatibility, 2026-10-16

Upstream now defaults new installations and first-upgrade configurations to its
Rust Native engine. The local Credits admission policy remains implemented in
Go; retaining its UI and stored fields alone would not enforce it in Native.
The integration therefore preserves these boundaries:

- First-upgrade configurations written by the local build (identified by the
  existing `creditsPolicy` or `accountCreditsAllowed` fields) keep Go, including
  default/all-allowed policies. Stock/fresh configurations retain upstream's
  Native default. An explicit later Native selection remains possible when
  there are no Credits restrictions.
- A nondefault policy or any account-level denial requires Go. Native selection
  and restrictive edits while Native is selected fail explicitly before saving
  or stopping the service; no policy is silently relaxed.
- Cold loading an incompatible saved Native configuration returns it to Go
  without losing its policy. Native request admission and persistent profile
  overrides also validate the current restriction, so an older profile snapshot
  cannot silently bypass a newly enabled policy.
- The error identifies the requirement to switch the API service and relevant
  instances to Go. Existing UI error handling retains the saved setting on
  rejection; there is no second frontend policy implementation.

The native compatibility helpers and production collection model passed 12
offline Rust tests, including migration/roundtrip preservation and rejected
selection. The full 1,178-test frontend suite passed, including Credits controls;
the complete outer Go suite passed through the native retry verifier. Added
production `native_gateway_credits` tests cover actual temporary-file migration,
request admission and later explicit engine selection; those GUI-linked tests
were added but were not executed by the offline harness. The full native v1.3.70
app build passed, compiling the production guards. No real configuration, quota
request, account or conversation data was used by validation.

### Initial implementation verification, 2026-10-02 (superseded by repair below)

All 10 focused Go credits tests passed, including a race-detector run; the
complete outer sidecar suite and native CT-CONNECT-RETRY-001 verifier passed.
All 24 focused frontend/regression tests and three production-model Rust
migration/serialization tests passed. The final native local build passed its
TypeScript check, Vite build and Rust/app bundling in 256.9 seconds (incremental
Rust stage: 3m 31s). Both bundled executables are exclusively x86_64; the bundled
sidecar is byte-identical to the native sidecar output. Existing unused-code and
frontend chunk-size warnings remain. No application was installed or restarted,
and no real account quota/credits were consumed for validation.

## 2026-10-02 availability regression repair

The initial test set missed real quota shapes. In particular, replacing a full
snapshot with a partial response discarded the other window's valid evidence;
zero-filled absent-secondary placeholders could become expired active windows;
`floor(100 - used_percent)` misclassified positive fractional quota as exhausted.
A five-minute freshness guard had no on-demand refresh, and downstream errors
reported only the accounts left after filtering, hiding the actual failure.
The initial successful build did not validate these production situations.

The repair retains the three policy semantics and account-level denials:

- Merge immutable observations per window, recording independent millisecond
  sampling times and source (`host`, `response`, `refresh`). Missing response
  fields preserve existing evidence without renewing its age. Preserve explicit
  absent windows; a zero reset-after placeholder cannot replace a known future
  reset. New exhaustion wins over older positive observations; equal-time
  exhaustion wins; unrelated later headers cannot resurrect old host quota.
- Preserve exact remaining percentages separately from integer display values,
  including the host manifest and quota-pool file. A newer (or same-observation
  exact) confirmed positive regular snapshot can supersede an old host cooldown,
  but never clears runtime 429, disabled-account or model cooldown state.
- Under a credits restriction, unknown/stale quota triggers a read-only
  `GET /backend-api/wham/usage` through the selected credential's existing Codex
  HTTP executor and proxy transport. The GET has an eight-second deadline,
  four concurrent slots per sidecar, shared in-flight work per account, and a
  thirty-second minimum interval after completion (including failures). Selection
  waits at most nine seconds and then re-evaluates the original candidates.
  This does not send inference, consume reset credits, register a new retry
  loop, or change the five-minute freshness limit. Cancellation of one caller
  does not cancel other callers' shared read. An unsuccessful/unverifiable read
  remains blocked, not assumed available.
- Emit full key-pool diagnostics after inner filters, with each account's stage,
  regular status, per-window source/age and refresh outcome. No tokens, response
  bodies or email addresses are added. The existing pool UI shows these details.
  Policy failures return explicit local 503 codes (`credits_policy_blocked` or
  `credits_quota_unconfirmed`), not a generic upstream 502.

Offline regression fixtures cover weekly-only Pro with 2%/6% remaining, Plus
dual windows, absent placeholders, partial/unrelated headers, fractional quota,
reset boundaries, stale and out-of-order evidence, full-pool credit permission,
stable repeated affinity selection, concurrent reads, cancellation, timeouts,
failure backoff/recovery, four-account diagnostics and the read-only probe's
bound credential/URL. The transport still invokes the original fixed
CT-CONNECT-RETRY-001 connection-setup budget; no helper code was changed.

Repair verification: all 29 focused Go credits tests (including race detection),
the complete outer sidecar suite, native CT-CONNECT-RETRY-001 verifier, 24 focused
frontend tests, and three Rust configuration-compatibility tests passed. The
final `build:app:local` passed TypeScript, Vite and native app bundling in 77.3s
(Cargo 52.11s), reusing the first repair build's caches. Both bundled executables
are x86_64 and the sidecar matches the native output byte-for-byte. Existing
unused-code/chunk-size warnings remain. No live policy was changed, app installed
or restarted, or real inference/usage request issued by these tests. Real traffic
after installation remains to be verified; passing offline fixtures is not a
claim that the currently running old version has recovered.

## Native migration regression coverage appendix, 2026-10-22

The comparison baseline is local checkpoint `54012972`. All 37 old Go
`TestCredits*` cases were reviewed below; this table maps behaviors, not test
counts. `P` is the production pure policy/parser module
`src-tauri/src/modules/codex_local_access_credits_policy.rs`; `R` is the production
bounded refresh controller `codex_local_access_credits_refresh.rs`; `N` is
`codex_local_access_native_credits_tests.rs`, which calls the production Native
pool, guard and (where named `actual_route`) `native_select_route_once`; `M` is
`tests/codex-credits-policy/gateway_compat_tests.rs`. Upstream tests (`U`) remain
upstream implementations, not local replacement implementations.

| Old Go regression | Native coverage / retained boundary |
| --- | --- |
| `TestCreditsPolicyMatrix` | P `policy_matrix_and_account_denial`; N `native_credits_per_account_denial_survives_complete_pool_exhaustion` |
| `TestCreditsDefaultMigrationAndUnknownQuota` | M `existing_local_configs_migrate_to_native_without_losing_policy`; P `first_partial_positive_observation_remains_unknown_until_other_window_is_confirmed` |
| `TestCreditsQuotaFreshnessAndWindows` | P `fractional_weekly_only_absent_windows_and_reset`, `partial_observation_preserves_other_window_and_age` |
| `TestCreditsPreferRegularWholePoolAcrossRetriesAndKeyScopes` | N `native_credits_complete_pool_preserves_paused_and_excluded_evidence`; U `native_key_account_scopes_preserve_explicit_empty_and_legacy_fixed_pools` |
| `TestCreditsPolicyPrecedesAffinityPriorityImagesAndReserve` | N `native_credits_actual_route_refreshed_capacity_keeps_affinity_until_exhaustion`, `native_credits_image_pool_does_not_expand_to_conversation_catalog`, `native_credits_actual_route_concurrency_never_reintroduces_blocked_accounts_and_keeps_429`; Credits filtering precedes upstream affinity/reserve/priority and its send guard has no reserve-model exception |
| `TestCreditsQuotaHotReloadRecoveryCannotEraseRestriction` | N `native_credits_cached_exhaustion_survives_missing_host_quota_and_recovery`, `native_credits_host_reread_cannot_renew_a_get_that_preceded_exhaustion`; ordinary error recovery never clears the Credits observation store |
| `TestCreditsPassiveResponseOverridesHostSnapshot` | N `native_credits_send_guard_observes_exhaustion_and_preserves_other_window`, `native_credits_host_reread_cannot_renew_a_get_that_preceded_exhaustion` |
| `TestCreditsQuotaRejectsOutOfOrderHostSnapshots` | P `exhaustion_wins_equal_time_and_newer_positive_recovers`; N host reread test |
| `TestCreditsEqualSecondExhaustionWinsAndPartialResponseStaysUnknown` | P `exhaustion_wins_equal_time_and_newer_positive_recovers`, `first_partial_positive_observation_remains_unknown_until_other_window_is_confirmed` |
| `TestCreditsLimitReachedObservationWithoutWindowPercent` | P `raw_headers_limit_reached_is_authoritative_without_percent`; N send-guard test uses the actual raw header callback |
| `TestCreditsRegressionProTwoAndSixPercentWithAbsentSecondary` | P `raw_headers_preserve_pro_absent_placeholder_and_future_reset_but_accept_real_exhaustion` |
| `TestCreditsRegressionPartialResponsePreservesWindowWithoutRenewingAge` | P `partial_observation_preserves_other_window_and_age`, `raw_stream_events_preserve_partial_evidence_and_ignore_model_specific_limits` |
| `TestCreditsRegressionAbsentPlaceholderDoesNotHideRealNewExhaustion` | P raw Pro placeholder test then replaces secondary with a real 100%-used observation |
| `TestCreditsRegressionFractionalQuotaAndResetSemantics` | P `fractional_weekly_only_absent_windows_and_reset`, `complete_refresh_without_reset_does_not_inherit_expired_partial_reset`; Native host adapter reads unrounded raw usage |
| `TestCreditsRegressionMergesOnlyNewerEvidence` | P `exhaustion_wins_equal_time_and_newer_positive_recovers`, `unrelated_observation_never_resurrects_exhaustion` |
| `TestCreditsRegressionRefreshesBeforeSelectionAndPreservesAffinity` | N `native_credits_actual_route_refreshed_capacity_keeps_affinity_until_exhaustion`; R shared refresh tests |
| `TestCreditsRegressionCoalescesConcurrentRefreshAndBacksOffFailure` | R `refresh_is_shared_survives_caller_cancellation_and_backs_off`, `timeout_and_failed_completion_are_bounded_and_back_off` |
| `TestCreditsRegressionCancellationDoesNotCancelOtherWaiters` | R `refresh_is_shared_survives_caller_cancellation_and_backs_off` |
| `TestCreditsRegressionUnknownPoolNeverEnablesCreditsAndFullDiagnostics` | N `native_credits_actual_route_mixed_unknown_exhausted_prioritizes_unconfirmed`, `native_credits_actual_route_keeps_full_four_account_diagnostics_after_runtime_cooldown` |
| `TestCreditsRegressionRefreshSupersedesOldHostCooldownNotRuntime429` | N `native_credits_snapshot_isolates_workspace_and_recovers_only_host_cooldown`, actual-route refreshed-capacity test and four-account runtime-cooldown test |
| `TestCreditsRegressionProbeOnlyReadsUsageWithBoundCredentials` | Native reuses `codex_quota::fetch_quota_for_credits` and `native_prepare_account`; the Credits and ordinary quota entries share `fetch_quota_impl` and the same transport. Static call-site review confirms the fixed usage GET and supplied account; U `quota_sync_rejects_an_upstream_identity_mismatch` and `agent_identity_quota_request_uses_common_headers_and_recovers_task_once` cover the shared authority/header path. No live probe is part of validation |
| `TestCreditsRegressionRefreshTimeoutAndFourRequestLimit` | R `timeout_and_failed_completion_are_bounded_and_back_off`, `account_refreshes_never_exceed_four_concurrent_reads` |
| `TestCreditsRegressionFailedRefreshRetriesAfterCooldown` | R `failed_refresh_recovers_after_backoff_without_renewing_old_evidence` |
| `TestCreditsRegressionPolicyErrorIsLocal503NotUpstream502` | N `native_credits_actual_route_mixed_unknown_exhausted_prioritizes_unconfirmed`; root Native HTTP admission-guard tests verify unsent rejection on the actual transport boundary |
| `TestCreditsRegressionExactHostSnapshotOverridesSameTimeIntegerCooldown` | N `native_credits_snapshot_isolates_workspace_and_recovers_only_host_cooldown`; raw host adapter uses exact `used_percent`; ordinary runtime/model cooldown is not cleared |
| `TestCreditsRegressionCachedExhaustionSurvivesUnrelatedPassiveHeaders` | P `raw_headers_limit_reached_is_authoritative_without_percent`, `unrelated_observation_never_resurrects_exhaustion` |
| `TestCreditsRegressionFullFourAccountDiagnosticsAfterInnerCooldownRejection` | N `native_credits_actual_route_keeps_full_four_account_diagnostics_after_runtime_cooldown`: two unconfirmed members and two runtime-cooled members, all four retained and none marked available |
| `TestCreditsRegressionProbeFailuresAreBoundedAndRedacted` | R timeout/failure tests; Native wrapper maps credential/usage failures to fixed local outcome codes and never includes token, response body or email in pool diagnostics. The shared upstream transport is not duplicated |
| `TestCreditsRegressionSuccessfulFailoverLogsWhyOtherAccountWasSkipped` | N actual-route refreshed-capacity/affinity test; final pool report retains the excluded member and post-upstream scheduling stages even when another account is selected |
| `TestCreditsIntegrationScopeRejectsBeforeRefreshAndKeepsDiagnostics` | N `native_credits_actual_route_rejects_revoked_key_before_quota_refresh`; authorization runs before loading/refreshing the pool |
| `TestCreditsIntegrationUsesCanonicalKeyAndImageScopes` | N complete-pool and independent-image-pool tests; U key-scope test; Credits scope intentionally retains paused members as quota evidence |
| `TestCreditsIntegrationFullScopedPoolStillGovernsRetryCandidates` | N `native_credits_complete_pool_preserves_paused_and_excluded_evidence` supplies only the remaining retry candidate while retaining the full original pool |
| `TestCreditsIntegrationPreservesPerKeyRotationAcrossQuotaFiltering` | U `rotation_is_scoped_by_caller_model_and_authorized_pool` plus N actual-route affinity test; local code filters candidates and retains upstream routing cursor implementation |
| `TestCreditsIntegrationImageSourcePreservesFullPool` | N `native_credits_image_pool_does_not_expand_to_conversation_catalog`; independent configured image IDs define the complete image policy pool |
| `TestCreditsIntegrationConcurrencyWaitUsesOnlyAdmittedAccounts` | N `native_credits_actual_route_concurrency_never_reintroduces_blocked_accounts_and_keeps_429`; the free slot on a rejected account cannot be used |
| `TestCreditsIntegrationConcurrencyRejectionKeeps429` | Same production-route concurrency test preserves upstream `account_busy` / 429 |
| `TestCreditsIntegrationKeepsCancellationErrors` | R cancellation test; Native route keeps upstream cancellation/drop behavior and does not wrap downstream errors in a Credits error |

Additional migration-specific tests cover official/local authority separation,
`rate_limit.limit_reached` in complete usage snapshots, raw SSE observations,
workspace changes, Native migration and rejection of restrictive Go selections.
The first migration pass missed raw limit-reached signals, absent placeholders,
initial partial-window uncertainty, full-refresh reset replacement and final
post-scheduler diagnostics. Those omissions were found by comparing the old
fixtures and repaired; policy-matrix success alone was not accepted as parity.

Intentional improvement over the old Go wrapper: a downstream reserve,
concurrency, model or cooldown error keeps its upstream status/code. A separate
full-pool diagnostic records Credits evidence and the actual rejection stages.
Credits itself returns `credits_quota_unconfirmed` when any blocked candidate is
unknown, otherwise `credits_policy_blocked`, both local 503 errors. Official
quota is never inferred from the source OAuth display. Under a restriction the
existing safe preflight fallback may use the source's separately verified local
OAuth authority; unsupported official-only paths remain explicitly blocked.

Verified on 2026-10-22: all 31 independent production-model, policy/parser and
refresh-controller tests passed offline. All 20 production Native Credits tests
passed inside the 913-test Local Access suite, and the shared usage-parser test
passed with the 16 quota tests. Native HTTP, image, Responses WebSocket, Realtime
and lifecycle admission tests passed with the Native API suite. Final credential
identity tests cover workspace, organization and Agent changes, token rotation,
later policy tightening, and Grok/API-key exemptions. App-bundle verification is
recorded separately in `native-engine-migration.md`; it is not inferred from
these source tests.
