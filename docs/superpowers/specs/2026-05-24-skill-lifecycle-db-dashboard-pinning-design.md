# Skill Lifecycle DB And Dashboard Pinning Design

## Context

The skill-learning lifecycle currently stores mutable state in
`<agent>/.claude/skills/.usage.json`. The original reason was proximity to the
file-based skill packages: lifecycle metadata could be backed up with the
skills directory, opened by hand, and written atomically without a database
migration.

That tradeoff no longer fits the product shape. Lifecycle state now drives the
curator, dashboard lifecycle views, pinning, provenance, and operator control.
Those are control-plane concerns. Keeping them in a sidecar JSON file creates
duplicate writers, file/database drift, and an unsafe mirrored schema in
`crates/right/src/skill_lifecycle.rs` that can round-trip non-active lifecycle
states back to `active`.

This feature has not shipped, so there is no legacy migration or `.usage.json`
import path.

## Goals

- Make SQLite `data.db` the source of truth for learned-skill lifecycle state.
- Remove `.usage.json` lifecycle writes and the duplicate schema/writer.
- Move pin/unpin out of CLI and into the authenticated dashboard control plane.
- Register probe-writer and curator MCP invocations so
  `mcp__right__skill_learning_start` and `mcp__right__skill_learning_finish`
  can update lifecycle state with correct provenance.
- Preserve the existing file-based skill package format under
  `.claude/skills/<skill_name>/SKILL.md`.
- Keep `skill_learning_events` as the append-only audit log and add a mutable
  lifecycle table for current state.
- Keep foreground skill usage detection based on `used_skill_receipts`.
- Remove lifecycle dead-code warnings without `#[allow(dead_code)]`.

## Non-Goals

- No one-time importer from `.usage.json`.
- No compatibility mode where `.usage.json` remains a source of truth.
- No automatic detection of Claude Code skill activation from file reads or
  prompt loading.
- No pin/unpin CLI replacement. Dashboard is the operator surface.
- No change to the `rightx-*` skill package on-disk format.

## Data Model

Add a `skill_lifecycle` table in `right-db`:

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

`skill_learning_events` remains append-only history: starts, finishes, status,
hint outcome, message, and refs. `skill_lifecycle` is the current-state row
used by dashboard, curator transitions, and pinning.

## Crate Boundaries

Add a small shared crate, `right-lifecycle`, for DB-backed lifecycle operations.
It should depend on `rusqlite` and define DTO-neutral types:

- `LifecycleState`: `Active`, `Stale`, `Archived`
- `CreatedBy`: `Foreground`, `ProbeWriter`, `Curator`, `Bundled`
- `SkillLifecycleRow`
- `TransitionConfig`

Operations:

- `mark_created(conn, skill_name, created_by, now_utc)`
- `bump_patch(conn, skill_name, created_by, now_utc)`
- `bump_use(conn, skill_name, now_utc)`
- `set_pinned(conn, skill_name, pinned)`
- `apply_automatic_transitions(conn, now_utc, config)`
- read helpers for dashboard lifecycle overview and skill-list enrichment

Consumers:

- `right-bot`: receipt usage bumps, curator transitions, dashboard write route.
- `right`: MCP backend lifecycle writes from `skill_learning_finish`.
- `right-dashboard`: read-model helpers can call `right-lifecycle` or receive
  lifecycle rows from bot-owned dashboard handlers.

Delete `crates/right/src/skill_lifecycle.rs`. Replace
`crates/bot/src/lifecycle/usage.rs` with `right-lifecycle` calls, or remove it
if no local adapter remains useful.

## Invocation Identity And Provenance

The current `ProgressInvocationKind::BackgroundReview` is too coarse for the
new lifecycle model. Probe-writer and curator need distinct provenance.

Supported learning-capable invocation kinds:

- `Foreground`
- `ProbeWriter`
- `Curator`

`BackgroundReview` can remain only for legacy report-only paths if still
needed, but probe-writer and curator must not use it.

Probe-writer and curator must:

1. Generate a fresh invocation id and token.
2. Register with the aggregator through `/progress/register` using
   `ProbeWriter` or `Curator`.
3. Write a per-invocation MCP config containing `X-Right-Invocation`.
4. Upload that config to the sandbox when running in OpenShell mode.
5. Pass the per-invocation config path to `ClaudeInvocation`.
6. Unregister and clean up the config file on completion, timeout, init
   failure, spawn failure, or cancellation.

The existing registration name is progress-oriented, but these background runs
need identity registration for MCP tool access and provenance, not Telegram
progress delivery. Learning messages should be delivery-gated by invocation
kind:

- `Foreground`: send start and successful-finish Telegram learning messages as
  today.
- `ProbeWriter`: record events and lifecycle only; do not send Telegram
  learning messages in this design.
- `Curator`: record events and lifecycle only; do not send Telegram learning
  messages in this design.

This avoids requiring a bot-local `ProgressState` chat target for curator runs.

## Skill Learning Tool Flow

`mcp__right__skill_learning_start` keeps validating action, skill name, and
package expectation. It records a `skill_learning_events` start row.

`mcp__right__skill_learning_finish` validates the finish payload and package
state. On successful `created` or `updated` status it must update
`skill_lifecycle` in the same logical operation:

- `created`:
  - upsert row
  - `created_by = invocation kind`
  - `created_at = now`
  - `state = active`
- `updated`:
  - upsert row if absent
  - increment `patch_count`
  - set `last_patched_at = now`
  - preserve existing `created_by` unless the row was absent

If the lifecycle write fails after a successful finish payload, the tool should
return an error instead of only logging. The caller must not believe durable
lifecycle state exists when it does not.

## Usage Detection

There is no reliable automatic runtime signal for "Claude used this skill."
Claude Code may auto-load skills by description, but the platform does not get
a trustworthy per-skill activation event. File reads are not usage because
probe-writer and curator can scan many skills without applying them.

Usage remains receipt-based:

1. Normal foreground reply schema requires `used_skill_receipts`, possibly an
   empty array.
2. Each receipt has `{ package_name, message }`.
3. Worker filters to `rightx-*` package names.
4. Worker deduplicates names per foreground turn.
5. Worker appends receipt text to the Telegram reply.
6. Worker calls `right_lifecycle::bump_use(conn, package_name, now_utc)`.

If a usage bump sees a missing lifecycle row, it creates one with
`created_by = foreground`, `state = active`, `use_count = 1`, and
`last_used_at = now`. This covers explicit/manual `rightx-*` skills that exist
before lifecycle rows are created.

Usage bump failures are logged but do not block Telegram delivery. Reply
delivery is user-facing; lifecycle usage metrics are secondary.

Fix the current comment drift in `ProbeAnchor.used_skill_receipts`: the names
come from reply `used_skill_receipts`, not from
`mcp__right__use_skill` tool calls.

## Curator Behavior

The curator transition pass reads `skill_lifecycle`, not `.usage.json`.

Rules:

- Pinned rows are skipped by automatic stale/archive transitions.
- Archived rows are not transitioned again.
- Latest activity is the max of `last_used_at` and `last_patched_at`.
- Rows with no activity are left unchanged.
- `probe_writer` and `curator` rows are curator-managed.
- `foreground` and `bundled` rows are curator-immune.

The LLM curator inventory lists only unpinned, curator-managed `rightx-*`
skills. It includes state, use count, patch count, and pinned state from
`skill_lifecycle`.

## Dashboard Pinning

The dashboard becomes the only operator pinning surface.

Backend:

- Existing lifecycle overview reads from `skill_lifecycle`.
- Skill list/detail responses include lifecycle metadata for `rightx-*` skills
  where available: `state`, `pinned`, `created_by`, `use_count`,
  `last_used_at`, and `last_patched_at`.
- Add an authenticated bot-owned write route:
  - `PATCH /dashboard/{agent}/api/v1/knowledge/skills/{skill_name}/pin`
  - body: `{ "pinned": true | false }`
- The route validates:
  - dashboard auth and agent path as existing routes do;
  - skill name starts with `rightx-`;
  - skill package exists;
  - lifecycle row exists and is curator-managed (`probe_writer` or `curator`).

Responses:

- `200`: updated row/summary.
- `400`: invalid skill name or non-`rightx-*`.
- `404`: skill package or lifecycle row missing.
- `409`: skill is not curator-managed, so pinning is not applicable.
- `500`: DB failure.

Frontend:

- Learned skill rows show pinned state.
- Selected learned curator-managed skill exposes a pin/unpin icon button.
- Core, bundled, foreground/manual, and other skills do not expose pin controls.
- After mutation, refresh the selected skill and lifecycle overview.

Remove `right agent skill pin`, `right agent skill unpin`, and
`right agent skill list-pins`.

## Error Handling

- Background invocation registration/config/upload failure skips that
  probe-writer or curator run and logs the concrete failure.
- Registration cleanup failures are warned, but cleanup must be attempted for
  aggregator registration, local config file, and sandbox config file.
- DB lifecycle write failures in `skill_learning_finish` return tool errors.
- Receipt `bump_use` failures warn and do not block Telegram delivery.
- Dashboard write errors return structured JSON errors with stable codes.
- Do not silently swallow filesystem, DB, or registration errors.

## Tests

Database and lifecycle:

- Migration creates `skill_lifecycle` with constraints and indexes.
- `mark_created` creates/updates provenance and active state.
- `bump_patch` increments patch count and preserves existing provenance.
- `bump_use` increments use count, sets `last_used_at`, and reactivates stale
  rows.
- Missing-row `bump_use` creates a foreground active row.
- Pinned rows are skipped by stale/archive transitions.
- Archived rows are not transitioned again.

Invocation and MCP:

- Probe-writer builds a per-invocation MCP config path and registers
  `ProbeWriter`.
- Curator builds a per-invocation MCP config path and registers `Curator`.
- Missing registration/header returns `learning_unavailable`.
- `skill_learning_finish` writes `created_by = foreground`, `probe_writer`, or
  `curator` based on invocation kind.
- Background learning invocation does not require bot-local Telegram progress
  target registration.

Worker usage:

- `used_skill_receipts` bump DB `use_count` and `last_used_at`.
- Duplicate receipts count once per turn.
- Non-`rightx-*` receipts are ignored.
- Empty receipts do nothing.
- DB bump failure does not block reply delivery.

Dashboard:

- Lifecycle overview reads DB rows, not `.usage.json`.
- Skill summaries include lifecycle metadata.
- Pin/unpin succeeds for curator-managed learned skills.
- Pin/unpin rejects non-`rightx-*`, missing, bundled, and foreground skills.

Cleanup:

- `crates/right/src/skill_lifecycle.rs` is removed.
- No `.usage.json` lifecycle writer remains.
- No lifecycle dead-code warnings remain.

## Documentation Updates

Update:

- `ARCHITECTURE.md` skill-learning sections.
- `docs/architecture/lifecycle.md`.
- `docs/architecture/sessions.md` invocation-kind and MCP config details.
- `docs/architecture/mcp.md` learned-skill tool availability.
- `PROMPT_SYSTEM.md` `used_skill_receipts` text to point at `data.db`.
- The existing 2026-05-22 skill-learning writer/curator plan or a new
  implementation plan that supersedes its `.usage.json` tasks.

## Acceptance

- `devenv shell -- cargo test --workspace`
- `devenv shell -- cargo build --workspace`
- No lifecycle dead-code warnings.
- No `.usage.json` lifecycle writer remains.
- Probe-writer and curator successful `skill_learning_finish` calls update
  `skill_lifecycle` in `data.db`.
- Dashboard lifecycle counts distinguish `foreground`, `probe_writer`,
  `curator`, and `bundled` skills.
- Dashboard pin/unpin controls update `skill_lifecycle.pinned`.
