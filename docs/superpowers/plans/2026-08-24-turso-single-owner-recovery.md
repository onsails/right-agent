# Turso multiprocess-WAL containment and Riskoff recovery — implementation plan

## Decision

Recover Riskoff offline before any runtime cutover. Then make the shared MCP Aggregator the sole live owner of every per-agent `data.db`, route bot database behavior through typed domain operations over `~/.right/run/internal.sock`, and finally remove Turso experimental multiprocess WAL from production opens. Do not expose SQL, table names, raw rows, arbitrary parameters, or generic key/value operations over IPC. Tests and CLI init/restore/repair operations may open a database directly only while runtime quiescence is explicit and enforced.

The first cutover targets per-agent `data.db`, which is the observed Turso multiprocess-WAL failure. `providers.db` uses the same `right-db` filesystem builder and is currently open in both aggregator and bot processes, so disabling multiprocess WAL globally would otherwise create a new mixed-process failure. Keep provider domain behavior unchanged, but move its live store ownership to the Aggregator in the same ownership stage: dashboard CRUD is already aggregator-backed; replace bot-local reads with narrowly typed binding-resolution/convergence data over `internal.sock`. Secret-bearing responses must use `secrecy::SecretString` inside DTOs with manual redacted `Debug`, `ZeroizeOnDrop` where practical, and exist only for the sandbox apply path. Direct provider-store opens remain CLI/offline-only under quiescence.

## Preconditions and release gate

- Keep #197 and #203 open until Riskoff recovery and the production ownership cutover are verified.
- Do not deploy mixed modes: an old multiprocess bot must never run beside a new standard-local aggregator, and a new bot must never fall back to opening `data.db`.
- Do not start #205 until the final cutover, full verification, deployment, and production smoke checks pass.
- Preserve the current worktree stashes and never use live `~/.right` state in tests.

## Stage 1 — Add an offline repair module and recover Riskoff

### Code

1. Add operator-only `right agent db-repair <name>...` in `crates/right/src/main.rs` as `AgentCommands::DbRepair { names: Vec<String> }`, requiring at least one validated agent name. Put orchestration in `crates/right/src/db_repair.rs`. One invocation preflights every selected agent, establishes one project-wide quiescence session, then repairs each selected database sequentially; it accepts no database path or SQL. Per-agent repair is atomic. If a later agent fails, keep the runtime down and preserve prior successful manifests rather than trying a cross-agent rollback.
2. Add one public, scoped `right_db::repair_legacy_wal(RepairRequest) -> RepairReport` operation that owns the complete database filesystem transaction. The operation acquires the private bootstrap lock and retains its guard through forensic copy, staging, validation, the complete live swap, and any rollback; `crates/right/src/db_repair.rs` supplies validated paths/options but never manipulates `data.db*` itself. It must:
   - acquire the existing `.right-db-migrate.lock`;
   - operate on a staged copy, never the live files;
   - preserve `data.db` and every present `data.db-*` artifact byte-for-byte in a timestamped forensic directory;
   - remove only staged `data.db-tshm` and `data.db-shm` before the recovery open;
   - open the staged database with the current production Turso configuration so a valid WAL prefix can replay;
   - create a clean standalone replacement with `VACUUM INTO`;
   - validate the replacement read-only with `PRAGMA quick_check`, `PRAGMA user_version`, and a fixed non-secret invariant report: table existence plus row counts for `auth_tokens`, `cron_specs`, `async_runs`, `usage_events`, and `conversation_messages`; never read or print secret-bearing columns;
   - return paths, hashes, schema version, and counts only.
3. The CLI must prove quiescence before any copy or swap. Add `PcClient::shutdown_and_wait(timeout)` (or equivalent typed helper) that snapshots process names, calls project shutdown, then polls the process-compose health/process endpoints until the server is down and every previously active `right-mcp-server`/`*-bot` is terminal. If `<home>/run/state.json` is absent, treat the runtime as not started. If state exists but health is unreachable before a successful shutdown request, fail closed. Use a fixed 30-second deadline and 100-millisecond delay. After runtime quiescence, acquire the database bootstrap lock before file mutation. Do not use best-effort stop behavior.
4. Inside that still-locked `right-db` operation, build the replacement in the same filesystem as the agent directory. After validation, rename the original `data.db` plus all sidecars into `backups/<agent>/wal-recovery-<timestamp>/live-pre-swap/`, rename the recovered snapshot to `data.db`, preserve original file mode/ownership, and leave no copied legacy sidecars beside it. If any pre-swap step fails, leave the live set unchanged. If a rename after the first live move fails, restore the complete original set before releasing the lock and return the original error plus rollback context.
5. Write a recovery manifest containing schema version, file names, sizes, SHA-256 hashes, non-secret counts, tool version, timestamp, and swap status. It must never contain row values, credentials, prompts, message text, or tokens.
6. Do not restart automatically. Print the explicit next command using the current worktree binary. This keeps recovery acceptance separate from mutation and prevents an automatic start before the operator reviews the manifest.

### TDD and focused verification

- First add a deterministic `right-db` fixture test that creates a migrated DB with representative rows, creates legacy `-tshm`/`-shm` sentinels, runs the repair against a copy, and proves: main/WAL forensic bytes remain unchanged, coordination copies are removed only in staging, and the recovered snapshot has `quick_check=ok` plus expected schema/counts. The Stage 1 validator still uses the current multiprocess builder, so absence of a newly-created `data.db-tshm` is not a Stage 1 assertion; that standard-local filesystem invariant belongs to Stage 5.
- Add `right` tests with an injected/fake runtime controller for: state absent; healthy runtime shutdown before copy; state present but unreachable fails before mutation; still-running process times out before mutation; swap failure restores the full original set; manifest redacts values.
- Keep `crates/right-db/tests/wal_short_read.rs` as legacy evidence during this stage. The repair test must not depend on reproducing the nondeterministic live short-read.
- Run `devenv shell -- cargo nextest run -p right-db` and the focused `right` db-repair tests.

### Production recovery runbook

1. Build and review the release artifact with `devenv shell -- cargo build --release --locked --bin right`, then set `RIGHT_BIN="$PWD/target/devenv/release/right"`; every recovery command below uses this exact artifact.
2. Inventory Riskoff `data.db` and sidecars without reading rows.
3. Run `"$RIGHT_BIN" agent db-repair riskoff`. This command snapshots current holders, performs the fail-closed `shutdown_and_wait`, confirms quiescence, then repairs; do not run `right down` first because a retained `state.json` plus an already-dead process-compose endpoint must fail closed.
4. Review the recovery manifest and compare approved schema/count invariants with the incident record.
5. Run `"$RIGHT_BIN" up --agents riskoff --detach --non-interactive`.
6. Verify aggregator and bot status, one harmless Telegram turn, auth availability, cron loading, and absence of offset-16512 short reads or reset loops. On failure, run `"$RIGHT_BIN" down`, restore the complete `live-pre-swap` set offline, and retain all artifacts.

Gate: Riskoff reads/writes normally from the recovered standalone snapshot, no original artifact was deleted, and rollback remains available. Record the recovery evidence on #203, but keep #197/#203 open until Stage 5 removes the recurrence mechanism from production; close both only after the single-owner standard-local production smoke passes.

## Stage 2 — Introduce the Aggregator-owned database module

1. Add `crates/right/src/db_owner.rs` with `DbOwnerRegistry` and one `AgentDbOwner` per registered agent. Each owner retains one writable `right_db::Connection` and serializes operations with a per-agent `tokio::sync::Mutex`; do not add an actor/channel layer unless the retained connection proves non-`Send` under compilation. Add `AgentRuntimeBundle` beside it: owner, cancellation token, and tracked `JoinSet`/handles for proxy connect, health, refresh, reconnect, and memory tasks. `DbOwnerRegistry` is injected into `RightBackend`, `InternalState`, MCP proxy/refresh/reconnect state, and memory state. No `Connection` leaves this module.
2. In the `Commands::McpServer` startup loop in `crates/right/src/main.rs`, build `DbOwnerRegistry` first, open/migrate every registered agent DB, then restore MCP and Hindsight state through each owner before publishing routing. Replace the current log-and-continue `open_connection` branch at the startup restore block with fail-fast startup. Retain one owner-local connection per agent after restore.
3. Add `DbOwnerState::{Starting, Ready, Draining, Failed}` and typed `DbReadyRequest/DbReadyResponse` plus `InternalClient::db_ready`. Register `POST /db/ready` in `crates/right/src/internal_api.rs`. In `crates/bot/src/lib.rs`, construct `InternalClient` before all database-backed startup work and retry this handshake with a fixed 30-second deadline and 100-millisecond delay. Timeout, `Failed`, or transport failure aborts bot startup; there is no direct-open fallback.
4. Harden `internal.sock`: remove stale socket, bind, set owner-only mode on Unix, and publish readiness only after the listener and all initial owners are ready. Keep all DB routes off TCP `:8100`, `bot.sock`, Cloudflared, and sandbox-visible MCP.
5. Extend `handle_reload` in `crates/right/src/internal_api.rs`: adding an agent first constructs a complete `AgentRuntimeBundle`, opens/migrates the owner, restores MCP/memory state, starts tracked tasks, then atomically publishes dispatcher/token-map/readiness; any failure cancels and awaits the partial bundle and leaves it absent. Removal sets `Draining`, rejects new work, cancels and awaits every tracked per-agent task, then waits for the owner mutex with a fixed 10-second deadline, removes routing, and drops the bundle. A timeout returns an error and keeps the agent registered in failed/draining state for explicit recovery. Aggregator shutdown applies the same order to every bundle.
6. Preserve startup ordering in `crates/right-codegen/templates/process-compose.yaml.j2`, but do not trust `condition: process_started` as readiness; the bot handshake is authoritative.

Tests: startup fails on one broken DB; readiness remains false until migrations finish; reload add/remove is atomic; owner unavailable has a typed error; shutdown rejects new requests and drains accepted requests; socket permissions are restrictive.

## Stage 3 — Add typed domain IPC

Place shared wire DTOs and typed `InternalClient` methods in new `crates/right-mcp/src/internal_db.rs`, re-exported by `right-mcp`; keep only the private generic HTTP `post<Req, Res>` transport in `internal_client.rs`. Add owner handlers in `crates/right/src/internal_api_db.rs` and nest their finite router into `internal_router`. Production callers depend on typed client methods, never route strings or JSON values.

Expose four deep interfaces, not one method per SQL statement:

1. `InteractionState`
   - archive/mark-routed message operations with idempotency keys;
   - active-session transitions and session queries;
   - scoped thread/chat search and message lookup;
   - thread focus and scoped error-detail storage/read.
2. `RunLedgerAndDelivery`
   - enqueue/start/finish background and cron runs;
   - atomically persist output plus delivery decision;
   - claim/mark delivery and deduplicate retries;
   - recover interrupted handoffs/runs and shutdown interruption.
3. `SecretsAndMcpRegistry`
   - auth status, setup-token save/load, and notice-token creation;
   - MCP register/remove/header/OAuth mutations, redacted list/status, startup restore, and instructions cache;
   - secret-bearing DTOs have redacted `Debug`; list/status returns no values.
4. `LearningMemoryAndUsage`
   - usage and learning event batches, lifecycle transitions, budget checks/skips;
   - retain-queue enqueue plus `claim_batch(limit, lease_ttl) -> {items, claim_token, lease_expires_at}`, token-guarded acknowledge/nack, and expired-lease reclaim;
   - alert check-and-record;
   - typed dashboard activity/usage/learning projections.

Every mutation that may be retried across a lost response carries a request ID or natural idempotency key. Owner handlers retain current transaction boundaries. Map failures into typed categories (`Unavailable`, `NotReady`, `NotFound`, `Conflict`, `Transient`, `Invalid`, `Internal`) and preserve complete server-side error chains in logs without returning secrets.

Tests: round-trip each DTO; unknown routes/malformed payloads fail closed; serialized contracts contain no `sql`, `table`, `params`, `row`, or arbitrary operation fields; idempotent retries cannot duplicate messages/runs; session replacement and multi-write run transitions remain atomic; retain crash-after-claim requeues after lease expiry and stale tokens cannot ack; secret DTO logging is redacted.

## Stage 4 — Migrate every live caller

1. Replace all production direct DB opens and `Connection`-taking helpers in `crates/bot/src` with the typed client. The inventory includes `lib.rs`, `async_delivery.rs`, `background.rs`, `cron.rs`, `idle_compaction.rs`, `keepalive.rs` runtime path, `learning_*`, `login.rs`, `reflection.rs`, `telegram/archive.rs`, `alerts.rs`, `dashboard.rs`, `error_details.rs`, `handler.rs`, `worker.rs`, `session.rs`, `memory_alerts.rs`, `dashboard/focus.rs`, and `dashboard/skills.rs`. Extend `InteractionState` for bootstrap-answer/finalization state, reply-gate and turn-preparation reads, active-session transitions, and lifecycle bumps currently performed by worker/session code.
2. Keep `validate_init_auth` direct read-only access only as a CLI-init adapter: it runs before the aggregator exists and therefore must document and enforce quiescence. Split it from the runtime auth-status path so live bot code cannot call the offline adapter.
3. Refactor `right-memory::ResilientHindsight` so it accepts an injected `PendingRetainSink` interface instead of an `agent_db_path`; supply an owner-local adapter in the Aggregator and a typed-IPC adapter in the bot. Replace `run_drain_loop` with lease-based typed claim/ack/nack operations. The owner atomically records claim token and expiry; success/delete and retry/drop transitions require that token; startup and each claim reclaim expired leases. Tests use an in-memory adapter and cover bot crash after claim, lease expiry, duplicate claim exclusion, and stale ack rejection; no raw DB interface escapes `right-db`.
4. Keep aggregator-side `right-mcp` proxy/refresh/reconnect persistence owner-local. Change `ProxyBackend`, reconnect, and refresh scheduler constructors to receive an `Arc<AgentDbOwner>` (or narrow owner adapter) instead of an `agent_dir`; replace the opens in `proxy.rs`, `reconnect.rs`, and `refresh.rs`.
5. Route dashboard database projections and mutations through typed owner operations. Keep the provider domain separate from per-agent state, but make the Aggregator its sole live `providers.db` owner too: remove `ProviderStore::open(&home)` from `crates/bot/src/lib.rs`, keep metadata/mutations on the existing provider routes, and add `ResolveProviderBindings` plus `ResolveNamedProviderBinding` typed calls for sandbox create/reconcile. Requests carry only agent/provider identities. Responses carry `SecretBindingDto` fields plus a private `SecretString` value. Authenticate every secret-resolution request with an HMAC token derived from that agent's existing `agent.yaml::secret` using a new `provider-binding-ipc` label; the Aggregator verifies it in constant time and requires the requested agent to match the authenticated identity before reading a credential. Keep `internal.sock` mode 0600 as defense in depth. Implement field-level `serialize_with` using `ExposeSecret` only during UDS body encoding, normal `SecretString` deserialization, `ZeroizeOnDrop`, and manual redacted `Debug`; cap request/response body size; ensure `InternalClientError::Server` never includes secret response bodies; test that JSON round-trip delivers the value while every debug/error/log form omits it. Convert to `right_sandbox::SecretBinding` immediately and drop the DTO. Preserve owner/borrower rules, the per-agent advisory convergence lock, and existing durable-state/YAML compensation.
6. Migrate owner-local `RightBackend` tools to the owner interface. Remove the outdated per-operation multiprocess-WAL comments and replace them with the single-owner invariant.
7. Add source-policy tests that scan production code. In `crates/bot/src`, forbid `right_db::Connection`, `open_connection*`, raw query/execute/transaction calls, and direct database-domain imports outside the named offline-init adapter. Across `crates/right/src`, `right-mcp/src`, `right-memory/src`, and `right-agent/src`, allow direct opens only in `db_owner.rs`, tests, and named offline CLI modules (`db_repair`, init/restore, backup/destroy, rebootstrap, sandbox migration, doctor/memory inspection). Each offline command must call one shared `require_runtime_quiesced(home)` helper before its first `data.db` or `providers.db` open; commands intended to work while Right is running must instead use `InternalClient`. The policy test carries an explicit allowlist of files/functions and fails on any new opener.

Run focused package tests after each coherent domain migration. Gate: no live bot path opens `data.db`; all runtime aggregator access goes through the owner; owner unavailability fails clearly and never falls back.

## Stage 5 — Disable experimental multiprocess WAL

1. Change `crates/right-db/src/connection.rs` so every filesystem production open uses `turso::Builder::new_local(path).experimental_index_method(true)` with normal upstream IO. Remove `.experimental_multiprocess_wal(true)`, `multiprocess_io::new()`, `crates/right-db/src/multiprocess_io.rs`, the workspace `turso_core` dependency if now unused, and multiprocess-only test helpers.
2. Retain standard `journal_mode=WAL`, busy timeout, migrations, immediate transactions, read-only query guards, and the bootstrap migration lock. Standard local mode may create ordinary `data.db-wal`/`data.db-shm`; it must not create `data.db-tshm`.
3. Remove automatic live sidecar deletion and stringly `is_wal_corruption` recovery from normal opens. Keep the explicit offline legacy repair primitive for one release window. A standard open error propagates unchanged.
4. Rewrite multiprocess smoke tests: prove standard local write/read/reopen; prove `data.db-tshm` is absent; prove a second process cannot become a supported concurrent writer; prove read-only opens create no files; prove the explicit legacy repair preserves forensic DB/WAL artifacts.
5. Update `ARCHITECTURE.md` with the review-blocking invariant. Update the touched descriptive satellites: `docs/architecture/modules.md`, `lifecycle.md`, `mcp.md`, `layout.md`, `upgrades.md`, `memory.md`, and `providers.md`; make owner/IPC, pending-retain drain, provider secret lifetime, and reconciliation text match code. Mark `docs/superpowers/specs/2026-06-14-wal-desync-self-healing-design.md` and the multiprocess parts of `2026-05-25-turso-local-foundation-design.md` superseded. Keep `docs/research/2026-08-24-turso-multiprocess-wal-production-risk.md` as incident evidence.

Gate: production code contains no `experimental_multiprocess_wal`, custom NoLock IO, second live per-agent opener, or second live `providers.db` opener; standard opens never create `-tshm`; every direct offline opener in the audited allowlist proves quiescence through the shared helper.

## Stage 6 — Review, full verification, deploy, and soak handoff

1. Run the required Rust review through `review-rust-code`; turn every supported finding into a tracked repair, fix it, and run focused re-review.
2. Run:
   - `devenv shell -- cargo nextest run --workspace --no-fail-fast`
   - `devenv shell -- cargo test --doc --workspace`
   - `devenv shell -- cargo build --workspace`
   - `devenv shell -- cargo fmt --all -- --check`
   - `devenv shell -- cargo build --release --locked --bin right`; this produces the single reviewed deployment artifact at `target/devenv/release/right`.
3. Commit with Conventional Commits. Push only after every gate is green.
4. Deploy with the same explicit reviewed artifact that performs shutdown, repair, codegen, and start: set `RIGHT_BIN="$PWD/target/devenv/release/right"`; run `"$RIGHT_BIN" agent db-repair right him riskoff` (one invocation owns shutdown/quiescence and cleans every legacy database); then run `"$RIGHT_BIN" up --agents right,him,riskoff --detach --non-interactive` and `"$RIGHT_BIN" status`. `right up` launches the Aggregator and bots together; accept bot behavior only after each bot's typed owner-readiness handshake succeeds. Never invoke bare `right`, imply an unsupported aggregator-only process-compose start, or roll back to a mixed old/new topology.
5. Smoke all three bots: Telegram turn, auth status, cron load/trigger, async/background delivery, dashboard reads/mutations, conversation archive/search, learning/usage write, MCP list/tool call/OAuth refresh, provider reconcile, clean shutdown/restart. Verify owner logs show one DB owner per agent and no short-read/reset/-tshm events.
6. After smoke passes, post the recovery/cutover evidence and close #197 and #203 with `gh issue close 197 --comment <evidence>` and `gh issue close 203 --comment <evidence>`. Then start #205 at an explicit UTC `T0` and freeze a candidate WARN/ERROR allowlist containing exact classes, rationale, and owner. Capture one contiguous `[T0, T0+24h]` window from `~/.right/logs/{right,him,riskoff}.log.*`, `~/.right/logs/mcp-aggregator.log`, matching `~/.right/logs/streams/*.ndjson`, and process-compose status/restart/log evidence for `{right,him,riskoff}-bot` plus `right-mcp-server`. At `T0+24h`, enumerate every WARN/ERROR class, curate the final accepted allowlist only from explained evidence, and require zero unexplained classes, zero data loss, and all three bots functional. Post the window bounds, sources, allowlist, classifications, and zero-unexplained declaration to #205, then close it with `gh issue close 205 --comment <report>`. Any unexplained class or repair reopens/creates the relevant incident and restarts the full 24-hour window.

## Rollback rules

- Riskoff recovery rollback: while fully offline, restore the complete preserved original `data.db` plus all sidecars; never combine original sidecars with the recovered snapshot.
- Pre-WAL-disable code rollback: stop all processes first, then deploy one internally consistent owner/IPC version. Never run a direct-opening bot beside a standard-local aggregator.
- Post-WAL-disable rollback: restore the last verified standalone snapshot, not legacy multiprocess coordination files, unless the incident commander explicitly chooses the forensic original set.
- Keep forensic and pre-swap artifacts through the completed #205 soak and the normal backup-retention window.
