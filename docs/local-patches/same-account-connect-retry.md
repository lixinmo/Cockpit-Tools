# CT-CONNECT-RETRY-001: same-account connection retry

The active implementation is Rust Native only. Its setup hooks, error adaptation
and executable acceptance suite are specified in [native-connect-retry.md](native-connect-retry.md).
The original Go v1.0 implementation and T01–T22 reference remain in
[Git history](https://github.com/lixinmo/Cockpit-Tools/blob/6d1535c3/docs/local-patches/same-account-connect-retry.md).
They are not bundled or compiled after the user retired Go on 2026-11-08.

The original reference below preserves the fixed semantics; Go-specific type
names describe that reference, while the Native document maps them to Rust.

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

## Current verification and upstream review

Run `scripts/verify-native-connect-retry.sh` in GitHub Actions on standard hosted
runners. The Native suite covers exact transient classification, cancellation,
shared deadline/backoff, H2 readiness, one send/body ownership and permanent-error
precedence. The application uses the narrowly patched reqwest 0.12.28 and
hyper-util 0.1.20 hooks; no request replay loop is added above them.

For each upstream update, inspect connector retry/pooling/readiness and explicit
application response recovery separately. `configure` is applied once to the
Native client; HTTP/1 fallback preserves that setup boundary, while WebSocket
Upgrade/CONNECT and other hosts remain excluded. Upstream's explicit compressed-
body rejection fallback is its own proven-unsent response handling, not a retry
on transport EOF; retained Credits checks also run before that fallback send.

Current maintained scope and verification are recorded in
[native-only-cleanup.md](native-only-cleanup.md). Old build/installation records
remain in Git instead of requiring obsolete Go tools or runtime compatibility.
