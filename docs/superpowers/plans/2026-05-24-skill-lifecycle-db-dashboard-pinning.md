# Skill Lifecycle DB Dashboard Pinning Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Move learned-skill lifecycle state from `.usage.json` to SQLite, preserve receipt-based foreground usage tracking, register background learning invocations with correct provenance, and make authenticated dashboard pinning the only operator pin surface.

**Architecture:** Add `right-lifecycle`, a shared Rust crate over a new `skill_lifecycle` table in `data.db`. `right`, `right-bot`, and `right-dashboard` use the crate instead of local JSON writers. Foreground usage still comes from `used_skill_receipts`; probe-writer and curator use per-invocation MCP config headers so `skill_learning_start` and `skill_learning_finish` can distinguish `foreground`, `probe_writer`, and `curator`.

**Tech Stack:** Rust 2024, rusqlite, right-db migrations, axum dashboard routes, Vue 3/TypeScript dashboard frontend, Claude Code MCP invocation plumbing.

**Spec:** `docs/superpowers/specs/2026-05-24-skill-lifecycle-db-dashboard-pinning-design.md`.

---

## File Structure

- Create `crates/right-lifecycle/Cargo.toml`
- Create `crates/right-lifecycle/src/lib.rs`
- Create `crates/right-db/src/sql/v32_skill_lifecycle.sql`
- Modify `Cargo.toml`
- Modify `crates/right-db/src/migrations.rs`
- Modify `crates/right/Cargo.toml`
- Modify `crates/bot/Cargo.toml`
- Modify `crates/right-dashboard/Cargo.toml`
- Modify `crates/right/src/right_backend.rs`
- Delete `crates/right/src/skill_lifecycle.rs`
- Modify `crates/right/src/main.rs`
- Modify `crates/right-mcp/src/internal_client.rs`
- Modify `crates/right/src/progress.rs`
- Modify `crates/right/src/internal_api.rs`
- Modify `crates/right/src/learning.rs`
- Modify `crates/bot/src/cc/invocation.rs`
- Modify `crates/bot/src/telegram/worker.rs`
- Modify `crates/bot/src/learning_probe_writer.rs`
- Modify `crates/bot/src/learning_curator.rs`
- Modify `crates/bot/src/lifecycle/transitions.rs`
- Remove or reduce `crates/bot/src/lifecycle/usage.rs` to a thin compatibility module with no JSON writer
- Modify `crates/bot/src/telegram/dashboard.rs`
- Modify `crates/bot/src/telegram/dashboard/skills.rs`
- Modify `crates/right-dashboard/src/api_types.rs`
- Modify `crates/right-dashboard/src/read_model/learning.rs`
- Modify `crates/right-dashboard/frontend/src/types.ts`
- Modify `crates/right-dashboard/frontend/src/api.ts`
- Modify `crates/right-dashboard/frontend/src/views/SkillsView.vue`
- Modify `PROMPT_SYSTEM.md`
- Modify `ARCHITECTURE.md`
- Modify `docs/architecture/lifecycle.md`
- Modify `docs/architecture/sessions.md`
- Modify `docs/architecture/mcp.md`
- Update `docs/superpowers/plans/2026-05-22-skill-learning-writer-curator.md` with a superseded-by note

## Baseline

- [ ] **Step 0.1: Confirm Rust skill availability**

Run:

```bash
devenv shell -- rg -n "rust-dev" AGENTS.md AGENTS.rust.md
```

Expected: project instructions mention `rust-dev:rust-dev`. If the skill is still unavailable in the session skill list, record that in the implementation notes and proceed with direct Rust edits. Do not block the plan on a missing external skill.

- [ ] **Step 0.2: Re-read the approved spec**

Run:

```bash
devenv shell -- sed -n '1,260p' docs/superpowers/specs/2026-05-24-skill-lifecycle-db-dashboard-pinning-design.md
```

Expected: the spec says no one-time `.usage.json` importer, no CLI pinning, dashboard pinning only, and usage detection through `used_skill_receipts`.

- [ ] **Step 0.3: Re-read architecture docs for touched subsystems**

Run:

```bash
devenv shell -- sed -n '1,220p' docs/architecture/lifecycle.md
devenv shell -- sed -n '1,220p' docs/architecture/sessions.md
devenv shell -- sed -n '1,220p' docs/architecture/mcp.md
devenv shell -- sed -n '1,220p' ARCHITECTURE.md
```

Expected: note any drift before editing. Later tasks must update these docs where the implementation changes storage, invocation registration, dashboard behavior, or MCP learning semantics.

- [ ] **Step 0.4: Run targeted baseline checks**

Run:

```bash
devenv shell -- cargo test -p right-db
devenv shell -- cargo test -p right-dashboard
devenv shell -- cargo test -p right --lib skill_learning
devenv shell -- cargo test -p right-bot --lib learning
devenv shell -- npm --prefix crates/right-dashboard/frontend run build
```

Expected: PASS or record pre-existing failures before edits. Do not run full workspace tests yet.

## Task 1: Add DB Schema And Shared Crate

**Files:**
- Create `crates/right-db/src/sql/v32_skill_lifecycle.sql`
- Modify `crates/right-db/src/migrations.rs`
- Create `crates/right-lifecycle/Cargo.toml`
- Create `crates/right-lifecycle/src/lib.rs`
- Modify `Cargo.toml`

- [ ] **Step 1.1: Write migration test expectation before migration code**

Add a test in `crates/right-db/src/migrations.rs` test module, or extend the existing migration tests, that migrates an in-memory database and asserts:

- `skill_lifecycle` exists.
- `skill_name` is the primary key.
- `state` accepts only `active`, `stale`, `archived`.
- `created_by` accepts only `foreground`, `probe_writer`, `curator`, `bundled`.
- `pinned` defaults to `0`.
- `use_count` and `patch_count` default to `0`.

Run:

```bash
devenv shell -- cargo test -p right-db skill_lifecycle
```

Expected: FAIL because v32 does not exist yet.

- [ ] **Step 1.2: Implement v32 migration**

Create `crates/right-db/src/sql/v32_skill_lifecycle.sql` with:

```sql
CREATE TABLE IF NOT EXISTS skill_lifecycle (
  skill_name       TEXT PRIMARY KEY,
  state            TEXT NOT NULL DEFAULT 'active'
                   CHECK (state IN ('active', 'stale', 'archived')),
  pinned           INTEGER NOT NULL DEFAULT 0 CHECK (pinned IN (0, 1)),
  created_by       TEXT NOT NULL DEFAULT 'foreground'
                   CHECK (created_by IN ('foreground', 'probe_writer', 'curator', 'bundled')),
  use_count        INTEGER NOT NULL DEFAULT 0 CHECK (use_count >= 0),
  patch_count      INTEGER NOT NULL DEFAULT 0 CHECK (patch_count >= 0),
  created_at       TEXT,
  last_used_at     TEXT,
  last_patched_at  TEXT,
  archived_at      TEXT,
  absorbed_into    TEXT
);

CREATE INDEX IF NOT EXISTS idx_skill_lifecycle_state
  ON skill_lifecycle(state);

CREATE INDEX IF NOT EXISTS idx_skill_lifecycle_created_by_state
  ON skill_lifecycle(created_by, state);

CREATE INDEX IF NOT EXISTS idx_skill_lifecycle_pinned
  ON skill_lifecycle(pinned);
```

Wire it in `crates/right-db/src/migrations.rs` by adding `V32_SCHEMA`, bumping `LATEST_SCHEMA_VERSION` to `32`, and appending `M::up(V32_SCHEMA)`.

Run:

```bash
devenv shell -- cargo test -p right-db skill_lifecycle
```

Expected: PASS.

- [ ] **Step 1.3: Add `right-lifecycle` crate skeleton**

Add workspace member `crates/right-lifecycle` to root `Cargo.toml`.

Create `crates/right-lifecycle/Cargo.toml` with dependencies:

- `chrono`
- `rusqlite`
- `serde`
- `thiserror`

Create `crates/right-lifecycle/src/lib.rs` with public types:

- `LifecycleState`
- `CreatedBy`
- `SkillLifecycleRow`
- `TransitionConfig`
- `LifecycleError`

Use `snake_case` conversion methods for DB strings rather than exposing serde as the database parser. Unknown DB strings must return `LifecycleError::InvalidState` or `LifecycleError::InvalidCreatedBy`.

- [ ] **Step 1.4: Write crate-level tests before operations**

Add tests in `crates/right-lifecycle/src/lib.rs` using in-memory SQLite migrated through `right_db::MIGRATIONS.to_latest` if `right-db` is added as a dev-dependency, or by executing the v32 schema inside the test. Test names:

- `mark_created_inserts_active_row_with_provenance`
- `bump_patch_preserves_existing_created_by`
- `bump_use_creates_foreground_row_when_missing`
- `set_pinned_toggles_existing_row`
- `automatic_transitions_skip_pinned_foreground_and_bundled_rows`

Run:

```bash
devenv shell -- cargo test -p right-lifecycle
```

Expected: FAIL because operations are not implemented.

- [ ] **Step 1.5: Implement lifecycle operations**

Implement:

- `mark_created(conn, skill_name, created_by, now_utc)`
- `bump_patch(conn, skill_name, created_by, now_utc)`
- `bump_use(conn, skill_name, now_utc)`
- `set_pinned(conn, skill_name, pinned)`
- `get(conn, skill_name)`
- `list(conn)`
- `list_curator_candidates(conn)`
- `apply_automatic_transitions(conn, now_utc, config)`

Rules:

- `mark_created` upserts active rows and sets `created_by`, `created_at`, clears `archived_at`, clears `absorbed_into`.
- `bump_patch` increments `patch_count`, sets `last_patched_at`, sets active state, and preserves `created_by` if the row already exists.
- `bump_use` increments `use_count`, sets `last_used_at`, sets active state, and creates missing rows as foreground.
- `set_pinned` creates no row for an unknown skill. Return `Ok(false)` when no row changed and `Ok(true)` when a row changed.
- `apply_automatic_transitions` skips pinned, archived, foreground, and bundled rows.
- Transition latest activity is the max of `last_used_at` and `last_patched_at`.

Run:

```bash
devenv shell -- cargo test -p right-lifecycle
devenv shell -- cargo test -p right-db skill_lifecycle
```

Expected: PASS.

## Task 2: Replace Bot-Side Usage JSON Writer

**Files:**
- Modify `crates/bot/Cargo.toml`
- Modify `crates/bot/src/telegram/worker.rs`
- Modify `crates/bot/src/lifecycle/transitions.rs`
- Remove or reduce `crates/bot/src/lifecycle/usage.rs`

- [ ] **Step 2.1: Add bot dependency**

Add `right-lifecycle = { path = "../right-lifecycle", version = "*" }` to `crates/bot/Cargo.toml`.

- [ ] **Step 2.2: Write regression test for receipt bumps into DB**

In `crates/bot/src/telegram/worker.rs` tests, add a focused test around the receipt handling helper. If the existing code does not expose a helper, extract one named `record_used_skill_receipts` that accepts an agent DB dir, receipt list, and timestamp.

Test assertions:

- two receipts for the same `rightx-*` package increment use count once per foreground turn.
- non-`rightx-*` receipts are ignored.
- missing lifecycle row is created with `created_by = foreground`.
- failure to open DB returns an error to the helper but the caller logs and keeps reply delivery separate.

Run:

```bash
devenv shell -- cargo test -p right-bot --lib used_skill_receipts
```

Expected: FAIL because the worker still writes `.usage.json`.

- [ ] **Step 2.3: Move foreground usage bumps to DB**

Change `crates/bot/src/telegram/worker.rs` receipt path:

- keep filtering with `is_rightx_skill`
- keep per-turn dedupe through `BTreeSet`
- keep appending receipt text to the reply
- replace `crate::lifecycle::usage::bump_use(&usage_path, ...)` with `right_lifecycle::bump_use(&conn, ...)`
- open DB through `right_db::open_connection(&ctx.agent_dir, false)`
- log lifecycle update failures without blocking Telegram send

Fix `ProbeAnchor.used_skill_receipts` comments so they say receipts come from the reply schema.

- [ ] **Step 2.4: Replace curator transition adapter**

Change `crates/bot/src/lifecycle/transitions.rs` so it either re-exports `right_lifecycle::apply_automatic_transitions` or disappears if all callers can use the shared crate directly.

Do not leave a JSON read/write path in `crates/bot/src/lifecycle/usage.rs`. If keeping the module to reduce call-site churn, it must only wrap DB operations and must not mention `.usage.json`.

Run:

```bash
devenv shell -- cargo test -p right-bot --lib used_skill_receipts
devenv shell -- cargo test -p right-bot --lib lifecycle
```

Expected: PASS.

## Task 3: Replace MCP Finish Lifecycle Writes

**Files:**
- Modify `crates/right/Cargo.toml`
- Modify `crates/right/src/right_backend.rs`
- Delete `crates/right/src/skill_lifecycle.rs`
- Modify `crates/right/src/main.rs`

- [ ] **Step 3.1: Add right dependency and write backend tests**

Add `right-lifecycle = { path = "../right-lifecycle", version = "*" }` to `crates/right/Cargo.toml`.

In `crates/right/src/right_backend_tests.rs`, add tests:

- `skill_learning_finish_created_updates_lifecycle_with_foreground_provenance`
- `skill_learning_finish_updated_bumps_patch_and_preserves_created_by`
- `skill_learning_finish_returns_error_when_lifecycle_write_fails`

Use existing `right_db::open_connection(&agent_dir, true)` setup. For the failure test, make the DB unavailable by removing or renaming `data.db` only inside a temp test directory, or inject an invalid agent dir through the existing backend test harness if available.

Run:

```bash
devenv shell -- cargo test -p right --lib skill_learning_finish_created_updates_lifecycle
devenv shell -- cargo test -p right --lib skill_learning_finish_updated_bumps_patch
```

Expected: FAIL because `right_backend` still writes through `crates/right/src/skill_lifecycle.rs`.

- [ ] **Step 3.2: Replace writer in `right_backend`**

In `crates/right/src/right_backend.rs`:

- remove imports of `crate::skill_lifecycle`
- map invocation kind to `right_lifecycle::CreatedBy`
- for successful `created`, call `right_lifecycle::mark_created`
- for successful `updated`, call `right_lifecycle::bump_patch`
- return a tool error if the lifecycle DB write fails after a successful finish payload
- keep `skill_learning_events` insertion as the audit log

- [ ] **Step 3.3: Delete duplicate right lifecycle writer**

Delete `crates/right/src/skill_lifecycle.rs`.

Remove `mod skill_lifecycle;` from `crates/right/src/main.rs` or the crate root where it is declared.

Run:

```bash
devenv shell -- cargo test -p right --lib skill_learning_finish
devenv shell -- cargo test -p right-lifecycle
```

Expected: PASS.

## Task 4: Remove CLI Pin Surface

**Files:**
- Modify `crates/right/src/main.rs`

- [ ] **Step 4.1: Write CLI shape test or snapshot update**

Search for existing CLI tests:

```bash
devenv shell -- rg -n "AgentSkillCommands|list-pins|pin|unpin|try_parse_from|CommandFactory" crates/right/src
```

If there is a CLI parse test module, add assertions that:

- `right agent skill list` still parses.
- `right agent skill pin rightx-test` fails to parse.
- `right agent skill unpin rightx-test` fails to parse.
- `right agent skill list-pins` fails to parse.

Run the narrowest matching test:

```bash
devenv shell -- cargo test -p right --lib agent_skill
```

Expected: FAIL until CLI variants are removed.

- [ ] **Step 4.2: Remove pin variants and handlers**

In `crates/right/src/main.rs`:

- remove `AgentSkillCommands::Pin`
- remove `AgentSkillCommands::Unpin`
- remove `AgentSkillCommands::ListPins`
- remove matching branches in `cmd_agent_skill`
- remove any helper functions that only served CLI pinning and were introduced for this feature

Do not remove unrelated pre-existing dead code.

Run:

```bash
devenv shell -- cargo test -p right --lib agent_skill
devenv shell -- cargo check -p right
```

Expected: PASS and no references to `list-pins`.

## Task 5: Add Invocation Kinds And Learning Delivery Semantics

**Files:**
- Modify `crates/right-mcp/src/internal_client.rs`
- Modify `crates/right/src/progress.rs`
- Modify `crates/right/src/internal_api.rs`
- Modify `crates/right/src/learning.rs`
- Modify `crates/right/src/right_backend.rs`

- [ ] **Step 5.1: Write progress kind tests**

Add tests in `crates/right/src/progress.rs` or `crates/right/src/internal_api.rs`:

- `probe_writer_and_curator_are_learning_capable`
- `probe_writer_and_curator_do_not_have_conversation_scope`
- `probe_writer_and_curator_do_not_send_telegram_learning_messages`
- `foreground_keeps_existing_learning_message_delivery`

Run:

```bash
devenv shell -- cargo test -p right --lib progress_invocation_kind
```

Expected: FAIL because only `Foreground` and `BackgroundReview` exist.

- [ ] **Step 5.2: Add DTO and domain variants**

In `crates/right-mcp/src/internal_client.rs`, add:

- `ProgressInvocationKindDto::ProbeWriter`
- `ProgressInvocationKindDto::Curator`

In `crates/right/src/progress.rs`, add:

- `ProgressInvocationKind::ProbeWriter`
- `ProgressInvocationKind::Curator`

Map these in `crates/right/src/internal_api.rs`.

Keep `BackgroundReview` only if existing non-learning callers still need it. Do not route probe-writer or curator through `BackgroundReview`.

- [ ] **Step 5.3: Gate learning messages by kind**

In `crates/right/src/learning.rs`, make message delivery explicit:

- `Foreground`: current start and successful finish behavior remains.
- `ProbeWriter`: insert `skill_learning_events`, update lifecycle on finish, no Telegram message send.
- `Curator`: insert `skill_learning_events`, update lifecycle on finish, no Telegram message send.

Avoid calling bot `/progress/send` for background kinds because they have no chat target.

Run:

```bash
devenv shell -- cargo test -p right --lib progress_invocation_kind
devenv shell -- cargo test -p right --lib learning
```

Expected: PASS.

## Task 6: Register Probe-Writer And Curator Invocations

**Files:**
- Modify `crates/bot/src/cc/invocation.rs`
- Modify `crates/bot/src/learning_probe_writer.rs`
- Modify `crates/bot/src/learning_curator.rs`

- [ ] **Step 6.1: Write tests for background MCP config**

Add tests that validate:

- probe-writer builds a Claude invocation with an MCP config path containing a generated invocation id.
- curator builds a Claude invocation with an MCP config path containing a generated invocation id.
- both register `ProbeWriter` or `Curator`, not `BackgroundReview`.
- cleanup unregisters and removes the temporary MCP config path on success and spawn failure.

Use existing invocation builder tests if present:

```bash
devenv shell -- rg -n "write_invocation_mcp_config|mcp_config_path|ProgressRegisterRequest|BackgroundReview" crates/bot/src
```

Run:

```bash
devenv shell -- cargo test -p right-bot --lib background_invocation
```

Expected: FAIL because background runs use normal `mcp.json`.

- [ ] **Step 6.2: Extract reusable invocation registration helper**

In `crates/bot/src/cc/invocation.rs`, add a small helper or type for registered non-foreground invocations that:

- generates invocation id and token
- calls aggregator `/progress/register` with supplied kind
- writes per-invocation MCP config through existing `write_invocation_mcp_config`
- uploads the generated file into the sandbox when OpenShell mode requires `/sandbox/.claude/mcp-<id>.json`
- returns the path that must be passed to `ClaudeInvocation`
- unregisters and removes files in `Drop` or through an explicit async cleanup method

Reuse the foreground implementation concepts from `crates/bot/src/telegram/worker.rs`. Do not duplicate large chunks if a shared helper can cover foreground and background safely.

- [ ] **Step 6.3: Wire probe-writer**

In `crates/bot/src/learning_probe_writer.rs`:

- register kind `ProbeWriter`
- pass the generated per-invocation MCP config path into `ClaudeInvocation`
- keep existing allowed tools including `mcp__right__skill_learning_start` and `mcp__right__skill_learning_finish`
- ensure cleanup happens on system/init failure, spawn failure, timeout, and normal completion

- [ ] **Step 6.4: Wire curator**

In `crates/bot/src/learning_curator.rs`:

- register kind `Curator`
- pass the generated per-invocation MCP config path into `ClaudeInvocation`
- ensure cleanup happens on system/init failure, spawn failure, timeout, and normal completion

Run:

```bash
devenv shell -- cargo test -p right-bot --lib background_invocation
devenv shell -- cargo test -p right-bot --lib learning_probe_writer
devenv shell -- cargo test -p right-bot --lib learning_curator
```

Expected: PASS.

## Task 7: Move Curator Reads And Transitions To DB

**Files:**
- Modify `crates/bot/src/learning_curator.rs`
- Modify `crates/bot/src/lifecycle/transitions.rs`
- Modify `crates/bot/src/lifecycle/usage.rs`

- [ ] **Step 7.1: Write curator DB behavior tests**

Add tests asserting:

- pinned skills are skipped by automatic transitions.
- foreground and bundled skills are not listed as curator candidates.
- probe_writer and curator skills can transition to stale or archived.
- curator candidate rendering includes state, pinned, use count, patch count, and latest activity from DB.

Run:

```bash
devenv shell -- cargo test -p right-bot --lib curator_lifecycle
```

Expected: FAIL if curator still reads `.usage.json`.

- [ ] **Step 7.2: Replace curator JSON reads**

In `crates/bot/src/learning_curator.rs`:

- open DB through `right_db::open_connection(&ctx.agent_db_dir, false)`
- call `right_lifecycle::apply_automatic_transitions`
- call `right_lifecycle::list_curator_candidates`
- render only DB lifecycle candidates
- skip pinned, foreground, bundled, and archived rows as defined by the shared crate

Run:

```bash
devenv shell -- cargo test -p right-bot --lib curator_lifecycle
devenv shell -- cargo test -p right-lifecycle
```

Expected: PASS.

## Task 8: Add Dashboard Lifecycle And Pin API

**Files:**
- Modify `crates/right-dashboard/Cargo.toml`
- Modify `crates/right-dashboard/src/api_types.rs`
- Modify `crates/right-dashboard/src/read_model/learning.rs`
- Modify `crates/bot/src/telegram/dashboard.rs`
- Modify `crates/bot/src/telegram/dashboard/skills.rs`

- [ ] **Step 8.1: Add dashboard dependency**

Add `right-lifecycle = { path = "../right-lifecycle", version = "*" }` to `crates/right-dashboard/Cargo.toml`.

- [ ] **Step 8.2: Extend DTOs with lifecycle fields**

In `crates/right-dashboard/src/api_types.rs`, add:

- `SkillLifecycleState`
- `SkillCreatedBy`
- lifecycle fields on `SkillSummary`: `state`, `pinned`, `created_by`, `use_count`, `patch_count`, `created_at`, `last_used_at`, `last_patched_at`
- `PinSkillRequest { pinned: bool }`
- `PinSkillResponse { skill_name, pinned }`

If `SkillLifecycleOverviewResponse` already exists, move it from JSON-file parsing to DB rows without changing frontend field names unless the current shape is wrong.

- [ ] **Step 8.3: Write read-model tests**

In `crates/right-dashboard/src/read_model/learning.rs` tests, assert:

- lifecycle overview reads `skill_lifecycle`.
- pinned count comes from DB.
- provenance counts distinguish foreground, probe_writer, curator, bundled.
- `.usage.json` is not required for lifecycle overview.

Run:

```bash
devenv shell -- cargo test -p right-dashboard skill_lifecycle
```

Expected: FAIL while read model still parses `.usage.json`.

- [ ] **Step 8.4: Implement DB-backed read model**

In `crates/right-dashboard/src/read_model/learning.rs`:

- remove direct `.usage.json` parsing for lifecycle overview
- query `skill_lifecycle`
- derive counts from DB rows
- keep empty responses stable when the table exists but has no rows

Run:

```bash
devenv shell -- cargo test -p right-dashboard skill_lifecycle
```

Expected: PASS.

- [ ] **Step 8.5: Write route tests for pinning**

In `crates/bot/src/telegram/dashboard.rs` tests, assert:

- `PATCH /dashboard/{agent}/api/v1/knowledge/skills/{skill_name}/pin` requires dashboard auth like existing routes.
- body `{ "pinned": true }` sets DB pinned state.
- body `{ "pinned": false }` clears DB pinned state.
- unknown skill returns `404`.
- non-`rightx-*` skill returns `400`.

Run:

```bash
devenv shell -- cargo test -p right-bot --lib dashboard_skill_pin
```

Expected: FAIL until the route exists.

- [ ] **Step 8.6: Implement dashboard pin route**

In `crates/bot/src/telegram/dashboard.rs` and `crates/bot/src/telegram/dashboard/skills.rs`:

- mount `PATCH /api/v1/knowledge/skills/{skill_name}/pin`
- validate authenticated dashboard request through existing route stack
- validate `skill_name` starts with `rightx-`
- open writable DB connection with `right_db::open_connection(&state.agent_dir, false)`
- call `right_lifecycle::set_pinned`
- return `404` if no lifecycle row changed
- return `PinSkillResponse`

Enrich skills overview and detail responses with lifecycle fields by joining filesystem skill summaries with DB lifecycle rows. File-based skills without lifecycle rows should display neutral defaults: `state = null`, `pinned = false`, counters `0`, provenance `null`.

Run:

```bash
devenv shell -- cargo test -p right-bot --lib dashboard_skill_pin
devenv shell -- cargo test -p right-dashboard skill_lifecycle
```

Expected: PASS.

## Task 9: Add Dashboard Pin Controls

**Files:**
- Modify `crates/right-dashboard/frontend/src/types.ts`
- Modify `crates/right-dashboard/frontend/src/api.ts`
- Modify `crates/right-dashboard/frontend/src/views/SkillsView.vue`

- [ ] **Step 9.1: Add frontend API types**

In `types.ts`, mirror backend lifecycle fields and add:

- `PinSkillRequest`
- `PinSkillResponse`

In `api.ts`, add:

```ts
export function setSkillPinned(skillName: string, pinned: boolean): Promise<PinSkillResponse> {
  return requestJson<PinSkillResponse>(`api/v1/knowledge/skills/${encodeURIComponent(skillName)}/pin`, {
    method: 'PATCH',
    body: JSON.stringify({ pinned }),
  })
}
```

If `requestJson` currently has no options parameter, extend it to accept `RequestInit` and merge headers without losing Telegram auth.

- [ ] **Step 9.2: Update Skills view**

In `SkillsView.vue`:

- show pinned state in the skill list and detail panel
- add a pin/unpin toggle button in the detail panel for `rightx-*` skills with lifecycle rows
- disable the control while the request is inflight
- update local state after successful response
- show route/API error through the existing dashboard error surface
- keep layout stable on mobile and desktop

Use existing icon conventions in the frontend. If lucide icons are already used, use a pin icon from that library. If no icon library is present, use the current button style without introducing a new dependency.

Run:

```bash
devenv shell -- npm --prefix crates/right-dashboard/frontend run build
```

Expected: PASS.

## Task 10: Remove `.usage.json` References From Runtime Paths

**Files:**
- Modify `PROMPT_SYSTEM.md`
- Modify runtime code found by search

- [ ] **Step 10.1: Search stale runtime references**

Run:

```bash
devenv shell -- rg -n "\\.usage\\.json|skill_lifecycle::|lifecycle::usage|list-pins|AgentSkillCommands::Pin|AgentSkillCommands::Unpin" crates PROMPT_SYSTEM.md
```

Expected before cleanup: runtime references remain.

- [ ] **Step 10.2: Update prompt and remove stale runtime code**

Update `PROMPT_SYSTEM.md` so `used_skill_receipts` says the platform records usage in the skill lifecycle database, not `.usage.json`.

Remove or update runtime references:

- no JSON lifecycle writer
- no CLI pin command strings
- no duplicate `skill_lifecycle` module in `right`
- no bot dead-code lifecycle functions caused by the previous JSON writer

Run:

```bash
devenv shell -- rg -n "\\.usage\\.json|skill_lifecycle::|list-pins|AgentSkillCommands::Pin|AgentSkillCommands::Unpin" crates PROMPT_SYSTEM.md
```

Expected after cleanup: no runtime references. Historical docs under `docs/superpowers/` may still mention `.usage.json` as design history.

## Task 11: Update Architecture And Plan Docs

**Files:**
- Modify `ARCHITECTURE.md`
- Modify `docs/architecture/lifecycle.md`
- Modify `docs/architecture/sessions.md`
- Modify `docs/architecture/mcp.md`
- Modify `docs/superpowers/plans/2026-05-22-skill-learning-writer-curator.md`

- [ ] **Step 11.1: Update lifecycle architecture**

In `docs/architecture/lifecycle.md`:

- `skill_lifecycle` table is mutable current state.
- `skill_learning_events` remains append-only audit history.
- skill package content remains under `.claude/skills/<skill_name>/SKILL.md`.
- foreground usage detection uses `used_skill_receipts`.
- curator transitions read and write DB rows.
- pinned rows are skipped by curator.

- [ ] **Step 11.2: Update session and MCP architecture**

In `docs/architecture/sessions.md` and `docs/architecture/mcp.md`:

- foreground invocations use `Foreground`.
- probe-writer uses `ProbeWriter`.
- curator uses `Curator`.
- all learning-capable invocations carry `X-Right-Invocation` through per-invocation MCP config.
- background kinds record lifecycle and events without Telegram learning-message delivery.

- [ ] **Step 11.3: Update prescriptive architecture**

In `ARCHITECTURE.md`, add or update load-bearing rules:

- lifecycle mutable state lives in `data.db`, not `.usage.json`.
- dashboard is the pin/unpin surface.
- background learning writers must register invocation identity before using learning MCP tools.

- [ ] **Step 11.4: Mark older plan superseded**

At the top of `docs/superpowers/plans/2026-05-22-skill-learning-writer-curator.md`, add a short note that the `.usage.json` lifecycle portion is superseded by `docs/superpowers/plans/2026-05-24-skill-lifecycle-db-dashboard-pinning.md`.

Run:

```bash
devenv shell -- rg -n "\\.usage\\.json|ProbeWriter|Curator|skill_lifecycle" ARCHITECTURE.md docs/architecture PROMPT_SYSTEM.md docs/superpowers/plans/2026-05-22-skill-learning-writer-curator.md
```

Expected: docs describe DB lifecycle as current behavior and only mention `.usage.json` as superseded history.

## Task 12: Focused Integration Checks

- [ ] **Step 12.1: Run backend targeted tests**

Run:

```bash
devenv shell -- cargo test -p right-db skill_lifecycle
devenv shell -- cargo test -p right-lifecycle
devenv shell -- cargo test -p right --lib skill_learning
devenv shell -- cargo test -p right --lib progress
devenv shell -- cargo test -p right-bot --lib used_skill_receipts
devenv shell -- cargo test -p right-bot --lib learning_probe_writer
devenv shell -- cargo test -p right-bot --lib learning_curator
devenv shell -- cargo test -p right-bot --lib dashboard_skill_pin
devenv shell -- cargo test -p right-dashboard skill_lifecycle
```

Expected: PASS. Fix failures before final verification.

- [ ] **Step 12.2: Run frontend build**

Run:

```bash
devenv shell -- npm --prefix crates/right-dashboard/frontend run build
```

Expected: PASS.

- [ ] **Step 12.3: Run final workspace verification**

Run:

```bash
devenv shell -- cargo fmt --all --check
devenv shell -- cargo test --workspace
devenv shell -- cargo build --workspace
```

Expected:

- PASS.
- no lifecycle dead-code warnings for `bump_patch`, `mark_created`, `mark_archived`, or `set_pinned`.
- no runtime `.usage.json` writer remains.
- probe-writer and curator successful `skill_learning_finish` update `skill_lifecycle`.
- dashboard lifecycle counts distinguish `foreground`, `probe_writer`, `curator`, and `bundled`.
- dashboard pin/unpin updates DB and curator skips pinned rows.

## Execution Notes

- Do not add a `.usage.json` importer. The feature is unreleased.
- Do not keep CLI pinning as a hidden compatibility path.
- Do not infer usage from file reads, skill directory scans, or prompt text. The only usage signal is `used_skill_receipts` from normal foreground replies.
- Do not make background invocations depend on bot-local Telegram progress targets.
- Use narrow targeted tests while iterating and the full workspace test/build only at the end.
- Keep unrelated pre-existing dead code untouched unless the edits in this plan make it newly unused.
