# Local-project recovery with larger histories

## Scope (2026-11-09)

The user requested removal of two upstream quick-repair gates: the 2048-thread
limit and the 2,000,000-byte limit on `.codex-global-state.json` or its `.bak`.
The limits could skip an entire account/project restore despite a small local
project registry. The thread count included child-agent and guardian records
that do not themselves receive project assignments.

This patch extends the upstream implementation; it adds no alternate project
store, watcher, credential migration or history-repair service. It does not
change Native retry, Credits, model configuration or Reset automation.

## Behavior and protections

- Account-state preparation, project planning and pre-commit thread revalidation
  all read without the retired thread-count gate. Quick planning no longer
  rejects the state or adjacent backup merely for exceeding 2 MB.
- Existing per-operation deadline checks, SQLite interruption/busy timeouts,
  profile/process ownership, state hash comparisons, conditional row writes,
  durable transaction intent and rollback remain intact.
- The common 20 MB file guard and other existing project/root/catalog bounds
  remain. This change removes the two requested gates, not every resource guard.
- Remote/cloud projects, account-private state, explicit projectless threads,
  child agents, pending deletions and ambiguous/newer assignments retain their
  existing protection. Chat bodies, credentials and history contents are not
  modified by this patch.
- Account-state preparation precedes the generic project planner. A later
  deferred maintenance result does not undo its already committed account state.

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

The frontend build passed with one TypeScript check. The final local Intel App
build and installation verification are pending.
