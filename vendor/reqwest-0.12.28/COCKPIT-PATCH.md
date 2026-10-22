# Cockpit Native setup-only retry patch

Baseline: crates.io `reqwest-0.12.28` archive, SHA-256
`eddd3ca559203180a307f12d114c268abf583f59b03cb906fd0b3ff8646c1147`.
The archive exactly matches the original main workspace Cargo.lock checksum.
`COCKPIT.patch` is the complete source/manifest delta against that archive.

An opt-in ClientBuilder hook passes the classifier and existing connect timeout to hyper-util. Its narrowly matching request scope cannot clone a body for automatic request replay. A hidden helper exposes only the private typed connection-timeout test; no string classification or transport replacement is used.

The `cockpit-connect-retry` Cargo feature and per-client builder option default
off. The host opts in only for HTTPS chatgpt.com HTTP contexts. It uses upstream
Native OAuth's rustls + system root store for all protected HTTP authentication
modes because macOS native-tls hides concrete TLS failure causes. Other hosts
and WebSocket clients keep their previous TLS/transport policy.

Modified/new implementation files:
- `Cargo.toml`
- `src/async_impl/client.rs`
- `src/lib.rs`
- `src/retry.rs`

Verification: `scripts/verify-native-connect-retry.sh` from the repository root.
The maintained harness includes actual production source and dependency code.
See `docs/local-patches/native-connect-retry.md` for the fixed-contract mapping,
request cancellation/ownership boundary, and the Mac-only validation scope.
No general dependency update, upstream retry-budget change, or post-send replay
is included. Reconcile this patch before replacing either vendored baseline.
