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
