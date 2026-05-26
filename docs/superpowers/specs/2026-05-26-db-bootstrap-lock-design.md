# DB Bootstrap Lock Design

## Context

Right Agent stores per-agent runtime state in `agents/<name>/data.db` through
`right-db`. During the Turso local migration, legacy SQLite FTS5 virtual tables
became incompatible with opening some old databases through Turso. The current
code works around that by running a bundled-`rusqlite` legacy FTS5 scrubber
before every writable `open_connection`, including `migrate = false` runtime
opens.

That places schema repair on the hot path. Even when a database is already at
schema version 34 or newer, the scrubber still opens the file with `rusqlite`
and reads `PRAGMA user_version` to decide that no work is needed. Under normal
live activity, `right-mcp-server`, bot runtime tasks, cron, async delivery, and
learning workers can hold the same database open. The repeated `rusqlite` probe
can therefore hit SQLite busy/lock errors even though no migration is needed.

Old FTS index data is not required. The durable data is in base tables such as
`memories` and `conversation_messages`; the legacy `*_fts` virtual tables and
their triggers may be dropped during schema bootstrap.

## Goals

- Remove legacy FTS5 probing and schema repair from `migrate = false` runtime
  opens.
- Keep legacy database upgrades reliable: old v33 databases must still become
  openable through Turso and migrate to the latest schema.
- Make concurrent startup of `right-mcp-server` and bot processes correct
  without relying on process-compose ordering.
- Keep schema-changing work in startup/bootstrap paths, not hidden inside
  normal runtime access.

## Non-Goals

- Do not introduce a single runtime DB writer task.
- Do not preserve old FTS index contents.
- Do not delete or rewrite base tables.
- Do not replace the `open_connection(agent_dir, migrate)` API in this change.
  A later refactor may split migration into a clearer explicit API.

## Architecture

`right-db` will define one schema bootstrap path: `open_connection(agent_dir,
true)`. That path is responsible for all pre-open legacy cleanup and migration.
It will acquire a per-agent OS advisory lock before touching schema state.

`open_connection(agent_dir, false)` becomes a normal runtime open. It opens
Turso, applies connection pragmas, and returns. It does not use `rusqlite`, does
not inspect legacy FTS tables, does not apply migrations, and does not acquire
the migration lock.

The v34 migration remains the canonical schema change that drops legacy FTS5
objects:

- `memories_fts`
- `conversation_messages_fts`
- `memories_ai`, `memories_ad`, `memories_au`
- `conversation_messages_ai`, `conversation_messages_ad`,
  `conversation_messages_au`

The pre-Turso cleanup still exists, but only as part of the locked
`migrate = true` bootstrap path. It is needed because Turso may fail to open a
legacy database that still contains SQLite FTS5 virtual tables.

## Startup Concurrency

Bot startup already opens the agent DB with `migrate = true` before starting the
main Telegram, cron, and async-delivery runtime. The MCP memory server also
opens its DB with `migrate = true` before serving requests.

Those processes may start concurrently. Correctness must come from `right-db`,
not from external process ordering. `open_connection(agent_dir, true)` will:

1. Create or open a per-agent lock file, for example
   `agent_dir/.right-db-migrate.lock`.
2. Acquire an exclusive advisory lock.
3. Run the pre-Turso legacy FTS5 cleanup if the DB exists and is legacy.
4. Open the Turso connection.
5. Apply connection pragmas.
6. Run `MIGRATIONS.to_latest()`.
7. Release the lock when the guard drops.

If two startup processes race, one performs bootstrap while the other waits.
After the first process commits, the second process enters the lock, observes
the latest schema, and its migration work is a no-op.

The existing migration transaction remains useful. It protects ordinary
migrations and handles internal race safety. The new advisory lock protects the
pre-Turso cleanup phase, which happens before a Turso migration transaction can
exist.

## Error Handling

Startup migration errors are fatal to the process. If the lock cannot be
acquired, legacy cleanup fails, Turso cannot open the database, or migrations
fail, the process must not continue into runtime loops with an unknown schema.

Runtime `migrate = false` opens do not attempt repair. If they encounter a
legacy DB because startup bootstrap did not run, the error should surface with
context. That is a startup invariant violation, not a runtime self-healing path.

The migration lock must be an OS advisory lock rather than a marker file.
Process crashes release the lock automatically. Lock wait/failure logs should
include the database or lock path and process identity where available.

## Tests

The test suite should encode the new invariant:

- `open_connection(..., false)` does not run the legacy scrubber. A legacy v33
  FTS5 fixture should not have `memories_fts` or `conversation_messages_fts`
  dropped by a runtime open. If Turso cannot open that fixture, the test should
  assert failure without schema mutation.
- `open_connection(..., true)` upgrades a real legacy v33 FTS5 DB: legacy FTS
  tables/triggers are removed, Turso FTS indexes exist, and `user_version`
  reaches the latest schema version.
- concurrent `open_connection(..., true)` calls on the same legacy DB serialize
  through the migration lock and both complete at the latest schema.
- the existing test named
  `open_connection_without_migration_scrubs_legacy_fts5` should be replaced by
  the opposite invariant: runtime opens do not scrub legacy FTS5.

Targeted verification while implementing should start with `right-db` tests
around legacy FTS and migration locking. Final verification must run:

- `devenv shell -- cargo test --workspace`
- `devenv shell -- cargo build --workspace`

## Documentation

`ARCHITECTURE.md` must be updated with the new migration ownership rule:

- `migrate = true` is the only schema bootstrap path.
- startup bootstraps are concurrency-safe through a per-agent advisory lock.
- `migrate = false` runtime opens do not run legacy cleanup or migrations.

The current architecture text saying the scrubber runs before any writable
Turso open, including `migrate = false` backup paths, must be removed.
