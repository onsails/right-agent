# Local libSQL Finalization Design

## Context

Stage 1 moved Right Agent's per-agent `data.db` from direct `rusqlite`
ownership to a local-only `libsql` driver boundary in `right-db`. The prior
design intentionally excluded Turso cloud configuration, embedded replicas,
sync health, cloud backup, and local-to-cloud migration.

This finalization stage keeps that boundary local-only. Its job is to prove the
local migration is shippable, remove migration-era risk where it is local and
concrete, and document the final local database contract.

Current repo observations:

- `right-db` owns project database types: `Connection`, `Transaction`, `Row`,
  params, and `DbError`.
- Direct `rusqlite` and `rusqlite_migration` usage is absent from `Cargo.toml`
  and `crates/`.
- `ARCHITECTURE.md` already describes local libSQL as hidden behind `right-db`.
- `Transaction` currently implements `Deref<Target = Connection>` with a
  documented backend-swap warning. That is acceptable only if local-handle
  transaction semantics are tested and treated as a local-only invariant.

Context7 documentation for `libsql` confirms that local-only Rust usage is
through `Builder::new_local`, optional `OpenFlags`, `Database::connect`,
`transaction_with_behavior(TransactionBehavior::Immediate)`, async
`execute`/`query`, and SQLite-compatible local files. This stage uses only that
local mode.

## Goals

- Finalize the local `libsql` migration for production use.
- Verify no raw `rusqlite` dependency or driver type leaks remain in project
  code.
- Re-audit `right-db` runtime, transaction, migration, read-only, parameter,
  row, and error semantics.
- Fix only confirmed local-driver defects or sharp edges.
- Preserve existing `data.db` paths, schema versions, migrations, dashboard
  read-only behavior, FTS, triggers, `RETURNING`, and concurrent bot/aggregator
  startup behavior.
- Update architecture docs only where the audit changes or clarifies local
  database rules.

## Non-Goals

- No Turso remote URL, token, or config model.
- No embedded replica mode.
- No sync loop, sync health, or cloud doctor checks.
- No cloud backup or restore UX.
- No local-to-cloud migration path.
- No schema redesign unrelated to local migration correctness.
- No broad cleanup outside the `right-db` local database boundary and immediate
  callers needed to verify it.

## Recommended Approach

Use a full local finalization pass with strict scope control:

1. Audit the local DB surface and recent migration commits.
2. Run targeted `right-db` and dependent package checks to expose current
   failures before edits.
3. Harden confirmed local risks with narrow regression tests first.
4. Clean up only APIs whose current shape creates a local correctness or
   maintainability risk.
5. Update docs when a rule is clarified or changed.
6. Finish with full workspace tests and debug build.

This is broader than a pure verification pass but narrower than cloud prep. The
stage ends when local `data.db` behavior is verified and there are no known
local-driver footguns in `right-db`.

## Architecture

The architecture stays:

```text
bot / aggregator / CLI / dashboard
        |
    right-db API
        |
 local libSQL database
        |
 agent data.db
```

`right-db` remains the sole owner of `libsql` types and local database opening.
Other crates use project-owned APIs and must not expose raw driver connection,
transaction, row, parameter, or error types in public interfaces.

The stage may adjust internals of `right-db`, but it must not introduce a
runtime storage mode switch. `open_connection`, `open_db`,
`open_connection_readonly`, and `open_database_path_readonly` remain local-file
operations.

## Components To Audit

### Connection Runtime

Audit `Connection::open_local`, the shared Tokio runtime, `block_on_libsql`,
open flags, WAL setup, and `busy_timeout`. The implementation must be safe when
called from async bot/dashboard code and from synchronous CLI code. If runtime
behavior is changed, add tests or comments that explain the boundary.

### Transactions

Audit `Connection::transaction`, `with_immediate_transaction`, `Transaction`,
rollback-on-error, and the `Deref<Target = Connection>` invariant. The local
contract is:

- multi-write operations use one immediate transaction;
- operation errors trigger rollback and return the original error;
- rollback failure is logged but does not hide the operation error;
- helper writes reached through `Transaction` participate in the transaction
  only because the current local libSQL backend shares the underlying handle.

If that last invariant is not adequately tested, add a narrow regression test.
Do not design for remote transaction semantics in this stage.

### Migrations

Audit the custom migration runner for:

- all pending migrations inside one immediate transaction;
- all-or-nothing rollback with `user_version` unchanged on failure;
- idempotent column/table/index/trigger creation;
- concurrency behavior when bot and aggregator both open an unmigrated DB;
- clear migration errors with path and version.

Fix migration issues only if they affect local databases.

### Read-Only Opens

Audit dashboard and restore read-only paths. Read-only opens must not create
`data.db`, write WAL files in surprising paths, run migrations, or mutate
schema. Missing database files must fail structurally.

### Parameters, Rows, And Errors

Audit `params!`, `IntoParams`, `Row::get`, optional-row handling, constraint
classification, and domain error wrapping. The API should stay project-owned
and boring. Cleanup is in scope only where the current compatibility layer is
misleading, lossy, or likely to cause local DB bugs.

## Data Flow

The data flow is unchanged:

1. Bot and aggregator open `<agent>/data.db` through
   `right_db::open_connection(path, migrate: true)`.
2. CLI and dashboard paths use `migrate: false` or read-only helpers unless
   they intentionally own startup migration.
3. `right-db` applies local pragmas and migrations.
4. Domain crates execute project-owned query helpers against
   `right_db::Connection` or `right_db::Transaction`.

No cloud service is contacted. No data leaves the local `data.db`.

## Error Handling

Driver errors stay normalized as `right_db::DbError` or domain errors wrapping
it. The finalization pass should preserve:

- `NotFound` for absent rows;
- structural open errors for read-only/missing database cases;
- constraint detection for SQLite/libSQL constraint failures;
- migration errors that include database path and migration version;
- visible busy/locked failures rather than silent fallback.

Any new error variant must be justified by a concrete local failure mode.

## Testing And Verification

Use TDD for behavior changes: write the narrowest failing regression test
before fixing a confirmed bug.

Baseline and audit checks:

```bash
devenv shell -- rg -n "rusqlite::|use rusqlite|rusqlite =|rusqlite_migration" Cargo.toml crates
devenv shell -- cargo test -p right-db
```

Targeted checks depend on findings, but likely include:

```bash
devenv shell -- cargo test -p right-memory
devenv shell -- cargo test -p right-mcp
devenv shell -- cargo test -p right-agent
devenv shell -- cargo test -p right-dashboard
devenv shell -- cargo test -p right-bot
devenv shell -- cargo test -p right
```

Final verification is mandatory:

```bash
devenv shell -- cargo test --workspace
devenv shell -- cargo build --workspace
```

## Documentation

Re-read and update these files if the audit changes or clarifies their touched
subsystems:

- `ARCHITECTURE.md`
- `docs/architecture/modules.md`
- `docs/architecture/memory.md`
- any local DB comments that still imply direct `rusqlite` ownership

`PROMPT_SYSTEM.md` is not expected to change because this stage does not alter
agent-facing prompts, schemas, or MCP tool instructions.

## Acceptance Criteria

- Direct `rusqlite` and `rusqlite_migration` usage remains absent from project
  code and manifests.
- Local `libsql` open, read-only open, migrations, FTS/triggers, `RETURNING`,
  transactions, and rollback behavior are covered by tests or existing verified
  tests.
- Any confirmed local defects found during audit are fixed with regression
  coverage.
- `right-db` public APIs remain project-owned and do not leak raw `libsql`
  driver types.
- Architecture docs match the final local DB contract.
- `devenv shell -- cargo test --workspace` passes.
- `devenv shell -- cargo build --workspace` passes.

## Explicit Deferred Work

These remain separate future projects:

- Turso cloud configuration model.
- Embedded replica and sync mode.
- Cloud backup and restore UX.
- Sync health checks and doctor output.
- Migration from local-only `data.db` to cloud-backed storage.
