# WAL desync self-healing + per-op aggregator connections

**Date:** 2026-06-14
**Status:** Design — pending implementation plan
**Crates touched:** `right-db`, `right` (aggregator / `right_backend`)

## Problem

Agent `riskoff` went functionally dead while its process stayed alive. Every
per-operation DB open failed in a hot loop:

```
ERROR right_bot::cron: failed to load cron specs from DB: database error:
  I/O error: short read on WAL frame at offset 2566792: expected 4096 bytes, got 0
```

The process never exited (`process-compose` reported `Running`, 0 restarts), so
nothing recovered it. Only a manual stop + sidecar removal + `right-mcp-server`
restart brought it back.

### Root cause

Per-agent `data.db` is opened in Turso's **experimental multiprocess-WAL** mode
(`experimental_multiprocess_wal(true)` + `OpenFlags::NoLock` in
`crates/right-db/src/multiprocess_io.rs`). Two host processes hold the same
`data.db` concurrently: the per-agent bot and the shared aggregator
(`right-mcp-server`). Cross-process safety relies entirely on Turso's shared-WAL
coordination, which is **not production ready**
([tursodatabase/turso#769](https://github.com/tursodatabase/turso/issues/769)).

A runtime checkpoint truncated `data.db-wal` to 0 bytes but did not atomically
reset the persisted authority snapshot in `data.db-tshm`, which still recorded
`mxFrame=641`. On every subsequent open, Turso trusts `-tshm`, seeks to frame
623 at offset 2,566,792 in the now-empty WAL, reads past EOF, and fails with
"short read on WAL frame". The desync survives restarts because it lives in the
on-disk `-tshm`/`-shm` sidecars, not in process memory.

### Spike findings (turso 0.7.0-pre.7)

Validated against the real incident fixture
(`crates/right-db/tests/wal_desync_spike.rs`, throwaway):

1. **Turso does not checkpoint on connection drop.** After writing 200 rows and
   dropping the connection, `data.db` stayed at 4 KB (header only) and
   `data.db-wal` held 1.16 MB. Data is durable in the WAL until an internal,
   asynchronous checkpoint runs. We do not control checkpoint timing.
2. **The error surfaces as** `DbError::Open { source: turso::Error::Error("I/O
   error: short read on WAL frame …") }`, with `is_transient() == false`.
3. **Recovery recipe:** delete `data.db-tshm` **and** `data.db-shm`, keep
   `data.db-wal`, then reopen. Turso cold-rebuilds the WAL index from
   `data.db` plus any valid WAL prefix; data is intact (`cron_specs` count
   preserved). Deleting `-shm` alone does **not** work — `-tshm` is the
   authority and must go.
4. **The aggregator holds a long-lived cached connection per agent**
   (`ConnCache = Arc<DashMap<String, Arc<tokio::sync::Mutex<Connection>>>>` in
   `crates/right/src/right_backend.rs`). This is why the live recovery needed an
   `right-mcp-server` restart: deleting sidecars under the aggregator's live FDs
   would leave it writing to unlinked inodes while the bot creates fresh ones —
   split brain.

### Concurrent-writer audit

Every host holder of a per-agent `data.db` connection was reviewed to confirm
the aggregator is the only long-lived concurrent writer left to fix:

- **Bot** (`crates/bot`): opens per-operation via `right_db::open_connection`
  and drops; `worker.rs` / `archive.rs` use short-lived owned/local
  connections. Already correct — no change.
- **Aggregator** `right-mcp-server` (`right_backend.rs` `ConnCache`): long-lived
  per-agent cache. **The one thing to fix (Change 1).**
- **stdio `memory-server`** (`crates/right/src/memory_server.rs`,
  `run_memory_server` → `MemoryServer.conn`) and the older `HttpMemoryServer`:
  **legacy, not launched.** The live `mcp.json` wires only the HTTP aggregator
  (`generate_mcp_config_http`; `pipeline.rs` calls only that), and
  `aggregator.rs` dispatches through `ToolDispatcher` + `RightBackend`
  ("Replaces HttpMemoryServer"). The stdio path's `data.db` is a host path the
  sandbox cannot reach anyway. Not a runtime concurrent writer — no change.

## Goals

- The platform self-heals from this WAL desync without manual intervention,
  consistent with the "self-healing platform" convention in `AGENTS.md`.
- No agent stays wedged in a hot error loop.
- Zero data loss in the observed desync class (empty/truncated WAL after a
  checkpoint).
- Backward compatible: deployable to running agents via `right restart`, no
  sandbox recreation, no migration.

## Non-goals

- Fixing Turso's multiprocess-WAL coordination (upstream; we file an issue).
- Preventing the desync by serializing all cross-process access (rejected — see
  Alternatives).
- Recovering data that only ever existed in a WAL that was truncated before
  checkpoint (already lost at truncation; out of our reach).
- Reworking `memory-server` / `HttpMemoryServer` — audited and confirmed legacy
  and unlaunched (see Concurrent-writer audit). No change needed; removing the
  dead code is a separate cleanup, out of scope.

## Design

Two composing changes.

### Change 1 — Aggregator opens `data.db` per-operation (drop the connection cache)

Remove `ConnCache` from `right_backend.rs`. `get_conn` opens a fresh
`right_db::Connection` for each operation and the caller drops it when done,
matching how the bot already works.

Rationale (this exact wording, or close, goes in a code comment at the cache
removal site and on `get_conn`):

> The aggregator must NOT hold a long-lived `data.db` handle. Turso's
> experimental multiprocess-WAL coordination (tursodatabase/turso#769, not
> production ready) can desync the `-wal`/`-tshm` sidecars under concurrent
> cross-process access. Self-healing recovery repairs the desync by deleting the
> `-tshm`/`-shm` sidecars; a cached connection here would keep writing to the
> unlinked inodes while the bot rebuilds fresh ones — split brain. Opening
> per-operation (like the bot) keeps the concurrency window small and lets
> recovery delete sidecars safely. Do not reintroduce a connection cache.

This change alone removes the split-brain hazard and shrinks the concurrent-open
window that triggers the desync.

### Change 2 — Self-healing recovery in `right_db::open_connection`

Extend the existing retry loop in `crates/right-db/src/lib.rs`. Today it retries
only on `is_transient()` (BUSY / file-lock). Add a one-shot recovery branch for
the WAL-desync signature.

**Detector** (`crates/right-db/src/error.rs`): add
`DbError::is_wal_corruption() -> bool`. True when the error is a
`turso::Error::Error(msg)` (directly or inside `DbError::Open { source, .. }`)
whose `msg` contains `"short read on WAL"` or `"WAL short read"` — the stable
substrings Turso emits for all four short-read variants in
`turso_core::storage::wal`. Deliberately narrow: never match
`turso::Error::Corrupt` or "database header" — main-`data.db` corruption is not
sidecar-recoverable and must propagate.

**Recovery** (`right-db`, new internal fn, e.g. `recover_wal_sidecars`):

1. Acquire the existing per-agent advisory lock (`bootstrap_lock::acquire`) so
   concurrent processes serialize and never reset sidecars simultaneously.
2. Re-probe with a cheap open; if it now succeeds, another process already
   healed it — return.
3. Otherwise delete `data.db-tshm` and `data.db-shm`. Keep `data.db-wal` so a
   non-empty WAL's valid prefix is salvaged on rebuild.
4. Return; the caller reopens once.

**Open loop:** on `is_wal_corruption()` and not-yet-recovered, call
`recover_wal_sidecars`, set a `recovered` flag, and retry the open exactly once.
If the reopen still fails, propagate (FAIL FAST — no infinite loop, no swallow).

A code comment at the recovery branch references
[tursodatabase/turso#769](https://github.com/tursodatabase/turso/issues/769) and
summarizes the recipe ("delete -tshm/-shm authority+index, keep -wal, rebuild
from data.db").

### Concurrency & safety

- Per-op connections (Change 1) mean the bot and aggregator hold `data.db` only
  briefly. When recovery deletes sidecars, a sibling either (a) is not currently
  open, or (b) hits the same corruption on its next open and enters its own
  recovery, blocking on the shared advisory lock until the first finishes, then
  reopens clean. Cold rebuild from a consistent `data.db` + WAL is idempotent.
- Recovery is bounded to one attempt per open call; failure propagates.
- Scope is the open path. Corruption surfacing mid-use on an already-open
  connection is not repaired in place; the next open repairs it. Per-op
  connections make this window negligible.

### Error handling

FAIL FAST throughout. Recovery logs a `tracing::warn!` (agent, path, the turso
error) and proceeds; if the post-recovery reopen fails, the original error
propagates unchanged. Recovery never returns `Ok` on a still-broken database.
Preserve error chains with `format!("{e:#}")` in any string conversion.

### Observability

Emit a `tracing::warn!` on each recovery (fields: `agent`/path, `error`,
`removed` sidecars). Frequent firing is the signal that the experimental Turso
path is chronically unreliable and that the deferred prevention work (below)
should be revisited.

## Testing

Targeted `right-db` / `right` tests, then the full workspace suite (cadence
below).

**Reproduction constraint (spike finding).** The short-read desync cannot be
synthesized deterministically by file manipulation. The spike showed Turso
always rescans the WAL on open and tolerates a torn/truncated tail: truncating
`data.db-wal` to 0 bytes, or mid-frame, just makes Turso rebuild from `data.db`
plus the valid WAL prefix — no error. The real desync occurs only when Turso
trusts a **stale persisted authority** (`-tshm`) accumulated across process
generations without rescanning. Turso's heal-after-sidecar-removal is therefore
established by the spike (Q3) and the live `riskoff` recovery, not re-proven in
CI. Committed tests cover **our** code deterministically:

1. **Detector unit test** (`error.rs`): construct
   `turso::Error::Error("I/O error: short read on WAL frame at offset … expected
   4096 bytes, got 0")` directly; assert `is_wal_corruption()` is true both bare
   (`DbError::Database`) and wrapped (`DbError::Open`), and false for `Busy`,
   `Constraint`, `NotFound`, and `Corrupt("database header …")`.
2. **Recovery file-op unit test** (`right-db`): create a temp dir with dummy
   `data.db`, `data.db-tshm`, `data.db-shm`, `data.db-wal` files; call
   `recover_wal_sidecars`; assert `-tshm` and `-shm` are removed, `-wal` and
   `data.db` remain. Re-run on a dir missing `-shm` to assert idempotent
   `NotFound` tolerance.
3. **Fixture-gated integration test** (`right-db/tests/`): if `RIGHT_WAL_FIXTURE`
   points at a real desync fixture (the incident backup), copy it into a temp
   dir and assert `open_connection` now returns `Ok` and a known table is
   readable; **self-skip** (early return, not `#[ignore]`) when the env var is
   unset, so the default suite stays green without the uncommittable fixture.
   This is the committed successor to the spike file.
4. **Aggregator per-op test** (`right`): two sequential `get_conn(agent)` calls
   return connections backed by distinct opens (no shared cached handle).
   Existing `right_backend` tool tests must still pass.

The throwaway spike (`right-db/tests/wal_desync_spike.rs`) is deleted before
landing; tests 1–3 supersede it.

## Verification cadence

- During implementation: `devenv shell -- cargo nextest run -p right-db` and
  `-p right` after the TDD red/green loop for each change.
- `devenv shell -- cargo clippy --workspace --tests -- -D warnings`.
- Final, mandatory in the worktree: `devenv shell -- cargo nextest run
  --workspace` plus `devenv shell -- cargo test --doc --workspace`.

## Rollout / upgrade

Both changes are pure code (no codegen output, no sandbox policy, no schema).
Running agents adopt them via `right restart <agent>` / bot restart. No sandbox
recreation, no migration, no `right agent init`. Backward compatible by
construction.

## Alternatives considered

- **Serialize all cross-process DB access with a lock (rejected).** A
  connection-lifetime lock is incompatible with any long-lived handle, and the
  spike showed Turso checkpoints asynchronously outside our operations, so a
  per-operation lock cannot serialize the checkpoint that triggers the desync.
  Making serialization correct would require both dropping the aggregator cache
  *and* controlling/disabling auto-checkpoint — strictly more invasive than
  detect-and-repair, for prevention we can approximate more cheaply by just
  removing the cache (Change 1).
- **Restart-based recovery / external supervisor (rejected).** The desync is
  persisted in `-tshm`; a restart re-opens and re-fails. Recovery must reset the
  sidecars regardless, so the logic belongs at the `open_connection` layer where
  every process already converges.

## Deferred / future

- **Drop `experimental_multiprocess_wal` entirely (variant D2).** With the
  aggregator per-op, standard Turso WAL might suffice. The spike confirmed
  standard WAL works in-process but did **not** verify serialized cross-process
  opens. Needs a subprocess spike before considering. Out of scope here.
- **File the upstream issue** against turso multiprocess-WAL: stale `-tshm`
  authority + empty `-wal` slips past the existing authority-rebuild path
  (`test_classify_authority_snapshot_marks_truncated_wal_for_rebuild`) into a
  hard short-read instead of a rebuild. Reference #769.

## Code-comment requirements (explicit, per request)

- At the aggregator cache removal and on `get_conn`: the "must NOT hold a
  long-lived handle … do not reintroduce a connection cache" comment with the
  turso#769 link.
- At the `open_connection` recovery branch and `recover_wal_sidecars`: why
  `-tshm`+`-shm` are deleted and `-wal` is kept, with the turso#769 link.
