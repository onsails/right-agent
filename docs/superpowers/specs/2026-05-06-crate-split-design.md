# Crate split — speed up incremental Rust builds

## Problem

The workspace has 3 crates today: `right-agent` (≈30k LoC), `right-bot`
(≈20k LoC), `right` (CLI binary, ≈11k LoC). `right-agent` is a god crate
holding 22 top-level modules — `agent`, `codegen`, `config`, `cron_spec`,
`doctor`, `error`, `init`, `mcp`, `memory`, `openshell`, `platform_store`,
`process_group`, `rebootstrap`, `runtime`, `sandbox_exec`, `stt`,
`tunnel`, `ui`, `usage`, plus a generated `openshell_proto`.

Both `right-bot` and `right` depend on `right-agent`. Any one-line edit
inside `right-agent` (e.g. tweaking `codegen/pipeline.rs` or adding a
log line in `memory/hindsight.rs`) triggers a full rebuild of
`right-agent` (30k LoC) → `right-bot` (20k LoC) → `right` (CLI). Hot
edit areas in `right-agent` over the last 30 days: `codegen` (42
commits), `memory` (36), `ui` (34), `mcp` (21). Hot edit areas in
`bot/`: `bot/telegram` (398), `bot/lib.rs` (103), `bot/cron*` (98).

Inside `right-bot`, the `bot/telegram/` sub-tree (13.6k LoC) carries two
disjoint roles tangled together:

1. Telegram-specific glue (`handler`, `dispatch`, `mention`,
   `oauth_callback`, `webhook`, …).
2. Generic Claude Code invocation plumbing — `invocation::ClaudeInvocation`,
   `prompt::build_prompt_assembly_script`, `stream::StreamEvent`,
   `worker::parse_reply_output`, `attachments::OutboundAttachment`,
   `markdown::{html_escape, strip_html_tags}` — all used outside the
   Telegram path by `cron`, `cron_delivery`, `reflection`. This is
   architectural debt: shared CC plumbing misnamed under `telegram::`.

Result: incremental edit-compile-run loops are slow, and the natural
cut-point for a `right-telegram` leaf crate is blocked by the tangle.

The optimization target is the **incremental** build path (cargo build
after a small edit), not clean rebuilds, not CI throughput, not IDE
check latency.

## Goals

- Cut typical incremental rebuild time after a one-file edit by an order
  of magnitude. Measured by: edit `bot/telegram/handler.rs`, then
  `cargo build --workspace --timings` wall-time before vs after.
- Hot-edit zones (`bot/telegram`, `codegen`, `memory`, `mcp`) end up in
  leaf crates so a touch rebuilds 3-10k LoC plus thin orchestrator
  crates, not 50k LoC of bundled god crates.
- Keep `right-agent::memory::migrations` as the single SQLite migration
  registry — split must preserve the "one place to add a table"
  invariant.
- Preserve `release-plz` workflow: workspace version-group, single
  GitHub release per `right` tag, no crates.io publication, single
  `CHANGELOG.md` covering all internal crates.
- Preserve `feature = "test-support"` semantics (TestSandbox, panic-hook
  cleanup, parallel slot acquisition).

## Non-goals

- Crates.io publication. Internal crates remain `publish = false`.
- Cross-platform optimizations (mold/lld, sccache). Out of scope —
  separate effort if needed.
- Renaming `right-agent`. The slim crate after the split keeps the
  name; "agent platform brain" is still a fitting label.
- Touching SOUL.md, IDENTITY.md, USER.md, or other agent-owned files.
- Major rewrites of the migration registry — only the move from
  `right-agent::memory::migrations` to `right-db::migrations`.

## Decisions

### Granularity: medium-aggressive (10 crates total)

Considered three options:

- **Minimal (4 crates)**: extract one foundation crate. Modest gain;
  `right-agent` still ~20k LoC.
- **Medium (6-7 crates)** ← starting point.
- **Aggressive (12+ crates)**: per-domain everything. Diminishing
  returns vs `Cargo.toml` overhead.

User chose medium **plus a separate `right-db` crate** (anticipating
future databases beyond SQLite) **plus a `right-telegram` extraction
from `right-bot`**. Final count: 10 crates.

### `right-cc` is its own crate, not folded into `right-core`

`right-cc` carries CC-invocation plumbing (no teloxide, no rusqlite,
no openshell — just serde + JSON). Folding into `right-core` would
co-mingle "platform foundation" with "subprocess protocol logic". A
separate crate keeps the boundary explicit and lets `right-telegram`
depend on `right-cc` without dragging in core's heavier deps.

### `right-codegen → right-mcp` edge stays

Code-search showed 4 references from `codegen/` into `mcp/`:
`McpServerEntry`, `generate_agent_secret`, `derive_token`. Pipeline
generates `.mcp.json` with per-agent bearer tokens — codegen needs
mcp's primitives. Two options considered:

- **(a)** Accept the edge: `codegen` depends on `mcp`. Editing `mcp`
  rebuilds `codegen`.
- **(b)** Push `derive_token` + `generate_agent_secret` down to
  `right-core::secret`. Reduces the edge to one type
  (`McpServerEntry`); doesn't fully eliminate it.

Picked **(a)**. Mild downgrade — `mcp` is not a hot edit zone (21
commits / 30 days vs codegen's 42). The 4-ref reduction in (b) doesn't
justify the extra crate-level surgery.

### Migration ownership stays centralized

`memory/migrations.rs` (1000 LoC, 17 versions) moves to `right-db`
unchanged. All domain crates (codegen, memory, mcp, agent, bot) call
`right_db::open_connection` to get a migrated connection. New tables
are added by editing the central migration array, same as today —
ARCHITECTURE.md "SQLite Rules" section stays accurate.

### Stepped migration over big-bang

Six stages, each landing as one or more PRs, each compilable and
test-green between stages. Justifications in `Architecture` below.

## Architecture

### Final crate graph

```
                       right-core
                            ▲
              ┌─────────────┴──────────────────┐
              │                                │
          right-db                          (used directly
              ▲                              by every crate)
              │
   ┌──────────┼──────────┬─────────┐
   │          │          │         │
right-mcp  right-memory  right-cc  │
   ▲                       ▲       │
   │                       │       │
right-codegen ──────────────────┐   │
   ▲                            │   │
   │                            │   │
right-agent (slim) ◄────────────┴───┘
   ▲       ▲
   │       └────────── right (CLI bin) ──────┐
   │                       ▲                 │
   │                       │                 │
   └────────── right-bot (slim) ─────────────┤
                  ▲                          │
                  └─── right-telegram ───────┘
                            ▲
                            └── (depends on right-cc)
```

Edge summary (each leaf crate's deps beyond `right-core`):

- `right-db` → `right-core`
- `right-mcp` → `right-core`, `right-db`
- `right-memory` → `right-core`, `right-db`
- `right-cc` → `right-core`, `right-db`
- `right-codegen` → `right-core`, `right-db`, `right-mcp`
- `right-agent` → `right-core`, `right-db`, `right-codegen`,
  `right-memory`, `right-mcp` (no `right-cc` — agent-side never
  parses CC output)
- `right-telegram` → `right-core`, `right-cc`
- `right-bot` → `right-core` (incl. `platform_store`), `right-db`,
  `right-cc`, `right-telegram`, `right-agent`
- `right` (CLI) → `right-core`, `right-db`, `right-agent`,
  `right-bot`, `right-mcp`

10 crates. No cycles (verified by enumeration above). Parallel build
paths: `right-memory`, `right-mcp → right-codegen`, and `right-cc`
all independent at the same depth — Cargo can build them in parallel.

### Crate contents and sizing

| Crate | LoC (approx) | Hot? | Heavy deps |
|---|---|---|---|
| `right-core` | 5k | No | tonic + tonic-prost-build, prost (openshell-proto), reqwest (stt download, tunnel health) |
| `right-db` | 1.5k | No | rusqlite, rusqlite_migration |
| `right-codegen` | 5-6k | **Yes** | minijinja, include_dir |
| `right-memory` | 3k | **Yes** | reqwest (Hindsight) |
| `right-mcp` | 4-4.5k | Medium | rmcp, hyper |
| `right-agent` (slim) | 7-8k | Medium | inherits via leaves |
| `right-cc` | 5k (incl. `usage/`) | No (changes via telegram-specific code) | serde, serde_json, chrono; rusqlite via `right-db` for `usage::insert` |
| `right-telegram` | 10k | **Hottest** | teloxide |
| `right-bot` (slim) | 5k | Medium | whisper-rs (cfg-gated, macos metal) |
| `right` (CLI bin) | 11k | Low | axum, rmcp |

#### `right-core`

Pure utilities and platform primitives. Stable layer.

- `error`, `ui`, `config`, `tunnel`, `process_group`, `sandbox_exec`,
  `stt` (model download + path helpers, NOT inference), `test_cleanup`,
  `test_support` (`TestSandbox` + panic-hook cleanup).
- `openshell` (gRPC client, mTLS) and the `proto/openshell/*.proto`
  build script.
- `platform_store` — content-hashed atomic deployment of files to
  sandbox via `openshell`. Domain-neutral primitive; lives here
  rather than in `right-codegen` because: (1) it carries no
  codegen-specific logic — just `sha2`, `walkdir`, and openshell
  transfers; (2) consumers span the dep graph (`right-codegen`'s
  pipeline AND `bot::sync` both call it), and putting the helper
  in core avoids a `right-bot → right-codegen` edge.
- **Two constants migrate here**: `IDLE_THRESHOLD_MIN` and
  `IDLE_THRESHOLD_SECS`, currently in
  `right-agent::cron_spec`. They are referenced by
  `codegen/skills.rs` (template substitution) — moving them to
  `right-core::time_constants` (or similar) avoids forcing
  `right-codegen` to depend on `right-agent::cron_spec` (which would
  create a cycle, since `right-agent` depends on `right-codegen`).
- `secret` is **not** added in v1 (per the `(a)` decision above).
  Token primitives stay in `right-mcp`.

#### `right-db`

Per-agent SQLite plumbing.

- `open_db`, `open_connection`, `migrations::MIGRATIONS`,
  `sql/v*.sql` files (re-bundled via `include_str!`).
- `DbError` (renamed from `MemoryError`; the old name was a
  category-error since the type covered cron, mcp, usage, telegram_sessions
  errors as well).
- The dynamic `migrations.rs` array stays a single source of truth.
  Domain crates do not register their own migrations — they edit the
  central array.

#### `right-codegen`

- `codegen/*` only — `pipeline`, `contract`, `agent_def`,
  `mcp_instructions`, `process_compose`, `system_prompt`, registry
  tests, etc.

Depends on: `right-core` (which now hosts `platform_store`),
`right-db`, `right-mcp`.

`cron_spec` is **not** moved here. See the slim `right-agent`
section below. `platform_store` is **not** moved here either —
it lives in `right-core` (rationale in the `right-core` section).

#### `right-memory`

Hindsight-resilience layer. Pure HTTP-driven semantic memory.

- `hindsight`, `circuit`, `classify`, `resilient`, `prefetch`,
  `status`, `guard`, `error`.
- `retain_queue` — SQLite-backed pending-retain queue (uses
  `right_db::open_connection`; queue table `pending_retains` is part
  of the central migration list).
- `alert_types` constants (referenced by doctor + memory_alerts).

Depends on: `right-core`, `right-db`.

#### `right-mcp`

- `mcp/*` — aggregator backend types, proxy, reconnect, refresh,
  credentials.
- `credentials::McpServerEntry`, `save_mcp_server`, etc.
- **New**: token helpers `generate_agent_secret`, `derive_token`
  (HMAC-SHA256 over per-agent secret). Currently in `mcp::mod.rs`,
  stay there. Re-exported at crate root.
- **New**: auth-token helpers — `save_auth_token`, `get_auth_token`,
  `delete_auth_token` migrate **into** this crate from
  `right-agent::memory::store` (Stage A). They were misplaced under
  `memory` because they shared a SQLite table name; semantically they
  are MCP credentials.

Depends on: `right-core`, `right-db`.

#### `right-agent` (slim)

What's left after extraction:

- `agent/*` — agent CRUD (`AgentConfig`, `AgentEntry`, types,
  destroy).
- `runtime/*` — process-compose orchestration (`PcClient`,
  `state.json` r/w).
- `init`, `doctor`, `rebootstrap` — top-level CLI command
  implementations (called from `right`).
- `cron_spec` — `CronSpec`, `ScheduleKind`, parsing/storage helpers.
  Stays here because consumers span domains (CLI's `right_backend`,
  `memory_server`, `bot::cron`, `right-agent::doctor`); moving it
  to `right-codegen` would make `right-bot` reach into a codegen
  crate just for these types. The two `IDLE_THRESHOLD_*` constants
  used by `codegen/skills.rs` migrate to `right-core` (see Stage B).

`usage/` does **not** stay here — it migrates to `right-cc`. CLI
consumers of usage (`right_backend.rs`, `memory_server.rs`) are
zero today; the only callers are `bot::cron`, `bot::reflection`,
`bot::telegram::worker`, `bot::telegram::stream` — all in the bot
side of the dep graph.

Depends on: `right-core`, `right-db`, `right-codegen`, `right-memory`,
`right-mcp`.

#### `right-cc`

Claude Code subprocess plumbing + token-usage accounting. New crate.
Created in Stage E from the in-bot refactor of Stage D.

- `invocation` — `ClaudeInvocation` builder, `OutputFormat`,
  `baseline_disallowed_tools`, `mcp_config_path`,
  `build_prompt_assembly_script` (currently `bot::telegram::prompt`).
- `stream` — `StreamEvent`, `parse_stream_event`, `parse_usage_full`,
  `parse_api_key_source` (currently `bot::telegram::stream`).
- `attachments_dto` — `OutboundAttachment` (currently
  `bot::telegram::attachments::OutboundAttachment`; the `send_*`
  Telegram-side functions stay in `right-telegram`).
- `markdown_utils` — `html_escape`, `strip_html_tags` (currently
  `bot::telegram::markdown`; `md_to_telegram_html` and
  `split_html_message` stay in `right-telegram`).
- `worker_reply` — `parse_reply_output` (currently
  `bot::telegram::worker::parse_reply_output`).
- `usage/*` — entire current `right-agent::usage/` subtree
  (`UsageBreakdown`, `ModelTotals`, `WindowSummary`, `pricing`,
  `format`, `aggregate`, `insert::{insert_cron, insert_interactive,
  insert_reflection_*}`, `error::UsageError`). Lives here because
  `parse_usage_full` produces `UsageBreakdown` and `bot::{cron,
  reflection, telegram::worker}` consumers all fall on the
  bot-side of the graph. Moving usage with the parser keeps the
  type and its persistence in one crate.

Depends on: `right-core`, `right-db` (for the usage SQLite
operations). **No teloxide.**

#### `right-telegram`

Telegram-specific surface only. Not a generic UI crate.

- `handler` (2.3k LoC), `dispatch`, `filter`, `mention`,
  `allowlist_commands`, `oauth_callback`, `webhook`, `bot::build_bot`,
  `session`, `worker` (debounce + `SessionKey`; the `parse_reply_output`
  helper migrated to `right-cc`), `attachments::send_*`,
  `bootstrap_photo`, `memory_alerts`, `markdown::md_to_telegram_html`,
  `split_html_message`.
- `BotType`, `SessionLocks`, `BgRequests` — re-exported public types.
- `run_telegram` — public entry point used by `right-bot::lib.rs`.
- `broadcast_to_chats` — used by cron_delivery (in `right-bot`).
- `IdleTimestamp` — used by `cron_delivery` (in `right-bot`).

Depends on: `right-core`, `right-cc`, and (transitively) the agent-side
graph through items it must call. **`right-telegram` does NOT depend on
`right-bot`** — that's the whole point of the refactor.

#### `right-bot` (slim)

Bot orchestrator only. Wires CC plumbing + Telegram into the running
bot process.

- `lib.rs` (entry point, including `run`, `run_telegram` invocation,
  signal handling).
- `cron`, `cron_delivery` — scheduled CC job execution.
- `reflection` — failure-summary turn.
- `sync`, `login`, `upgrade`, `keepalive`, `config_watcher`.
- `stt/` — whisper-rs transcription (live inference).

Depends on: `right-core`, `right-db`, `right-agent`, `right-cc`,
`right-telegram`. The CLI binary launches the bot via `right-bot::run`.

#### `right` (CLI binary, unchanged role)

- `main.rs` with all CLI subcommands.
- `aggregator`, `internal_api`, `memory_server`, `right_backend`,
  `wizard`.
- Stays the only crate that produces a `[[bin]]`.

Depends on: `right-core`, `right-agent`, `right-bot`, `right-mcp`.

### Migration order — six stages

Each stage lands as a PR (or small chain). After each stage:
`cargo build --workspace`, `cargo test --workspace`, `cargo build
--workspace --release` must all pass before merging.

#### Stage A — `right-db`

- Create `crates/right-db/`.
- Move `right-agent::memory::{open_db, open_connection}`,
  `migrations.rs`, `sql/v*.sql`.
- Rename `MemoryError` → `DbError`.
- Move `memory::store::{save_auth_token, get_auth_token,
  delete_auth_token}` to `right-agent::mcp::credentials` (still in
  `right-agent` — it migrates to `right-mcp` only at Stage C).
- Update call sites: `right-agent::cron_spec`, `right-agent::mcp::*`,
  `right-agent::usage::*`, `right-agent::agent::destroy`,
  `right-agent::doctor`, `bot::*` (the bot also opens connections).

Effect: small. Clears up the `memory/` confusion before Stage C.

#### Stage B — `right-core`

- Create `crates/right-core/`.
- Move `error`, `ui`, `config`, `openshell` (+ `proto/openshell/`,
  `build.rs`), `tunnel`, `process_group`, `sandbox_exec`, `stt`,
  `test_cleanup`, `test_support`, **`platform_store`** (with its
  tests; consumers `right-agent::codegen::pipeline` and `bot::sync`
  rewrite imports `right_agent::platform_store::*` →
  `right_core::platform_store::*`).
- Move two constants `IDLE_THRESHOLD_MIN` / `IDLE_THRESHOLD_SECS`
  out of `right-agent::cron_spec` into a new
  `right-core::time_constants` module. Re-export from `cron_spec`
  for one PR cycle; remove after Stage F.
- `feature = "test-support"` belongs to `right-core` now. Other
  crates' `[dev-dependencies]` write `right-core = { path = "...",
  features = ["test-support"] }`.
- `tonic-prost-build` only runs as part of `right-core/build.rs`. The
  `openshell_proto` re-export at `right-agent::openshell_proto` stays
  available via `pub use right_core::openshell_proto;` for one
  release; remove after Stage F.

Effect: tonic build script lives in a stable, cacheable crate.
Subsequent leaf-crate edits do not retrigger it.

#### Stage C — leaf extraction (`right-codegen`, `right-memory`, `right-mcp`)

Three coordinated PRs (or one combined PR).

- `right-codegen`: move `codegen/*` only. (`cron_spec` and
  `platform_store` stay put — see Stage B for `platform_store` and
  the slim `right-agent` for `cron_spec`.)
- `right-memory`: move `memory/{hindsight, circuit, classify,
  resilient, prefetch, status, guard, retain_queue, error,
  alert_types}`. (After Stage A `memory/` is already lean.)
- `right-mcp`: move `mcp/*`. **Pull in** `save_*_auth_token` helpers
  from where Stage A parked them.

After this stage `right-agent` slims to `agent`, `runtime`, `init`,
`doctor`, `rebootstrap`, `cron_spec`, `usage` — orchestration layer
plus shared cron-storage types and (still) usage. `usage/*` moves to
`right-cc` in Stage E.

Effect: hot-edit incremental builds for `codegen`, `memory`, `mcp`
collapse to ~3-6k LoC per touched leaf instead of 30k.

#### Stage D — pre-refactor inside `right-bot`

No new crate created. Internal refactor only.

- Create `bot::cc` module.
- Move from `bot::telegram::` → `bot::cc::`:
  - `invocation` → `bot::cc::invocation`
  - `prompt` → `bot::cc::prompt`
  - `stream` → `bot::cc::stream`
  - `worker::parse_reply_output` → `bot::cc::worker_reply::parse_reply_output`
  - `attachments::OutboundAttachment` → `bot::cc::attachments_dto`
    (the `send_*` functions stay in `bot::telegram::attachments`)
  - `markdown::{html_escape, strip_html_tags}` → `bot::cc::markdown_utils`
    (`md_to_telegram_html`, `split_html_message` stay in
    `bot::telegram::markdown`)
- Update call sites in `bot::cron`, `bot::cron_delivery`,
  `bot::reflection` to use `crate::cc::*` instead of
  `crate::telegram::*`. Update Telegram-side users to use
  `crate::cc::*` for shared items as well.
- Optionally leave one-line re-exports in old paths for the duration
  of the PR; remove before merge.

Independent of Stages A/B/C — can run in parallel.

Effect: zero build-time effect on its own. Required for Stage E.

#### Stage E — `right-cc` + `right-telegram` extraction

- Create `crates/right-cc/`, move `bot::cc::*`.
- **Move `right-agent::usage/*` into `right-cc::usage`** as part of
  the same PR. Update consumers (`bot::cron`, `bot::reflection`,
  `bot::telegram::worker`, plus the `parse_usage_full` callsite
  inside the new `right-cc::stream`) to use `right_cc::usage::*`.
- Create `crates/right-telegram/`, move `bot::telegram::*` (cleaned
  by Stage D).
- Update `right-bot` to depend on `right-cc` and `right-telegram`,
  re-export the few public types it forwards to consumers
  (`broadcast_to_chats`, `IdleTimestamp`, `BotType`).

Effect: edits to `bot/telegram/handler.rs` (the #1 hot file at 398
edits / 30 days) rebuild `right-telegram` (10k) + `right-bot` (5k) +
`right` only. Currently they rebuild a 20k-LoC bundle.

#### Stage F — `release-plz` + cleanup

- Add `[[package]]` entries to `release-plz.toml` for each new crate
  with just `version_group = "workspace"`. They inherit
  `git_tag_enable = false` / `git_release_enable = false` /
  `publish = false` / `git_only = true` from `[workspace]` defaults.
- Extend `[[package]] name = "right"` `changelog_include` to list
  every internal crate (so a single CHANGELOG.md keeps tracking all
  changes).
- Remove transitional re-exports left during Stages A-E.
- Update `ARCHITECTURE.md` `Workspace`, `Module Map`,
  `Configuration Hierarchy`, `Codegen categories` to reflect the new
  shape.
- Run `cargo update -p` per new crate where needed.

### Stage dependency DAG

```
A ─┐
   ├─→ C
B ─┘

D ─→ E

A,B,C,D,E ─→ F
```

A and B can run in parallel. D can run in parallel with A/B/C. C
blocks on both A and B. E blocks on D. F is final.

Realistic single-developer order: **A → B → D (start) → C → E → F**.

### Estimates

| Stage | Effort |
|---|---|
| A | 1-2 days |
| B | 2-3 days |
| C | 3-5 days (3 leaves, lots of import rewriting) |
| D | 2-3 days (internal refactor, careful re-export bookkeeping) |
| E | 2-3 days |
| F | 0.5 day |

Total: ~1.5-2 weeks of focused work.

## Verification

For each stage:

1. `cargo build --workspace` (debug): must succeed with zero warnings.
2. `cargo test --workspace`: all tests green, including
   `TestSandbox`-using integration tests (dev machine has OpenShell
   running per CLAUDE.md).
3. `cargo build --workspace --release`: release build succeeds.
4. After Stages B, C, E: capture a build-time benchmark.
   `cargo clean && cargo build --workspace --timings`. Save
   `target/cargo-timings/cargo-timing-*.html` artifacts and note
   wall-time. Compare to the pre-split baseline.
5. After Stages C, E: run the `rust-dev:review-rust-code` agent.
   File any issues as TODOs and fix in the same PR or a follow-up.
6. After Stage F: run `right --help`, `right up` against a test
   home, smoke-test one CC turn end-to-end.

## Edge cases & risks

- **Migration registry coupling**: Stage A is the first risky move
  because every domain reads from `migrations::MIGRATIONS`. The list
  is pure SQL strings — no Rust types crossing crate boundaries —
  so the move is mechanical. Tests covering each migration version
  exist in `right-agent::memory::store_tests` (will move to
  `right-db`).
- **`build.rs` ordering**: `tonic-prost-build` in `right-core` runs
  before `right-codegen` etc. Cargo handles this via the dep graph
  automatically; no manual sequencing needed. If `OUT_DIR` includes
  drift between releases, pin `tonic-prost-build` patch version
  explicitly.
- **`test-support` feature confusion**: only `right-core` has the
  feature flag. Other crates that need TestSandbox in dev-deps
  enable it via `right-core = { ..., features = ["test-support"] }`.
  No transitive feature plumbing.
- **Cycle risk in Stage D**: if Stage D leaves a stray
  `crate::telegram::` import inside `bot::cc::*`, Stage E extraction
  will fail at compile time (clean signal, easy fix).
- **Public-surface drift**: each new crate writes a deliberate
  `lib.rs` with explicit `pub use` re-exports. Internal modules
  default to `pub(crate)`. CI `cargo check` catches accidental
  publication of types meant to be crate-private.
- **`right-agent::openshell_proto` external consumers**: nothing
  outside the workspace uses it (`publish = false`). The re-export
  path is bot/CLI-internal; the move is safe.
- **`Cargo.lock` churn**: 7 new crates appear. Resolution time
  increase is minimal (single-digit ms on modern Cargo).
- **release-plz behavior with new internal crates**: confirmed — packages
  not listed inherit the `[workspace]` defaults
  (`git_tag_enable = false`, `git_release_enable = false`,
  `publish = false`, `git_only = true`). Adding minimal
  `version_group = "workspace"` entries keeps versions in sync.
- **ARCHITECTURE.md drift**: per CLAUDE.md "Cite-on-touch" rule, this
  spec mandates updating `ARCHITECTURE.md` Workspace/Module Map
  sections in Stage F as part of the same PR.

## Out of scope, but flagged

- A future Stage G could push `derive_token` + `generate_agent_secret`
  into `right-core::secret` to break the `codegen → mcp` edge. Not done
  in v1 because the gain (one fewer rebuild trigger for `right-codegen`
  when editing `right-mcp`) doesn't justify the additional crate-level
  surgery and the residual `McpServerEntry` cross-reference.
- A future Stage H could extract `bot::cron*` into `right-cron` if
  cron logic grows. Currently not warranted (98 commits / 30 days
  across cron files, but they're tightly coupled to the bot
  orchestrator).
- Build-tool optimizations (mold/lld linker, sccache, cargo-chef in
  CI). Separate effort.
