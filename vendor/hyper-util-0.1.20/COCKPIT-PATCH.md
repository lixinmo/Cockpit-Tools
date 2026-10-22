# Cockpit Native setup-only retry patch

Baseline: crates.io `hyper-util-0.1.20` archive, SHA-256
`96547c2556ec9d12fb1578c4eaf448b04993e7fb79cbaad930a656880a6bdfa0`.
The archive exactly matches the original main workspace Cargo.lock checksum.
`COCKPIT.patch` is the complete source/manifest delta against that archive.

An opt-in setup-only loop surrounds the existing connector and HTTP/2 initialization, with one timeout, fixed 200/400 ms waits, dedicated connection ownership and an abort-on-drop setup dispatcher. The request body is unavailable to the loop. URI and Upgrade/CONNECT checks constrain the hook to HTTPS chatgpt.com HTTP requests.

The `cockpit-connect-retry` Cargo feature and per-client builder option default
off. The host opts in only for HTTPS chatgpt.com HTTP contexts. It uses upstream
Native OAuth's rustls + system root store for all protected HTTP authentication
modes because macOS native-tls hides concrete TLS failure causes. Other hosts
and WebSocket clients keep their previous TLS/transport policy.

Modified/new implementation files:
- `Cargo.toml`
- `src/client/legacy/client.rs`
- `src/client/legacy/cockpit_connect_retry.rs`

Verification: `scripts/verify-native-connect-retry.sh` from the repository root.
The maintained harness includes actual production source and dependency code.
See `docs/local-patches/native-connect-retry.md` for the fixed-contract mapping,
request cancellation/ownership boundary, and the Mac-only validation scope.
No general dependency update, upstream retry-budget change, or post-send replay
is included. Reconcile this patch before replacing either vendored baseline.
