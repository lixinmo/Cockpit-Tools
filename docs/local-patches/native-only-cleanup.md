# Maintained scope after Native-only cleanup

On 2026-11-09 the user additionally requested the narrow
[large-history project recovery fix](project-recovery-large-history.md).
The retirement and installation evidence below describes the preceding cleanup.

The user narrowed this fork on 2026-11-08 to three maintained additions:

1. Native same-account connection-setup retry, with the fixed three-attempt,
   200/400 ms, shared-deadline and no-post-send-replay contract.
2. Native OpenAI Credits admission, per-account permissions, quota evidence and
   final-identity send guards. This remains distinct from upstream API-key budgets.
3. Build tooling: local personal App builds by default (updated 2026-11-09),
   with optional manual standard-hosted cloud builds, platform selection,
   verified publication and rolling development downloads.

## Retired additions

- The Go engine, executable discovery/selector, bundled binary, Go toolchain jobs
  and Go-only verifier. The original reference implementation and earlier audit
  records remain in Git history.
- Local provider metadata discovery/cache, manual reasoning/context policies,
  fixed legacy model bindings and the extra atomic account/profile publication
  implementation. Upstream provider/account/catalog behavior is used directly.
- The additional Chat Lite/discovery advertisement, validation wrappers and
  related local extensions. Upstream supports ordinary third-party function,
  custom and namespace tool calls and has its own discovery conversion. A
  difference in advertised catalog flags alone does not prove ordinary tool
  calling is unsupported, so it is not a reason to keep an unverified patch.
- Local Auto Reset settings, worker, durable-barrier compatibility and migration
  code. Upstream Reset automation and its current settings/journal remain.
- Runtime tolerance of Go's exactly empty reasoning `encrypted_content` marker,
  after the existing local histories have been repaired once.

Go was originally the location of the Credits/retry implementations and an early
fallback during Native compatibility work. Those protections and tested provider
tool loops had already moved to Native in October. Its continued presence in the
previous integration followed the older maintenance rule, which the user has now
explicitly superseded.

## Private state cleanup boundaries

Only obsolete fork-owned fields/directories are removed. Account inventory,
credentials, upstream model lists, current Reset rules/journal, and Native
identity/continuation databases are preserved. In particular, a directory named
`codex_provider_gateway_sidecars` may contain active Native identity data and
must not be deleted wholesale.

The one-time history repair targets only reasoning items with an exactly empty
`encrypted_content` string. Nonempty ciphertext, whitespace strings, null,
messages, tool data, item IDs, timestamps and project assignments remain intact.
JSONL line byte lengths/offsets are retained, compressed rollouts use their
standard zstd representation, and publication respects Codex's thread writer
coordination locks. No whole-history backup or permanent repair service is added.
Private scans, tools and any migration report stay outside the public repository.

## Validation status

Source cleanup and the one-time private history/settings cleanup are complete.
The Native retry and Credits harnesses passed in standard GitHub-hosted run
[42499629901](https://github.com/lixinmo/Cockpit-Tools/actions/runs/42499629901).
Its Native integration run passed 2,757 tests, with 13 ignored, and exposed two
test-fixture issues: an uninitialized Credits migration flag in a persisted
snapshot, and a 25 ms image-header budget racing SQLite preparation. Those test
fixtures have been corrected without changing the production paths.

Both affected tests passed locally on 2026-11-09 (2 passed, 0 failed), using
synthetic fixtures and an isolated data directory. The frontend build and 42
local/build-workflow tests also passed.

The final Intel App was built locally from
`0939f36e82d191a801a991f55909617c0fc17c01`, installed and successfully launched:

- Version: `1.3.80`; one thin `x86_64` Mach-O executable; ad-hoc signature verified.
- No Go API executable in the App. The CLIProxyAPI license remains a legal resource.
- Main executable SHA-256:
  `dcedaba0e37bcee530e5fe141e14be272693728a051b7fad01f967d99dd71964`.
- Local account inventory/current selections, credential storage key, API keys,
  member selection and Credits policy passed before/after integrity checks.
- Retired cache/configuration fields remained absent after startup.
- Cold local test compilation took 21m 19s (the two tests ran in 0.10s);
  the optimized App build took 27m 34s. Local builds avoid cloud dispatch and
  publication overhead, but clearing build caches still makes subsequent builds
  cold; this run is not evidence that local compilation is inherently faster.

No live third-party inference request was sent as part of this verification.
No App from the failed cloud run was published. Real account data and the private
repair tooling are never part of the public build inputs. Build/install/cleanup
follow the current policy in [cloud-builds.md](../cloud-builds.md).

After successful installation and startup checks, project `target`,
`node_modules`, `dist`, build scratch, the previous App, 32 obsolete cloud
diagnostics and the private one-time repair workspace were removed (about
3.78 GiB). Source/Git history/remotes, all runtime data, shared toolchains and the
current installed App remain. No permanent history-repair tool or duplicate App
was retained.

## Upstream v1.3.81 integration (2026-11-12)

Merged upstream `7ca7f0db` in `eed11d12`. The upstream recovery order and manual
launch handling were adopted. The local large-history recovery, setup-only
retry and Credits admission remain required. The retry review found one
production setup-retry entry point. Subscription refresh now invalidates the
stored quota when it reconciles the workspace identity.

On App source `a3a6d60e`, 2,082 frontend tests, 18 Native setup-retry cases,
29 Credits cases and 41 isolated project-recovery cases passed. The isolated
recovery suite skipped its opt-in real-profile preview. The upstream notice
was tested with simulated newer releases. The current upstream and built App
both report `1.3.81`.

The local build verified a thin `x86_64` App with an ad-hoc signature and no Go
API executable. Its updater embeds the fork endpoints and its release notice
queries the upstream repository. Project build intermediates were cleaned.
The verified App is in `.local-app-output/` and awaits installation.

## Upstream v1.3.84 integration (2026-11-14)

Merged upstream `0c96cf74`, including the v1.3.82–v1.3.84 releases. The merge
uses upstream's large-state recovery, project-only repair, SSH/sidebar state
handling and transport liveness checks. The fork retains the deadline-bounded
scan for histories over 2,048 threads, the setup-only retry contract and the
Credits admission checks for the final credential identity. Both API Service
error surfaces use the upstream formatter, and the official release notice
uses the upstream release URL helper.

Local checks passed for the production frontend build, 2,267 frontend tests,
18 locale files, 130 build and publication tests, 18 Native setup-retry cases,
four isolated HTTP/2 liveness cases, 29 Credits policy cases and 66 isolated
project-recovery cases. The opt-in real-profile recovery preview was excluded.
The final Intel Rust test binary passed 106 Native integration regressions and
61 project-recovery and workspace-identity regressions in two filtered runs.
Credits admission uses the final subscription identity, including when the
routing and policy reads straddle an account update. Grok and API-key provider
credentials remain outside the subscription quota policy. This synchronization
validated source and tests; it did not bundle or install an App.

## Local Intel v1.3.84 installation (2026-11-15)

Built source `c2692ac92f1529d81100ef3daea8b2bb1a6eb2f2` with
`npm run build:app:local`. The optimized Rust build took 39m 33s. Version
1.3.84 contains one thin `x86_64` Mach-O executable, passes strict ad-hoc
signature verification, and contains no Go API executable. The main executable
SHA-256 was identical in the verified bundle and the installed App:
`974c857d838abe868c77565babee9efac0c39ed37ba3f3a16f911b64583099df`.

The replacement launched from `/Applications/Cockpit Tools.app` and rendered
the dashboard and its navigation controls. Before/after checks matched the
Codex account inventory and current selection, API Service credentials and
pool, port and enabled state, and Credits settings. The new SQLite account
index matched the previous JSON index.

After bundle verification the builder removed `target/`, `node_modules/`,
`dist/`, and the build scratch directory. These held at least 2.46 GiB before
packaging. After startup validation the old v1.3.81 App, the duplicate staged
App, and the generated Tauri schemas were removed, totaling about 284 MiB.

## Upstream v1.4.0 integration (2026-11-15)

Merged upstream `795e91f5`. The merge adopts the upstream project recovery
readers and targeted commit-time revalidation. The fork keeps the launch and
repair deadlines, SQLite interruption, and rollback path required by its project
recovery contract. The earlier fork-specific uncapped thread reader and
whole-history commit scan are superseded.

Upstream owns reset-card timing, including its new preference to wait for
usage Credits to run out. The fork still checks regular quota for the actual
subscription credential before dispatch. WebSocket history preloads and the
final generating request use that check while retaining the upstream replay
rules. The setup-only connection retry still wraps the same Native HTTP client.

The merge also uses upstream's Codex API Service, Cursor and MiniMax frontend
test fixtures in place of the equivalent v1.3.84 merge fixes.

On the Intel Mac, the production frontend build, 2,283 frontend tests, 130
build and publication tests, the locale check, and the Chromium platform sort
smoke test passed. Two runs of the App's Rust test binary passed 280 distinct
cases. They include Credits denial between WebSocket preload segments on both
new and reused sockets, DNS and IPv6 settings, launch-time recovery, and reset
card timing. The standalone retry, Credits and recovery suites passed 18, 29
and 65 tests respectively. The opt-in real-profile preview was excluded. This
merge validated source and tests. The installed App remains at v1.3.84.

## Local Intel v1.4.0 installation and Cloud diagnosis (2026-11-15)

Built source `08c88f6f7003b24c792a16cf841d6d1c7ae43a91` with
`npm run build:app:local`. Optimized Rust compilation took 50m 58s. The App
reports v1.4.0 and contains one thin `x86_64` Mach-O executable with a verified
ad-hoc signature and no Go API executable. The build and installed executables
have the same SHA-256:
`b755ac271d3f590c71b780cb9a8e47e866c93353e3a161bda78816487c95f077`.

The installed v1.4.0 process started from `/Applications/Cockpit Tools.app`.
The active Codex account index, selected account, API Service credentials and
pool, port and enabled state, Credits settings, and existing configuration keys
matched the pre-install checks. After the display was unlocked, the frontend
reported `react_mounted` and macOS exposed the WebView and its navigation
controls in the installed App window. The builder removed `target/`,
`node_modules/`, `dist/`, and its scratch after bundle verification. After the
startup check, the previous v1.3.84 bundle, the duplicate App, and generated
Tauri schemas were removed, totaling about 299 MiB.

The [previous Intel Cloud Build](https://github.com/lixinmo/Cockpit-Tools/actions/runs/43693997617/job/132751194305#step:12:7171)
used v1.3.84 and failed in Native gateway regressions before packaging. Four
tests expected outdated route-selection counts while the upstream request
counts remained within their budgets. The v1.4.0 fixtures check same-account
reselection and preserve the request-count and replay checks. All four passed
both alone and in a local run of the cloud workflow's Native filters.

With the same filters, the local Debug binary passed 3,014 cases, ignored 14
opt-in cases, and hit one timeout on the 65 MiB continuity `prepare` fixture.
The timeout reproduced individually. The prior cloud run passed that unchanged
fixture with release-profile dependencies. The workflow retains its normal
regression gate and the existing production operation deadline.

## Upstream v1.4.2 integration (2026-11-22)

The upstream baseline is `ccac8637`, following v1.4.1 and v1.4.2. The
fork uses upstream's cached system TLS roots, named account pools, minimum
remaining quota, model and price updates, and manual reset-card redemption.
The setup-only retry now attaches to the upstream TLS client. The fork also
shares one configured-member lookup between routing and Credits. A positive
quota refresh still respects the upstream pool's minimum remaining quota.
Immediately before sending, the Credits check uses the selected credential.

Local project recovery uses upstream's checks for ungrouped local chats.
Those chat IDs also participate in the saved-ID lookup when the full scan
runs out of time. Account switches reuse upstream's SSH identity helpers to
carry the selected host's reconnect preference into the next launch.

Formal releases use upstream's shared frontend build, Windows ARM64 installer
and standalone Linux CLI packaging. The fork keeps the same-commit release
gate and manually triggered personal cloud builds. Windows ARM64 is available
individually and in `all`. The release cache warm-up also requires a manual
run. The App checks the fork for updates and can show a separate notice for a
newer official version.
