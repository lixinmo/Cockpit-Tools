# Native provider model identity and client tools

## Problem and boundary

Codex can choose different tool protocols for a known model shell and an unknown
provider ID. In CLI 0.162.1, an unknown raw model without a managed catalog uses
full Responses and includes hosted `web_search`. The generic provider template
also sets `use_responses_lite=false`. Existing official shells may use Lite and
client-executed function/custom/namespace tools instead.

The Native Chat bridge implements client-executed tools, including declarations
in `input.additional_tools`. It does not implement OpenAI hosted web search, file
search, or hosted MCP. Rejecting those capabilities before dispatch is correct.
The old generic "no compatible account" error hid which capability caused that
rejection. A successful plain Chat request or a Go request that skipped an
unsupported declaration is not evidence of hosted-tool compatibility.

The protocol distinction follows [OpenAI Docs on tool search and additional
tools](https://developers.openai.com/api/docs/guides/tools-tool-search): client
tool discovery is explicitly separate from hosted discovery, and
`additional_tools` is a developer-role input declaration. The provider baseline
uses the function-call/result loop described in [StepFun's tool-calling
guide](https://platform.stepfun.com/cookbook/step-5-tool-calling).

## Change

- Native `/v1/models` and managed profile catalogs declare
  `use_responses_lite=true` only when at least one eligible account exists and
  every eligible account for that model uses the Chat wire protocol.
- Mixed Chat/Responses pools retain their existing protocol declaration, so
  hosted capabilities can still use an eligible Responses account. Unknown wire
  protocols, excluded accounts, and accounts outside the key scope cannot grant
  this declaration.
- `supports_search_tool` remains the separate client tool-discovery gate. The
  Chat bridge now adapts explicit client discovery, dynamic declarations, call
  history and restored output across JSON, SSE and WebSocket. Explicit capability
  refusals still win. It is not a web-search or hosted-MCP flag.
- Background profile maintenance applies the same capability gates before one
  catalog write. Unchanged maintenance does not repeatedly invalidate the model
  cache or restore template flags.
- Failed capability fallback exposes only allowlisted tool/history types,
  execution tags, capability results, and trigger/failure stages in
  `error.capability_diagnostics`. It does not capture names, schemas, arguments,
  messages, credentials, or account identifiers.

No request-time tool filtering, wider account grant, engine switch, or Credits
exception is introduced. An explicitly submitted hosted
`web_search`, including one inside `additional_tools` with Lite metadata, still
fails preflight without an upstream request. Lite is not an implementation of
hosted web search. A separately available client search function or connector
must provide that functionality.

## Verification and activation

Compilation and Rust checks run in the public repository's Cloud Build on
standard hosted runners. The selected Native regressions cover catalog and
profile consistency, mixed pools, permissions, maintenance cache stability,
explicit hosted-tool rejection, Chat function/custom conversion, and Credits
guards. Existing Native/Go retry and Credits/Auto Reset contract jobs remain.
The test invocation uses a package-scoped `opt-level=0` override for the large
`cockpit-tools` test crate, while retaining release dependencies and their target
cache. The separately built App keeps its normal release optimization. The
test driver is for protocol acceptance, not performance measurement.

For bounded real-provider acceptance, Cloud Build's optional `native_probe`
input packages the exact compiled test executable, checksums, and provenance in
a separate one-day artifact. No real-account test runs in Actions. The ignored
`native_provider_process_driver` can then run locally without compilation. It
requires a private configuration, reads one existing StepFun account without
migration, retains the saved Native/`prefer_regular` policy, limits models to the
saved catalog, and uses a temporary listener/history database. Acceptance uses
the original raw IDs and rejects any route that redirects them to another model.
Saved aliases are not mutated. Only allowlisted structural diagnostics are
written by the driver.
An existing auxiliary OAuth binding is retained as metadata, as in the production
provider builder. It neither changes the StepFun API-key authentication nor adds
the bound identity to the acceptance scope. The final route and send guard still
require the original StepFun account, model, Chat protocol, and HTTPS destination.

The optional `native_probe_only` mode rebuilds this test driver without producing
another App. It uses a single standard thin-macOS runner, runs the same Native
regression filters on the packaged executable, and retains frontend and contract
jobs. It has a separate concurrency group and cannot publish an App or satisfy
the formal release gate; that gate requires successful App build and archive
steps on the exact release commit.

The real acceptance checks must show Codex selecting a tool, executing a
local fixture read/edit/verify operation, returning its result, and receiving a
final answer for both `step-5-preview` and `step-3.7-flash`. A 200 response alone is not
sufficient. Explicit hosted web search must still fail with its actual capability
reason. Private acceptance configuration, history, and diagnostics stay local.

The new application must be restarted to load the Rust changes. Re-activate the
provider profile or refresh its managed model catalog, then reload the Codex
client so it consumes the updated metadata. A `.bak` catalog is not evidence of
the active client's configuration. Existing conversations carrying incompatible
hosted-tool history remain subject to the normal history and capability checks.

## Stable model identity and complete metadata

Native provider catalogs expose every allowed upstream model under its own ID.
There is no positional allocation from the three legacy GPT compatibility aliases
and no limit based on the number of shipped Codex models. Go reference behavior
is retained. Explicit user alias rules still apply; they are not expanded.

Existing automatic aliases are frozen once from an owned managed catalog or a
trusted runtime snapshot. The migration checks account/key/port ownership and
never uses `.bak` files or guesses from the current model ordering. Removed
models retain their recorded binding; unknown old bindings remain blocked until
the user chooses an unambiguous raw ID. Colliding identities fail explicitly.
A valid currently selected model survives provider refresh and reordering.

Metadata comes from the resolved upstream identity and eligible protocol, not
from the GPT-looking name of an old alias. The same complete producer drives the
Native HTTP catalog and the saved profile. Explicit account context limits and
saved profile context/reasoning settings remain authoritative. Unknown provider
models use the existing 128K client fallback, with no invented reasoning or
verbosity support; the public API does not claim that fallback as an upstream
limit. Known StepFun limits/efforts are scoped to the official endpoint, based on
its [Codex guide](https://platform.stepfun.com/docs/zh/guides/developer/integrations/codex).

Step 5 does not offer the built-in Chat web-search tool, per the official
[tool-call reference](https://platform.stepfun.ai/docs/en/api-reference/tool-call).
Real search acceptance uses the official same-plan
[Search MCP](https://platform.stepfun.com/docs/zh/guides/developer/integrations/search-mcp):
`https://api.stepfun.com/step_plan/v1/mcp/web_search/mcp`, with `web_search` and
`web_fetch` executed by the client. The gateway forwards their ordinary function
calls/results. It does not silently substitute the standard-billing search API
or claim hosted Responses search support. API credentials and search content
never enter the cloud build.

## Earlier negotiation-only validation record

Production App: `a741f403211e95f0fdffd04c40b56c7d766822c0`.
Successful full Cloud Build:
[39834828776](https://github.com/lixinmo/Cockpit-Tools/actions/runs/39834828776).

The first full run,
[39820677934](https://github.com/lixinmo/Cockpit-Tools/actions/runs/39820677934),
built the App successfully but reached the previous 90-minute job limit while
`rustc` was still compiling the optimized test binary. No main-library test had
started. The scoped test-package override above and a 120-minute job limit address
that build-time issue on the same standard runner.

Completed checks in the successful full run: frontend typecheck; 1,637 frontend
tests; 57 release-packaging tests; 9 cloud-build script tests; 97 main-library
Native regressions; Native setup retry (18 tests); Go
`CT-CONNECT-RETRY-001` v1.0 on linux/amd64; Credits policy (31 tests); Auto Reset
(71 tests). Cross-platform retry checks were not requested or needed.

Downloaded App provenance and archive checksums match that run and commit. Both
the main binary and Go sidecar are `x86_64`; strict deep signature verification
passes (ad-hoc signing, not an Apple notarization claim). SHA-256:

- Main binary: `8362803810f61a3ba39814fbe00dca086acdae9dcd726d4620729ca45ef1c0fe`.
- Go sidecar: `2c4aa4fb4091f9a65085ae9edcb86067b855aeedc3f067d66587d599e8c2e3a0`.
- App archive: `8fb69a916d8930e838a20f26e9ff8325fcc3b827246353d16c469bdc4cb6323f`.

The first local driver launch stopped before sending a request because its
test-only eligibility guard unnecessarily rejected auxiliary OAuth metadata.
Commit `2a3b89f49ad7350f2f8caf1bbc43825636a32749` fixes that driver guard and
preserves the metadata, with a synthetic production-route regression for original
credentials, account scope, Credits, and send-guard enforcement. Production Rust
code is unchanged relative to the validated App. Driver-only cloud validation
passed in [39846824595](https://github.com/lixinmo/Cockpit-Tools/actions/runs/39846824595):
98 Native main-library tests, including the new binding regression, plus the
same frontend, retry, Credits, and Auto Reset checks (10 cloud-build script
tests). Both thin Intel artifacts have verified checksums and ad-hoc signatures.
The formal-release gate correctly counts this driver-only run as zero successful
App builds.

Direct StepFun baseline: both `step-5-preview` and `step-3.7-flash` completed a
real function-call/result/final-answer loop (two HTTP 200 responses per model).
The function generated a random nonce only after receiving the model's call;
the final answer matched that nonce. This establishes ordinary function support,
not hosted-tool support. Only success/status summaries were retained.

### Real Codex acceptance (2026-10-23, Asia/Shanghai)

Codex CLI 0.162.1 used isolated ephemeral profiles and the cloud-built production
Native host implementation, with one read-only snapshot of the original StepFun
account. All five final cases passed:

| Case | Observed result |
| --- | --- |
| Explicit hosted `web_search`, Step 5 | HTTP 503; `unsupported=[web_search]`; trigger `preflight`, failure `candidate_selection`; no upstream dispatch. |
| Raw Step 5 without managed catalog | Real Codex request required `function`, `namespace`, and `web_search`; only `web_search` was unsupported; HTTP 503 before upstream dispatch. |
| Original Step 3.7 shell (`gpt-5.6-sol`) | Real local shell execution, successful Native turns, tool-result replay, and an exact final random-fixture match. Existing alias unchanged. |
| Raw Step 5 with served Native catalog | One successful shell read, two successful Native HTTP 200 turns, `function_call_output` replay, and exact final fixture match. |
| Raw Step 3.7 with served Native catalog | One successful shell read, two successful Native HTTP 200 turns, `function_call_output` replay, and exact final fixture match. |

Both comparison paths used the same observed client feature flags. The final
answer check selects the last agent message, excluding intermediate commentary.
No model mapping, account grant, saved binding, or Credits policy was changed.
The driver verified saved Native/`prefer_regular` settings and the rejection of
a switch to Sidecar. StepFun's ordinary API-key authentication remains outside
ChatGPT subscription Credits, through the existing production policy.

This establishes a reproduced trigger and a real client/tool/provider round trip;
the historical failed request body was not retained, so it does not reconstruct
every possible tool or history item from the original session. Acceptance uses
an independent bounded listener and a startup account snapshot, not the running
App's production listener or a test of live account revocation. Hosted MCP and
hosted web-search execution are not claimed to be supported by plain Chat.

The verified App was installed at `/Applications/Cockpit Tools.app` and restarted.
The new process was confirmed at that exact executable path, with two application
WebSocket Pong checks separated by sustained operation; installed binary hashes
match the provenance above. The version remains 1.3.73, so the build commit and
hashes identify this patch. Activate the StepFun profile/refresh its managed
catalog and reload Codex to consume the new metadata.

Post-install cleanup removed 1,393,952 KiB (about 1.33 GiB) of obsolete material;
those cleanup targets now occupy zero space:

- Two clean old source copies: 250,464 KiB. Their commit and history remain
  reachable from the retained local/upstream Git references.
- Renamed old App: 176,684 KiB; the new installed App remains in place.
- Downloaded cloud artifacts and extraction copies: 683,192 KiB.
- Synthetic acceptance profiles, diagnostics, fixtures, and temporary helpers:
  283,612 KiB. Private driver configuration was deleted with these directories.

No common project build-cache/dependency directories were present. Shared
toolchains, runtime accounts/configuration, source, Git history/remotes, and the
current App were retained. Saved Native/`prefer_regular` settings were confirmed
after restart and cleanup. No local application compilation was performed.

## Stable identity and client discovery validation (2026-10-23)

Production changes: `575b0b8f5e0324f050de226ca4acea88c3ec1e2b`.
Validated source, including corrected synthetic fixtures:
`318f61f7eceda04121744c638404680722836a78`.
Complete successful Cloud Build:
[39956666995](https://github.com/lixinmo/Cockpit-Tools/actions/runs/39956666995).

The two first runs ([full](https://github.com/lixinmo/Cockpit-Tools/actions/runs/39950734527),
[driver-only](https://github.com/lixinmo/Cockpit-Tools/actions/runs/39952898635))
compiled successfully but reported 219 passing and five failing tests. All five
stopped while deserializing newly added synthetic collections that lacked the
required `accountIds` field. The follow-up commit only supplies those fixture
account scopes; production permissions and validation were not relaxed. The
complete rerun passed all 224 selected main-library Native regressions.

Coverage includes raw model counts through 257 and binding allocation through
1,000; reorder/add/remove/restore; unresolved/conflicting aliases; per-key scope;
actual profile publication and repeated refresh preserving the selected model;
full profile/API metadata agreement and explicit reasoning/context overrides;
Chat client discovery over JSON/SSE/WebSocket with EOF and DONE endings; and
shared plan/Grok discovery regressions. Explicit hosted-tool rejection remains
covered. This is not a claim that every upstream model implements every tool.

Other successful checks: frontend typecheck and 1,637 frontend tests; 57 packaging
tests; 10 cloud-build script tests; Native retry 18; Go
`CT-CONNECT-RETRY-001` v1.0 on linux/amd64; Credits policy 31; Auto Reset 71.
All compilation occurred on standard GitHub-hosted runners; the personal App is
Intel-only, while the existing multi-platform build and release paths remain.

Downloaded App and probe provenance, SHA-256, x86_64 architecture and ad-hoc
signatures were verified. App version remains 1.3.73; use the commit and hashes
to identify this patched build. SHA-256:

- App archive: `7c810d1a258f50be19ab74ec0b0da15c48eaa5fa3bde70a293447a30131d8986`.
- Main binary: `d2d697d0ba22cc0f07e62087319562e63d6f96ee2ce19eade9137e97a637e6a5`.
- Go sidecar: `7ffd7afd4cbdb2d1712a8734ad0a7caa3d24c376bd656057ca6a71be14f998ae`.
- Native probe: `6b440eae1d136843f348b9394552cc2cd53b4aa745585e77efdfbbf5eb080b6d`.

### Real client acceptance and installation

Codex CLI 0.162.1 used the cloud-built Native host from the validated commit,
an independent bounded listener, and the original saved StepFun API-key account.
The profile was temporary and the upstream destination was restricted to the
original Step Plan endpoint. No saved account, binding, model mapping or client
configuration was changed. No `web_search="disabled"` override was used.

| Case | Verified result |
| --- | --- |
| Raw `step-5-preview` | Two shell operations and one actual `apply_patch`: read, edit only the fixture, read/verify, exact final random token; four successful Native turns, custom call/result and shell result replay. |
| Raw `step-3.7-flash` | Same complete read/edit/verify and exact final token checks; five successful Native turns. |
| Step 5 + official Step Plan Search MCP | Actual client discovery call/output, one successful search, one successful fetch, and final citation of the same official URL in that order; four successful Native turns, no failed tool, no shell/file change. |

The search harness initially lacked per-tool approval in its isolated
noninteractive client. A direct same-key MCP check succeeded; structural CLI
error diagnostics identified a local approval rejection. Only the temporary
child's two configured `web_search`/`web_fetch` tools then received
`approval_mode="approve"`; the read-only sandbox, credentials, account/model
scope, provider endpoint and production capability checks remained unchanged.
No global approval bypass or persistent client permission was added.

The test URL extractor was also corrected for Markdown backticks and Chinese
terminal punctuation. The final strict rerun checked only the actual fetch
`url`/`urls` fields, required search-before-fetch-before-final-answer order, and
matched one exact official source across all three stages. Its citation matched
even before formatting normalization. The final temporary harness SHA-256 was
`53a9a2a9737696c921a7e86be029ebcf5ed7fb6f6ed4102002acc1f47c531518`, recorded in
both overall and per-case safe summaries. None of these harness fixes changed
production Rust code or weakened the source/call/result assertions.

All real driver processes exited normally. They confirmed saved
`gatewayMode=native`, `creditsPolicy=prefer_regular`, preserved auxiliary OAuth
binding metadata, and the existing prohibition on switching to Sidecar. StepFun
API-key requests remained outside ChatGPT subscription Credits as before.
Search here is client-executed Step Plan MCP, not OpenAI hosted `web_search`;
the normal client must have a search connector configured and authorized.

The verified App was installed at `/Applications/Cockpit Tools.app` and restarted.
The new process at that exact path passed two application WebSocket Pong checks
five seconds apart. Installed main/sidecar hashes match the download above. The
old bundle was removed only after these checks; it occupied 176,516 KiB. Saved
Native/`prefer_regular` settings were rechecked after restart.

The user's current Codex configuration was not a StepFun/Cockpit gateway profile,
so it was not switched. On selecting the StepFun account again, Cockpit rebuilds
the managed catalog using this Native implementation; reload the Codex client to
consume it. No `.bak` file was used as runtime evidence. Existing conversations
with unsupported hosted-tool history continue to receive explicit validation
errors rather than silently losing tools. These real checks cover an independent
listener, not the GUI production listener's live account-revocation behavior.

Post-install cleanup removed 807,644 KiB of this round's downloaded artifacts,
probe binary, temporary client homes/fixtures, diagnostic summaries, helpers and
installation receipt directory. Together with the replaced old App, this round
removed 984,160 KiB (about 0.94 GiB); all targeted paths are absent. No project
`target/`, `node_modules/`, `dist/` or `src-tauri/target/` directories were present.
The installed App, runtime accounts/configuration, shared toolchains, source and
Git history/remotes were retained. No compilation ran on this Mac and no private
configuration, credentials or raw conversations were uploaded.

## Client configuration publication fix (2026-10-24, Asia/Shanghai)

The user's actual provider switch exposed a separate configuration failure:
Codex rejected `model_providers.codex_local_access.wire_api=chat_completions`.
The earlier bounded gateway acceptance supplied its own client provider options,
so it did not validate the account-page publication path. It must not be treated
as evidence that the old UI switch/edit/sync paths generated valid client config.

Root cause: ordinary account projection reused the upstream account's Chat
protocol, upstream address and API key for the Codex-facing provider. Full switch
first published that intermediate state before starting the gateway. Current
credential editing and provider synchronization could overwrite the gateway
without rebuilding it; catalog/header refresh could also rewrite the provider.

Commit `15f1ee99ceba5ef9a11c356c655c8c6de9140b4e` separates those responsibilities:

- Gateway accounts go through one asynchronous client-publication entrypoint.
  The existing gateway start/publish/rollback transaction completes before the
  switch updates its current-account index or an edit reports success.
- Credential edits retain ordered per-ID locks and old detail records until
  publication and index migration succeed. Current provider sync awaits the same
  publication path. Bound OAuth identities and existing Credits rules remain.
- Raw Chat account projection is refused before auth/config writes. It is not
  relabeled Responses while retaining an upstream Chat-only destination.
- Original gateway accounts cannot overwrite their published endpoint, protocol,
  local credential or catalog during catalog/header refresh. Instance preparation
  defers to its existing final ensure stage to avoid a second gateway rebuild.
- Saved StepFun upstream configuration remains Chat. The generated client profile
  uses a local gateway address, local credential and Responses.

Complete successful standard-hosted Intel Cloud Build:
[40001505539](https://github.com/lixinmo/Cockpit-Tools/actions/runs/40001505539).
Main-library regressions: 241 passed, zero failed; one synthetic profile exporter
was intentionally ignored in CI and run explicitly for local parser acceptance.
Frontend: typecheck and 1,637 tests; packaging 57; CI scripts 10; Native retry 18;
Go `CT-CONNECT-RETRY-001` v1.0 linux/amd64; Credits 31; Auto Reset 71 all passed.

Codex CLI 0.162.1 then parsed four complete profiles produced by the real backend
switch, credential-edit, provider-sync and catalog/header-refresh entrypoints.
All four were accepted with no manually supplied wire override. A copied negative
control changing only Responses back to Chat reproduced the exact parser failure.
The synthetic full-entrypoint test is distinct from the bounded real-account test.

The real-account driver now uses production `build_runtime_account`, account
bundle and Native catalog writers to create its whole isolated client profile.
Codex reads that profile without overriding wire, model or base URL. Both original
models completed actual read/`apply_patch`/verify/tool-result/final-token loops:
Step 5 had seven successful Native turns; Step 3.7 had five. CLI and driver exit
codes were zero, with no timeout or failed Native turn. Saved account files,
index, encryption key and Native configuration hashes were unchanged by acceptance.
The original routing snapshot keeps auxiliary OAuth binding metadata; its real
OAuth credentials are not copied or refreshed for the isolated test client.
The same Native/prefer_regular and Sidecar-switch protections were checked.
This does not claim an automated click inside the user's Codex GUI.

Verified artifact SHA-256:

- App archive: `11497d94bbf0743065c49852e22eddba37f75f739ca1d30ddd165ec431e4b3ac`.
- Main binary: `2e74cfbc0b187d2b73c00505275cc421e7c08fb5fe7b927c1b10887f06495ee4`.
- Go sidecar: `7a55d344e59c6419fe5fb64f04b644c844c639b14d3f933cd49109d7726a144f`.
- Native probe: `7977009ec9cc403fd3328c8b523520d4048d262a9fa5062b64d81d8503da4e4e`.
- Temporary acceptance script: `e77ae4baff8cacac1913cd98076ae40795c4b2605280a0b93367729d30771805`.

The verified App was installed at `/Applications/Cockpit Tools.app` and restarted.
Both binaries are x86_64 and pass signature verification; the new exact process
passed Ready/version and two Pong checks five seconds apart. The old 176,656 KiB
App was removed only after successful verification. Version remains 1.3.73; the
commit/hash identify this fix. The current user's default config remained valid
and was accepted by the installed Codex parser; it did not select the local
StepFun gateway, so no unrelated active account was switched. Reselect StepFun
in Cockpit to publish the fixed configuration; reload Codex if its old error
banner remains. No manual TOML edit is required.

All private acceptance roots were removed by the harness. After install,
280,592 KiB of probe/App downloads, safe summaries and task helpers were removed;
including the old App, this round removed 457,248 KiB (about 0.44 GiB). Targeted
paths are absent, no project build-cache/dependency directories were present,
and the verified App remains running. Runtime accounts/configuration, shared
toolchains, source and Git history/remotes were retained. No local compilation
or private-data upload occurred.
