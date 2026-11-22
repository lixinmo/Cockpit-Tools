# Local-project recovery with larger histories

## Scope (2026-11-09)

The user requested removal of two upstream quick-repair gates: the 2048-thread
limit and the 2,000,000-byte limit on `.codex-global-state.json` or its `.bak`.
The limits could skip an entire account/project restore despite a small local
project registry. The thread count included child-agent and guardian records
that do not themselves receive project assignments.

Upstream v1.4.0 now owns the project, thread, catalog and state-file reads.
The fork preserves the operation deadlines and the recovery path on timeout.

## Behavior and protections

- The upstream readers have no fixed thread or state-file size gate. Commit
  revalidation queries only the thread IDs the plan will update.
- Existing per-operation deadline checks, SQLite interruption/busy timeouts,
  profile/process ownership, state hash comparisons, conditional row writes,
  durable transaction intent and rollback remain intact.
- Account-state preparation reads the project registry first. If a full thread
  scan times out, it tries the saved thread IDs within the remaining budget so
  the account switch can still publish its project groups. The launch and repair
  paths check their deadlines while reading large files and querying SQLite.
- Remote/cloud projects, account-private state, explicit projectless threads,
  child agents, pending deletions and ambiguous/newer assignments retain their
  existing protection. Chat bodies, credentials and history contents are not
  modified by this patch.
- Account-state preparation precedes the generic project planner. A later
  deferred maintenance result does not undo its already committed account state.

## Upstream v1.4.2 review (2026-11-22)

Upstream `ccac8637` restores verified local chats without a project when
switching to API-key mode. Its SQLite verification excludes remote, cloud
and child-agent chats. The saved-ID fallback also includes the ungrouped
chat IDs so the same verification runs after a full history scan times out.
The fork keeps the launch deadlines and uses upstream's process check,
instance-relative rollout paths and failed-backup cleanup.

With the v1.4.2 SSH startup path, two existing upstream tests reproduced
lost auto-connect settings during an account switch. The merged path calls
upstream's `inherit_scoped_auto_connect` and `ssh_server_identity::aliases`
for configured SSH hosts. Both scoped and legacy preferences use that same
alias lookup.

The isolated release suite passed all 66 fixture tests, including the two
SSH cases, saved-ID and full-scan projectless recovery, the 3,500-thread
round trip and conditional rollback. The opt-in live profile preview was
skipped. All 13 Python project-repair tests also passed.

## Upstream review (2026-11-14)

Upstream `v1.3.84` at `0c96cf74` removed the 2 MB quick-repair gate and gives
larger state files more time within the existing launch deadline. Project-only
repair, verified sidebar presentation, and SSH preference restoration use the
upstream implementations. The fork still removes the total thread-count gates
from project planning and commit-time revalidation. Account-state preparation
reads memberships within the upstream separate budget. If the read times out,
it uses the upstream saved-membership query with the remaining launch time.

The isolated recovery suite passed 66 tests with one opt-in preview excluded.
It covered the large-profile round trip, large state and backup files, the slow
SQLite fallback, sidebar presentation, and SSH state and startup restoration.

## Upstream v1.4.0 review

Upstream `795e91f5` removes the thread-count ceiling from planning and account
preparation and revalidates only affected thread IDs at commit. It also removes
the 20 MB state-file limit and quick limits on projects, roots, and catalog
rows. The fork adopts these readers and retains the existing launch, switch,
and project repair deadlines. In-progress SQLite work is interrupted at its
deadline, and a failed write can finish rollback after the deadline.

The isolated release suite passed 65 tests with the opt-in profile preview
excluded. It covered state and backup files larger than 20 MB, the 3,500-thread
account round trip, deadline expiry, SQLite interruption and conditional
rollback.

## Earlier upstream review (2026-11-12)

Upstream `v1.3.81` and `upstream/main` at `7ca7f0db` restore project groups
earlier and prepare the stopped instance's project state for a switch followed
by a manual launch. They still leave thread assignments out of account-state
preparation above 2048 threads. The upstream quick planner and commit-time
revalidation also retain the 2048-thread limit, and the planner defers on a
state file or backup above 2 MB. The merge adopts the upstream switch and
launch paths while retaining this patch's deadline-bounded thread reads and
state-file handling.

## Validation

The isolated Rust module suite passed on 2026-11-09: 41 tests passed, with one
explicit opt-in read-only preview excluded from the ordinary suite. Coverage
includes larger thread tables, child-agent exclusion, state and backup files
above 2 MB, account A→B→A transitions, conditional rollback, concurrent changes,
unknown schemas, deadlines and the common memory guard.

The isolated test harness includes production project-recovery, deadline and
atomic-write code; it does not claim to replace full UI/process/credential-launch
integration tests. Ordinary tests use synthetic profiles, never private account
data. A separate opt-in read-only preview of the local profile completed within
the existing 500 ms budget without applying a plan or switching accounts.

The frontend build passed with one TypeScript check. The local Intel App build
passed from source `1fd95d27ff0c8e3db6730749fe764907eb76fe9a` in 25m 56s.
The staged App is version 1.3.80, contains one thin x86_64 Mach-O executable,
passes strict ad-hoc signature verification and contains no Go API executable.
Main executable SHA-256:
`96ecb1b279f0807d7b1ee795bd4600f2d55c5474888c08198cb4f8441a9c62ed`.

The verified App was installed and successfully launched on 2026-11-09 after
the user unlocked the Mac. The running installed executable matches the build
hash above and passes strict signature verification. Before/after checks confirm
unchanged account inventory and current selections, API keys and membership,
Credits policy, credential-storage key, provider configuration and application
configuration fields. No real account switch was performed during installation;
account-switch coverage comes from the synthetic roundtrip tests, and the live
profile check was a read-only preview.

Project compilation output, dependencies, frontend output, build scratch, the
previous App, the duplicate staged App and private diagnostics were removed
(about 3.53 GiB in total). Source/Git history/remotes, runtime accounts and
configuration, shared toolchains and the current installed App remain.
