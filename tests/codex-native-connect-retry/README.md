# Native connection setup regression harness

Run from the repository root:

```sh
scripts/verify-native-connect-retry.sh
```

The crate includes the production classifier and tests from
`src-tauri/src/modules/codex_native_connect_retry*.rs`. It uses the patched
production reqwest/hyper-util libraries, actual loopback TLS/H2 servers, and
real hyper HTTP/2 initialization over in-memory transports. No accounts or
external service requests are used. The lockfile is derived from the main
workspace's dependency versions. The build cache remains under
`target/local-app-cache/native-connect-retry-harness/target`.

The verifier checks that every named regression is present before running the
suite, so removal/renaming of an acceptance case cannot silently pass.

See `docs/local-patches/native-connect-retry.md` for scope, ownership,
cancellation, dependency baselines and the unverified non-Unix DNS boundary.
