# Maintained scope after Native-only cleanup

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
local/build-workflow tests also passed. The final Intel App is being built under
the current policy in [cloud-builds.md](../cloud-builds.md). No App from the failed
cloud run was published. Real account data and the private repair tooling are
never part of the public build inputs.
