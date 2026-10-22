# CT-CONNECT-RETRY-001: Native Rust integration

This integration preserves the fixed setup-only behavior of
`same-account-connect-retry.md` for the requested native macOS application. The
original Go implementation, T01–T22 tests and verifier remain the reference;
the fixed v1.0 contract has not been silently revised or deleted.

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
replay after the send boundary. All 18 passed with the main workspace's locked
hyper 1.9.0, h2 0.4.13 and tokio 1.52.0 on 2026-10-22.

The Go verifier must still pass when retaining or updating the reference patch.
After any dependency/upstream refresh, inspect retry/pooling/handshake layers
again before running both verifiers. The native build remains
`npm run build:app:local`, preserving the established native caches.
