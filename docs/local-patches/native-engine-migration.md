# Local Native integration: Cockpit Tools v1.3.73

Selected release: `v1.3.73`, commit
`5330f75ae6b3c39fbd72f528ac2188207047d49a`.

The local API service follows upstream's Rust Native engine. Existing local
configuration migrates once while preserving Credits policy, account overrides,
API keys, pools and unrelated JSON fields. An explicit later unrestricted Go
selection remains possible; restricted Credits cannot run without its Native
implementation.

## Upstream implementation retained

- Official ChatGPT authorization, permitted local fallback, capability fallback,
  account permissions, concurrency and OAuth quota reserves.
- Responses/Chat/image/Realtime protocol conversion, stream handling and durable
  continuation/history ownership.
- Plaintext collaboration marking from the final transmitted declarations,
  including custom Responses JSON/SSE and WebSocket output. The superseded Go
  direct-provider marker patch is removed.
- Native quota synchronization and atomic account/credential identity checks.
- Existing frontend and platform behavior from the selected release.

## Local behavior retained or migrated

- **Credits admission** is implemented in Native, with complete Key-pool and
  independent image-pool semantics, per-account permissions, precise per-window
  evidence, bounded read-only refresh and diagnostics. The old Go implementation
  is removed. See `codex-credits-policy.md`, including the mapping of all 37
  previous Go regressions. Ordinary API-key providers and Grok remain exempt.
- **Auto Consume Reset** remains the existing Rust feature: individual opt-in and
  threshold, independent background worker, fresh-only checks and durable
  duplicate-consumption prevention. Its wrapper now shares upstream's updated
  quota-only refresh and persistence path. See `auto-consume-reset.md`.
- **Connection setup retry** has a Native implementation below the request-send
  boundary: three attempts, 200/400 ms waits, a shared deadline, cancellation and
  no replay after sending. The narrowly patched reqwest/hyper-util dependencies
  preserve the existing Native connector. See `native-connect-retry.md`.
  The unchanged Go v1.0 implementation and T01–T22 remain a verified reference
  in the optional upstream sidecar, not a second loop in Native requests.
- **Local build** still uses `npm run build:app:local`, one frontend typecheck,
  this Mac's `.app` and matching sidecar, and the existing local Rust/TypeScript
  caches. No DMG, updater archive or other architecture is required.

Credits admission is not an upstream spending lock. Official grants have a
different quota authority; restrictive policies cannot borrow source OAuth
quota to authorize official consumption. They use upstream's permitted local
fallback when possible and otherwise reject unconfirmed quota. Final credential,
workspace, organization and Agent identities are checked before attaching quota
evidence, and later sends recheck the current policy.

## Recovery point

All 83 original uncommitted files were captured before integration:

- Checkpoint: `54012972cdc3b04b5b79b638b9c1150ac265fa25`.
- Backup branch: `codex/backup-pre-native-v1.3.73-20261022-171747`.
- Working-file archive and original patches were retained locally at integration
  time; their machine-specific location is not published.

The source update and synthetic verification do not install or restart the
application, edit live account settings, redeem Reset credits or rewrite saved
conversation history. The configuration migration runs when this build is used.

## Verification record

Logs are retained under `target/local-app-cache/native-v1.3.73/`.

| Check | Result |
| --- | --- |
| Frontend suite | 1,637 tests passed |
| Locales | 18 languages, 7,584 keys each; all checks passed |
| Credits policy/configuration/refresh harness | 31 tests passed |
| Auto Reset harness | 71 tests passed |
| Go CT-CONNECT-RETRY-001 verifier | All native stages passed |
| Native setup retry verifier | 18 acceptance tests passed |
| Main-library integration tests | 1,965 passed: Native API 954, Local Access 913, quota 16, Auto Reset 55, config 27; 7 explicit real-account tests remain ignored |
| Native app build and bundle inspection | Passed: v1.3.73, x86_64 main and sidecar, all 17 resources identical; 193.1 MiB app |

The engine-switch patch retains upstream's ability to patch incomplete JSON
objects without decoding unrelated account/key fields. An upstream fixed-sleep
test, `native_transport_terminal_record_survives_tracker_drop`, is run separately:
its 80 ms background delay and 120 ms observation can race under load. Native API
passed as 953 tests plus that unchanged isolated test; no assertion or production
behavior was weakened. The earlier four 100 ms database-startup timeouts did not
recur in sequential verification. Initial and final logs are both retained.

`npm run build:app:local` completed in 1436.9 seconds (Cargo 23m 30s), including
one frontend typecheck in the successful build. The initial typecheck identified
two imports left unused by the merge; they were removed before the successful
build. The app is
`target/x86_64-apple-darwin/release/bundle/macos/Cockpit Tools.app`.
Main executable SHA-256:
`a72db9516705ce0268db7110d004d90ca904a0c5909ec5c0445b9a77fb658106`.
Native/bundled sidecar SHA-256:
`aae8974b708f465a5c6dad7887f13037918d18065b45d75c8247ac25f3d92a54`.
The bundle contains the Native Credits enforcement and migration markers. It
was not installed or launched, and no DMG, updater archive or other architecture
was built. Full machine-readable checks are in `bundle-verification.json` beside
the logs.
