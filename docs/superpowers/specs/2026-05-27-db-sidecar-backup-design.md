# DB Sidecar Backup Contract Design

## Context

Right Agent stores per-agent runtime state in `agents/<name>/data.db` through
`right-db` and the local Turso driver. Filesystem-backed Turso opens now use
multiprocess WAL so the bot process and MCP aggregator process can open the
same per-agent database concurrently.

Multiprocess WAL can create runtime sidecar files next to the database, such
as `data.db-wal`, `data.db-shm`, and `data.db-tshm`. These files are derived
runtime coordination state. They are not the portable backup format.

Current full backups already create the durable DB backup with `VACUUM INTO`
at `backups/<agent>/<timestamp>/data.db`. The gap is no-sandbox backup/restore:
no-sandbox backup tars the host agent directory directly and currently excludes
`data.db` but not `data.db-*` sidecars. A legacy or no-sandbox backup can
therefore restore stale sidecars beside the canonical `data.db` snapshot.

## Decision

The durable database backup contract is:

- `backup/data.db` is the only portable database state.
- `backup/data.db` must be produced by the existing `VACUUM INTO` path.
- Files matching `data.db-*` are disposable runtime sidecars.
- Backup flows must not archive `data.db-*`.
- Restore flows must remove `data.db-*` from the restored agent directory before
  any restored database is opened.

This rule applies globally to agent backup and restore paths, including
sandboxed agents, no-sandbox agents, destroy safety backups when they use the
agent backup flow, and legacy backups created before this rule existed.

`--include-rebuildable` does not include database sidecars. Rebuildable caches
and database sidecars are different categories: sidecars can be inconsistent
with the `VACUUM INTO` snapshot and must not be treated as forensic database
state without a separate coherent database-freeze design.

## Backup Flow

Full backup keeps the current control-plane structure:

1. Archive sandbox or no-sandbox files into `sandbox.tar.gz`.
2. Copy control-plane files such as `agent.yaml`, `allowlist.yaml`, and
   `policy.yaml`.
3. Create `backup/data.db` with `VACUUM INTO`.
4. Write `backup.json`.

For no-sandbox agents, tar creation must exclude both `data.db` and
`data.db-*`, because the no-sandbox tar source is the host agent directory.
For sandboxed agents, the sandbox tar is separate from host-side `data.db`, but
the same durable database contract still applies.

## Restore Flow

Restore materializes the canonical `backup/data.db` into the target agent
directory and removes stale sidecars from that directory at the last point where
the restore flow can introduce database files, before any code opens the
restored database.

Cleanup must be:

- scoped to the target agent directory only;
- non-recursive;
- basename-based, matching files whose names start with `data.db-`;
- conservative about `data.db` itself;
- safe for symlinks by removing only the matched directory entry, never a
  symlink target;
- safe for legacy backups whose tar archives include old sidecars.

For no-sandbox restore, cleanup must run after `sandbox.tar.gz` extraction,
because extraction can reintroduce stale `data.db-*` files after config files
have already been copied. For sandboxed restore, cleanup should run after
config-file restore and before codegen or any other path can open the restored
database.

## Non-Goals

- Do not add a forensic backup mode for DB sidecars.
- Do not change the existing `VACUUM INTO` snapshot strategy.
- Do not introduce process-wide stop-the-world backup coordination.
- Do not alter sandbox file backup semantics beyond excluding host-side DB
  sidecars from no-sandbox archives.

## Tests

The implementation should include targeted regression coverage for:

- no-sandbox backup tar excludes `data.db` and `data.db-*`;
- restore removes stale `data.db-*` sidecars from a restored agent directory;
- legacy-style backups containing sidecars are cleaned during restore;
- full backup still produces canonical `backup/data.db` through the existing
  `VACUUM INTO` path.

Final verification for implementation must include:

- targeted backup/restore tests while iterating;
- `devenv shell -- cargo test --workspace`;
- `devenv shell -- cargo build --workspace`.

## Documentation

Update the architecture and lifecycle docs to state that `data.db-*` files are
runtime sidecars and not backup state. The docs should say that `VACUUM INTO`
is the durable database snapshot path and that restored agents recreate sidecar
files on first open.
