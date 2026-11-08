# Maintained scope after Native-only cleanup

The user narrowed this fork on 2026-11-08 to three maintained additions:

1. Native same-account connection-setup retry, with the fixed three-attempt,
   200/400 ms, shared-deadline and no-post-send-replay contract.
2. Native OpenAI Credits admission, per-account permissions, quota evidence and
   final-identity send guards. This remains distinct from upstream API-key budgets.
3. Standard-hosted cloud builds, platform selection, dependency/incremental caches,
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

Source cleanup and private history inventory are in progress. Before delivery,
run the Native retry and Credits harnesses, upstream Native/tool-call/Reset
integration checks, frontend checks and the Intel App build in GitHub Actions.
Record the verified source and cloud run here after they pass. No local Rust/Go
compilation or real-account history is used by the cloud jobs.
