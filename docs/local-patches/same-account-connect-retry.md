# CT-CONNECT-RETRY-001: same-account connection retry

> Current maintenance policy (2026-10-22): builds and compilation-dependent
> verification run in GitHub Actions; this Mac no longer compiles the app.
> Follow [cloud builds and public-fork synchronization](../cloud-builds.md).
> Local-build/cache-preservation instructions and the private-branch workflow
> below are historical records, superseded by that policy. The fixed v1.0
> runtime contract, acceptance checks, and duplicate-retry review remain binding.

## Patch identity

| Field | Value |
| --- | --- |
| Patch ID | `CT-CONNECT-RETRY-001` |
| Contract | v1.0, fixed |
| Original project baseline | commit `e6f73b3e1c21ffc60955415e4781dd111a5cee75`, Cockpit Tools v1.3.40 + cask-only commit |
| Latest verified upstream integration | commit `5330f75ae6b3c39fbd72f528ac2188207047d49a` (release tag `v1.3.73`); Go reference, Native retry and integration checks passed 2026-10-22 |
| Current native app build | v1.3.73 `.app` built and verified on x86_64 macOS, 2026-10-22; Rust Native migration included; not installed |
| Vendored CLIProxyAPI baseline | v7.2.155 (`7fac6b15`) with targeted v7.2.157 (`09a29bd3`) Codex/API alignment and Cockpit compatibility shims |
| Upstream tracking ref | `upstream/main` |
| Local integration branch | `local/ct-connect-retry-001` |
| Verification | `scripts/verify-cockpit-connect-retry.sh`; Native port: `scripts/verify-native-connect-retry.sh` |

This document is the durable specification for the local patch. “Fixed” means an
upstream refresh may adapt the implementation to surrounding refactors, but it
must not silently change the behavior below. A behavior change requires an
explicit contract-version update and a corresponding acceptance-test update.

The Go transport contract below remains the fixed reference. The requested Rust
Native port is documented separately in `native-connect-retry.md`, including its
scoped dependency hooks, TLS/DNS adaptation and native macOS acceptance. It does
not add a second setup loop to a Go or Native request.

## v1.0 behavior

The patch gives one already-selected account a small chance to recover from a
transient **connection-setup** failure before normal account failover sees the
error. It does not increase the number of accounts selected or retried.

The scope is the HTTP/HTTP-streaming Chrome-fingerprint uTLS transport selected
for HTTPS requests to `chatgpt.com`. It does not change account selection or
failover, WebSocket transports, the separate Anthropic/Claude transport, the
standard transport used for other hosts, or application-level response retry.

- The initial connection attempt and every retry use the same request context,
  host, address, selected account, and overall request budget.
- There are at most three connection attempts: one initial attempt plus two
  retries. The waits are exactly 200 ms and 400 ms, with no jitter.
- Retry ends immediately on success, cancellation, deadline expiry, a permanent
  error, or exhaustion of the three-attempt budget.
- Only DNS, proxy dial, TCP, TLS handshake, and HTTP/2 connection initialization
  happen inside this budget. Once HTTP/2 `RoundTrip` is invoked, the request may
  have been sent and must never be replayed by this patch.
- The budget is deliberately local and constant. v1.0 adds no UI, configuration
  key, environment variable, telemetry setting, persistence, or account-policy
  change.

The implementation belongs in the vendored nested module under
`sidecars/cockpit-cliproxy/third_party/CLIProxyAPI/internal/runtime/executor/helps`.
The production uTLS round tripper must call the retry helper directly; a helper
that is exercised only by tests does not satisfy this contract.

## Conservative error policy

Retry classification is a positive whitelist. Wrapping is inspected with Go's
`errors.Is`/`errors.As`, but a wrapper such as `*net.OpError` or `*url.Error` is
not itself proof that an error is transient.

Retryable connection-setup causes are:

- `io.EOF`, `io.ErrUnexpectedEOF`, `io.ErrClosedPipe`, and `net.ErrClosed`;
- a `*net.DNSError` that reports timeout or temporary failure, except that
  `IsNotFound`/NXDOMAIN anywhere in a wrapped or joined error tree always wins
  and is permanent; a concrete cause in `DNSError.UnwrapErr` remains eligible
  for the rest of this whitelist;
- `ECONNABORTED`, `ECONNREFUSED`, `ECONNRESET`, `EHOSTUNREACH`, `ENETUNREACH`,
  `EPIPE`, and `ETIMEDOUT` when exposed through the error chain, plus the
  corresponding Winsock and broken-network-pipe errors on Windows; and
- any concrete node in the wrapped/joined error tree whose timeout signal
  reports `Timeout() == true`, after the permanent exclusions above.

Everything else is non-retryable unless this contract is deliberately revised.
In particular, do not retry a canceled or expired context, NXDOMAIN, an unknown
error, a generic non-timeout `net.Error`, certificate/x509 or other permanent TLS
failures, an unsupported non-context dialer, or a proxy-configuration error.
When the request context has ended, its `context.Canceled` or
`context.DeadlineExceeded` result takes precedence over a concurrent connection
error.

## Request-body and connection ownership

Connection setup never consumes the request body. Intermediate connection
failures must not close it. If all setup attempts fail (or retry stops early),
the outer uTLS round tripper closes `req.Body` exactly once and returns the last
connection error unchanged; a body-close error is diagnostic only and must not
replace or wrap that connection error.

If a failed setup attempt returns a non-nil partial connection, that connection
is closed before continuing or returning. After setup succeeds, the HTTP/2
connection receives exactly one `RoundTrip` call. A round-trip error or an empty
response closes the connection. A successful response wraps its body so that
closing the response body also closes the dedicated HTTP/2 connection, and
repeated `Close` calls remain idempotent.

## Acceptance checklist (T01–T22)

The focused tests use one top-level Go test per item, named
`TestCockpitConnectRetry_TNN<Description>`. The verification script requires
every ID from T01 through T22 so deleting tests cannot produce a false green.

- [ ] **T01** — The first attempt succeeds: one connect call and no wait.
- [ ] **T02** — One transient failure followed by success makes two connect
  calls and waits exactly 200 ms.
- [ ] **T03** — Two transient failures followed by success make three connect
  calls and wait exactly 200 ms, then 400 ms.
- [ ] **T04** — Three transient failures exhaust the budget and return the exact
  third error with its original error chain intact.
- [ ] **T05** — A permanent first failure stops immediately with no wait.
- [ ] **T06** — A permanent second failure stops after the first 200 ms wait,
  with no third connect call.
- [ ] **T07** — An unknown error is permanent; a nil connection paired with a
  nil error is converted to the internal permanent setup error. Neither loops.
- [ ] **T08** — A context already canceled before entry performs no connection
  attempt and returns `context.Canceled`.
- [ ] **T09** — Cancellation caused by a connection attempt wins over that
  attempt's transient error and prevents a wait or another attempt.
- [ ] **T10** — Cancellation during backoff returns `context.Canceled` and
  performs no subsequent connection attempt.
- [ ] **T11** — Deadline expiry during backoff returns
  `context.DeadlineExceeded` and performs no subsequent connection attempt.
- [ ] **T12** — A temporary DNS failure is retryable.
- [ ] **T13** — A DNS timeout is retryable.
- [ ] **T14** — NXDOMAIN/`IsNotFound` is not retryable, even when timeout or
  temporary flags are also set or another joined DNS node is temporary.
- [ ] **T15** — Typed `io.EOF` and `io.ErrUnexpectedEOF` in the error chain are
  retryable; lookalike text without those typed causes is not.
- [ ] **T16** — Closed-pipe/closed-connection errors plus reset, aborted,
  refused, broken-pipe, timeout, host-unreachable, and network-unreachable
  syscall causes are on the retry whitelist.
- [ ] **T17** — A concrete error whose timeout signal is true is retryable, even
  behind a false-valued timeout wrapper or sibling; a generic non-timeout
  `net.Error` is not.
- [ ] **T18** — `%w`, `*net.OpError`, and `*url.Error` wrappers preserve a
  whitelisted inner cause (as does `DNSError.UnwrapErr`) but never grant
  retryability to an unknown inner cause merely because of the container type.
- [ ] **T19** — x509/certificate and other permanent TLS errors, unsupported
  non-context dialers, and proxy-configuration errors are not retried.
- [ ] **T20** — Every attempt uses the same context, host, and address and one
  shared three-attempt budget; connection setup preserves the same request,
  account headers, and open request body until the single send.
- [ ] **T21** — On final setup failure, the outer round tripper owns and closes
  `req.Body` exactly once; a body-close error does not replace or decorate the
  exact last connection error.
- [ ] **T22** — After setup, HTTP/2 `RoundTrip` is called exactly once and a
  post-send EOF is never reconnected or replayed. Partial/failed connections are
  closed (including a dialer returning a connection with an error), HTTP/2
  initialization is interruptible by the request context, and a nil response
  closes its connection. Closing a successful response body closes the body and
  dedicated HTTP/2 connection idempotently.

## Initial implementation verification

On 2026-09-06, the implementation was verified from the project baseline above
with an isolated Go 1.26.8 toolchain on Darwin/amd64:

- all T01–T22 focused tests passed;
- the focused race-detector pass and the complete vendored `helps` package
  passed;
- all packages in the outer `cockpit-cliproxy` module passed;
- the release-style `CGO_ENABLED=0` sidecar build passed without writing to the
  checked-out `bin` directory; and
- additional CGO-disabled sidecar cross-builds passed for Windows/amd64,
  Linux/amd64, and Darwin/arm64. The Windows version of the focused test package
  also compiled successfully.

`go vet` passed for the affected vendored package. The outer module retains a
pre-existing, unrelated context-cancel warning in `relay_execution.go`, so its
whole-module vet pass is not part of this patch's acceptance gate. No live
account, real ChatGPT request, installation, binary replacement, or process
restart was used in verification.

## Verified upstream integrations

### 2026-09-07 — Cockpit Tools v1.3.41

The selected release tag `v1.3.41` and fetched `upstream/main` both resolved to
`ceebe545abe627d6708e98ab9f5c1f6f4a4b8146`. Local `main` was fast-forwarded to
that commit, and the two private patch commits were rebased onto it without
conflicts. `git range-diff` reported both patch commits unchanged; the complete
vendored `helps` directory also remained byte-for-byte identical to the previous
local version. The pre-update branch is retained as
`backup/ct-connect-retry-001-pre-v1.3.41-20260907` at `2e81e884`.

The duplicate-retry review traced the outer relay, account/model dispatch and
OAuth refresh, stream-open and SDK bootstrap wrappers, Codex HTTP executor,
context-provided transports, usage-timing wrapper, host-selecting fallback,
proxy dialer, uTLS setup, and direct HTTP/2 `ClientConn.RoundTrip` call. v1.3.41
introduced no equivalent connection-setup retry or new request-body replay on
this path. Its account quota-cooldown filtering and client-identity header
changes do not add a setup retry loop. The production path retains exactly one
fixed setup loop and one HTTP/2 `RoundTrip` invocation after setup succeeds.
Existing higher-level account/model failover, OAuth refresh, stream-open and
SDK bootstrap retries are unchanged and remain outside this per-setup budget;
this patch does not impose an end-to-end request-attempt cap. The v1.0 contract
and T01–T22 acceptance tests therefore remain unchanged.

With the isolated Go 1.26.8 Darwin/amd64 toolchain,
`scripts/verify-cockpit-connect-retry.sh` passed all eight stages: local module
replacement validation, T01–T22 enumeration and execution, the focused race
pass, the complete vendored `helps` package, Linux/Windows test cross-compilation,
all outer sidecar packages, and the disposable CGO-disabled sidecar build.
`go vet` also passed for the affected vendored `helps` package.

The frontend production build (`npm run build`, including TypeScript checks and
version synchronization) passed. Nine related frontend test files passed all
47 tests using the existing Node v24.19.0 runtime, including the new statistics
range preference tests. Vite reported only a non-blocking chunk-size warning.
The Rust application check also passed:
`COCKPIT_SKIP_CLIPROXY_BUILD=1 cargo check --locked --release -p cockpit-tools --target x86_64-apple-darwin`.
It emitted non-blocking unused-code warnings. The existing x86_64, arm64, and
universal sidecar binaries were deliberately reused; their SHA-256 hashes were
unchanged before and after validation.

The final delta from the selected upstream baseline contains only the retained
local patch, its tests, and maintenance artifacts, including this integration
record. No live account or real ChatGPT request was used, and no installed
binary was replaced or process restarted.

### 2026-09-12 — Cockpit Tools v1.3.49

For the requested upstream refresh, fetched `upstream/main` resolved to
`95725f2ca74f7a4ae9445616f6a4f69c996ad5a7`, 47 commits beyond the previous
integration. The latest release, `v1.3.49` (`41dcebc4`), differs from that main
commit only in its Homebrew cask; the application source is identical. Local
`main` was fast-forwarded, and all three existing private commits were rebased
without conflicts. The original branch is retained as
`backup/ct-connect-retry-001-pre-v1.3.49-20260912` at `c2bf8edf`.
`git range-diff` confirmed the runtime patch and previous integration record
unchanged; the maintenance guard only moved with its surrounding `UPSTREAM.md`
context. All local `utls*` implementation and acceptance-test files remain
byte-for-byte identical to the pre-update branch.

The bundled dependency baseline is now CLIProxyAPI v7.2.155 (`7fac6b15`), with
targeted Codex/API policy alignment from v7.2.157 (`09a29bd3`) and Cockpit
compatibility shims. This is not a full v7.2.157 dependency-tree upgrade.

The duplicate-retry review traced the outer relay, runtime/service,
account/model dispatch, OAuth refresh, stream bootstrap, Codex HTTP executor,
context transport, usage wrapper, host fallback, proxy dialer, uTLS setup, and
direct HTTP/2 call. Upstream introduced no equivalent setup retry. The
production round tripper still calls `createConnectionWithRetry` directly and
then invokes `h2Conn.RoundTrip` exactly once. The transport, usage wrapper,
proxy utilities, outer runtime auth, and relay execution did not change in
this upstream interval. CT-CONNECT-RETRY-001 v1.0 and T01–T22 remain unchanged.

Upstream did change application-level failure classification: bootstrap
overload detection now also recognizes model capacity and `server_error`
responses whose message contains `you can retry your request`. These can enter
the existing account/model failover path after an upstream response. The new
zero-token empty-incomplete error is request-scoped and therefore stops default
model/account/round retries.
Existing configurable request-scoped continuation remains an application
policy. The SDK HTTP bootstrap retry wrapper is unchanged, defaults to off,
and is not an additional wrapper on the current outer-relay HTTP Responses
route. These policies are retained explicitly; they do not add a second
connection-setup loop or move request replay into the local patch. The fixed
three-attempt setup budget remains per transport call, not an end-to-end
request-attempt cap.

One unrelated upstream test expectation was stale: v1.3.49 changed the
DeepSeek startup fallback to `deepseek-flash`, while
`codexDeepSeekAccess.test.ts` still expected `deepseek-v4-flash`. The local
integration updates that single assertion to the upstream runtime behavior.

Using an isolated, official-checksum-verified Go 1.26.8 Darwin/amd64 toolchain,
`scripts/verify-cockpit-connect-retry.sh` passed all eight stages: local module
replacement validation, T01–T22 enumeration and execution, the focused race
pass, the complete vendored `helps` package, Linux/Windows test
cross-compilation, all outer sidecar packages, and the disposable CGO-disabled
sidecar build. With Node v24.19.0, all 207 TypeScript regression tests passed
after the assertion correction. All 13 release-script tests and the locale
check (18 files, 5,827 keys each) also passed.

`go vet` passed for the affected vendored `helps` package. The Rust core
regression command
`cargo test --locked --release --target x86_64-apple-darwin -p cockpit-core -- --test-threads=1`
passed 124 tests; the one manual local-account repair smoke test remained
ignored as intended. The frontend production build, including TypeScript
checks and version synchronization, passed with only Vite's non-blocking
chunk-size warning.

The complete macOS release build passed with Go 1.26.8, Node v24.19.0, and
Rust 1.98.0, using:

```sh
COCKPIT_SKIP_CLIPROXY_BUILD=0 npm run tauri -- build --ci \
  --target x86_64-apple-darwin --bundles app \
  --config '{"bundle":{"createUpdaterArtifacts":false}}' -- --locked
```

The normal build script regenerated the x86_64, arm64, and universal macOS
sidecars from the integrated source. Tauri produced exactly one bundle:
`target/x86_64-apple-darwin/release/bundle/macos/Cockpit Tools.app` (135 MiB).
Artifact inspection confirmed version 1.3.49, x86_64 main and sidecar
executables, all 17 configured resources, and a byte-identical bundled copy
of the newly built x86_64 sidecar. No DMG or updater archive was generated;
the existing v1.3.40 DMG retained its original size and modification time.
The release build emitted non-blocking unused-code warnings. The application
was not installed or launched, and no running process was replaced or restarted.

The final delta from `upstream/main` consists of the retained local connection
retry patch and tests, its maintenance artifacts, and the single corrected
DeepSeek test assertion. No runtime compatibility adjustment to the local
retry patch was necessary.

### 2026-09-17 — Cockpit Tools v1.3.56

Fetched `upstream/main` and release tag `v1.3.56` both resolve to
`cc8ea8994753ad8508802b84a5e8b28a2aafa8d7`, 16 commits beyond the previous
baseline. Local `main` was fast-forwarded, and the private patch branch was
rebased onto it. The original branch is retained as
`backup/ct-connect-retry-001-pre-v1.3.56-20260917` at `a5435f31`.
Upstream now contains the DeepSeek fallback assertion correction, with an
explanatory comment; its redundant local commit was dropped when resolving
the rebase conflict. All six remaining commits are unchanged according to
`git range-diff`. The complete local `utls*` implementation and acceptance
tests remain byte-for-byte identical to the pre-update branch.

The duplicate-retry review traced the outer relay and automatic routing,
runtime/service, account/model dispatch, OAuth and Agent Identity refresh,
stream-open and SDK bootstrap wrappers, Codex HTTP executor, context-provided
transport, usage wrapper, host selection, proxy dialer, uTLS setup, and direct
HTTP/2 `RoundTrip`. The vendored helpers, auth/service dispatch, SDK handlers,
proxy utilities, and Agent Identity wrapper have no upstream changes in this
interval. The production uTLS path still has exactly one setup retry helper,
followed by one HTTP/2 send; v1.0 and T01–T22 are unchanged.

The new automatic model routing is a separate application-level policy gated
by `ModelRouting.Automatic`. It can move from a failed native request to
configured provider candidates on eligible HTTP errors before downstream
output starts. Each provider candidate is removed after selection, and output,
cancellation, and non-retryable statuses stop further routing. This policy is
retained as upstream behavior outside the local transport budget; it is not a
second connection-setup loop. The three-attempt limit remains per setup call,
not a total request-attempt cap. The stream-open change releases its attempt
context at stream completion without adding an attempt. New DeepSeek reasoning
history restoration and Codex call-ID normalization transform request payloads
before sending; they do not add a transport retry or resend after `RoundTrip`.

`scripts/verify-cockpit-connect-retry.sh --native-only` passed on Darwin/amd64
with the cached Go 1.26.8 toolchain: T01–T22, the focused race pass, the complete
affected package, outer sidecar tests, and the disposable native sidecar build.
Linux/Windows compilation was explicitly skipped. All 13 focused DeepSeek
tests passed. The local build's single TypeScript check and Vite production
build passed, with only Vite's non-blocking chunk-size warning. No full release
matrix or additional Rust test build was run.

`npm run build:app:local` succeeded using Node v24.19.0, Go 1.26.8, and Rust
1.98.0. Existing dependencies and native caches were reused. The Rust release
stage finished in 11m 56s with non-blocking unused-code warnings. It produced
one 136 MiB bundle at
`target/x86_64-apple-darwin/release/bundle/macos/Cockpit Tools.app`.
Artifact inspection confirmed version 1.3.56, exclusively x86_64 main and
sidecar executables, all 17 configured resources, and identical SHA-256 hashes
for the bundled sidecar and the newly built native sidecar. The sidecar output
directory contains only the native binary. No DMG, updater archive,
Universal/arm64 sidecar, or other-platform build was generated. The application
was not installed or launched. The final upstream delta contains only the
retained retry patch and its maintenance/native-build workflow artifacts.

### 2026-09-18 — Cockpit Tools v1.3.57

Fetched `upstream/main` resolved to
`ab8ef7f21dc3a18707dc017a6c06157080afeff6`, five commits beyond the previous
baseline. Release tag `v1.3.57` is `3a83b85a`; subsequent commits only update
the Homebrew cask and retract an announcement. Local `main` was fast-forwarded,
and all seven private commits were rebased without conflicts. `git range-diff`
reported each retained commit unchanged. The pre-update branch is preserved
as `backup/ct-connect-retry-001-pre-v1.3.57-20260918` at `e034e03c`.
All local `utls*` implementation and acceptance-test files remain byte-for-byte
identical to that backup.

The duplicate-retry review followed the outer relay/provider routing,
runtime/service and auth dispatch, existing stream-open/bootstrap and identity
refresh wrappers, Codex HTTP executor, context transport, usage wrapper,
host-selecting transport, proxy dialer, uTLS setup, and direct HTTP/2 send.
The Codex executors, vendored helpers, SDK auth/service/handler layers, proxy
utilities, and outer stream-open loop are unchanged in this interval.
The production path still calls the single `createConnectionWithRetry` helper
and then invokes HTTP/2 `RoundTrip` once. No equivalent setup retry or new
ChatGPT transport replay was introduced; v1.0 and T01–T22 remain unchanged.

Upstream adds xAI/Grok HTTP executor integration and provider/model dispatch,
while retaining the existing automatic-routing failover loop. These are
application-level provider choices outside the local ChatGPT setup budget.
The new moderation-notice handlers format an existing error as a local
JSON/SSE response rather than resend the upstream request. The Ollama bridge
uses its own HTTP request path and does not wrap the patched ChatGPT transport.
The three-attempt limit remains per connection setup, not an end-to-end cap.

`scripts/verify-cockpit-connect-retry.sh --native-only` passed on Darwin/amd64
using the cached Go 1.26.8 toolchain: T01–T22, the focused race pass, full
affected package, outer sidecar tests, and disposable native sidecar build.
Linux/Windows compilation was explicitly skipped. The local app command's
single TypeScript check and Vite production build passed; Vite emitted only
its non-blocking chunk-size warning. No dependency reinstall, full release
matrix, or separate Rust test build was needed.

`npm run build:app:local` completed successfully with the existing Node
v24.19.0, Go 1.26.8, and Rust 1.98.0 toolchains. The Rust release stage took
10m 45s and emitted non-blocking unused-code warnings. Tauri produced one
137 MiB bundle at
`target/x86_64-apple-darwin/release/bundle/macos/Cockpit Tools.app`.
Inspection confirmed version 1.3.57, x86_64-only main and sidecar binaries,
all 17 configured resources, and a bundled sidecar hash identical to the newly
built native sidecar. Only the native sidecar is present in its output directory.
No DMG, updater archive, other-architecture sidecar, or other-platform build
was produced. The application was not installed or launched. The final local
delta consists solely of the retained retry patch and maintenance/native-build
workflow artifacts.

### 2026-10-01 — Cockpit Tools v1.3.64

Fetched `upstream/main` and release tag `v1.3.64` resolve to
`4434c32e7bb02f33246941eb5179b0252a93481a`. Local `main` was fast-forwarded,
and all eight private patch/maintenance commits were rebased onto it. The
original branch is retained as
`backup/ct-connect-retry-001-pre-v1.3.64-20261001` at `87bd2dc7`.
The sole conflict was the `utlsRoundTripper` fields: the resolution retains
upstream's `proxyURL` route metadata alongside the local `connectFn` and
`waitFn` test seams. `git range-diff` shows the runtime patch adapted only to
that upstream context, the maintenance guard moved with its `UPSTREAM.md`
context, and the other six commits unchanged. The retry helper, error policy,
delays, ownership behavior, and T01–T22 acceptance tests are unchanged.

The duplicate-retry review traced the outer relay and automatic model routing,
runtime/service delegation, account/model dispatch and OAuth refresh,
stream-open and SDK bootstrap wrappers, Agent Identity wrapper, Codex HTTP
executor, context-provided transports, usage-timing wrapper, host fallback,
proxy dialer, uTLS setup, and direct HTTP/2 `ClientConn.RoundTrip`. Upstream
added no equivalent setup-only retry and no replay inside these transport
wrappers. The production uTLS path retains exactly one
`createConnectionWithRetry` loop and invokes HTTP/2 `RoundTrip` once after
setup. Upstream's new `GetConn`/`GotConn` hooks and `proxyRouteConn` annotation
remain intact. The route observer's bounded queries are asynchronous side
channel requests to the proxy controller; they do not resend the Codex body.
The new auth-file read retries likewise retry local file reads only.

Upstream **did change higher-level request retry policy**:
`shouldRetryAfterErrorWithAttempted` now treats response-less connection
lifecycle errors as eligible for the existing bounded `request-retry` rounds.
Its classifier is intentionally broader than this patch's typed setup-error
whitelist (including DNS failures and lifecycle message matches); it also
covers ambiguous errors returned after a request may have been sent. The
review retains that upstream application policy without moving it into or
duplicating it around the setup helper. It can reexecute an entire request,
including selecting the same account again, subject to configured retry-round
and credential limits. Cancellation, request-stop, and request-scoped errors
still stop it. Therefore the fixed three-attempt limit remains **per transport
setup call**, not an end-to-end request cap or an end-to-end no-replay promise.
It would be incorrect to claim that higher-level retry behavior is unchanged.
The separate stream timeout error now uses `server_error`, which also allows
client-side retry; the stream-open, SDK bootstrap, and Agent Identity replay
mechanisms themselves have no changes in this upstream interval.

Using the retained native Go 1.26.8 toolchain,
`scripts/verify-cockpit-connect-retry.sh` passed on Darwin/amd64: local module
replacement validation, all T01–T22 tests, focused race detection, the complete
vendored `helps` package (including upstream proxy-route tests), every outer
sidecar package, and the disposable CGO-disabled native build. Cross-platform
compilation was skipped. Three focused upstream auth regression tests also
passed: `TestTransportRetryRemainsBoundedAndRequestScoped`,
`TestExecuteTransportFailureRetriesWithoutPoolOutage`, and
`TestTriedExhaustionOnlySuppressesHealthyPoolDiagnostics`. These use mock
executors and verify the new round policy's bounds and cancellation behavior.
No real account or live ChatGPT request was used in this verification, and no
binary was installed or running process restarted.

The subsequent `npm run build:app:local` also passed, including the separately
requested Auto Consume Reset addition documented in
[`auto-consume-reset.md`](auto-consume-reset.md). It produced one 145.8 MiB
v1.3.64 `.app` with exclusively x86_64 main/sidecar executables and a bundled
sidecar hash matching the newly built native sidecar. No DMG, updater archive,
Universal/arm64 sidecar, or other-platform artifact was generated. The app was
not installed or launched; the local delta now also includes that feature and
its offline tests.

### 2026-10-02 — Cockpit Tools v1.3.65

Fetched `upstream/main` resolved to `2d0f13f9`, three commits beyond the
v1.3.64 baseline; release tag `v1.3.65` is `0b6514b4`. Local `main` was
fast-forwarded and all eight local commits were rebased without conflicts.
`git range-diff` reports every retained commit unchanged. The previous branch
is retained as `backup/ct-connect-retry-001-pre-v1.3.65-20261002`.
The uncommitted per-account Auto Consume Reset feature was restored without
conflicts and remains uncommitted. Its pre-integration snapshot, including
untracked source and tests, is retained in stash commit
`54539d7f693a4406285f6df20299e5e38ada3845`.

Duplicate-retry review confirmed that the entire sidecar tree is unchanged
from the previous verified integration. This includes outer relay and routing,
runtime/auth dispatch, stream-open/bootstrap and Agent Identity refresh
wrappers, Codex HTTP executors, context transports, usage timing, host selection,
proxy dialers, uTLS setup, and direct HTTP/2 send. The production transport
still calls one `createConnectionWithRetry` loop and then one HTTP/2
`RoundTrip`; no equivalent setup retry or new replay was introduced. Existing
application-level retries remain outside the fixed per-setup budget. The v1.0
contract and T01–T22 are unchanged.

The default native-only `scripts/verify-cockpit-connect-retry.sh` passed on
Darwin/amd64 with cached Go 1.26.8 in 161.52 seconds: T01–T22, race detection,
the full affected package, outer sidecar tests, and a disposable native build.
All 26 focused frontend tests passed, covering Auto Consume Reset settings and
the upstream plan badge, account type, coalesced writer, profile refresh, and
serial refresh queue changes. The local build's single TypeScript check and
Vite build passed. No dependency reinstall or separate Rust test build was run.

`npm run build:app:local` succeeded with Node 20.19.5 and the retained native
Go/Rust toolchains. Wall time was 925.26 seconds (15m 25s), including the Rust
release stage of 13m 32s. The retry verifier ran concurrently, so its time is
not additive. Existing dependency and compilation caches were preserved.
The build generated exactly one 146.7 MiB v1.3.65 bundle at
`target/x86_64-apple-darwin/release/bundle/macos/Cockpit Tools.app`.
Both executables are exclusively x86_64, and the bundled sidecar is byte-identical
to the freshly built native sidecar. No DMG, updater archive, Universal or
foreign-architecture output was built. Only non-blocking unused/deprecated-code
and Vite chunk-size warnings were emitted. The application was not installed
or launched, and no live account or Reset consumption was used for validation.

Main executable SHA-256:
`1930f248e7742becca0827b06e66d0769ad703c79e6d153c47a1b537ce0b0d8f`.
Native/bundled sidecar SHA-256:
`ed3030c8b69b8c058235087f28bea298998ff006648fe0dde9e9fd124a299359`.

### 2026-10-08 — Cockpit Tools v1.3.66

The user selected release tag `v1.3.66`, whose peeled commit is
`5273335ef9a96611336c4b927759c125a0f092d7`. Local `main` was fast-forwarded
to fetched `upstream/main` (`be24ce61`, only a Homebrew cask change beyond
the release), while the local integration branch was rebased onto the selected
release itself. All eight private commits are unchanged according to
`git range-diff`. The pre-update branch is retained as
`codex/backup-ct-connect-retry-001-pre-v1.3.66-20261008` at `67a52a90`.
All pre-existing uncommitted changes, including untracked source/tests and
the native incremental build configuration, are recoverable from retained
stash commit `b3ed263869f39191a845cc1857c311f3402f62ae`. They remain uncommitted
after integration; no user changes were silently included in a new commit.

Eight files required conflict resolution when restoring those changes. The
integration preserves both upstream request diagnostics/image-model fields and
the local Credits policy, upstream quota-threshold translations and local
Credits translations, and the new request-log commands alongside the local
Credits command. Credits error recognition now lives in upstream's extracted
pool-diagnostics module. The selector keeps authorization scope outermost,
then concurrency, Credits policy, and quota cooldown. Credits pool lookup now
uses upstream's canonical scope helper so whitespace/duplicate account IDs and
image scopes agree with authorization; full-pool quota preference and per-key
rotation remain intact. Auto Reset's offline harness imports the new upstream
model dependency; its production policy and background worker are retained.
The local `utls*` implementation and acceptance tests remain byte-for-byte
identical to the pre-update branch.

The duplicate-retry review traced the outer relay and automatic routing,
runtime service, account/model dispatch and retry rounds, OAuth refresh,
stream-open and SDK bootstrap wrappers, Codex HTTP executor, context-provided
transports, usage/diagnostic wrapper, host-selecting fallback, proxy dialer,
uTLS setup, and direct HTTP/2 call. v1.3.66 adds no equivalent connection-setup
retry and no new transport-level request replay. The production path retains
one fixed setup loop and exactly one HTTP/2 `RoundTrip` after setup succeeds;
the v1.0 error policy, context budget, and ownership semantics remain intact.

Existing higher-level retry loops are unchanged, but their application policy
is not entirely unchanged: with Codex stream-bootstrap buffering enabled,
structured quota and disabled-account failures received before output now
become credential-scoped 429/403 errors eligible for existing failover, while
specific input faults take precedence. Buffering is bounded by 16 events and
64 KiB. These application-level resubmissions occur after an upstream request
was sent and remain outside the per-setup three-attempt budget. Credential
version/registration guards protect replacement credentials from stale results;
provider-gateway Retry-After admission waits or rejects later requests without
adding same-call send retries. The new audio route uses one existing
`HttpRequest` invocation and inherits the same host-selected transport without
introducing a second retry loop.

All 938 frontend tests, 62 native offline Auto Reset tests, and four native
offline Credits model/configuration tests passed. The Credits tests include
coexistence with v1.3.66 request-payload logging and image main-model settings.
The outer Go integration tests additionally cover scope-before-refresh,
canonical conversation/image scopes, full-pool preference under narrowed retry
candidates, and per-key rotation through quota filtering/recovery.
The focused Credits/concurrency/scope/rotation suite and its Credits/scope/
rotation race-detector pass both passed.

The default native-only `scripts/verify-cockpit-connect-retry.sh` passed on
Darwin/amd64 with the retained Go 1.26.8 toolchain: local module replacement,
T01–T22 enumeration and execution, focused race detection, the complete
vendored `helps` package, every outer sidecar package, and the disposable
CGO-disabled native sidecar build. Cross-platform compilation was skipped.

The locale checker reports the pre-existing missing Credits translation keys
in 14 languages; the same keys are absent in the retained pre-update snapshot.
All 18 locale files parse, required static source keys have definitions, and
the existing English/Chinese Credits translations and other-language fallbacks
are preserved. This is not a clean locale-check result.

`npm run build:app:local` passed with Node 20.19.5, Go 1.26.8, and Rust 1.98.0.
The single incremental TypeScript check and Vite production build passed;
total build time was 896.4 seconds (14m 56s), including 12m 04s in Cargo.
Existing native compilation caches and `target/local-app-cache` were preserved,
with the explicit local package optimization profile and no dependency reinstall.
Only non-blocking unused/deprecated-code, documentation, and Vite chunk-size
warnings were emitted. No production/configuration source changed during the
build.

The build produced exactly one 167.3 MiB v1.3.66 bundle at
`target/x86_64-apple-darwin/release/bundle/macos/Cockpit Tools.app`.
Both version fields are 1.3.66, both main and sidecar executables are exclusively
x86_64, all 17 configured resources match their sources, and the bundled sidecar
is byte-identical to the freshly built native sidecar. The sidecar output
directory contains only the native binary. No DMG, updater archive,
Universal/other-architecture sidecar, or other-platform artifact was generated.
No application was installed, launched, or restarted; tests did not use real
account quota, Credits, or Reset consumption.

Main executable SHA-256:
`e3e33e6e724027e4d4981ef3f1eeb19ffbed1d5d9374a7aeeba5a1c89cf0e757`.
Native/bundled sidecar SHA-256:
`b892bdf258f42a64443e52771b3508c2755dfb133c3dca544571c1e07ed80311`.
Logs are retained under `target/local-app-cache/upstream-v1.3.66-*.log`,
and Cargo timing reports remain in `target/cargo-timings`.

### 2026-10-13 — Cockpit Tools v1.3.68

The user selected release tag `v1.3.68`, whose peeled commit is
`af5e90bc12151945e0f9c2058f849471c4ed529a`. Local `main` was fast-forwarded
to `upstream/main` (`915e7a20`, only a Homebrew cask update beyond the release),
while the integration branch was rebased onto the selected release itself.
All eight private commits are unchanged according to `git range-diff`.
The pre-update branch is retained as
`codex/backup-ct-connect-retry-001-pre-v1.3.68-20261013` at `b8eac197`.
The complete pre-update dirty tree, including untracked sources, tests and
native build configuration, is recoverable from retained stash commit
`182fb74a76decfb84a318818230f3986abe62578`. Existing local changes remain
uncommitted, including Auto Reset, Credits policy and plaintext collaboration
response metadata. No account settings or saved conversation history were
modified during integration.

Three files conflicted during stash restoration. The resolutions preserve
the upstream image-source manifest fields alongside local Credits fields,
safe Retry-After/cancellation handling alongside explicit local policy errors,
and proxy-preserving account saves alongside Auto Reset freshness and durable
consumption barriers. The local retry implementation and original T01–T22
test files are byte-identical to the old branch; upstream adds its own uTLS
integration tests without replacing or nesting the fixed setup helper.

The new image-source selection narrows executor candidates before scheduling.
Credits decisions now retain the original authorized image pool separately:
disabled, unavailable or excluded accounts do not falsely count as exhausted
regular quota, while executor authorization remains narrowed. Integration
testing also exposed pre-existing masking of concurrency/cancellation errors
by the local Credits wrapper. The narrow correction keeps authorization
outermost and admits Credits-eligible candidates before concurrency queueing;
blocked accounts cannot acquire a slot, valid accounts can wait, and the
original concurrency 429 and cancellation errors are preserved. Quota evidence,
refresh limits, three Credits policies and the connection retry budget are
unchanged. Four new regression cases failed before these adjustments and
passed afterward, together with the focused scope/concurrency/rotation/image/
service-tier/error suite and its Credits/scope/concurrency/rotation race pass.

The duplicate-retry review traced outer relay/runtime service and automatic
routing, account/model rounds and OAuth refresh, stream-open and SDK bootstrap,
Codex executors, context/usage transports, host fallback, proxy dialing, uTLS
setup and direct HTTP/2 send. No equivalent connection-setup loop was added.
The local production transport still invokes one fixed setup retry helper and
one HTTP/2 `RoundTrip` after successful setup; v1.0 remains unchanged.

Upstream **does add application-level request-body replay** for rejected
encrypted reasoning/compaction. This is explicitly retained as release behavior,
not folded into the setup retry helper: OAuth non-compat requests may perform
one additional `client.Do` only after an actual HTTP 400 with the specific
encrypted-content rejection classification and removable rejected items.
The client, account, headers and context are retained; the first response is
closed, unmatched content is preserved, and second rejection/cancellation
stops recovery. It does not recover transport errors, HTTP 200/SSE failures,
or responses already delivered downstream, and it does not strip agent-message
content. Bounded digest state is scoped to credential, endpoint, model and
session. The three-attempt contract is per transport connection setup, not
an end-to-end cap or no-replay promise. Targeted executor/helps regressions
cover these gates and limits and passed. The image API path has no automatic
generation replay; WebSocket interruption stays outside this HTTP setup patch.

The plaintext collaboration patch auto-merged without runtime changes.
Its final-boundary annotation still precedes output caching/reconstruction.
Expanded tests cover `response.incomplete`, upstream/local interrupted output,
explicitly empty output, continuation tool inheritance and unchanged argument
text. Both complete required vendored packages and focused race tests passed;
the earlier TCP-bind validation limitation did not recur in this run. See
[`plaintext-collaboration-replay.md`](plaintext-collaboration-replay.md).

All 1,134 frontend tests, 64 native offline Auto Reset tests and six native
offline Credits configuration tests passed. The latter cover v1.3.68 image
catalog/source-model fields, explicit unlimited concurrency and old-config
defaults. The Auto Reset harness uses the real URL parser required by new
model code without linking an HTTP client. All 90 local translation values
are unchanged. The locale checker still reports the pre-existing missing
Credits keys in 14 languages; required static source keys are defined, but
this is not a clean locale-check result.

The default native-only `scripts/verify-cockpit-connect-retry.sh` passed on
Darwin/amd64 with retained Go 1.26.8: module replacement validation, all
T01–T22 tests, their race pass, the complete affected `helps` package, all
outer sidecar tests and the disposable CGO-disabled native build. No
cross-platform compilation was run.

`npm run build:app:local` passed with Node 20.19.5, Go 1.26.8 and Rust 1.98.0.
The single incremental TypeScript check and Vite production build passed;
total build time was 1396.5 seconds (about 23m 17s), including 21m 01s in
Cargo. The running compiler was verified to use the local opt-level 1,
256-codegen-unit incremental profile. Existing caches were retained. Only
non-blocking unused/deprecated-code, documentation and Vite chunk-size warnings
were emitted. No production/configuration input changed during this build;
only these maintenance records were updated.

The build produced exactly one 170.1 MiB bundle at
`target/x86_64-apple-darwin/release/bundle/macos/Cockpit Tools.app`.
Both bundle version fields are 1.3.68, both executables are exclusively x86_64,
all 17 configured resources match their source files, and the bundled sidecar
is byte-identical to the freshly compiled native output. Its
`PlaintextCollaborationTools.MarkResponse` implementation marker is present,
confirming the previously unbundled plaintext fix is included. The sidecar
output directory contains only the native binary. No DMG, updater archive,
Universal/other-architecture sidecar or other-platform artifact was generated.
The application was not installed, launched or restarted, and no account quota,
Credits, Reset or saved conversation history was changed for validation.

Main executable SHA-256:
`040a31da819b9cf308fe15b1d4ceb753cd289209afffb3e397c1f0d7ce35fb7b`.
Native/bundled sidecar SHA-256:
`81b25bb74199c49d840f35d505d5ebea500cb7b2ad0a83b7e76cd6a5ff2825c0`.
Logs are retained under `target/local-app-cache/upstream-v1.3.68-*.log`,
with Cargo timing reports in `target/cargo-timings`.

### 2026-10-16 — Cockpit Tools v1.3.70 integration and verification

The user selected exactly release `v1.3.70`, whose peeled commit is
`5e7b3c1e383e16f3ef2f0af2432d93d3944c7e16`. The pre-integration state is
retained by `backup/pre-v1.3.70-20261016-154219`, stash
`2bdbb1812d142a05ba5988ec66e62d841d337b9d`, and a local raw backup.
The backup's machine-specific location is not published.
The rebase preserved all eight private commits with an identical range-diff.
Existing uncommitted settings, Credits, automatic reset and native-build changes
were restored separately. No saved conversation or account data is part of
this source integration.

The duplicate-retry review compared v1.3.68 with v1.3.70 and followed the
outer relay/runtime dispatch, account/model retry rounds, OAuth refresh,
continuity preparation and binding, Codex HTTP/stream executors, usage/context
transports, managed identity pools, host fallback, proxy dialing, uTLS setup and
the HTTP/2 send. The new `helps/managed_transport.go` caches or retires
identity-specific transports; it does not send or retry requests. The changes
to `helps/utls_client.go` add that cache branch and idle-connection cleanup,
without changing upstream `createConnection` or the direct HTTP/2 send.
The usage and fallback wrappers each delegate to one underlying `RoundTrip`.

`sdk/cliproxy/auth/conductor_execution.go` adds continuity preparation and
release ownership around the existing execution loops; it does not add
account/model retry rounds or another OAuth refresh retry. The local
`continuity_client.go` may repeat a failed loopback control RPC once using the
same request/attempt/sequence identity. This is an idempotent local state
operation, not another upstream model request. Continuity binding occurs before
each already-selected executor attempt, and observation does not resend it.

The earlier `CodexHTTPEncryptedRecovery` helper is unchanged. The HTTP and
stream executors now disable that legacy one-shot HTTP 400 rejected-content
recovery while `ContinuityActive(ctx)` is true, avoiding two overlapping history
recovery policies. Without continuity, its previously reviewed gates and bounds
remain in force. These application policies stay separate from connection
setup; the local helper still must not replay after HTTP/2 `RoundTrip` begins.

After integration, the production Go `utlsRoundTripper.RoundTrip` still contains
one call to `createConnectionWithRetry` and one subsequent `h2Conn.RoundTrip`.
The fixed v1.0 budget remains one initial setup attempt plus two retries with
200 ms and 400 ms waits, and its cancellation, classification and ownership
rules are unchanged. No equivalent upstream setup retry was found to remove or
consolidate.

**Engine scope:** v1.3.69 introduced the Rust native engine as the upstream
default. `codex_local_access_gateway_runtime.rs` selects that path before
starting the Go sidecar. Native requests use the Rust HTTP client and
`codex_native_api/retry.rs` policies; they do not pass through Go uTLS or this
local helper. The v1.0 contract remains applicable to the Go engine's protected
HTTPS transport and is not a promise of identical Rust retry behavior. This
integration does not port the helper to Rust or stack the two engines' retries.

The native `scripts/verify-cockpit-connect-retry.sh` run passed all eight stages:
local module configuration, T01–T22 enumeration and acceptance, focused race
detection, the complete vendored `helps` package, the complete outer sidecar
module, and a disposable CGO-disabled native sidecar build. Cross-platform
compilation was intentionally skipped. The complete upstream collaboration,
Multi-Agent V2 and OpenAI SDK handler packages also passed, including their
local HTTP/WebSocket tests. This supersedes the October 9 sandbox limitation
for these verification runs; no process was restarted or binary installed.

Logs are retained in
`target/local-app-cache/upstream-v1.3.70-integration/connect-retry.log` and
`target/local-app-cache/upstream-v1.3.70-integration/upstream-collaboration.log`.
`npm run build:app:local` completed in 1138.9 seconds (Cargo: 17m 41s), with one
incremental frontend typecheck, Vite production build and native app bundling.
The v1.3.70 bundle is 182.9 MiB; its main executable and sidecar are exclusively
x86_64. All 17 configured resources match their source files. The bundled
sidecar is byte-identical to the newly compiled native sidecar:
`982b90b929e05e92861a93edb9d434664b0990e6429e8c12e71c31dcaf723c00`.
The main executable SHA-256 is
`f3a482b12380aa3b04c126a3766353b8b29ed615db109650e1d653ad2cba241e`.
The app is at `target/x86_64-apple-darwin/release/bundle/macos/Cockpit Tools.app`.
Only the native `.app` and matching sidecar were built; all native compilation
caches were retained. No DMG, updater archive or foreign-architecture output
was generated.

After the build, three focused tests used the new binary's headless continuity
worker with temporary databases and synthetic local upstreams. Plaintext marker
persistence and real-cipher preservation, cross-account/restart continuation,
and continuation with a 17 MiB history all passed. The GUI was not launched,
no installed app was replaced, and no real account or saved conversation was
used. Logs and artifact checks are retained beside the verification logs.

### 2026-10-22 — v1.3.73 and Rust Native migration

Integrated the explicitly selected release `v1.3.73`
(`5330f75ae6b3c39fbd72f528ac2188207047d49a`). All 83 original uncommitted files
were first captured in checkpoint `54012972cdc3b04b5b79b638b9c1150ac265fa25`,
backup branch `codex/backup-pre-native-v1.3.73-20261022-171747`, and a local
working-file archive whose machine-specific location is not published.
The existing local build caches and native Go toolchain were preserved.

The duplicate-retry review found no upstream modification to `utls*.go` between
v1.3.70 and v1.3.73. The retained Go implementation and T01–T22 are byte-identical
to the checkpoint. Upstream's new send guard, execution-evidence stop rules and
capability/official fallback remain application-level policies. They neither
replace the fixed setup helper nor introduce another setup loop around it.

Native adds the missing setup-only behavior at reqwest/hyper-util's actual
connector and HTTP/2 readiness boundary, before the request sender receives the
body. It has one three-attempt budget and 200/400 ms waits, preserves the existing
connect deadline and cancellation, and disables automatic body-clone replay only
for protected requests. Other hosts and WebSocket setup retain upstream behavior.
The Go reference and Native helper are alternative engine paths, never stacked
on one request. Higher-level upstream recovery remains outside this per-setup
budget. See `native-connect-retry.md` for ownership and platform details.

The Credits policy now runs in Native, retaining full-Key-pool and independent
image-pool decisions, per-account settings, fresh per-window evidence and bounded
read-only quota refresh. Final credential/workspace identities and later sends
are checked. Auto Reset retains its independent worker and durable consumption
barrier while reusing upstream quota synchronization. The old Go Credits and
direct-provider plaintext deltas were retired. Native plaintext declarations,
JSON/SSE/WebSocket markers and continuity are upstream-owned; no second marker
implementation was added. The migration and 37-test Credits parity map are in
`native-engine-migration.md` and `codex-credits-policy.md`.

Both retry verifiers passed in native-only scope. The Go verifier ran T01–T22,
race detection, the full vendored helps package, all outer sidecar tests and a
disposable native build. The Native verifier passed 18 acceptance tests, including
actual loopback TLS/H2, exact backoff, cancellation, connection/body ownership and
no replay after sending. The frontend passed 1,637 tests; all 18 locales have
7,584 matching keys. Offline Credits and Auto Reset harnesses passed 31 and 71
tests. Main-library integration passed 1,965 tests: Native API 954, Local Access
913, quota 16, Auto Reset 55 and config 27. Seven real-account tests remained
ignored. One upstream fixed-sleep terminal-record test passed separately from the
953-test Native API batch; its assertions were unchanged.

`npm run build:app:local` passed its frontend typecheck, Vite production build,
native Rust build and app bundling in 1436.9 seconds (Cargo: 23m 30s). The bundle
is 193.1 MiB; version and build version are both 1.3.73, and the main executable
and bundled sidecar are exclusively x86_64. All 17 configured resources are
byte-identical to their sources. The Native policy/migration markers are present
in the main executable. Native and bundled sidecar SHA-256:
`aae8974b708f465a5c6dad7887f13037918d18065b45d75c8247ac25f3d92a54`.
Main executable SHA-256:
`a72db9516705ce0268db7110d004d90ca904a0c5909ec5c0445b9a77fb658106`.

The app is `target/x86_64-apple-darwin/release/bundle/macos/Cockpit Tools.app`.
Only this Mac's app and matching sidecar were built; no DMG, updater archive or
other-architecture artifact was generated. Logs and bundle inspection are in
`target/local-app-cache/native-v1.3.73/`. No application was installed or launched,
no running service was restarted, and no real account settings, Reset credits or
conversation data were changed by verification. The saved configuration migrates
when the new build is used.

## Local macOS build scope

On 2026-09-12, the user clarified that local updates need only this Mac's
`.app` and its matching sidecar. Use:

```sh
npm run build:app:local
```

Node.js and the native Rust toolchain must be available on `PATH`. If Go is
absent, the local command reuses the retained Go toolchain described below.

The local command selects the installed Rust host target, generates only an
`app` bundle, disables updater archives, and runs TypeScript checking once
before Vite. It sets `COCKPIT_NATIVE_APP_BUILD=1`, so the Rust build script
builds only the selected host sidecar and skips the automatic arm64/universal
sidecar work. The build script rejects a target that differs from the host in
this mode. `COCKPIT_SKIP_CLIPROXY_BUILD=0` ensures the sidecar is rebuilt from
the current source rather than trusting an old binary merely because it exists.
Use `npm run build:app:local -- --dry-run` to inspect the plan without building.

The local command explicitly passes `.cargo/local-app.toml` to Cargo. This
overrides only the `cockpit-tools` package: `opt-level = 1`,
`incremental = true`, and `codegen-units = 256`. Third-party dependencies retain
their existing release settings and caches. Cargo's normal release profile,
panic strategy, debug assertions, and upstream CI are unchanged. No global
`RUSTFLAGS`, LTO override, linker replacement, or extra architecture is used.
This is a build-speed tradeoff: main-crate code (including instantiated generics)
has less aggressive optimization; runtime throughput has not been benchmarked.
Use `npm run build:app:local -- --full-opt` when upstream release optimization
is required. Both modes use the same app output path; switching modes may
recompile the app, while compatible dependency artifacts remain reusable.

The single TypeScript check now uses `--incremental` and writes its build-info
file under `target/local-app-cache/`. It still checks changed inputs before
Vite runs; it does not skip validation based only on the existence of `dist`.
Keep that directory and `target/x86_64-apple-darwin/release/incremental` during
routine updates. Rust compiler updates and broad source/dependency changes can
invalidate caches. Cargo timing reports are emitted under `target/cargo-timings`
and the local command prints its total elapsed time.

These settings follow the [Cargo profile documentation](https://doc.rust-lang.org/cargo/reference/profiles.html)
and [TypeScript incremental documentation](https://www.typescriptlang.org/tsconfig/incremental.html).

#### 2026-10-02 local-build measurements

The previous v1.3.65 build used default release optimization and no incremental
Rust/TypeScript cache: 925.26 seconds total, including 13m 32s in Cargo.
The first build with the local overrides took 1021.22 seconds total, including
15m 41s in Cargo. This first run created new incremental caches; it was slower,
not a cold-build improvement. Cargo's timing report showed only the application
package and its build script needed compilation; third-party artifacts were
reused. The running app compiler was inspected and confirmed `opt-level=1`,
`codegen-units=256`, and an explicit `incremental` output directory.

A temporary change to the startup log string in `src-tauri/src/lib.rs` then
produced a successful native `.app` in 373.17 seconds (6m 13s), including
5m 46s in Cargo. This was a real app-library source change, not a no-op build.
The measured total was about 60% shorter than the prior default-release build;
this is a small-edit scenario comparison, not a guarantee for arbitrary upstream
updates or a controlled runtime-performance benchmark. The temporary string
was removed and the complete source was byte-verified against its saved
pre-benchmark copy before the final build. No application was launched.

The final build after restoring that string passed in 351.74 seconds
(5m 52s), including 5m 31s in Cargo. The resulting v1.3.65 `.app` is
164.3 MiB versus 146.7 MiB with the original release settings. Both executables
are native x86_64. The main binary contains no benchmark marker, and its
SHA-256 is `9d4ae08fe3070d90979897fc6465c8d74ab02fc66ece000c238e7d3c7a369819`.
The bundled sidecar is byte-identical to the native output and retains the
previous SHA-256 `ed3030c8b69b8c058235087f28bea298998ff006648fe0dde9e9fd124a299359`;
its implementation and optimization flags were unchanged. Rust incremental
data occupied about 1.9 GiB after the small-edit run. The existing uncommitted
Auto Consume Reset source and settings were preserved; the app was not launched
or installed. First-build, edit-build and final-build Cargo reports are retained
in `target/cargo-timings/` for inspection.

The retry verifier now defaults to native-only verification (also available
explicitly as `--native-only`). It still checks the local module replacement,
enumerates and runs T01–T22, runs the race detector and full affected package,
tests the outer sidecar module, and builds a disposable native sidecar. It
does not cross-compile Linux/Windows tests unless `--cross-platform` is
explicitly requested. Its result identifies the host and whether cross-platform
checks were skipped. This changes verification scope, not the fixed v1.0
runtime contract or the required duplicate-retry review.

The earlier v1.3.49 `.app` already contains only x86_64 executables and remains
usable; changing this build workflow does not require rebuilding it. Normal
local updates should not run the full release matrix or generate DMGs,
updater archives, Universal binaries, or other-platform artifacts.

The local workflow passed Node/Bash syntax checks, Rust formatting checks,
and a dry run selecting `x86_64-apple-darwin`. An isolated harness using the
real build script with mocked Go/lipo verified one native sidecar build,
no universal merge, and rejection of a host/target mismatch. The native-only
retry verifier passed on Darwin/amd64. This workflow validation did not
rebuild the `.app` or compile any other-platform artifacts.

## Updating from upstream

Keep upstream history and the local patch separable. Do not copy a fresh source
tree over this checkout. Keep local `main` as a fast-forward-only mirror of
`upstream/main`; keep the patch commits on `local/ct-connect-retry-001`.
When an update request does not name a release, tag, or commit, list the
available targets and obtain a choice before moving the integration baseline;
do not silently chase the latest `main`.

1. Start with a clean worktree on `local/ct-connect-retry-001` and retain a
   recoverable reference to the current local branch.
2. Fetch the canonical repository: `git fetch upstream`, then fast-forward the
   mirror with `git switch main && git merge --ff-only upstream/main`.
3. Inspect upstream changes to the vendored module and especially the uTLS
   round tripper before integrating them.
4. Return to `local/ct-connect-retry-001`, then rebase it onto `upstream/main`
   (preferred for a private local patch) or merge `upstream/main` if the shared
   branch policy requires merge commits. Resolve conflicts by preserving the
   v1.0 behavior, not by blindly choosing either side.
5. Perform the duplicate-retry check below, then run
   `scripts/verify-cockpit-connect-retry.sh` from anywhere in the repository.
6. Review the final diff relative to `upstream/main`; it should contain only the
   still-needed local delta and its maintenance artifacts. Record the new tested
   upstream baseline in the integration commit or release notes.

### Duplicate-retry check

Before retaining the patch after every upstream update, search the new upstream
code for connection retries, transport retries, or request replay around the
same uTLS/HTTP2 path. Trace the full call chain, including wrappers outside the
vendored helper.

```sh
git diff upstream/main...local/ct-connect-retry-001 -- \
  sidecars/cockpit-cliproxy/third_party/CLIProxyAPI/internal/runtime/executor/helps
rg -n 'retry|attempt|backoff|createConnection|RoundTrip' \
  sidecars/cockpit-cliproxy
```

- If upstream added no equivalent retry, keep this patch and its tests.
- If upstream added a setup-only retry, compare attempt count, delays, error
  classification, context behavior, and ownership semantics item by item against
  T01–T22. Consolidate to exactly one retry loop; never stack upstream and local
  loops, which would multiply attempts and latency.
- If upstream retries `RoundTrip` or request bodies, do not compose that replay
  with this patch. Reconcile it explicitly because replay can duplicate a
  non-idempotent request.
- If upstream fully satisfies v1.0, remove the redundant runtime delta only
  after the same prefixed acceptance suite passes against the upstream path.
  Keep or update this specification so the removal decision remains auditable.

The verifier builds a disposable binary for confidence, but it intentionally
does **not** install it, write to `sidecars/cockpit-cliproxy/bin`, replace a
running executable, or restart Cockpit Tools. Installation and restart are
separate, explicitly authorized operational steps.

The bundled CLIProxyAPI tree is a selectively synchronized Cockpit snapshot,
not a standalone upstream checkout: at this baseline, an all-package
`go test ./...` has unrelated missing/duplicate integration symbols. The
verifier therefore runs the complete affected `helps` package inside that
nested module, then separately runs every package in the product's outer
`cockpit-cliproxy` module and builds the actual sidecar entry point.
