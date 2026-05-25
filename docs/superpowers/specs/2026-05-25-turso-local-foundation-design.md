# Turso Local Foundation Migration Design

## Context

Right Agent currently uses local `libsql` behind the `right-db` boundary for
each agent's `<agent>/data.db`. The earlier local `libsql` migration achieved
the important boundary: other crates no longer expose direct SQLite driver
types. The paused local-finalization plan was scoped to proving local `libsql`
behavior only.

The future cloud backup direction is now Turso Cloud push/pull, not generic
object-storage backup. Official Turso Rust docs recommend the `turso` crate
with the `sync` feature for local databases that can later synchronize with
Turso Cloud. This means the local foundation should move to the `turso` crate
instead of hardening more `libsql`-specific assumptions.

This design is still local-only behavior. It prepares the storage engine for
future Turso Cloud support, but it does not implement cloud sync, credentials,
UI, CLI, bot commands, or scheduler behavior.

Relevant upstream references:

- `https://docs.turso.tech/sdk/rust/quickstart`
- `https://docs.turso.tech/sdk/rust/reference`
- `https://github.com/tursodatabase/turso`

## Goals

- Replace `libsql` with the `turso` crate inside `right-db`.
- Enable the `turso` `sync` feature as dependency preparation for future Turso
  Cloud push/pull work.
- Preserve current local `data.db` behavior, file paths, schema, migrations,
  and caller APIs.
- Keep raw database-driver types hidden inside `right-db`.
- Prove compatibility with existing local database requirements before broad
  migration work proceeds.
- Remove or re-prove any local `libsql` assumptions that would be unsafe with
  the `turso` backend.

## Non-Goals

- No Turso Cloud URL, token, org, database name, or config model.
- No `push()`, `pull()`, `checkpoint()`, sync stats, or sync scheduler.
- No dashboard, Telegram bot, CLI, or agent-facing UI for cloud storage.
- No cloud backup/restore UX.
- No migration from local-only `data.db` to an attached cloud database.
- No object-storage backup design.
- No schema redesign unrelated to driver migration correctness.

## Recommended Approach

Use a foundation migration with a hard compatibility gate:

1. Query the current crate registry for the latest `turso` crate version during
   planning/implementation, then add it with the `sync` feature.
2. Add isolated compatibility tests or probes before replacing the production
   `right-db` internals.
3. If `turso` passes the local compatibility gate, migrate `right-db` internals
   from `libsql` to `turso`.
4. If a gate fails, stop and document the blocker instead of forcing the
   migration.
5. Keep all cloud sync behavior deferred, even though the sync-capable crate is
   present.

This is intentionally narrower than a cloud feature and broader than a pure
dependency swap.

## Architecture

The local runtime architecture becomes:

```text
bot / aggregator / CLI / dashboard
        |
    right-db project API
        |
 local turso database engine
        |
     agent data.db
```

`right-db` remains the sole database boundary. Other crates continue to use
project-owned types:

- `right_db::Connection`
- `right_db::Transaction`
- `right_db::Row`
- `right_db::DbError`
- `right_db::open_connection`
- `right_db::open_db`
- read-only open helpers

No crate outside `right-db` should depend on `turso` or expose raw `turso`
connection, transaction, row, value, parameter, or error types.

The public API should stay stable unless the implementation proves that a
current API is semantically unsafe with `turso`. Any unavoidable public API
change must be justified by a concrete compatibility failure.

## Components

### Dependency Boundary

The workspace should remove the `libsql` dependency once no project code needs
it. `right-db` should depend on `turso` with the `sync` feature enabled. The
sync feature is present only to avoid another dependency migration when future
Turso Cloud support is designed.

The implementation plan must not assume the currently known `turso` version.
It must query the registry before editing `Cargo.toml`.

### Connection

`right_db::Connection` should wrap local `turso` database state and expose the
same project-level operations where possible:

- `open_in_memory`
- local file open with create/read-only behavior
- `execute`
- `execute_batch`
- `query_row`
- `query_all`
- `prepare`
- `last_insert_rowid`
- transaction creation
- connection pragmas or their `turso` equivalents

The local file path remains `<agent>/data.db`.

### Transactions

The current `Transaction` implementation has a documented
`Deref<Target = Connection>` invariant that depends on local `libsql` handle
behavior. This is the riskiest part of the backend swap.

The `turso` migration must choose one of two paths:

- prove the same invariant with a regression test showing helper writes reached
  through `&Transaction` roll back with the outer transaction; or
- remove the `Deref` convenience and require transaction-aware helper calls.

The implementation should prefer removing the invariant if the churn is
reasonable. Keeping it is acceptable only if the compatibility test is explicit
and the warning is rewritten for `turso`.

The transaction rule remains unchanged: any operation with two or more writes
uses one immediate transaction, and `with_immediate_transaction` centralizes
rollback-on-error.

### Rows, Params, And Errors

Rows, params, values, and errors remain project-owned. Existing conversions
from `libsql` types are replaced with `turso` equivalents inside `right-db`.

Public callers should continue seeing `DbError`, `Row`, and `IntoParams`
behavior rather than driver-specific types.

### Migrations

`right-db` remains the sole migration owner. The migration runner must preserve:

- ordered `MIGRATIONS`;
- `PRAGMA user_version` semantics;
- all pending migrations inside one immediate transaction;
- all-or-nothing rollback when a later migration fails;
- idempotent migration behavior;
- concurrent bot/aggregator startup safety.

If `turso` differs in PRAGMA, transaction, or DDL behavior, the migration must
adapt inside `right-db` without exposing that difference upward.

## Compatibility Gate

Before broad migration, the implementation must prove or reject these local
requirements with narrow tests/probes:

- `turso` can open a `data.db` created by the current local `libsql`
  implementation.
- Current migrations run to latest and preserve `user_version` expectations.
- Failed migration batches roll back and include database path plus migration
  version in the error.
- FTS/search tables work for existing memory and conversation search.
- Triggers still fire.
- `RETURNING` queries still work.
- Read-only opens do not create or mutate database files.
- Transactions roll back helper writes correctly.
- Constraint, not-found, type-conversion, busy, and locked errors remain
  observable through `DbError`.
- Concurrent cold-boot migrators do not corrupt schema state.

If any gate fails, stop the implementation and write down the exact blocker and
minimum future work needed. Do not paper over compatibility gaps with silent
fallbacks.

## Data Flow

Current local behavior remains:

```text
right_db::open_connection(agent_dir, migrate: true)
  -> <agent>/data.db
  -> local turso engine
  -> right-db migrations if requested
  -> project-owned Connection
```

Read-only consumers keep using read-only helpers and must not create, migrate,
or mutate `data.db`.

Future cloud sync is only a deferred extension point:

```text
local data.db
  -> future right-db sync API
  -> future turso push/pull
  -> Turso Cloud
```

No step in that future flow is implemented in this stage.

## Error Handling

Driver errors are normalized in `right-db`. Public code should not learn about
`turso::Error`.

The migration should preserve project-level handling for:

- absent rows;
- read-only open failures;
- constraint violations;
- busy or locked database failures;
- invalid parameters and type conversion failures;
- migration failures with database path and migration version;
- backend unsupported-feature failures found during compatibility testing.

The implementation does not need byte-for-byte driver error compatibility, but
it must preserve the semantic categories that callers and tests rely on.

## Documentation

Update architecture docs if the migration lands:

- `ARCHITECTURE.md`: local database driver rule changes from local `libsql` to
  local `turso`.
- `docs/architecture/modules.md`: `right-db` module map and database boundary.
- `docs/architecture/memory.md`: only if memory storage behavior or search
  details drift.

`PROMPT_SYSTEM.md` is not expected to change because this work does not affect
agent-facing prompts, schemas, or MCP tool instructions.

## Testing And Verification

Use TDD for behavior changes. The implementation plan should use this cadence:

- Run a targeted `right-db` baseline before edits.
- Add compatibility-gate tests before production internals are broadly
  migrated.
- Run targeted `right-db` tests after each coherent migration slice.
- Run dependent package tests after `right-db` compiles and passes.
- Finish with the mandatory full workspace checks:

```bash
devenv shell -- cargo test --workspace
devenv shell -- cargo build --workspace
```

## Acceptance Criteria

- `right-db` uses `turso` with the `sync` feature instead of `libsql`.
- `libsql` is removed from workspace dependencies if no longer needed.
- Existing callers keep using project-owned `right_db` APIs.
- No cloud sync behavior, credentials, UI, CLI, bot command, or scheduler is
  introduced.
- Existing local `data.db` behavior is preserved.
- Compatibility-gate tests pass or the implementation stops with a documented
  blocker.
- Architecture docs match the new local `turso` foundation.
- `devenv shell -- cargo test --workspace` passes.
- `devenv shell -- cargo build --workspace` passes.

## Deferred Work

Future specs should cover these separately:

- Turso Cloud configuration and credential custody.
- Local-to-cloud attach/bootstrap flow.
- Explicit `push()` and `pull()` API design.
- Sync health, conflict behavior, and doctor output.
- Bot, CLI, and dashboard controls for enabling Turso Cloud backup.
- Restore semantics from Turso Cloud into an existing or new agent.
