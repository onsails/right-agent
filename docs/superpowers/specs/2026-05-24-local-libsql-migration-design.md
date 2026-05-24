# Local libSQL Migration Design

## Context

Right Agent currently stores each agent's local state in `<agent>/data.db`
through `rusqlite`. The central open/migrate functions live in
`crates/right-db`, but the raw `rusqlite::Connection`, `rusqlite::Transaction`,
`rusqlite::Row`, and `rusqlite::Error` types leak through many crates.

The first milestone is a local-only migration from `rusqlite` to `libsql`.
Cloud Turso databases, embedded replicas, backup/restore flows, and sync health
checks are intentionally future work.

Context7 documentation for `libsql` confirms the Rust API supports local
SQLite-compatible files through `Builder::new_local`, remote databases through
`Builder::new_remote`, embedded replicas through `Builder::new_remote_replica`,
and async transaction APIs. This design uses only the local file mode.

## Goals

- Use local `libsql` for per-agent `data.db`.
- Make `right-db` the only crate that owns driver-specific types.
- Preserve the existing local database file location and schema.
- Preserve current migration order and idempotency expectations.
- Preserve FTS5, triggers, `RETURNING`, read-only open, and busy/locked
  behavior through explicit tests.
- Keep the implementation compatible with already-deployed agents. No agent
  recreation, sandbox deletion, or manual database migration is allowed.

## Non-Goals

- No Turso remote URL configuration.
- No embedded replica mode.
- No cloud backup or restore feature.
- No dashboard or bot UX for cloud storage.
- No broad storage redesign beyond the driver boundary required for `libsql`.

## Architecture

`right-db` becomes the project database boundary. Public callers stop accepting
or returning raw driver types and instead use project-owned abstractions:

```rust
// crates/right-db/src/lib.rs
pub struct Connection { /* libsql-owned internals */ }
pub struct Transaction<'conn> { /* libsql-owned internals */ }

pub fn open_connection(agent_path: &Path, migrate: bool) -> Result<Connection, DbError>;
pub fn open_db(agent_path: &Path, migrate: bool) -> Result<(), DbError>;
pub fn open_connection_readonly(agent_path: impl AsRef<Path>) -> Result<Connection, DbError>;
```

The public contract is `right_db`, not `libsql`. Domain crates should not depend
on `libsql` unless they are tests that explicitly verify driver behavior. SQL
that is storage-owned should move into `right-db` modules when that avoids
leaking row or transaction APIs upward.

The data flow stays local:

```text
bot / aggregator / CLI / dashboard
        |
    right-db API
        |
 local libsql database
        |
 agent data.db
```

## Components

### Connection

`right_db::Connection` owns the `libsql::Database` and connected handle needed
for local operations. It exposes project-level methods for executing SQL,
querying rows, opening transactions, and setting required pragmas.

The abstraction must not try to mimic all of `rusqlite`. It should expose only
the operations Right Agent actually uses.

### Transactions

`right_db::Transaction` replaces direct `unchecked_transaction()` usage.
Multi-write operations continue to use one transaction. The current transaction
rule remains: any operation with two or more writes must be atomic.

The implementation plan must explicitly handle async transaction APIs. Hidden
blocking in Telegram worker hot paths is not acceptable unless the plan names
the `spawn_blocking` boundary and tests it.

### Rows And Typed Queries

Callers should not receive `libsql::Row`. Query-heavy logic should use one of
two patterns:

- Move storage-owned query functions into `right-db`.
- Return typed DTOs from small `right-db` helpers.

This is especially important for dashboard read models, conversation archive
search, MCP credential storage, async runs, cron runs, usage events, and memory
resilience queues.

### Migrations

`right-db` remains the sole migration owner. If `rusqlite_migration` cannot run
against `libsql`, replace it with a `right-db` migration runner that preserves
the current ordered migration list and `user_version` behavior.

Migrations must remain idempotent. Conditional column additions continue to
query `pragma_table_info`; table, index, trigger, and FTS creation continue to
use `IF NOT EXISTS` where SQLite supports it.

### Read-Only Opens

`open_connection_readonly` must keep the structural guarantee that dashboard
read paths do not create or mutate `data.db`. Missing database files must fail
instead of being created.

## Error Handling

Driver errors are normalized in `right-db`. Public code should see
`right_db::DbError` or domain errors that wrap it, not `libsql::Error` or
`rusqlite::Error`.

Current code that pattern-matches `rusqlite::Error::QueryReturnedNoRows`,
constraint failures, or invalid-parameter errors needs project-level
equivalents. Migration failures must include the database path and migration
version. Busy/locked failures must stay observable in logs and returned errors;
they must not be silently swallowed.

## Compatibility Requirements

- Existing `<agent>/data.db` files remain in place.
- Existing migrations run on existing databases without manual steps.
- FTS5 tables and triggers continue to work for memory and conversation search.
- `RETURNING` queries used by conversation archive continue to work.
- Dashboard read-only paths do not mutate state.
- Multiple processes can still open the same agent DB safely under the current
  bot/aggregator migration model.

## Testing And Verification

The implementation plan should use this cadence:

- Start with a baseline targeted check: `devenv shell -- cargo test -p right-db`.
- Add narrow failing tests for the new `right_db::Connection` contract before
  implementation.
- Test migrations against real temp `data.db` files, not only in-memory
  databases.
- Add targeted tests for FTS5, triggers, `RETURNING`, read-only open, and
  transaction rollback.
- Run targeted package tests as each migrated crate leaves raw `rusqlite`
  behind.
- Finish with the mandatory full workspace check:
  `devenv shell -- cargo test --workspace`.

## Documentation

Update these docs during implementation:

- `ARCHITECTURE.md`: SQLite/libSQL rules and transaction rule.
- `docs/architecture/memory.md`: pending-retain queue storage details if the
  API changes are visible there.
- `docs/architecture/modules.md`: module map for `right-db` and any moved
  storage helpers.

`PROMPT_SYSTEM.md` is not expected to change because this milestone does not
change agent-facing prompts, schemas, or MCP tool instructions.

## Future GitHub Issues

After this local-only migration design is accepted, create GitHub issues for
future work. These issues are out of scope for the local `libsql` migration:

- Turso cloud configuration model.
- Embedded replica and sync mode.
- Cloud backup and restore UX.
- Sync health checks and doctor output.
- Migration path from local-only `data.db` to cloud-backed storage.

Each issue should state that the local `libsql` migration is the prerequisite
and should avoid assuming a specific cloud rollout sequence beyond the
`right-db` boundary.
