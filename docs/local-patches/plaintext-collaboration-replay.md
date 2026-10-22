# CT-PLAINTEXT-COLLAB-001: preserve plaintext collaboration messages

## Current status — Rust Native, v1.3.73

The local build now uses the upstream Rust Native engine. Its existing
`multi_agent_compat_plaintext` policy captures the final transmitted declarations
and restores the plaintext marker at JSON, SSE and WebSocket boundaries,
including custom Responses providers. No second local marker implementation is
added to Native. The former Go direct-provider patch and its local tests have
been retired; commit `54012972cdc3b04b5b79b638b9c1150ac265fa25` preserves them.

Relevant upstream regressions include `native_plaintext_policy_*`,
`chat_collaboration_plaintext_markers_reach_clients_before_identity_persistence`,
`agent_communication_explicit_plaintext_does_not_issue_cipher_ownership`, and
`legacy_agent_public_responses_history_replays_over_json_sse_and_websocket_after_restart`.
Retain these tests when updating Native. Existing saved histories are not
migrated by this source update. The sections below document the earlier Go
integration and its original contract.

## Problem and scope

Cockpit's Multi-Agent V2 optimizer removes the `encrypted: true` declaration
from collaboration tools' `message` parameters. The resulting plaintext
`spawn_agent`, `send_message`, and `followup_task` calls must explicitly return
`encrypted_function_args: []` to Codex. If this metadata is absent, Codex uses
the legacy encrypted delivery path and persists plaintext under
`agent_message.content[].encrypted_content`. The gateway's request conversion
hides that mismatch until a conversation is resumed through a direct account
connection, where the upstream rejects the historical plaintext as ciphertext.

As of v1.3.70, the main collaboration marker implementation is upstream. The
remaining local patch covers the Go direct custom Responses ProviderGateway,
which bypasses the Codex executor's response policy. It reuses upstream policy
rather than maintaining a second SDK implementation. It prevents newly generated
malformed history; it does not migrate existing conversations, change
authentication, or decrypt any content.

Protocol reference: [OpenAI Codex #35845](https://github.com/openai/codex/pull/35845).

## Runtime contract

- Use upstream `collaboration.PlaintextPolicyForRequest` on the actual final
  transmitted body, after direct gateway identity rewriting. Earlier schema
  removal alone is insufficient evidence if later preparation changes tools.
- Retain upstream client/optimizer eligibility, final declaration conflict
  handling, and existing encryption/signature safeguards.
- Apply the policy only to direct Responses gateway output, after collaboration
  namespace restoration/normalization in JSON and SSE, keeping arguments intact.
- Authorize the reserved wire alias only when this request actually performed
  the optimizer rename. Include requests with only `send_message`, as well as
  requests containing `spawn_agent` and using the optimized namespace.
- Leave Chat conversion, Codex executor HTTP/WebSocket handling and Rust native
  handling to upstream. Do not restore the superseded local SDK marker inventory.
- Leave unrelated tools, reasoning items and disabled optimization unchanged.

The patch does not add retries or change account selection, request replay,
connection setup or the `CT-CONNECT-RETRY-001` v1.0 contract. The original local
integration did not include an upstream merge; later integrations are recorded
below.

## Implementation

- Upstream owns `internal/codex/collaboration/plaintext.go`, the optimizer
  response helpers, executor handling and Chat-to-Responses conversion.
- The local runtime delta is in outer `provider_gateway.go`: capture the final
  policy and invoke the upstream marker at the independent JSON/SSE exits.
- `provider_gateway_responses_plaintext_test.go` covers this remaining gap.
- The old local `provider_gateway_plaintext.go`, its tests, the separate SDK
  metadata inventory and the old SDK integration tests were superseded. The
  vendored OpenAI SDK handler directory is identical to v1.3.70.

## Verification and local build

Use the native Go toolchain required by `AGENTS.md`. From the vendored
`sidecars/cockpit-cliproxy/third_party/CLIProxyAPI` module:

```sh
go test -mod=readonly ./internal/codex/collaboration ./internal/client/codex/optimize-multi-agent-v2 ./sdk/api/handlers/openai -count=1
```

From the repository root, the native retry verifier also exercises the whole
outer sidecar module and builds its entry point:

```sh
scripts/verify-cockpit-connect-retry.sh
npm run build:app:local
```

Upstream tests cover communication tools, JSON/SSE/WebSocket output forms,
declaration conflicts, encrypted evidence and continuation policy inheritance.
Local regressions exercise the direct Responses JSON/SSE exits, send-only and
optimized-alias requests, disabled/other-client cases, mixed final declarations,
opaque metadata and signatures. A Chat check confirms upstream's own conversion
marker still applies without a second local policy. Original messages remain
unchanged. Only synthetic requests and temporary local servers are used.

Retain the existing Rust, TypeScript and native Go caches. Produce only this
Mac's `.app` and matching sidecar; no DMG, updater archive or foreign-architecture
artifact is required. Building does not install or restart the application.

Existing malformed history requires a separate, backed-up migration and
thread-specific offset reconciliation. It is deliberately outside this patch;
the already completed local history repair is not repeated during upgrades.

## 2026-10-16 — v1.3.70 upstream integration

Integrated the requested release commit
`5e7b3c1e383e16f3ef2f0af2432d93d3944c7e16`. Upstream now addresses the same
plaintext protocol gap, adds durable continuation history and introduces a
separate native Rust engine. The old broad local marker implementation was
removed instead of stacked on top of the upstream final-request policy.

The remaining Go direct custom Responses ProviderGateway gap was reproduced
against an isolated pristine v1.3.70 tree before adapting the local patch.
The small integration now uses the upstream policy after the actual outgoing
request is finalized. It does not modify Chat or the native Rust protocol.

The focused gateway tests and race pass succeeded. The full upstream
collaboration helper, optimizer and OpenAI SDK packages passed, including
HTTP/WebSocket tests. The native CT-CONNECT-RETRY-001 verifier passed all
stages, including the complete outer sidecar module and a native sidecar build.
The full native app build completed successfully in 1138.9 seconds. Version
1.3.70, x86_64 main/sidecar, 17 bundled resources and byte-identical native/bundled
sidecars were verified. The app is
`target/x86_64-apple-darwin/release/bundle/macos/Cockpit Tools.app`.

The newly built executable also passed three headless continuity-worker tests:
plaintext markers and ciphertext preserved through durable storage; account
change plus restart continuation; and continuation with a 17 MiB history.
These used temporary databases and synthetic localhost upstreams, not installed
GUI startup or real account traffic. The full frontend suite passed 1,178 tests;
the Auto Reset and Credits offline harnesses passed 64 and 12 tests respectively.

Local Credits policies are Go-only. To preserve existing behavior, configurations
written by the prior local build retain Go on first upgrade, and restricted
Credits cannot silently switch to Native. Explicit unrestricted Native remains
available. This compatibility guard is separate from the plaintext repair;
see `codex-credits-policy.md`.

Original work was retained in backup branch
`backup/pre-v1.3.70-20261016-154219`, stash
`2bdbb1812d142a05ba5988ec66e62d841d337b9d`, and a local working-file archive.
The archive's machine-specific location is not published.
No account settings, installed application or conversation data were changed
by this source integration and its synthetic validation.

## 2026-10-09 local integration result

The patch was integrated into the existing v1.3.66 working tree without changing
its pre-existing modified source files. The combined HTTP/WebSocket test was
split into separate tests so the in-memory HTTP assertions remain runnable when
the execution environment forbids listening on a TCP port; all original
WebSocket assertions are retained without skips.

Passed in this working tree:

- Complete Multi-Agent response helper package.
- In-memory HTTP JSON/SSE response boundaries, SSE reconstruction, WebSocket
  tool inventory inheritance/reset, and provider SSE frame preservation.
- Retry verifier T01–T22 acceptance tests and the focused race-detector pass.
- Frontend incremental typecheck and Vite production build.
- Native x86_64 sidecar compilation; the binary contains the new
  `PlaintextCollaborationTools.MarkResponse` implementation.

The complete SDK test run and the retry verifier's full `helps` package stage
were blocked by the execution sandbox's TCP bind restriction
(`listen tcp6 [::1]:0: bind: operation not permitted`). They are not recorded as
passing in this working tree. The original isolated patch's full integration
tests passed in the preceding investigation, but that does not replace a
re-run of this working tree outside the restricted environment.

`npm run build:app:local` was attempted. After moving this invocation's Go and
Swift compiler caches to writable project cache paths, SwiftPM still failed
because its manifest subprocess cannot create a nested sandbox:
`sandbox-exec: sandbox_apply: Operation not permitted`.
No sandbox controls were disabled. The complete `.app` build has therefore
**not** finished; the existing `.app` is unchanged and does not contain this
patch. No application was installed/restarted and no conversation was migrated.

Continue in an ordinary local terminal with the verification and build commands
above. Logs and artifact checks are retained under
`target/local-app-cache/plaintext-fix-20261009/`. Existing global caches and
project incremental caches were preserved.

## 2026-10-13 — v1.3.68 upstream integration

The existing runtime patch auto-merged onto release `v1.3.68` (`af5e90bc`)
without implementation changes. Annotations remain before WebSocket output
collection, so new upstream incomplete/interrupted completions reconstruct
already-marked items. Explicitly empty upstream output stays empty; a local
HTTP interruption can reconstruct its collected output; the next continuation
inherits the proven tool inventory without changing message arguments.

Two existing test files were expanded for these paths. The complete
`internal/client/codex/optimize-multi-agent-v2` and `sdk/api/handlers/openai`
packages passed, including actual local HTTP/WebSocket exchanges. Focused
plaintext race tests also passed. The October 9 TCP-bind restriction did not
recur; this is fresh verification of the integrated working tree.

The release's new one-shot HTTP rejected-reasoning/compaction recovery is a
separate upstream application policy. Its bounded executor/helps regressions
passed; it does not strip agent-message content, replace this metadata patch,
or repair existing saved conversations. The retry-policy reconciliation is
recorded in `same-account-connect-retry.md`.

The complete native retry verifier also passed: T01–T22, race detection,
full `helps`, the complete outer sidecar module and a disposable native binary.
`npm run build:app:local` completed successfully in 1396.5 seconds, including
one frontend typecheck, Vite build and native app bundling. The October 9
Swift sandbox failure did not recur; no sandbox controls were changed.
The resulting v1.3.68 app is 170.1 MiB with exclusively x86_64 executables.
The bundled sidecar is identical to the freshly compiled native output and
contains `PlaintextCollaborationTools.MarkResponse`; this app now includes
the plaintext collaboration repair. All 17 configured resources were verified.

Native/bundled sidecar SHA-256:
`81b25bb74199c49d840f35d505d5ebea500cb7b2ad0a83b7e76cd6a5ff2825c0`.
The app is at
`target/x86_64-apple-darwin/release/bundle/macos/Cockpit Tools.app`.
No application was installed or restarted, no conversation history was migrated,
and no DMG, updater archive or foreign-architecture artifact was built.
