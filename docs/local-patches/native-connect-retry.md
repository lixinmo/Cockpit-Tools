> Current runtime is Native-only. The Go implementation, its T01–T22 code and
> verifier are retired; their original contract remains available in Git history.
> This Native implementation and its fixed runtime limits remain unchanged.

# CT-CONNECT-RETRY-001: Native Rust integration

This integration preserves the fixed setup-only behavior of
`same-account-connect-retry.md` for the requested native macOS application. The
original Go implementation, T01–T22 tests and verifier remain a historical
reference in Git; they are not built or required by current verification. The
fixed v1.0 contract has not been silently revised or deleted.

## Runtime boundary

`codex_local_access_native_transport.rs` calls the shared
`native_connect_retry::configure` while constructing the selected account's
existing reqwest client. Only an HTTPS `chatgpt.com` HTTP context opts in.
WebSocket clients, Upgrade/CONNECT requests and other hosts retain upstream
behavior. The dependency hook checks the actual request URI again.

The patched hyper-util client completes the existing connector (including its
DNS, proxy and TLS work), HTTP/2 initialization and sender readiness within one
three-attempt budget. Delays are exactly 200 ms and 400 ms. The helper never has
access to the request body and returns before `PoolClient::try_send_request`.
The existing connect timeout surrounds all attempts and waits. The existing
Native total deadline and stop signal own the request future; cancellation
drops the entire setup future. A simultaneous setup deadline wins over an error
or completed backoff through a biased select.

Protected calls use dedicated connections, matching the Go patch's ownership
model. They do not race an idle pool connection and leave an unfinished connect
in the background. The setup dispatcher is aborted on cancellation or failed
readiness; after success, sender/response ownership closes the connection.
The request body is never polled by setup, and normal Rust ownership drops it
once on failure. Dropping a live response closes its dedicated connection.

Reqwest's automatic request cloning is disabled only for protected requests,
including protocol NACK replay. A peer EOF reported after invocation of the
sender cannot reenter this loop, even when no body bytes were polled. Upstream
application-level account failover/recovery remains outside this local budget;
the three-attempt limit is not an end-to-end request-attempt limit.

## Typed classification and necessary Native adaptation

Retry eligibility comes from concrete I/O and timeout causes, never error
strings. `io::Error::get_ref()` and `Error::source()` are both inspected because
`io::Error::source()` can skip its embedded error. NXDOMAIN and permanent TLS
evidence override a surrounding timeout. Unknown errors remain permanent.

The protected HTTP client uses the same rustls backend and native trust roots
as upstream Native OAuth Responses. This also covers API-key/Agent Identity
contexts targeting `chatgpt.com`: macOS native-tls hides the concrete OSStatus,
preventing reliable distinction between a transient closed TLS handshake and a
permanent TLS error. Other hosts and WebSocket TLS settings are unchanged.

The Unix system resolver keeps `getaddrinfo`'s EAI_AGAIN/EAI_NONAME codes as typed
evidence, preserving ordered system addresses. Dropping its future aborts any
queued worker. A system `getaddrinfo` already running cannot be interrupted;
its abandoned result cannot launch another connection attempt. This is not a
new network resolver or an external DNS service.

This integration is verified on this Mac. Non-Unix resolution retains the
standard resolver; native WinSock DNS classification and other-platform
acceptance have not been claimed. The retained Go contract is still the
cross-platform reference. Do not publish a claim of complete Native Windows
parity without adding and verifying typed WinSock resolution.

## Dependency maintenance

The workspace pins narrowly patched copies of reqwest 0.12.28 and hyper-util
0.1.20. Each vendor directory contains `COCKPIT-PATCH.md` and `COCKPIT.patch`,
with original crates.io archive hashes. The feature and builder hook default
off, so other workspace consumers retain upstream behavior.

Do not replace this boundary with a `client.send()`/request-clone retry or a
`connector_layer` retry. The latter does not include HTTP/2 initialization and
reqwest's connector argument is intentionally neither accessible nor Clone.

## Verification

Run `scripts/verify-native-connect-retry.sh`. The maintained standalone crate
in `tests/codex-native-connect-retry` includes the production classifier and
production transport tests and resolves the same core versions as the main
workspace lock. It uses only loopback TLS/H2 servers and in-memory byte
transports. It has no Tauri/GUI dependency, real-account access or app install.

The 18 tests cover concrete error classification and permanent precedence,
system localhost resolution, initial success, transient TLS failure followed
by one H2 POST, exhaustion, exact setup/H2 backoff, original final errors,
shared timeout and deadline precedence, cancellation before polling and during
HTTP/2 initialization, ownership of partial connections and unread request
bodies, early response drop, exclusion of other hosts/Upgrade, and refusal to
replay after the send boundary. All 18 passed with hyper 1.9.0, h2 0.4.13 and
tokio 1.52.0 on 2026-10-22. See the v1.3.84 review below for the updated
workspace versions.

The former Go implementation is retired; the original v1.0 reference remains in Git history.
After any dependency/upstream refresh, inspect retry/pooling/handshake layers
again before running the Native verifier. Under the current
[build policy](../cloud-builds.md), use the existing local toolchain and targeted
checks by default. `npm run build:app` and `npm run build:app:local` build the
personal Native-only App locally. `npm run build:app:cloud` is an explicit manual
alternative using standard GitHub-hosted runners; do not dispatch a full cloud
build for each test repair.

## Historical v1.3.80 integration review (2026-11-08)

This review predates Go retirement. References below to two retry verifiers
record that integration's validation; current verification uses the Native suite.

The upstream client cache now remembers a scoped HTTP/1 fallback after repeated
HTTP/2 TLS failures, and decodes response compression at the protocol boundary.
Both behaviors remain upstream-owned. Client construction calls `configure`
exactly once. Its exclusion argument is the original WebSocket-only transport
flag, not the cache's HTTP/1 fallback flag: ordinary HTTP fallback requests to
`https://chatgpt.com` must retain the same setup fence and reqwest replay denial.

The reqwest 0.12.28 / hyper-util 0.1.20 vendor hooks are unchanged. The setup loop
still owns only connector/handshake/readiness, uses three attempts and 200/400 ms
waits under one deadline, and ends before `PoolClient::try_send_request`.
WebSocket Upgrade/CONNECT and other hosts remain excluded. The cache loop only
reconciles client construction with concurrent cache policy changes; it does not
send a request or add a connection-attempt budget.

Upstream's explicit compressed-body rejection fallback remains at the existing
application request layer, with its execution-evidence check and one plaintext
fallback. It is not triggered by a transport EOF. A fresh Credits check is now
also required before that fallback send. The compression regression includes a
policy denial arriving with the rejection and expects exactly one upstream call.
This review is static; the merge's cloud verification record supplies execution
results for both retry verifiers and the Native integration suite.

## Upstream v1.3.84 review (2026-11-14)

Upstream `0c96cf74` adds HTTP/2 liveness checks, idle pool expiration and
retirement of a failed transport. Transport renewal uses the existing Native
application retry budgets and respects whether the failed request could have
been sent. The single setup-retry entry still wraps the Native HTTP client
builder and applies to eligible HTTP/1 fallback clients. Its attempt and timing
contract and the vendor replay fence are unchanged.

The standalone harness now matches the App lockfile's hyper 1.12.0, h2 0.4.20
and tokio 1.52.0. All 18 setup-retry tests passed with these versions. Four
isolated loopback tests using the merged client builder verified that it
detects a stalled HTTP/2 connection and allows a healthy 50-second response to
finish, both with and without the setup retry.
