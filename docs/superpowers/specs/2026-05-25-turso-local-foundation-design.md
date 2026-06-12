# Turso Local Foundation And FTS Migration Design

> **Superseded in part (onsails/right-agent#79):** the pre-Turso bundled-
> `rusqlite` legacy FTS5 scrubber specified here was removed after deployed
> databases soaked past migration v34. Pre-v34 SQLite FTS5 cleanup is no longer
> supported in-process; databases still carrying legacy FTS5 virtual tables
> that Turso cannot open now fail the open instead of being scrubbed. Migration
> v34 remains idempotent for databases Turso can already open.

## Context

Right Agent stores each agent's local state in `<agent>/data.db` behind the
`right-db` crate. The previous local `libsql` migration already established the
important boundary: callers use project-owned `right_db` types instead of raw
database-driver types.

The next storage direction is Turso Cloud push/pull backup. The local
foundation should therefore use the `turso` Rust crate with its `sync` feature
available, while leaving cloud URLs, credentials, push/pull calls, UI, CLI, bot
commands, and schedulers out of this stage.

The first implementation probe found a blocker for a pure driver swap:
`turso 0.7.0-pre.3` opens the existing database file, but cannot resolve a
legacy SQLite FTS5 schema reliably: tables that appear after an old FTS5
virtual table may be invisible to Turso's schema resolver. Turso's current Rust
surface provides a different FTS model: enable
`Builder::experimental_index_method(true)` and create indexes with
`CREATE INDEX ... USING fts (...)`.

This design therefore expands the local migration from a driver swap into a
small schema and query migration for search. That is the cost of choosing the
Turso path now.

Relevant upstream references:

- `https://docs.turso.tech/sdk/rust/quickstart`
- `https://docs.turso.tech/sdk/rust/reference`
- `https://github.com/tursodatabase/turso`
- `turso 0.7.0-pre.3` crate source:
  `Builder::experimental_index_method(true)` and
  `CREATE INDEX idx ON table USING fts (...)`

## Goals

- Replace runtime `libsql` usage in `right-db` with the `turso` crate.
- Enable the `turso` `sync` feature as local preparation for later Turso Cloud
  push/pull backup work.
- Enable Turso's experimental index method on every local database open.
- Replace SQLite FTS5 virtual-table based search with Turso FTS indexes.
- Remove legacy SQLite FTS5 virtual tables and sync triggers before Turso
  migrates old database files.
- Preserve the existing `<agent>/data.db` path, migration runner, and
  project-owned public API shape as far as Turso permits.
- Keep raw driver types hidden inside `right-db`.
- Prove fresh local databases and legacy `libsql`-created databases can still
  migrate, write, and search locally.

## Non-Goals

- No Turso Cloud URL, auth token, org, database-name, or config model.
- No `push()`, `pull()`, `checkpoint()`, sync stats, or sync scheduler.
- No dashboard, Telegram bot, CLI, or agent-facing UI for cloud storage.
- No cloud backup or restore UX.
- No object-storage backup design.
- No exact byte-for-byte compatibility with SQLite FTS5 snippets or ranking.
- No unrelated schema redesign beyond the FTS migration required by Turso.

## Recommended Approach

Use a local Turso migration with explicit FTS conversion:

1. Keep the `right-db` boundary. Do not expose `turso` types outside it.
2. Add `turso` with the `sync` feature while keeping `libsql` only long enough
   for compatibility probes and legacy fixture construction.
3. Replace the failed SQLite FTS5 gate with a Turso FTS gate that proves
   `experimental_index_method(true)`, `CREATE INDEX ... USING fts`, `MATCH`,
   triggers, `RETURNING`, and immediate transactions.
4. Port the runtime wrapper from `libsql` to `turso`.
5. Change fresh schema SQL to create Turso FTS indexes instead of SQLite FTS5
   virtual tables and sync triggers.
6. Add a pre-Turso legacy scrubber inside `right-db` that uses bundled
   `rusqlite` only to drop old SQLite FTS5 virtual tables and sync triggers
   before writable Turso opens through `open_connection`.
7. Add a new migration for existing databases that drops any remaining old FTS5
   sync triggers/tables and creates Turso FTS indexes over the base tables.
8. Rewrite search queries to search base tables with Turso `MATCH`.
9. Remove runtime and test dependency on `libsql` after the gates no longer need
   it.

The old SQLite FTS5 virtual tables must not remain in legacy database files
after a migrated open. Turso cannot perform that cleanup itself on real legacy
schemas, so the scrubber is the only allowed non-Turso database operation in
runtime code. It must be private to `right-db`, run before every writable
`open_connection(...)` open, skip read-only helpers, and drop only the known
legacy FTS5 objects. Backup paths that open with `migrate: false` still need
this cleanup because Turso must resolve the source schema before `VACUUM INTO`.

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

`right-db` remains the sole database boundary. Other crates continue to use:

- `right_db::Connection`
- `right_db::Transaction`
- `right_db::Row`
- `right_db::DbError`
- `right_db::open_connection`
- `right_db::open_db`
- read-only open helpers

No crate outside `right-db` should depend on `turso` or expose raw `turso`
connection, transaction, row, value, parameter, or error types.

## Components

### Dependency Boundary

The workspace should depend on `turso` with the `sync` feature enabled. Runtime
code should not depend on `libsql` after the migration lands. Temporary tests
may use `libsql` during the transition, but final committed code should remove
it unless a test fixture has no other practical replacement.

The implementation must query the crate registry before editing `Cargo.toml`.
At design time the current version is `0.7.0-pre.3`.

### Connection

`right_db::Connection` should wrap local `turso` database state and expose the
same project-level operations:

- `open_in_memory`
- local file open with create/read-only behavior
- `execute`
- `execute_batch`
- `query_row`
- `query_all`
- `prepare`
- `last_insert_rowid`
- transaction creation
- connection pragmas or Turso equivalents

Every local Turso builder used by `right-db` must call:

```rust
turso::Builder::new_local(path).experimental_index_method(true)
```

Future cloud-sync builders must enable the equivalent sync builder flag, but no
cloud builder is introduced in this stage.

### Search Schema

Fresh local databases should no longer create SQLite FTS5 virtual tables:

- no `memories_fts` virtual table for fresh schema;
- no `conversation_messages_fts` virtual table for fresh schema;
- no FTS5 sync triggers for fresh schema.

Fresh local databases should create Turso FTS indexes:

```sql
CREATE INDEX IF NOT EXISTS idx_memories_turso_fts
ON memories USING fts(content);

CREATE INDEX IF NOT EXISTS idx_conversation_messages_turso_fts
ON conversation_messages USING fts(content);
```

Legacy databases created by earlier versions may still contain
`memories_fts`, `conversation_messages_fts`, and their sync triggers. The
pre-Turso scrubber removes those real SQLite FTS5 objects before Turso opens the
file. Migration v34 remains idempotent for partially migrated or synthetic
legacy schemas and creates the Turso FTS indexes over the base tables.

### Search Queries

Conversation search should query `conversation_messages` directly:

```sql
WHERE content MATCH ?
```

The existing platform, chat, thread, role, and ordering constraints remain
server-side SQL predicates.

SQLite-specific `snippet(conversation_messages_fts, ...)` is not available.
Search result snippets should be generated in Rust from the matched
`conversation_messages.content`. Exact SQLite snippet formatting is not an
acceptance criterion; deterministic, bounded snippets are.

Legacy memory search in `crates/right/src/main.rs` must stop querying
`memories_fts` and use the base `memories` table with `content MATCH ?`.

### Transactions

The current `Transaction` type implements `Deref<Target = Connection>` so
helpers taking `&Connection` can run inside an open transaction. That invariant
was explicitly tied to the local `libsql` handle model.

The Turso migration must either:

- prove the same invariant with a regression test showing helper writes reached
  through `&Transaction` roll back with the outer transaction; or
- remove the `Deref` convenience and require transaction-aware helper calls.

Keeping `Deref` is acceptable only if the regression test passes under Turso and
the warning text is rewritten for the new backend.

The transaction rule remains unchanged: multi-write operations use one
immediate transaction and `with_immediate_transaction` centralizes
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

The new Turso FTS migration should advance `LATEST_SCHEMA_VERSION` from `33` to
`34`.

## Compatibility Gates

Before removing `libsql`, the implementation must prove:

- Turso can open a `data.db` created by the current `libsql` backend.
- Turso supports the required local SQL surface with index-method FTS:
  `CREATE INDEX ... USING fts`, `MATCH`, triggers, `RETURNING`, and immediate
  transaction rollback.
- Fresh schema under Turso creates Turso FTS indexes and no longer depends on
  SQLite FTS5 modules.
- Real legacy FTS5 virtual tables and sync triggers are removed before Turso
  opens the database for migration; v34 handles any remaining idempotent cleanup
  and creates Turso FTS indexes.
- Legacy database writes after v34 do not attempt to write old FTS5 virtual
  tables.
- Conversation search and memory search return rows through Turso FTS.
- Read-only opens do not create or mutate database files.
- Constraint, not-found, type-conversion, busy, and locked errors remain
  observable through `DbError`.
- Concurrent cold-boot migrators do not corrupt schema state.

If a gate fails, stop and document the exact blocker. Do not hide gaps with
fallback search paths that silently bypass Turso FTS.

## Data Flow

Current local behavior remains:

```text
right_db::open_connection(agent_dir, migrate: bool)
  -> <agent>/data.db
  -> legacy FTS5 scrubber if needed (writable opens only)
  -> local turso engine with index_method enabled
  -> right-db migrations if requested
  -> project-owned Connection
```

Read-only consumers keep using read-only helpers and must not create, migrate,
or mutate `data.db`.

Future cloud sync remains deferred:

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
- unsupported-feature failures found during compatibility testing.

The implementation does not need byte-for-byte driver error compatibility, but
it must preserve the semantic categories callers and tests rely on.

## Documentation

Update architecture docs if the migration lands:

- `ARCHITECTURE.md`: local database driver and search rules change from local
  `libsql` plus SQLite FTS5 to local `turso` plus Turso FTS indexes.
- `docs/architecture/modules.md`: `right-db` module map and database boundary.
- `docs/architecture/memory.md`: memory and conversation search storage details.

`PROMPT_SYSTEM.md` is not expected to change because this work does not affect
agent-facing prompts, schemas, or MCP tool instructions.

## Testing And Verification

Use TDD for behavior changes. The implementation plan should use this cadence:

- Run a targeted `right-db` baseline before edits.
- Add compatibility-gate tests before broad runtime migration.
- Run targeted `right-db` tests after each coherent migration slice.
- Run dependent package tests after `right-db` compiles and passes.
- Finish with:

```bash
devenv shell -- cargo test --workspace
devenv shell -- cargo build --workspace
```

## Acceptance Criteria

- Runtime `right-db` uses `turso` with the `sync` feature instead of `libsql`;
  bundled `rusqlite` is limited to the pre-Turso legacy FTS5 scrubber.
- Every local Turso open enables `experimental_index_method(true)`.
- Fresh databases use Turso FTS indexes, not SQLite FTS5 virtual tables.
- Legacy FTS5 virtual tables and sync triggers are removed before writable
  Turso opens.
- Conversation and memory search use Turso FTS over base tables.
- Existing callers keep using project-owned `right_db` APIs.
- No cloud sync behavior, credentials, UI, CLI, bot command, or scheduler is
  introduced.
- Architecture docs match the new local Turso foundation.
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
